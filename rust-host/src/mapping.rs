use crate::input::{DeviceMessage, InputFrame, RawInputEvent, is_event_node};
use crate::mouse::MouseConfig;
use crate::protocol::{Action, Frame, MAX_POINTERS, Pointer};
use evdev::{EventType, KeyCode};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const EV_KEY: u16 = EventType::KEY.0;
const INPUT_DOWN: i32 = 1;
const INPUT_UP: i32 = 0;

/// Keyboard codes reserved by the controller: F1/F2/F3 hotkeys and the
/// Ctrl+Alt+Esc emergency chord. Movement keys and bindings must avoid them.
const RESERVED_EMERGENCY_CODES: [u16; 8] = [
    KeyCode::KEY_LEFTCTRL.0,
    KeyCode::KEY_RIGHTCTRL.0,
    KeyCode::KEY_LEFTALT.0,
    KeyCode::KEY_RIGHTALT.0,
    KeyCode::KEY_ESC.0,
    KeyCode::KEY_F1.0,
    KeyCode::KEY_F2.0,
    KeyCode::KEY_F3.0,
];

/// Shared button-name limit across Rust, the Android service, and both editors.
const MAX_BUTTON_NAME_LENGTH: usize = 40;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ButtonSource {
    Key,
    Mouse,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Profile {
    #[serde(default)]
    pub(crate) layout_revision: u64,
    pub(crate) display: DisplayConfig,
    pub(crate) devices: DeviceConfig,
    pub(crate) movement: MovementConfig,
    #[serde(default)]
    pub(crate) mouse: MouseConfig,
    pub(crate) buttons: Vec<ButtonConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DisplayConfig {
    width: u32,
    height: u32,
    #[serde(default)]
    rotation: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeviceConfig {
    keyboard: Vec<PathBuf>,
    mouse: Vec<PathBuf>,
    #[serde(default)]
    suppress: Vec<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MovementConfig {
    center: [f32; 2],
    radius: f32,
    keys: MovementKeys,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct MovementKeys {
    up: u16,
    left: u16,
    down: u16,
    right: u16,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ButtonConfig {
    name: String,
    source: ButtonSource,
    code: u16,
    position: [f32; 2],
}

/// Shared Android/Linux layout and input-binding snapshot.
/// Optional fields accept the original position-only schema during migration.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct LayoutSnapshot {
    movement: LayoutMovement,
    buttons: Vec<LayoutButton>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct LayoutMovement {
    center: [f32; 2],
    radius: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    keys: Option<MovementKeys>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct LayoutButton {
    name: String,
    position: [f32; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<ButtonSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    code: Option<u16>,
}

impl Profile {
    pub(crate) fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let bytes = fs::read(path)?;
        let profile: Self = serde_json::from_slice(&bytes)?;
        profile.validate()?;
        Ok(profile)
    }

    pub(crate) fn display_size(&self) -> (u32, u32) {
        (self.display.width, self.display.height)
    }

    pub(crate) fn set_display_size(&mut self, width: u32, height: u32) {
        if width >= 2 && height >= 2 {
            self.display.width = width;
            self.display.height = height;
        }
    }

    pub(crate) fn display_rotation(&self) -> u32 {
        self.display.rotation.min(3)
    }

    pub(crate) fn set_display_calibration_rotation(&mut self, rotation: u32) {
        self.display.rotation = rotation.min(3);
    }

    pub(crate) fn has_required_devices(&self) -> bool {
        !self.devices.keyboard.is_empty() && !self.devices.mouse.is_empty()
    }

    pub(crate) fn replace_device_paths(
        &mut self,
        keyboard: Vec<PathBuf>,
        mouse: Vec<PathBuf>,
        suppress: Vec<PathBuf>,
    ) -> Result<(), String> {
        self.devices.keyboard = keyboard;
        self.devices.mouse = mouse;
        self.devices.suppress = suppress;
        self.validate()
    }

    pub(crate) fn layout_snapshot(&self) -> LayoutSnapshot {
        LayoutSnapshot {
            movement: LayoutMovement {
                center: self.movement.center,
                radius: self.movement.radius,
                keys: Some(self.movement.keys.clone()),
            },
            buttons: self
                .buttons
                .iter()
                .map(|button| LayoutButton {
                    name: button.name.clone(),
                    position: button.position,
                    source: Some(button.source),
                    code: Some(button.code),
                })
                .collect(),
        }
    }

    pub(crate) fn persist_atomic(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("profile.json");
        static TEMP_NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nonce = TEMP_NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let epoch_nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temporary = parent.join(format!(
            ".{name}.waytouch-{}-{epoch_nanos}-{nonce}.tmp",
            std::process::id()
        ));
        let bytes = serde_json::to_vec_pretty(self)?;
        let previous_permissions = fs::metadata(path)
            .ok()
            .map(|metadata| metadata.permissions());

        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&bytes)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            drop(file);

            if let Some(permissions) = previous_permissions {
                fs::set_permissions(&temporary, permissions)?;
            }
            fs::rename(&temporary, path)?;
            if let Ok(directory) = fs::File::open(parent) {
                directory.sync_all()?;
            }
            Ok(())
        })();

        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn validate(&self) -> Result<(), String> {
        if self.display.width < 2 || self.display.height < 2 {
            return Err("display width and height must both be at least 2".into());
        }
        let mut device_paths = HashSet::new();
        for path in self
            .devices
            .keyboard
            .iter()
            .chain(&self.devices.mouse)
            .chain(&self.devices.suppress)
        {
            if !is_event_node(path) {
                return Err(format!(
                    "invalid profile input device path: {}",
                    path.display()
                ));
            }
            if !device_paths.insert(path) {
                return Err(format!(
                    "input device path is duplicated or assigned to both roles: {}",
                    path.display()
                ));
            }
        }
        // Empty device lists are valid for a fresh template. Runtime control
        // requires both roles and reports a discovery instruction before capture.

        self.mouse.validate()?;

        let center = self.movement.center;
        if !valid_normalized(center) {
            return Err("movement center must be normalized to [0, 1]".into());
        }
        if !self.movement.radius.is_finite()
            || self.movement.radius <= 0.0
            || self.movement.radius > 0.5
        {
            return Err("movement radius must be finite and in (0, 0.5]".into());
        }

        let movement_codes = [
            self.movement.keys.up,
            self.movement.keys.left,
            self.movement.keys.down,
            self.movement.keys.right,
        ];
        let movement_set = movement_codes.into_iter().collect::<HashSet<_>>();
        if movement_set.len() != 4 || movement_set.iter().any(|code| *code > 767) {
            return Err("movement key codes must be unique and within the Linux key range".into());
        }
        if movement_set
            .iter()
            .any(|code| RESERVED_EMERGENCY_CODES.contains(code))
        {
            return Err(
                "F1 is reserved for mouse capture; F2 for mouse-button mapping; F3 for camera aim; Ctrl/Alt/Esc for emergency stop"
                    .into(),
            );
        }

        let mut binding_names = HashSet::new();
        let mut triggers = HashSet::new();
        for button in &self.buttons {
            if button.name.trim().is_empty()
                || button.name.chars().count() > MAX_BUTTON_NAME_LENGTH
                || !binding_names.insert(button.name.as_str())
            {
                return Err(
                    "button names must be non-empty, unique, and at most 40 characters".into(),
                );
            }
            if !valid_normalized(button.position) {
                return Err(format!(
                    "button '{}' position must be normalized to [0, 1]",
                    button.name
                ));
            }
            if !triggers.insert((button.source, button.code)) {
                return Err(format!(
                    "duplicate {:?} trigger code {}",
                    button.source, button.code
                ));
            }
            let valid_code = match button.source {
                ButtonSource::Key => button.code <= 767,
                ButtonSource::Mouse => (272..=287).contains(&button.code),
            };
            if !valid_code {
                return Err(format!(
                    "button '{}' has an invalid {:?} input code {}",
                    button.name, button.source, button.code
                ));
            }
            if button.source == ButtonSource::Key && movement_set.contains(&button.code) {
                return Err(format!(
                    "button '{}' reuses a movement key; each key must have one role",
                    button.name
                ));
            }
            if button.source == ButtonSource::Key && RESERVED_EMERGENCY_CODES.contains(&button.code)
            {
                return Err(format!(
                    "button '{}' uses a key reserved for a global WayTouch shortcut",
                    button.name
                ));
            }
        }
        if self.buttons.len() + 1 > MAX_POINTERS {
            return Err(format!(
                "profile defines too many touch controls: at most {} pointers including movement",
                MAX_POINTERS
            ));
        }
        Ok(())
    }

    pub(crate) fn device_paths(&self) -> Vec<PathBuf> {
        self.devices
            .keyboard
            .iter()
            .chain(&self.devices.mouse)
            .chain(&self.devices.suppress)
            .cloned()
            .collect()
    }

    pub(crate) fn suppressed_device_paths(&self) -> &[PathBuf] {
        &self.devices.suppress
    }

    fn is_keyboard(&self, path: &Path) -> bool {
        self.devices.keyboard.iter().any(|item| item == path)
    }

    pub(crate) fn is_keyboard_device(&self, path: &Path) -> bool {
        self.is_keyboard(path)
    }

    pub(crate) fn is_mouse_device(&self, path: &Path) -> bool {
        self.devices.mouse.iter().any(|item| item == path)
    }

    pub(crate) fn is_mapped_mouse_button(&self, code: u16) -> bool {
        self.buttons
            .iter()
            .any(|button| button.source == ButtonSource::Mouse && button.code == code)
    }

    fn movement_code(&self, code: u16) -> bool {
        code == self.movement.keys.up
            || code == self.movement.keys.left
            || code == self.movement.keys.down
            || code == self.movement.keys.right
    }

    fn button(&self, source: ButtonSource, code: u16) -> Option<&ButtonConfig> {
        self.buttons
            .iter()
            .find(|button| button.source == source && button.code == code)
    }

    fn point(&self, normalized: [f32; 2]) -> (f32, f32) {
        (
            normalized[0] * (self.display.width - 1) as f32,
            normalized[1] * (self.display.height - 1) as f32,
        )
    }
}

fn valid_normalized(point: [f32; 2]) -> bool {
    point
        .iter()
        .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct SourceKey {
    path: PathBuf,
    code: u16,
}

pub(crate) struct TouchMapper {
    profile: Profile,
    active: Vec<Pointer>,
    buttons_held: HashMap<SourceKey, u8>,
    movement_held: HashSet<SourceKey>,
    movement_pointer: Option<u8>,
    camera_pointer: Option<u8>,
    rotation_delta: u32,
}

impl TouchMapper {
    pub(crate) fn new(profile: Profile) -> Self {
        Self {
            profile,
            active: Vec::new(),
            buttons_held: HashMap::new(),
            movement_held: HashSet::new(),
            movement_pointer: None,
            camera_pointer: None,
            rotation_delta: 0,
        }
    }

    pub(crate) fn set_rotation_delta(&mut self, rotation_delta: u32) {
        self.rotation_delta = rotation_delta % 4;
    }

    pub(crate) fn set_display_calibration_rotation(&mut self, rotation: u32) {
        self.profile.set_display_calibration_rotation(rotation);
        self.rotation_delta = 0;
    }

    fn map_point(&self, normalized: [f32; 2]) -> (f32, f32) {
        let mut point = normalized;
        for _ in 0..self.rotation_delta {
            point = [1.0 - point[1], point[0]];
        }
        self.profile.point(point)
    }

    pub(crate) fn camera_drag_start<F>(
        &mut self,
        normalized: [f32; 2],
        send: &mut F,
    ) -> Result<bool, String>
    where
        F: FnMut(Action, u8, Vec<Pointer>) -> Result<(), String>,
    {
        if self.camera_pointer.is_some() {
            return Ok(true);
        }
        if !valid_normalized(normalized) {
            return Err("camera touch position must be normalized to [0, 1]".into());
        }
        let (x, y) = self.map_point(normalized);
        self.camera_pointer = self.add_pointer(x, y, send)?;
        Ok(self.camera_pointer.is_some())
    }

    pub(crate) fn camera_drag_move<F>(
        &mut self,
        normalized: [f32; 2],
        send: &mut F,
    ) -> Result<(), String>
    where
        F: FnMut(Action, u8, Vec<Pointer>) -> Result<(), String>,
    {
        if !valid_normalized(normalized) {
            return Err("camera touch position must be normalized to [0, 1]".into());
        }
        if let Some(id) = self.camera_pointer {
            let (x, y) = self.map_point(normalized);
            self.move_pointer(id, x, y, send)?;
        }
        Ok(())
    }

    pub(crate) fn camera_drag_end<F>(&mut self, send: &mut F) -> Result<(), String>
    where
        F: FnMut(Action, u8, Vec<Pointer>) -> Result<(), String>,
    {
        if let Some(id) = self.camera_pointer.take() {
            self.remove_pointer(id, send)?;
        }
        Ok(())
    }

    pub(crate) fn set_display_geometry<F>(
        &mut self,
        width: u32,
        height: u32,
        rotation_delta: u32,
        send: &mut F,
    ) -> Result<(), String>
    where
        F: FnMut(Action, u8, Vec<Pointer>) -> Result<(), String>,
    {
        let (old_width, old_height) = self.profile.display_size();
        let width = width.max(2);
        let height = height.max(2);
        let rotation_delta = rotation_delta % 4;
        let quarter_turns = (rotation_delta + 4 - self.rotation_delta) % 4;
        if (old_width, old_height) == (width, height) && quarter_turns == 0 {
            return Ok(());
        }

        let mut next_active = self.active.clone();
        for pointer in &mut next_active {
            let mut normalized = [
                pointer.x / (old_width - 1) as f32,
                pointer.y / (old_height - 1) as f32,
            ];
            for _ in 0..quarter_turns {
                normalized = [1.0 - normalized[1], normalized[0]];
            }
            pointer.x = normalized[0] * (width - 1) as f32;
            pointer.y = normalized[1] * (height - 1) as f32;
        }
        if !next_active.is_empty() {
            send(Action::Move, 0, next_active.clone())?;
        }
        self.active = next_active;
        self.profile.set_display_size(width, height);
        self.rotation_delta = rotation_delta;
        Ok(())
    }

    pub(crate) fn layout_revision(&self) -> u64 {
        self.profile.layout_revision
    }

    pub(crate) fn set_layout_revision(&mut self, revision: u64) {
        self.profile.layout_revision = revision;
    }

    pub(crate) fn layout_snapshot(&self) -> LayoutSnapshot {
        self.profile.layout_snapshot()
    }

    pub(crate) fn persist_profile(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        self.profile.persist_atomic(path)
    }

    /// Whether the live profile maps this Linux mouse-button code to a touch
    /// control. Reads the mapper's current profile so Android-side layout
    /// edits are honored immediately instead of the startup snapshot.
    pub(crate) fn is_mapped_mouse_button(&self, code: u16) -> bool {
        self.profile.is_mapped_mouse_button(code)
    }

    pub(crate) fn refresh_movement<F>(&mut self, send: &mut F) -> Result<(), String>
    where
        F: FnMut(Action, u8, Vec<Pointer>) -> Result<(), String>,
    {
        if self.movement_held.is_empty() {
            Ok(())
        } else {
            self.update_movement(send)
        }
    }

    pub(crate) fn apply_layout<F>(
        &mut self,
        layout: LayoutSnapshot,
        send: &mut F,
    ) -> Result<(), String>
    where
        F: FnMut(Action, u8, Vec<Pointer>) -> Result<(), String>,
    {
        if !valid_normalized(layout.movement.center)
            || !layout.movement.radius.is_finite()
            || !(0.001..=0.5).contains(&layout.movement.radius)
        {
            return Err("Android layout contains invalid movement coordinates or radius".into());
        }
        if layout.buttons.len() > MAX_POINTERS - 1 {
            return Err("Android layout exceeds the nine action-button limit".into());
        }

        let next_keys = layout
            .movement
            .keys
            .clone()
            .unwrap_or_else(|| self.profile.movement.keys.clone());
        let movement_codes = [
            next_keys.up,
            next_keys.left,
            next_keys.down,
            next_keys.right,
        ];
        let movement_set = movement_codes.into_iter().collect::<HashSet<_>>();
        if movement_set.len() != 4
            || movement_set
                .iter()
                .any(|code| RESERVED_EMERGENCY_CODES.contains(code))
        {
            return Err("Android layout contains invalid or reserved movement keys".into());
        }

        let existing = self
            .profile
            .buttons
            .iter()
            .map(|button| (button.name.as_str(), button))
            .collect::<HashMap<_, _>>();
        let mut names = HashSet::new();
        let mut triggers = HashSet::new();
        let mut next_buttons = Vec::with_capacity(layout.buttons.len());
        for button in layout.buttons {
            if button.name.trim().is_empty()
                || button.name.chars().count() > MAX_BUTTON_NAME_LENGTH
                || !names.insert(button.name.clone())
                || !valid_normalized(button.position)
            {
                return Err("Android layout contains an invalid or duplicate button".into());
            }
            let previous = existing.get(button.name.as_str()).copied();
            let (source, code) = match (button.source, button.code) {
                (Some(source), Some(code)) => (source, code),
                (None, None) => {
                    let Some(previous) = previous else {
                        return Err(format!(
                            "new Android button '{}' must define its input source and code",
                            button.name
                        ));
                    };
                    (previous.source, previous.code)
                }
                _ => return Err("Android button source and code must be supplied together".into()),
            };
            let valid_code = match source {
                ButtonSource::Key => {
                    code <= 767
                        && !movement_set.contains(&code)
                        && !RESERVED_EMERGENCY_CODES.contains(&code)
                }
                ButtonSource::Mouse => (272..=287).contains(&code),
            };
            if !valid_code || !triggers.insert((source, code)) {
                return Err(format!(
                    "Android layout contains an invalid or duplicate {:?} trigger code {}",
                    source, code
                ));
            }
            next_buttons.push(ButtonConfig {
                name: button.name,
                source,
                code,
                position: button.position,
            });
        }

        let movement_keys_changed = self.profile.movement.keys != next_keys;
        if movement_keys_changed {
            self.movement_held.clear();
            if let Some(id) = self.movement_pointer.take() {
                self.remove_pointer(id, send)?;
            }
        }

        let bindings_changed = self.profile.buttons.len() != next_buttons.len()
            || self.profile.buttons.iter().any(|old| {
                !next_buttons.iter().any(|new| {
                    old.name == new.name && old.source == new.source && old.code == new.code
                })
            });
        if bindings_changed {
            // Never leave a touch held by a key/button that has just been
            // removed or rebound while Android is processing the new schema.
            let held = std::mem::take(&mut self.buttons_held);
            for (_, id) in held {
                self.remove_pointer(id, send)?;
            }
        }

        self.profile.movement.center = layout.movement.center;
        self.profile.movement.radius = layout.movement.radius;
        self.profile.movement.keys = next_keys;
        self.profile.buttons = next_buttons;
        println!(
            "LAYOUT_UPDATED source=android buttons={} movement_keys_changed={movement_keys_changed}",
            self.profile.buttons.len()
        );
        Ok(())
    }

    pub(crate) fn refresh_buttons<F>(&mut self, send: &mut F) -> Result<(), String>
    where
        F: FnMut(Action, u8, Vec<Pointer>) -> Result<(), String>,
    {
        let held = self
            .buttons_held
            .iter()
            .map(|(source_key, id)| (source_key.clone(), *id))
            .collect::<Vec<_>>();
        for (source_key, id) in held {
            let source = if self.profile.is_keyboard(&source_key.path) {
                ButtonSource::Key
            } else if self.profile.is_mouse_device(&source_key.path) {
                ButtonSource::Mouse
            } else {
                continue;
            };
            if let Some(button) = self.profile.button(source, source_key.code) {
                let (x, y) = self.map_point(button.position);
                self.move_pointer(id, x, y, send)?;
            }
        }
        Ok(())
    }

    pub(crate) fn handle_message<F>(
        &mut self,
        message: DeviceMessage,
        send: &mut F,
    ) -> Result<(), String>
    where
        F: FnMut(Action, u8, Vec<Pointer>) -> Result<(), String>,
    {
        match message {
            DeviceMessage::Frame(frame) => self.handle_frame(&frame, send),
            DeviceMessage::Removed { path } => self.release_device(&path, send),
            DeviceMessage::Tick => Ok(()),
        }
    }

    fn handle_frame<F>(&mut self, frame: &InputFrame, send: &mut F) -> Result<(), String>
    where
        F: FnMut(Action, u8, Vec<Pointer>) -> Result<(), String>,
    {
        let source = if self.profile.is_keyboard(&frame.path) {
            ButtonSource::Key
        } else if self.profile.is_mouse_device(&frame.path) {
            ButtonSource::Mouse
        } else {
            return Ok(());
        };

        if frame.resynchronized {
            self.release_device(&frame.path, send)?;
        }

        for event in &frame.events {
            self.handle_raw_event(&frame.path, source, event, send)?;
        }
        Ok(())
    }

    fn handle_raw_event<F>(
        &mut self,
        path: &Path,
        source: ButtonSource,
        event: &RawInputEvent,
        send: &mut F,
    ) -> Result<(), String>
    where
        F: FnMut(Action, u8, Vec<Pointer>) -> Result<(), String>,
    {
        if event.event_type != EV_KEY {
            // Relative mouse motion is handled by MouseLookEngine and the uinput output device.
            return Ok(());
        }
        if event.value != INPUT_DOWN && event.value != INPUT_UP {
            // Ignore autorepeat and non-binary key states.
            return Ok(());
        }

        let source_key = SourceKey {
            path: path.to_path_buf(),
            code: event.code,
        };

        if source == ButtonSource::Key && self.profile.movement_code(event.code) {
            match event.value {
                INPUT_DOWN => {
                    if self.movement_held.insert(source_key) {
                        self.update_movement(send)?;
                    }
                }
                INPUT_UP if self.movement_held.remove(&source_key) => {
                    self.update_movement(send)?;
                }
                _ => {}
            }
            return Ok(());
        }

        let Some(binding) = self.profile.button(source, event.code) else {
            return Ok(());
        };
        let position = binding.position;
        let button_name = binding.name.clone();
        match event.value {
            INPUT_DOWN if !self.buttons_held.contains_key(&source_key) => {
                let (x, y) = self.map_point(position);
                if let Some(id) = self.add_pointer(x, y, send)? {
                    self.buttons_held.insert(source_key, id);
                    println!("MAP_DOWN source={source:?} control={button_name} pointer={id}");
                }
            }
            INPUT_UP => {
                if let Some(id) = self.buttons_held.remove(&source_key) {
                    self.remove_pointer(id, send)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn allocate_pointer_id(&self) -> Option<u8> {
        (0..=31).find(|id| self.active.iter().all(|pointer| pointer.id != *id))
    }

    fn add_pointer<F>(&mut self, x: f32, y: f32, send: &mut F) -> Result<Option<u8>, String>
    where
        F: FnMut(Action, u8, Vec<Pointer>) -> Result<(), String>,
    {
        if self.active.len() >= MAX_POINTERS {
            eprintln!("Touch limit reached; ignoring additional mapped control.");
            return Ok(None);
        }
        let Some(id) = self.allocate_pointer_id() else {
            return Err("no free Android pointer ID".into());
        };
        let pointer = Pointer::finger(id, x, y);
        let mut pointers = self.active.clone();
        pointers.push(pointer);
        let action = if self.active.is_empty() {
            Action::Down
        } else {
            Action::PointerDown
        };
        let index = (pointers.len() - 1) as u8;
        self.transmit(action, index, pointers, send)?;
        Ok(Some(id))
    }

    fn remove_pointer<F>(&mut self, id: u8, send: &mut F) -> Result<(), String>
    where
        F: FnMut(Action, u8, Vec<Pointer>) -> Result<(), String>,
    {
        let Some(index) = self.active.iter().position(|pointer| pointer.id == id) else {
            return Ok(());
        };
        let action = if self.active.len() == 1 {
            Action::Up
        } else {
            Action::PointerUp
        };
        self.transmit(action, index as u8, self.active.clone(), send)
    }

    fn move_pointer<F>(&mut self, id: u8, x: f32, y: f32, send: &mut F) -> Result<(), String>
    where
        F: FnMut(Action, u8, Vec<Pointer>) -> Result<(), String>,
    {
        let mut pointers = self.active.clone();
        let Some(pointer) = pointers.iter_mut().find(|pointer| pointer.id == id) else {
            return Ok(());
        };
        if (pointer.x - x).abs() < 0.01 && (pointer.y - y).abs() < 0.01 {
            return Ok(());
        }
        pointer.x = x;
        pointer.y = y;
        self.transmit(Action::Move, 0, pointers, send)
    }

    fn transmit<F>(
        &mut self,
        action: Action,
        action_index: u8,
        pointers: Vec<Pointer>,
        send: &mut F,
    ) -> Result<(), String>
    where
        F: FnMut(Action, u8, Vec<Pointer>) -> Result<(), String>,
    {
        let frame = Frame {
            sequence: 0,
            action,
            action_index,
            pointers: pointers.clone(),
        };
        let next_active = frame.next_active(&self.active)?;
        send(action, action_index, pointers)?;
        self.active = next_active;
        Ok(())
    }

    fn update_movement<F>(&mut self, send: &mut F) -> Result<(), String>
    where
        F: FnMut(Action, u8, Vec<Pointer>) -> Result<(), String>,
    {
        if self.movement_held.is_empty() {
            if let Some(id) = self.movement_pointer.take() {
                self.remove_pointer(id, send)?;
            }
            return Ok(());
        }

        let up = self
            .movement_held
            .iter()
            .any(|key| key.code == self.profile.movement.keys.up);
        let down = self
            .movement_held
            .iter()
            .any(|key| key.code == self.profile.movement.keys.down);
        let left = self
            .movement_held
            .iter()
            .any(|key| key.code == self.profile.movement.keys.left);
        let right = self
            .movement_held
            .iter()
            .any(|key| key.code == self.profile.movement.keys.right);

        let mut dx = f32::from(right as u8) - f32::from(left as u8);
        let mut dy = f32::from(down as u8) - f32::from(up as u8);
        let length = (dx * dx + dy * dy).sqrt();
        if length > 1.0 {
            dx /= length;
            dy /= length;
        }

        let scale = (self.profile.display.width.min(self.profile.display.height) - 1) as f32;
        let radius = self.profile.movement.radius * scale;
        let center = self.profile.movement.center;
        let x_norm =
            (center[0] + dx * radius / (self.profile.display.width - 1) as f32).clamp(0.0, 1.0);
        let y_norm =
            (center[1] + dy * radius / (self.profile.display.height - 1) as f32).clamp(0.0, 1.0);
        let (x, y) = self.map_point([x_norm, y_norm]);

        match self.movement_pointer {
            Some(id) => self.move_pointer(id, x, y, send),
            None => {
                self.movement_pointer = self.add_pointer(x, y, send)?;
                Ok(())
            }
        }
    }

    fn release_device<F>(&mut self, path: &Path, send: &mut F) -> Result<(), String>
    where
        F: FnMut(Action, u8, Vec<Pointer>) -> Result<(), String>,
    {
        let button_ids = self
            .buttons_held
            .iter()
            .filter(|(source, _)| source.path == path)
            .map(|(source, id)| (source.clone(), *id))
            .collect::<Vec<_>>();
        for (source, id) in button_ids {
            self.remove_pointer(id, send)?;
            self.buttons_held.remove(&source);
        }

        let removed = self
            .movement_held
            .iter()
            .filter(|source| source.path == path)
            .cloned()
            .collect::<Vec<_>>();
        let movement_changed = !removed.is_empty();
        for source in removed {
            self.movement_held.remove(&source);
        }
        if movement_changed {
            self.update_movement(send)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{InputFrame, RawInputEvent};
    use evdev::KeyCode;
    use std::sync::{Arc, Mutex};

    type RecordedFrames = Arc<Mutex<Vec<(Action, u8, Vec<Pointer>)>>>;
    type FrameSink = Box<dyn FnMut(Action, u8, Vec<Pointer>) -> Result<(), String>>;

    fn profile() -> Profile {
        Profile {
            layout_revision: 0,
            display: DisplayConfig {
                width: 1920,
                height: 1042,
                rotation: 0,
            },
            devices: DeviceConfig {
                keyboard: vec![PathBuf::from("/dev/input/event2")],
                mouse: vec![PathBuf::from("/dev/input/event13")],
                suppress: Vec::new(),
            },
            movement: MovementConfig {
                center: [0.16, 0.78],
                radius: 0.07,
                keys: MovementKeys {
                    up: KeyCode::KEY_W.0,
                    left: KeyCode::KEY_A.0,
                    down: KeyCode::KEY_S.0,
                    right: KeyCode::KEY_D.0,
                },
            },
            mouse: MouseConfig::default(),
            buttons: vec![
                ButtonConfig {
                    name: "jump".into(),
                    source: ButtonSource::Key,
                    code: KeyCode::KEY_SPACE.0,
                    position: [0.83, 0.81],
                },
                ButtonConfig {
                    name: "fire".into(),
                    source: ButtonSource::Mouse,
                    code: 272,
                    position: [0.88, 0.56],
                },
            ],
        }
    }

    fn key_frame(path: &str, code: u16, value: i32) -> InputFrame {
        InputFrame {
            path: PathBuf::from(path),
            device_name: "test".into(),
            timestamp_micros: 0,
            resynchronized: false,
            events: vec![RawInputEvent {
                event_type: EV_KEY,
                code,
                value,
            }],
        }
    }

    fn capture() -> (RecordedFrames, FrameSink) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&events);
        let collector: FrameSink = Box::new(move |action, index, pointers| {
            captured.lock().unwrap().push((action, index, pointers));
            Ok(())
        });
        (events, collector)
    }

    #[test]
    fn legacy_profiles_default_layout_revision_to_zero() {
        let mut value = serde_json::to_value(profile()).unwrap();
        value.as_object_mut().unwrap().remove("layout_revision");
        let loaded: Profile = serde_json::from_value(value).unwrap();
        assert_eq!(loaded.layout_revision, 0);
    }

    #[test]
    fn profile_persists_layout_revision_and_positions_atomically() {
        let mut original = profile();
        original.layout_revision = 42;
        original.movement.center = [0.23, 0.74];
        let path = std::env::temp_dir().join(format!(
            "waytouch-profile-{}-revision.json",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);

        original.persist_atomic(&path).unwrap();
        let restored = Profile::load(&path).unwrap();
        assert_eq!(restored.layout_revision, 42);
        assert_eq!(restored.movement.center, [0.23, 0.74]);
        assert_eq!(restored.layout_snapshot(), original.layout_snapshot());

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn android_layout_change_repositions_an_active_movement_pointer() {
        let mut mapper = TouchMapper::new(profile());
        let (events, mut send) = capture();
        mapper
            .handle_frame(
                &key_frame("/dev/input/event2", KeyCode::KEY_W.0, 1),
                &mut send,
            )
            .unwrap();
        let before = mapper.active[0];

        let layout: LayoutSnapshot = serde_json::from_value(serde_json::json!({
            "movement": { "center": [0.32, 0.38], "radius": 0.14 },
            "buttons": [
                { "name": "jump", "position": [0.75, 0.82] },
                { "name": "fire", "position": [0.91, 0.61] }
            ]
        }))
        .unwrap();
        mapper.apply_layout(layout, &mut send).unwrap();
        mapper.refresh_movement(&mut send).unwrap();

        let frames = events.lock().unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1].0, Action::Move);
        assert_ne!([before.x, before.y], [frames[1].2[0].x, frames[1].2[0].y]);
    }

    #[test]
    fn camera_drag_shares_the_ordered_multitouch_set_with_game_controls() {
        let mut mapper = TouchMapper::new(profile());
        let (events, mut send) = capture();
        assert!(mapper.camera_drag_start([0.72, 0.48], &mut send).unwrap());
        assert_eq!(mapper.active.len(), 1);
        let camera_id = mapper.camera_pointer.unwrap();

        mapper.camera_drag_move([0.76, 0.52], &mut send).unwrap();
        assert_eq!(mapper.active[0].id, camera_id);
        assert!((mapper.active[0].x - 0.76 * 1919.0).abs() < 0.01);
        assert!((mapper.active[0].y - 0.52 * 1041.0).abs() < 0.01);

        mapper
            .handle_frame(
                &key_frame("/dev/input/event2", KeyCode::KEY_SPACE.0, 1),
                &mut send,
            )
            .unwrap();
        assert_eq!(mapper.active.len(), 2);
        mapper.camera_drag_end(&mut send).unwrap();
        assert_eq!(mapper.active.len(), 1);
        assert_ne!(mapper.active[0].id, camera_id);
        mapper
            .handle_frame(
                &key_frame("/dev/input/event2", KeyCode::KEY_SPACE.0, 0),
                &mut send,
            )
            .unwrap();
        assert!(mapper.active.is_empty());
        assert!(mapper.camera_pointer.is_none());

        let frames = events.lock().unwrap();
        assert_eq!(frames[0].0, Action::Down);
        assert_eq!(frames[1].0, Action::Move);
        assert_eq!(frames[2].0, Action::PointerDown);
        assert_eq!(frames[3].0, Action::PointerUp);
        assert_eq!(frames[4].0, Action::Up);
    }

    #[test]
    fn android_layout_can_add_remove_and_rebind_controls_without_stuck_touches() {
        let mut mapper = TouchMapper::new(profile());
        let (events, mut send) = capture();

        mapper
            .handle_frame(
                &key_frame("/dev/input/event2", KeyCode::KEY_W.0, 1),
                &mut send,
            )
            .unwrap();
        mapper
            .handle_frame(
                &key_frame("/dev/input/event2", KeyCode::KEY_SPACE.0, 1),
                &mut send,
            )
            .unwrap();
        assert_eq!(mapper.active.len(), 2);

        let layout: LayoutSnapshot = serde_json::from_value(serde_json::json!({
            "movement": {
                "center": [0.21, 0.77],
                "radius": 0.08,
                "keys": { "up": 103, "left": 105, "down": 108, "right": 106 }
            },
            "buttons": [
                {
                    "name": "jump",
                    "position": [0.74, 0.82],
                    "source": "key",
                    "code": 18
                },
                {
                    "name": "aim",
                    "position": [0.92, 0.61],
                    "source": "mouse",
                    "code": 273
                }
            ]
        }))
        .unwrap();

        mapper.apply_layout(layout, &mut send).unwrap();
        assert!(mapper.active.is_empty());
        assert!(mapper.buttons_held.is_empty());
        assert!(mapper.movement_held.is_empty());
        assert_eq!(mapper.profile.movement.keys.up, 103);
        assert_eq!(mapper.profile.buttons.len(), 2);
        assert_eq!(mapper.profile.buttons[0].code, 18);
        assert_eq!(mapper.profile.buttons[1].name, "aim");

        mapper
            .handle_frame(&key_frame("/dev/input/event2", 18, 1), &mut send)
            .unwrap();
        mapper
            .handle_frame(&key_frame("/dev/input/event13", 273, 1), &mut send)
            .unwrap();
        assert_eq!(mapper.active.len(), 2);

        mapper
            .handle_frame(&key_frame("/dev/input/event2", 18, 0), &mut send)
            .unwrap();
        mapper
            .handle_frame(&key_frame("/dev/input/event13", 273, 0), &mut send)
            .unwrap();
        assert!(mapper.active.is_empty());

        let frames = events.lock().unwrap();
        assert_eq!(frames[2].0, Action::PointerUp);
        assert_eq!(frames[3].0, Action::Up);
    }

    #[test]
    fn android_layout_updates_positions_and_rejects_unknown_buttons_atomically() {
        let mut mapper = TouchMapper::new(profile());
        let layout: LayoutSnapshot = serde_json::from_value(serde_json::json!({
            "movement": { "center": [0.22, 0.72], "radius": 0.09 },
            "buttons": [
                { "name": "jump", "position": [0.75, 0.82] },
                { "name": "fire", "position": [0.91, 0.61] }
            ]
        }))
        .unwrap();
        let (_, mut send) = capture();
        mapper.apply_layout(layout, &mut send).unwrap();
        assert_eq!(mapper.profile.movement.center, [0.22, 0.72]);
        assert_eq!(mapper.profile.movement.radius, 0.09);
        assert_eq!(mapper.profile.buttons[0].position, [0.75, 0.82]);
        assert_eq!(mapper.profile.buttons[1].position, [0.91, 0.61]);

        let invalid: LayoutSnapshot = serde_json::from_value(serde_json::json!({
            "movement": { "center": [0.3, 0.7], "radius": 0.1 },
            "buttons": [
                { "name": "jump", "position": [0.1, 0.1] },
                { "name": "unknown", "position": [0.9, 0.9] }
            ]
        }))
        .unwrap();
        assert!(mapper.apply_layout(invalid, &mut send).is_err());
        assert_eq!(mapper.profile.movement.center, [0.22, 0.72]);
        assert_eq!(mapper.profile.buttons[0].position, [0.75, 0.82]);
    }

    #[test]
    fn wasd_uses_one_pointer_and_moves_diagonally() {
        let mut mapper = TouchMapper::new(profile());
        let (events, mut send) = capture();
        mapper
            .handle_frame(
                &key_frame("/dev/input/event2", KeyCode::KEY_W.0, 1),
                &mut send,
            )
            .unwrap();
        mapper
            .handle_frame(
                &key_frame("/dev/input/event2", KeyCode::KEY_D.0, 1),
                &mut send,
            )
            .unwrap();
        mapper
            .handle_frame(
                &key_frame("/dev/input/event2", KeyCode::KEY_W.0, 0),
                &mut send,
            )
            .unwrap();
        mapper
            .handle_frame(
                &key_frame("/dev/input/event2", KeyCode::KEY_D.0, 0),
                &mut send,
            )
            .unwrap();

        let events = events.lock().unwrap();
        assert_eq!(events.len(), 4);
        assert_eq!(events[0].0, Action::Down);
        assert_eq!(events[1].0, Action::Move);
        assert_eq!(events[2].0, Action::Move);
        assert_eq!(events[3].0, Action::Up);
        assert!(mapper.active.is_empty());
    }

    #[test]
    fn action_buttons_are_independent_simultaneous_pointers() {
        let mut mapper = TouchMapper::new(profile());
        let (events, mut send) = capture();
        mapper
            .handle_frame(
                &key_frame("/dev/input/event2", KeyCode::KEY_SPACE.0, 1),
                &mut send,
            )
            .unwrap();
        mapper
            .handle_frame(&key_frame("/dev/input/event13", 272, 1), &mut send)
            .unwrap();
        assert_eq!(mapper.active.len(), 2);
        mapper
            .handle_frame(
                &key_frame("/dev/input/event2", KeyCode::KEY_SPACE.0, 0),
                &mut send,
            )
            .unwrap();
        mapper
            .handle_frame(&key_frame("/dev/input/event13", 272, 0), &mut send)
            .unwrap();
        assert!(mapper.active.is_empty());

        let events = events.lock().unwrap();
        assert_eq!(events[0].0, Action::Down);
        assert_eq!(events[1].0, Action::PointerDown);
        assert_eq!(events[2].0, Action::PointerUp);
        assert_eq!(events[3].0, Action::Up);
    }

    #[test]
    fn key_autorepeat_does_not_create_extra_fingers() {
        let mut mapper = TouchMapper::new(profile());
        let (events, mut send) = capture();
        mapper
            .handle_raw_event(
                Path::new("/dev/input/event2"),
                ButtonSource::Key,
                &RawInputEvent {
                    event_type: EV_KEY,
                    code: KeyCode::KEY_SPACE.0,
                    value: 1,
                },
                &mut send,
            )
            .unwrap();
        mapper
            .handle_raw_event(
                Path::new("/dev/input/event2"),
                ButtonSource::Key,
                &RawInputEvent {
                    event_type: EV_KEY,
                    code: KeyCode::KEY_SPACE.0,
                    value: 2,
                },
                &mut send,
            )
            .unwrap();
        assert_eq!(mapper.active.len(), 1);
        assert_eq!(events.lock().unwrap().len(), 1);
    }

    #[test]
    fn movement_and_action_buttons_share_one_ordered_pointer_set() {
        let mut mapper = TouchMapper::new(profile());
        let (events, mut send) = capture();

        for (path, code, value) in [
            ("/dev/input/event2", KeyCode::KEY_W.0, 1),
            ("/dev/input/event2", KeyCode::KEY_SPACE.0, 1),
            ("/dev/input/event13", 272, 1),
            ("/dev/input/event2", KeyCode::KEY_W.0, 0),
            ("/dev/input/event2", KeyCode::KEY_SPACE.0, 0),
            ("/dev/input/event13", 272, 0),
        ] {
            mapper
                .handle_frame(&key_frame(path, code, value), &mut send)
                .unwrap();
        }

        let events = events.lock().unwrap();
        let actions = events.iter().map(|event| event.0).collect::<Vec<_>>();
        assert_eq!(
            actions,
            vec![
                Action::Down,
                Action::PointerDown,
                Action::PointerDown,
                Action::PointerUp,
                Action::PointerUp,
                Action::Up
            ]
        );
        assert!(mapper.active.is_empty());
    }

    #[test]
    fn keyboard_and_mouse_bindings_do_not_cross_trigger() {
        let mut mapper = TouchMapper::new(profile());
        let (events, mut send) = capture();

        mapper
            .handle_frame(
                &key_frame("/dev/input/event13", KeyCode::KEY_SPACE.0, 1),
                &mut send,
            )
            .unwrap();
        mapper
            .handle_frame(&key_frame("/dev/input/event2", 272, 1), &mut send)
            .unwrap();

        assert!(mapper.active.is_empty());
        assert!(events.lock().unwrap().is_empty());
    }

    #[test]
    fn profile_accepts_stable_udev_event_symlinks() {
        assert!(is_event_node(Path::new(
            "/dev/input/by-id/usb-15ca_USB_Optical_Mouse-event-mouse"
        )));
        assert!(is_event_node(Path::new(
            "/dev/input/by-path/platform-i8042-serio-0-event-kbd"
        )));
        assert!(is_event_node(Path::new(
            "/dev/input/by-path/platform-AMDI0010:03-event-mouse"
        )));
        assert!(!is_event_node(Path::new(
            "/dev/input/by-id/usb-15ca_USB_Optical_Mouse-mouse"
        )));
    }

    #[test]
    fn profile_button_name_limit_counts_unicode_characters() {
        let mut valid = profile();
        valid.buttons[0].name = "ی".repeat(MAX_BUTTON_NAME_LENGTH);
        assert!(valid.validate().is_ok());

        let mut invalid = profile();
        invalid.buttons[0].name = "ی".repeat(MAX_BUTTON_NAME_LENGTH + 1);
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn profile_reserves_f1_f2_f3_from_movement_and_action_bindings() {
        let mut movement_profile = profile();
        movement_profile.movement.keys.up = KeyCode::KEY_F2.0;
        assert!(movement_profile.validate().is_err());

        let mut action_profile = profile();
        action_profile.buttons[0].source = ButtonSource::Key;
        action_profile.buttons[0].code = KeyCode::KEY_F2.0;
        assert!(action_profile.validate().is_err());
        let mut f1_movement = profile();
        f1_movement.movement.keys.up = KeyCode::KEY_F1.0;
        assert!(f1_movement.validate().is_err());

        let mut f1_action = profile();
        f1_action.buttons[0].code = KeyCode::KEY_F1.0;
        assert!(f1_action.validate().is_err());

        let mut f3_movement = profile();
        f3_movement.movement.keys.up = KeyCode::KEY_F3.0;
        assert!(f3_movement.validate().is_err());

        let mut f3_action = profile();
        f3_action.buttons[0].code = KeyCode::KEY_F3.0;
        assert!(f3_action.validate().is_err());
    }

    #[test]
    fn profile_rejects_out_of_range_touch_coordinates() {
        let mut configured = profile();
        configured.buttons[0].position = [1.01, 0.5];
        assert!(configured.validate().is_err());
    }

    #[test]
    fn disconnected_device_releases_its_held_controls() {
        let mut mapper = TouchMapper::new(profile());
        let (events, mut send) = capture();
        mapper
            .handle_frame(
                &key_frame("/dev/input/event2", KeyCode::KEY_W.0, 1),
                &mut send,
            )
            .unwrap();
        mapper
            .handle_frame(
                &key_frame("/dev/input/event2", KeyCode::KEY_SPACE.0, 1),
                &mut send,
            )
            .unwrap();
        mapper
            .handle_message(
                DeviceMessage::Removed {
                    path: PathBuf::from("/dev/input/event2"),
                },
                &mut send,
            )
            .unwrap();
        assert!(mapper.active.is_empty());
        let events = events.lock().unwrap();
        assert_eq!(events.last().unwrap().0, Action::Up);
        assert!(events.iter().any(|event| event.0 == Action::PointerUp));
    }
    #[test]
    fn fresh_profile_template_can_be_validated_before_discovery_but_is_not_runnable() {
        let mut template = profile();
        template.devices.keyboard.clear();
        template.devices.mouse.clear();

        assert!(template.validate().is_ok());
        assert!(!template.has_required_devices());
        assert!(
            template
                .replace_device_paths(
                    vec![PathBuf::from("/dev/input/by-path/test-keyboard-event-kbd")],
                    vec![PathBuf::from("/dev/input/by-id/test-mouse-event-mouse")],
                    Vec::new(),
                )
                .is_ok()
        );
        assert!(template.has_required_devices());
    }
    #[test]
    fn live_display_resize_rescales_active_touch_points_without_releasing_them() {
        let mut mapper = TouchMapper::new(profile());
        let mut emitted = Vec::<(Action, Vec<Pointer>)>::new();
        mapper
            .camera_drag_start([0.75, 0.25], &mut |action, _index, pointers| {
                emitted.push((action, pointers));
                Ok(())
            })
            .unwrap();
        assert_eq!(mapper.active.len(), 1);
        let original = mapper.active[0];
        assert!((original.x / 1919.0 - 0.75).abs() < 0.0001);
        assert!((original.y / 1041.0 - 0.25).abs() < 0.0001);

        mapper
            .set_display_geometry(1042, 1920, 0, &mut |action, _index, pointers| {
                emitted.push((action, pointers));
                Ok(())
            })
            .unwrap();

        assert_eq!(mapper.active.len(), 1);
        let resized = mapper.active[0];
        assert!((resized.x / 1041.0 - 0.75).abs() < 0.0001);
        assert!((resized.y / 1919.0 - 0.25).abs() < 0.0001);
        assert_eq!(emitted.last().unwrap().0, Action::Move);
        assert_eq!(emitted.last().unwrap().1, mapper.active);
    }
    #[test]
    fn display_rotation_rotates_new_and_active_touch_points_clockwise() {
        let mut mapper = TouchMapper::new(profile());
        mapper.set_rotation_delta(0);
        let mut emitted = Vec::<(Action, Vec<Pointer>)>::new();
        mapper
            .camera_drag_start([0.2, 0.3], &mut |action, _index, pointers| {
                emitted.push((action, pointers));
                Ok(())
            })
            .unwrap();
        let before = mapper.active[0];
        assert!((before.x / 1919.0 - 0.2).abs() < 0.0001);
        assert!((before.y / 1041.0 - 0.3).abs() < 0.0001);

        mapper
            .set_display_geometry(1080, 1920, 1, &mut |action, _index, pointers| {
                emitted.push((action, pointers));
                Ok(())
            })
            .unwrap();
        let rotated = mapper.active[0];
        assert!((rotated.x / 1079.0 - 0.7).abs() < 0.0001);
        assert!((rotated.y / 1919.0 - 0.2).abs() < 0.0001);

        mapper
            .camera_drag_end(&mut |action, _index, pointers| {
                emitted.push((action, pointers));
                Ok(())
            })
            .unwrap();
        mapper
            .camera_drag_start([0.2, 0.3], &mut |action, _index, pointers| {
                emitted.push((action, pointers));
                Ok(())
            })
            .unwrap();
        let next = mapper.active.last().unwrap();
        assert!((next.x / 1079.0 - 0.7).abs() < 0.0001);
        assert!((next.y / 1919.0 - 0.2).abs() < 0.0001);
    }
}
