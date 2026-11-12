use crate::input::InputFrame;
use evdev::{EventType, RelativeAxisCode};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MouseFrame {
    pub(crate) action: u8,
    pub(crate) buttons: u16,
    pub(crate) action_button: u16,
    pub(crate) relative_x: f32,
    pub(crate) relative_y: f32,
    pub(crate) horizontal_scroll: f32,
    pub(crate) vertical_scroll: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum TouchDragEvent {
    Down([f32; 2]),
    Move([f32; 2]),
    Up,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MouseLookMode {
    #[default]
    RelativeMouse,
    TouchDrag,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AimActivationMode {
    #[default]
    Hold,
    Toggle,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct MouseAccelerationConfig {
    pub(crate) enabled: bool,
    /// Input velocity in raw mouse counts per millisecond before acceleration starts.
    pub(crate) threshold: f32,
    /// Maximum additional gain, clamped to a safe and predictable range.
    pub(crate) max_gain: f32,
    /// Power-curve exponent. Values below 1 accelerate earlier; values above 1 later.
    pub(crate) exponent: f32,
}

impl Default for MouseAccelerationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold: 1.2,
            max_gain: 2.5,
            exponent: 1.35,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MouseCursorStyle {
    #[default]
    Crosshair,
    Pointer,
    Dot,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct MouseCursorConfig {
    pub(crate) enabled: bool,
    pub(crate) style: MouseCursorStyle,
    pub(crate) size: f32,
    pub(crate) color: String,
    pub(crate) hide_while_aiming: bool,
    /// Retained only to deserialize older profile files; it has no runtime effect.
    #[serde(default, skip_serializing)]
    #[allow(dead_code)]
    pub(crate) lock_host_cursor: bool,
}

impl Default for MouseCursorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            style: MouseCursorStyle::Crosshair,
            size: 28.0,
            color: "#ff3158".into(),
            hide_while_aiming: true,
            lock_host_cursor: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct MouseConfig {
    pub(crate) enabled: bool,
    pub(crate) cursor_speed: f32,
    pub(crate) native_buttons: bool,
    pub(crate) cursor: MouseCursorConfig,
    pub(crate) look: MouseLookConfig,
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cursor_speed: 1.0,
            native_buttons: true,
            cursor: MouseCursorConfig::default(),
            look: MouseLookConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct MouseLookConfig {
    pub(crate) enabled: bool,
    pub(crate) mode: MouseLookMode,
    /// Deprecated compatibility keys: mouse buttons never toggle camera lock.
    pub(crate) hold_button: u16,
    pub(crate) activation_mode: AimActivationMode,
    pub(crate) hold_button_passthrough: bool,
    pub(crate) sensitivity_x: f32,
    pub(crate) sensitivity_y: f32,
    pub(crate) invert_y: bool,
    pub(crate) smoothing: f32,
    pub(crate) deadzone: f32,
    /// Maximum emitted TouchDrag MOVE rate; relative mouse is never capped.
    pub(crate) rate_cap_hz: u32,
    pub(crate) acceleration: MouseAccelerationConfig,
    /// Initial normalized position for the Android camera finger.
    pub(crate) touch_anchor: [f32; 2],
    /// Normalized [left, top, right, bottom] bounds for the camera finger.
    pub(crate) touch_bounds: [f32; 4],
    /// Android display pixels moved per raw mouse count, before X/Y sensitivity.
    pub(crate) touch_scale: f32,
    pub(crate) edge_recenter: bool,
}

impl Default for MouseLookConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: MouseLookMode::RelativeMouse,
            hold_button: BTN_RIGHT,
            activation_mode: AimActivationMode::Hold,
            hold_button_passthrough: true,
            sensitivity_x: 1.0,
            sensitivity_y: 1.0,
            invert_y: false,
            smoothing: 0.0,
            deadzone: 0.0,
            rate_cap_hz: 240,
            acceleration: MouseAccelerationConfig::default(),
            touch_anchor: [0.76, 0.55],
            touch_bounds: [0.52, 0.06, 0.98, 0.94],
            touch_scale: 1.0,
            edge_recenter: true,
        }
    }
}

const EV_KEY: u16 = EventType::KEY.0;
const EV_REL: u16 = EventType::RELATIVE.0;
const REL_X: u16 = RelativeAxisCode::REL_X.0;
const REL_Y: u16 = RelativeAxisCode::REL_Y.0;
const REL_HWHEEL: u16 = RelativeAxisCode::REL_HWHEEL.0;
const REL_WHEEL: u16 = RelativeAxisCode::REL_WHEEL.0;

const BTN_LEFT: u16 = 272;
const BTN_RIGHT: u16 = 273;
const BTN_MIDDLE: u16 = 274;
const BTN_SIDE: u16 = 275;
const BTN_EXTRA: u16 = 276;

const BUTTON_PRIMARY: u16 = 1;
const BUTTON_SECONDARY: u16 = 2;
const BUTTON_TERTIARY: u16 = 4;
const BUTTON_BACK: u16 = 8;
const BUTTON_FORWARD: u16 = 16;

const ACTION_CANCEL: u8 = 3;
const ACTION_HOVER_MOVE: u8 = 7;
const ACTION_HOVER_ENTER: u8 = 9;
const ACTION_SCROLL: u8 = 8;
const ACTION_BUTTON_PRESS: u8 = 11;
const ACTION_BUTTON_RELEASE: u8 = 12;

impl MouseConfig {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if !self.cursor_speed.is_finite() || !(0.1..=5.0).contains(&self.cursor_speed) {
            return Err("mouse.cursor_speed must be finite and in [0.1, 5.0]".into());
        }
        for (name, value) in [
            ("sensitivity_x", self.look.sensitivity_x),
            ("sensitivity_y", self.look.sensitivity_y),
        ] {
            if !value.is_finite() || !(0.01..=10.0).contains(&value) {
                return Err(format!(
                    "mouse.look.{name} must be finite and in [0.01, 10]"
                ));
            }
        }
        if !self.look.smoothing.is_finite() || !(0.0..=0.95).contains(&self.look.smoothing) {
            return Err("mouse.look.smoothing must be finite and in [0, 0.95]".into());
        }
        if !self.look.deadzone.is_finite() || !(0.0..=8.0).contains(&self.look.deadzone) {
            return Err("mouse.look.deadzone must be finite and in [0, 8] counts".into());
        }
        if !(30..=1000).contains(&self.look.rate_cap_hz) {
            return Err("mouse.look.rate_cap_hz must be in [30, 1000] Hz".into());
        }
        // Legacy hold_button/activation_mode fields are parsed for compatibility only.
        // The active camera lock is controlled exclusively by the F3 key path.
        let acceleration = &self.look.acceleration;
        if !acceleration.threshold.is_finite() || !(0.05..=20.0).contains(&acceleration.threshold) {
            return Err(
                "mouse.look.acceleration.threshold must be finite and in [0.05, 20]".into(),
            );
        }
        if !acceleration.max_gain.is_finite() || !(1.0..=5.0).contains(&acceleration.max_gain) {
            return Err("mouse.look.acceleration.max_gain must be finite and in [1, 5]".into());
        }
        if !acceleration.exponent.is_finite() || !(0.25..=3.0).contains(&acceleration.exponent) {
            return Err("mouse.look.acceleration.exponent must be finite and in [0.25, 3]".into());
        }
        if !self.look.touch_scale.is_finite() || !(0.1..=20.0).contains(&self.look.touch_scale) {
            return Err("mouse.look.touch_scale must be finite and in [0.1, 20]".into());
        }
        if !self
            .look
            .touch_anchor
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
        {
            return Err("mouse.look.touch_anchor must be normalized to [0, 1]".into());
        }
        let [left, top, right, bottom] = self.look.touch_bounds;
        if ![left, top, right, bottom]
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
            || left >= right
            || top >= bottom
        {
            return Err("mouse.look.touch_bounds must be an ordered normalized rectangle".into());
        }
        if self.look.touch_anchor[0] < left
            || self.look.touch_anchor[0] > right
            || self.look.touch_anchor[1] < top
            || self.look.touch_anchor[1] > bottom
        {
            return Err("mouse.look.touch_anchor must lie inside touch_bounds".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct MouseAxes {
    relative_x: f32,
    relative_y: f32,
    horizontal_scroll: f32,
    vertical_scroll: f32,
}

pub(crate) struct MouseLookEngine {
    config: MouseConfig,
    buttons: u16,
    filtered_x: f32,
    filtered_y: f32,
    hover_entered: bool,
    display_width: u32,
    display_height: u32,
    rotation_delta: u32,
    touch_active: bool,
    touch_position: [f32; 2],
    manual_lock: bool,
    last_motion_micros: Option<u128>,
    queued_touch_dx: f32,
    queued_touch_dy: f32,
    last_touch_emit_micros: Option<u128>,
}

impl MouseLookEngine {
    #[cfg(test)]
    pub(crate) fn new(config: MouseConfig) -> Self {
        Self::with_display(config, 1920, 1080)
    }

    pub(crate) fn with_display(config: MouseConfig, width: u32, height: u32) -> Self {
        let touch_position = config.look.touch_anchor;
        Self {
            config,
            buttons: 0,
            filtered_x: 0.0,
            filtered_y: 0.0,
            hover_entered: false,
            display_width: width.max(2),
            display_height: height.max(2),
            rotation_delta: 0,
            touch_active: false,
            touch_position,
            manual_lock: false,
            last_motion_micros: None,
            queued_touch_dx: 0.0,
            queued_touch_dy: 0.0,
            last_touch_emit_micros: None,
        }
    }

    pub(crate) fn set_display_geometry(&mut self, width: u32, height: u32, rotation_delta: u32) {
        self.display_width = width.max(2);
        self.display_height = height.max(2);
        self.rotation_delta = rotation_delta % 4;
        self.filtered_x = 0.0;
        self.filtered_y = 0.0;
        self.last_motion_micros = None;
        self.queued_touch_dx = 0.0;
        self.queued_touch_dy = 0.0;
        self.last_touch_emit_micros = None;
    }

    fn look_active(&self) -> bool {
        self.config.look.enabled && self.manual_lock
    }

    pub(crate) fn is_aiming(&self) -> bool {
        self.look_active()
    }

    /// Set Aim from the dedicated F3 hotkey. Mouse buttons never change this state.
    pub(crate) fn set_manual_lock<T>(
        &mut self,
        locked: bool,
        send_touch: &mut T,
    ) -> Result<(), String>
    where
        T: FnMut(TouchDragEvent) -> Result<(), String>,
    {
        if !self.config.look.enabled {
            self.manual_lock = false;
            return Ok(());
        }

        let was_active = self.look_active();
        self.manual_lock = locked;
        let is_active = self.look_active();

        if was_active != is_active {
            self.filtered_x = 0.0;
            self.filtered_y = 0.0;
            self.last_motion_micros = None;
            self.queued_touch_dx = 0.0;
            self.queued_touch_dy = 0.0;
            self.last_touch_emit_micros = None;

            if self.config.look.mode == MouseLookMode::TouchDrag {
                if is_active && !self.touch_active {
                    self.touch_position = self.config.look.touch_anchor;
                    self.touch_active = true;
                    send_touch(TouchDragEvent::Down(self.touch_position))?;
                } else if !is_active {
                    self.release_touch(send_touch)?;
                }
            }
        }
        Ok(())
    }

    fn forwarded_buttons(&self) -> u16 {
        self.buttons
    }

    fn emit<F>(
        &mut self,
        action: u8,
        action_button: u16,
        axes: MouseAxes,
        send: &mut F,
    ) -> Result<(), String>
    where
        F: FnMut(MouseFrame) -> Result<(), String>,
    {
        if !self.config.enabled {
            return Ok(());
        }
        let frame = MouseFrame {
            action,
            buttons: if self.config.native_buttons {
                self.forwarded_buttons()
            } else {
                0
            },
            action_button,
            relative_x: axes.relative_x,
            relative_y: axes.relative_y,
            horizontal_scroll: axes.horizontal_scroll,
            vertical_scroll: axes.vertical_scroll,
        };
        send(frame)
    }

    #[cfg(test)]
    pub(crate) fn handle_frame<F>(
        &mut self,
        frame: &InputFrame,
        send_mouse: &mut F,
    ) -> Result<(), String>
    where
        F: FnMut(MouseFrame) -> Result<(), String>,
    {
        self.handle_frame_with_touch(frame, send_mouse, &mut |_| Ok(()))
    }

    pub(crate) fn handle_frame_with_touch<F, T>(
        &mut self,
        frame: &InputFrame,
        send_mouse: &mut F,
        send_touch: &mut T,
    ) -> Result<(), String>
    where
        F: FnMut(MouseFrame) -> Result<(), String>,
        T: FnMut(TouchDragEvent) -> Result<(), String>,
    {
        if frame.resynchronized {
            self.release_touch(send_touch)?;
            self.manual_lock = false;
            if self.buttons != 0 {
                self.buttons = 0;
                self.emit(ACTION_CANCEL, 0, MouseAxes::default(), send_mouse)?;
            }
            self.filtered_x = 0.0;
            self.filtered_y = 0.0;
            self.last_motion_micros = None;
            self.queued_touch_dx = 0.0;
            self.queued_touch_dy = 0.0;
            self.last_touch_emit_micros = None;
        }

        let mut pending_x = 0.0f32;
        let mut pending_y = 0.0f32;
        let mut horizontal_scroll = 0.0f32;
        let mut vertical_scroll = 0.0f32;

        for event in &frame.events {
            if event.event_type == EV_REL {
                match event.code {
                    REL_X => pending_x += event.value as f32,
                    REL_Y => pending_y += event.value as f32,
                    REL_HWHEEL => horizontal_scroll += event.value as f32,
                    REL_WHEEL => vertical_scroll += event.value as f32,
                    _ => {}
                }
                continue;
            }

            if event.event_type == EV_KEY
                && let Some(bit) = linux_button_bit(event.code)
                && (event.value == 0 || event.value == 1)
            {
                self.flush_motion(
                    &mut pending_x,
                    &mut pending_y,
                    frame.timestamp_micros,
                    send_mouse,
                    send_touch,
                )?;
                let was_pressed = self.buttons & bit != 0;
                let pressed = event.value == 1;
                if was_pressed == pressed {
                    continue;
                }

                if pressed {
                    self.buttons |= bit;
                } else {
                    self.buttons &= !bit;
                }

                if self.config.native_buttons {
                    self.ensure_hover(send_mouse)?;
                    let action = if pressed {
                        ACTION_BUTTON_PRESS
                    } else {
                        ACTION_BUTTON_RELEASE
                    };
                    self.emit(action, bit, MouseAxes::default(), send_mouse)?;
                }
            }
        }

        self.flush_motion(
            &mut pending_x,
            &mut pending_y,
            frame.timestamp_micros,
            send_mouse,
            send_touch,
        )?;

        if horizontal_scroll != 0.0 || vertical_scroll != 0.0 {
            self.ensure_hover(send_mouse)?;
            self.emit(
                ACTION_SCROLL,
                0,
                MouseAxes {
                    horizontal_scroll: horizontal_scroll.clamp(-1000.0, 1000.0),
                    vertical_scroll: vertical_scroll.clamp(-1000.0, 1000.0),
                    ..MouseAxes::default()
                },
                send_mouse,
            )?;
        }
        Ok(())
    }

    fn ensure_hover<F>(&mut self, send: &mut F) -> Result<(), String>
    where
        F: FnMut(MouseFrame) -> Result<(), String>,
    {
        if self.config.enabled && !self.hover_entered {
            self.emit(ACTION_HOVER_ENTER, 0, MouseAxes::default(), send)?;
            self.hover_entered = true;
        }
        Ok(())
    }

    fn acceleration_gain(&self, dx: f32, dy: f32, elapsed_ms: f32) -> f32 {
        let curve = &self.config.look.acceleration;
        if !curve.enabled {
            return 1.0;
        }
        let speed = dx.hypot(dy) / elapsed_ms.max(0.25);
        let ramp = ((speed - curve.threshold) / (curve.threshold * 3.0)).clamp(0.0, 1.0);
        1.0 + (curve.max_gain - 1.0) * ramp.powf(curve.exponent)
    }

    fn filtered_look_delta(&mut self, dx: f32, dy: f32, timestamp_micros: u128) -> (f32, f32) {
        let elapsed_ms = self
            .last_motion_micros
            .map(|last| timestamp_micros.saturating_sub(last) as f32 / 1000.0)
            .unwrap_or(8.0)
            .max(0.25);
        self.last_motion_micros = Some(timestamp_micros);

        let gain = self.acceleration_gain(dx, dy, elapsed_ms);
        // deadzone is specified in raw hardware counts (not scaled pixels).
        let dx = if dx.abs() < self.config.look.deadzone {
            0.0
        } else {
            dx * self.config.look.sensitivity_x * gain
        };
        let mut dy = if dy.abs() < self.config.look.deadzone {
            0.0
        } else {
            dy * self.config.look.sensitivity_y * gain
        };
        if self.config.look.invert_y {
            dy = -dy;
        }

        let smoothing = self.config.look.smoothing;
        let weight = 1.0 - smoothing;
        self.filtered_x = self.filtered_x * smoothing + dx * weight;
        self.filtered_y = self.filtered_y * smoothing + dy * weight;
        (self.filtered_x, self.filtered_y)
    }

    fn flush_motion<F, T>(
        &mut self,
        pending_x: &mut f32,
        pending_y: &mut f32,
        timestamp_micros: u128,
        send_mouse: &mut F,
        send_touch: &mut T,
    ) -> Result<(), String>
    where
        F: FnMut(MouseFrame) -> Result<(), String>,
        T: FnMut(TouchDragEvent) -> Result<(), String>,
    {
        if *pending_x == 0.0 && *pending_y == 0.0 {
            return Ok(());
        }

        if self.look_active() {
            let (mut dx, mut dy) =
                self.filtered_look_delta(*pending_x, *pending_y, timestamp_micros);
            if self.config.look.mode == MouseLookMode::TouchDrag {
                // Filter every source sample, then aggregate its output until the
                // next allowed emission slot. No raw motion is discarded.
                self.queued_touch_dx += dx;
                self.queued_touch_dy += dy;
                let interval_micros =
                    (1_000_000u128 / u128::from(self.config.look.rate_cap_hz)).max(1);
                let due = self
                    .last_touch_emit_micros
                    .is_none_or(|last| timestamp_micros.saturating_sub(last) >= interval_micros);
                if !due {
                    return Ok(());
                }
                dx = std::mem::take(&mut self.queued_touch_dx);
                dy = std::mem::take(&mut self.queued_touch_dy);
                self.last_touch_emit_micros = Some(timestamp_micros);
                if dx == 0.0 && dy == 0.0 {
                    return Ok(());
                }
            }
            match self.config.look.mode {
                MouseLookMode::RelativeMouse => {
                    self.ensure_hover(send_mouse)?;
                    self.emit(
                        ACTION_HOVER_MOVE,
                        0,
                        MouseAxes {
                            relative_x: dx,
                            relative_y: dy,
                            ..MouseAxes::default()
                        },
                        send_mouse,
                    )?;
                }
                MouseLookMode::TouchDrag => {
                    if self.touch_active {
                        let [left, top, right, bottom] = self.config.look.touch_bounds;
                        let (normalized_dx, normalized_dy) = inverse_rotate_delta(
                            dx / self.display_width as f32,
                            dy / self.display_height as f32,
                            self.rotation_delta,
                        );
                        let raw_x =
                            self.touch_position[0] + normalized_dx * self.config.look.touch_scale;
                        let raw_y =
                            self.touch_position[1] + normalized_dy * self.config.look.touch_scale;
                        let next_x = raw_x.clamp(left, right);
                        let next_y = raw_y.clamp(top, bottom);
                        let moved = (next_x - self.touch_position[0]).abs() > 0.00001
                            || (next_y - self.touch_position[1]).abs() > 0.00001;
                        let reached_edge =
                            raw_x < left || raw_x > right || raw_y < top || raw_y > bottom;
                        if moved {
                            self.touch_position = [next_x, next_y];
                            send_touch(TouchDragEvent::Move(self.touch_position))?;
                        }
                        if reached_edge && self.config.look.edge_recenter {
                            // Lift and restart the virtual camera finger at its anchor.
                            // The new pointer remains in the same Android multi-touch set.
                            send_touch(TouchDragEvent::Up)?;
                            self.touch_active = false;
                            self.touch_position = self.config.look.touch_anchor;
                            send_touch(TouchDragEvent::Down(self.touch_position))?;
                            self.touch_active = true;

                            // Preserve any movement beyond the edge so high-speed swipes
                            // do not lose the tail of their delta during the recenter.
                            let rebased_x =
                                (self.touch_position[0] + raw_x - next_x).clamp(left, right);
                            let rebased_y =
                                (self.touch_position[1] + raw_y - next_y).clamp(top, bottom);
                            if (rebased_x - self.touch_position[0]).abs() > 0.00001
                                || (rebased_y - self.touch_position[1]).abs() > 0.00001
                            {
                                self.touch_position = [rebased_x, rebased_y];
                                send_touch(TouchDragEvent::Move(self.touch_position))?;
                            }
                        }
                    }
                }
            }
        } else {
            self.filtered_x = 0.0;
            self.filtered_y = 0.0;
            self.last_motion_micros = None;
            self.ensure_hover(send_mouse)?;
            self.emit(
                ACTION_HOVER_MOVE,
                0,
                MouseAxes {
                    relative_x: *pending_x * self.config.cursor_speed,
                    relative_y: *pending_y * self.config.cursor_speed,
                    ..MouseAxes::default()
                },
                send_mouse,
            )?;
        }

        *pending_x = 0.0;
        *pending_y = 0.0;
        Ok(())
    }

    fn release_touch<T>(&mut self, send_touch: &mut T) -> Result<(), String>
    where
        T: FnMut(TouchDragEvent) -> Result<(), String>,
    {
        if self.touch_active {
            self.touch_active = false;
            send_touch(TouchDragEvent::Up)?;
        }
        Ok(())
    }

    pub(crate) fn release_all_with_touch<F, T>(
        &mut self,
        send_mouse: &mut F,
        send_touch: &mut T,
    ) -> Result<(), String>
    where
        F: FnMut(MouseFrame) -> Result<(), String>,
        T: FnMut(TouchDragEvent) -> Result<(), String>,
    {
        self.release_touch(send_touch)?;
        self.manual_lock = false;
        if self.buttons != 0 {
            self.buttons = 0;
            self.emit(ACTION_CANCEL, 0, MouseAxes::default(), send_mouse)?;
        }
        self.filtered_x = 0.0;
        self.filtered_y = 0.0;
        self.last_motion_micros = None;
        Ok(())
    }
}

fn inverse_rotate_delta(mut dx: f32, mut dy: f32, rotation_delta: u32) -> (f32, f32) {
    for _ in 0..(rotation_delta % 4) {
        let previous_x = dx;
        dx = dy;
        dy = -previous_x;
    }
    (dx, dy)
}

fn linux_button_bit(code: u16) -> Option<u16> {
    match code {
        BTN_LEFT => Some(BUTTON_PRIMARY),
        BTN_RIGHT => Some(BUTTON_SECONDARY),
        BTN_MIDDLE => Some(BUTTON_TERTIARY),
        BTN_SIDE => Some(BUTTON_BACK),
        BTN_EXTRA => Some(BUTTON_FORWARD),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::RawInputEvent;
    use std::path::PathBuf;

    fn input(events: &[(u16, u16, i32)]) -> InputFrame {
        input_at(1_000, events)
    }

    fn input_at(timestamp_micros: u128, events: &[(u16, u16, i32)]) -> InputFrame {
        InputFrame {
            path: PathBuf::from("/dev/input/event13"),
            device_name: "test mouse".into(),
            timestamp_micros,
            resynchronized: false,
            events: events
                .iter()
                .map(|(kind, code, value)| RawInputEvent {
                    event_type: *kind,
                    code: *code,
                    value: *value,
                })
                .collect(),
        }
    }

    #[test]
    fn ordinary_mouse_motion_is_forwarded_as_relative_pointer_deltas() {
        let mut engine = MouseLookEngine::new(MouseConfig::default());
        let mut packets = Vec::new();
        engine
            .handle_frame(
                &input(&[(EV_REL, REL_X, 5), (EV_REL, REL_Y, -3)]),
                &mut |p| {
                    packets.push(p);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0].action, ACTION_HOVER_ENTER);
        assert_eq!(packets[1].action, ACTION_HOVER_MOVE);
        assert_eq!(packets[1].relative_x, 5.0);
        assert_eq!(packets[1].relative_y, -3.0);
    }

    #[test]
    fn right_button_is_forwarded_but_never_locks_mouse_or_camera() {
        let look = MouseLookConfig {
            mode: MouseLookMode::TouchDrag,
            activation_mode: AimActivationMode::Toggle,
            hold_button_passthrough: false,
            ..MouseLookConfig::default()
        };
        let mut engine = MouseLookEngine::new(MouseConfig {
            look,
            ..MouseConfig::default()
        });
        let mut packets = Vec::new();
        let mut touches = Vec::new();

        engine
            .handle_frame_with_touch(
                &input(&[(EV_KEY, BTN_RIGHT, 1)]),
                &mut |frame| {
                    packets.push(frame);
                    Ok(())
                },
                &mut |event| {
                    touches.push(event);
                    Ok(())
                },
            )
            .unwrap();
        engine
            .handle_frame_with_touch(
                &input_at(9_000, &[(EV_REL, REL_X, 7), (EV_REL, REL_Y, -4)]),
                &mut |frame| {
                    packets.push(frame);
                    Ok(())
                },
                &mut |event| {
                    touches.push(event);
                    Ok(())
                },
            )
            .unwrap();
        engine
            .handle_frame_with_touch(
                &input_at(17_000, &[(EV_KEY, BTN_RIGHT, 0)]),
                &mut |frame| {
                    packets.push(frame);
                    Ok(())
                },
                &mut |event| {
                    touches.push(event);
                    Ok(())
                },
            )
            .unwrap();

        assert!(!engine.is_aiming());
        assert!(touches.is_empty());
        assert!(
            packets
                .iter()
                .any(|frame| frame.action == ACTION_BUTTON_PRESS)
        );
        assert!(
            packets
                .iter()
                .any(|frame| frame.action == ACTION_BUTTON_RELEASE)
        );
    }

    #[test]
    fn relative_look_remains_available_when_native_button_passthrough_is_disabled() {
        let config = MouseConfig {
            native_buttons: false,
            ..MouseConfig::default()
        };
        let mut engine = MouseLookEngine::new(config);
        let mut packets = Vec::new();
        engine
            .handle_frame(&input(&[(EV_KEY, BTN_RIGHT, 1)]), &mut |p| {
                packets.push(p);
                Ok(())
            })
            .unwrap();
        engine
            .handle_frame(&input(&[(EV_REL, REL_X, 4)]), &mut |p| {
                packets.push(p);
                Ok(())
            })
            .unwrap();
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0].action, ACTION_HOVER_ENTER);
        assert_eq!(packets[1].action, ACTION_HOVER_MOVE);
        assert_eq!(packets[1].buttons, 0);
    }

    #[test]
    fn releasing_look_button_returns_to_hover_mode() {
        let mut engine = MouseLookEngine::new(MouseConfig::default());
        let mut packets = Vec::new();
        engine
            .handle_frame(&input(&[(EV_KEY, BTN_RIGHT, 1)]), &mut |p| {
                packets.push(p);
                Ok(())
            })
            .unwrap();
        engine
            .handle_frame(&input(&[(EV_KEY, BTN_RIGHT, 0)]), &mut |p| {
                packets.push(p);
                Ok(())
            })
            .unwrap();
        assert_eq!(packets.last().unwrap().action, ACTION_BUTTON_RELEASE);
        assert_eq!(packets.last().unwrap().action_button, BUTTON_SECONDARY);
        assert_eq!(packets.last().unwrap().buttons, 0);
    }

    #[test]
    fn mouse_look_applies_independent_xy_sensitivity_and_vertical_inversion() {
        let look = MouseLookConfig {
            sensitivity_x: 2.0,
            sensitivity_y: 0.5,
            invert_y: true,
            ..MouseLookConfig::default()
        };
        let config = MouseConfig {
            look,
            ..MouseConfig::default()
        };
        let mut engine = MouseLookEngine::new(config);
        let mut packets = Vec::new();
        engine.set_manual_lock(true, &mut |_| Ok(())).unwrap();
        engine
            .handle_frame(
                &input_at(9_000, &[(EV_REL, REL_X, 3), (EV_REL, REL_Y, 4)]),
                &mut |p| {
                    packets.push(p);
                    Ok(())
                },
            )
            .unwrap();
        let frame = packets.last().unwrap();
        assert_eq!(frame.relative_x, 6.0);
        assert_eq!(frame.relative_y, -2.0);
        assert_eq!(frame.buttons, 0);
    }

    #[test]
    fn deadzone_is_applied_before_high_sensitivity_scaling() {
        let look = MouseLookConfig {
            sensitivity_x: 10.0,
            sensitivity_y: 10.0,
            deadzone: 2.0,
            ..MouseLookConfig::default()
        };
        let config = MouseConfig {
            look,
            ..MouseConfig::default()
        };
        let mut engine = MouseLookEngine::new(config);
        let mut packets = Vec::new();
        engine.set_manual_lock(true, &mut |_| Ok(())).unwrap();
        engine
            .handle_frame(
                &input_at(9_000, &[(EV_REL, REL_X, 1), (EV_REL, REL_Y, -1)]),
                &mut |p| {
                    packets.push(p);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(packets.last().unwrap().relative_x, 0.0);
        assert_eq!(packets.last().unwrap().relative_y, 0.0);

        engine
            .handle_frame(&input_at(17_000, &[(EV_REL, REL_X, 3)]), &mut |p| {
                packets.push(p);
                Ok(())
            })
            .unwrap();
        assert_eq!(packets.last().unwrap().relative_x, 30.0);
    }

    #[test]
    fn touch_drag_rate_cap_accumulates_filtered_motion_without_losing_delta() {
        let look = MouseLookConfig {
            mode: MouseLookMode::TouchDrag,
            rate_cap_hz: 240,
            smoothing: 0.0,
            ..MouseLookConfig::default()
        };
        let mut engine = MouseLookEngine::new(MouseConfig {
            look,
            ..MouseConfig::default()
        });
        let mut touches = Vec::new();
        engine
            .set_manual_lock(true, &mut |event| {
                touches.push(event);
                Ok(())
            })
            .unwrap();

        engine
            .handle_frame_with_touch(
                &input_at(9_000, &[(EV_REL, REL_X, 3)]),
                &mut |_| Ok(()),
                &mut |event| {
                    touches.push(event);
                    Ok(())
                },
            )
            .unwrap();
        assert!(matches!(touches.last(), Some(TouchDragEvent::Move(_))));
        let first = match *touches.last().unwrap() {
            TouchDragEvent::Move(position) => position,
            _ => unreachable!(),
        };

        engine
            .handle_frame_with_touch(
                &input_at(10_000, &[(EV_REL, REL_X, 4)]),
                &mut |_| Ok(()),
                &mut |event| {
                    touches.push(event);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(
            touches
                .iter()
                .filter(|event| matches!(event, TouchDragEvent::Move(_)))
                .count(),
            1,
            "motion below the rate interval must be held, not emitted"
        );

        engine
            .handle_frame_with_touch(
                &input_at(13_167, &[(EV_REL, REL_X, 5)]),
                &mut |_| Ok(()),
                &mut |event| {
                    touches.push(event);
                    Ok(())
                },
            )
            .unwrap();
        let moves = touches
            .iter()
            .filter_map(|event| match event {
                TouchDragEvent::Move(position) => Some(*position),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(moves.len(), 2);
        let total_normalized_delta = moves[1][0] - first[0];
        assert!((total_normalized_delta - 9.0 / 1920.0).abs() < 0.000001);
    }

    #[test]
    fn deadzone_and_smoothing_are_deterministic_per_input_frame() {
        let look = MouseLookConfig {
            deadzone: 2.0,
            smoothing: 0.5,
            ..MouseLookConfig::default()
        };
        let config = MouseConfig {
            look,
            ..MouseConfig::default()
        };
        let mut engine = MouseLookEngine::new(config);
        let mut packets = Vec::new();
        engine.set_manual_lock(true, &mut |_| Ok(())).unwrap();
        engine
            .handle_frame(
                &input_at(9_000, &[(EV_REL, REL_X, 1), (EV_REL, REL_Y, 4)]),
                &mut |p| {
                    packets.push(p);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(packets.last().unwrap().relative_x, 0.0);
        assert_eq!(packets.last().unwrap().relative_y, 2.0);
        engine
            .handle_frame(
                &input_at(17_000, &[(EV_REL, REL_X, 4), (EV_REL, REL_Y, 4)]),
                &mut |p| {
                    packets.push(p);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(packets.last().unwrap().relative_x, 2.0);
        assert_eq!(packets.last().unwrap().relative_y, 3.0);
    }

    #[test]
    fn cursor_speed_scales_normal_motion_without_changing_button_state() {
        let config = MouseConfig {
            cursor_speed: 1.5,
            ..MouseConfig::default()
        };
        let mut engine = MouseLookEngine::new(config);
        let mut packets = Vec::new();
        engine
            .handle_frame(
                &input(&[(EV_REL, REL_X, 5), (EV_REL, REL_Y, -2)]),
                &mut |p| {
                    packets.push(p);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(packets.last().unwrap().relative_x, 7.5);
        assert_eq!(packets.last().unwrap().relative_y, -3.0);
        assert_eq!(packets.last().unwrap().buttons, 0);
    }

    #[test]
    fn acceleration_curve_increases_gain_for_faster_motion_and_respects_maximum() {
        let curve = MouseAccelerationConfig {
            enabled: true,
            threshold: 0.5,
            max_gain: 3.0,
            exponent: 1.0,
        };
        let look = MouseLookConfig {
            acceleration: curve,
            ..MouseLookConfig::default()
        };
        let mut engine = MouseLookEngine::new(MouseConfig {
            look,
            ..MouseConfig::default()
        });
        let mut packets = Vec::new();
        engine.set_manual_lock(true, &mut |_| Ok(())).unwrap();
        engine
            .handle_frame(&input_at(2_000, &[(EV_REL, REL_X, 1)]), &mut |p| {
                packets.push(p);
                Ok(())
            })
            .unwrap();
        let slow = packets.last().unwrap().relative_x;
        engine
            .handle_frame(&input_at(3_000, &[(EV_REL, REL_X, 100)]), &mut |p| {
                packets.push(p);
                Ok(())
            })
            .unwrap();
        let fast = packets.last().unwrap().relative_x;
        assert!(slow >= 1.0);
        assert!(fast > slow);
        assert!(fast <= 300.0);
    }

    #[test]
    fn touch_drag_creates_moves_and_releases_a_camera_finger_without_native_look_motion() {
        let look = MouseLookConfig {
            mode: MouseLookMode::TouchDrag,
            touch_anchor: [0.7, 0.5],
            touch_bounds: [0.5, 0.1, 0.95, 0.9],
            hold_button_passthrough: false,
            sensitivity_x: 2.0,
            sensitivity_y: 1.0,
            ..MouseLookConfig::default()
        };
        let mut engine = MouseLookEngine::with_display(
            MouseConfig {
                enabled: false,
                look,
                ..MouseConfig::default()
            },
            1000,
            500,
        );
        let mut mouse_packets = Vec::new();
        let mut touch_events = Vec::new();
        engine
            .set_manual_lock(true, &mut |event| {
                touch_events.push(event);
                Ok(())
            })
            .unwrap();
        engine
            .handle_frame_with_touch(
                &input_at(9_000, &[(EV_REL, REL_X, 20), (EV_REL, REL_Y, -10)]),
                &mut |p| {
                    mouse_packets.push(p);
                    Ok(())
                },
                &mut |event| {
                    touch_events.push(event);
                    Ok(())
                },
            )
            .unwrap();
        engine
            .set_manual_lock(false, &mut |event| {
                touch_events.push(event);
                Ok(())
            })
            .unwrap();
        assert_eq!(mouse_packets.len(), 0);
        assert_eq!(touch_events.len(), 3);
        assert_eq!(touch_events[0], TouchDragEvent::Down([0.7, 0.5]));
        assert_eq!(touch_events[1], TouchDragEvent::Move([0.74, 0.48]));
        assert_eq!(touch_events[2], TouchDragEvent::Up);
    }

    #[test]
    fn manual_hotkey_lock_toggles_touch_drag_without_mouse_button() {
        let look = MouseLookConfig {
            mode: MouseLookMode::TouchDrag,
            activation_mode: AimActivationMode::Hold,
            hold_button_passthrough: false,
            ..MouseLookConfig::default()
        };
        let mut engine = MouseLookEngine::with_display(
            MouseConfig {
                look,
                ..MouseConfig::default()
            },
            1920,
            1080,
        );
        let mut events = Vec::new();

        engine
            .set_manual_lock(true, &mut |event| {
                events.push(event);
                Ok(())
            })
            .unwrap();
        assert!(engine.is_aiming());
        assert_eq!(events, vec![TouchDragEvent::Down([0.76, 0.55])]);

        engine
            .set_manual_lock(false, &mut |event| {
                events.push(event);
                Ok(())
            })
            .unwrap();
        assert!(!engine.is_aiming());
        assert_eq!(events.last(), Some(&TouchDragEvent::Up));
    }

    #[test]
    fn right_button_does_not_unlock_an_f2_locked_camera() {
        let look = MouseLookConfig {
            mode: MouseLookMode::TouchDrag,
            hold_button_passthrough: true,
            ..MouseLookConfig::default()
        };
        let mut engine = MouseLookEngine::with_display(
            MouseConfig {
                look,
                ..MouseConfig::default()
            },
            1920,
            1080,
        );
        let mut touches = Vec::new();
        let mut mouse = Vec::new();

        engine
            .set_manual_lock(true, &mut |event| {
                touches.push(event);
                Ok(())
            })
            .unwrap();
        engine
            .handle_frame_with_touch(
                &input(&[(EV_KEY, BTN_RIGHT, 1)]),
                &mut |frame| {
                    mouse.push(frame);
                    Ok(())
                },
                &mut |event| {
                    touches.push(event);
                    Ok(())
                },
            )
            .unwrap();
        engine
            .handle_frame_with_touch(
                &input(&[(EV_KEY, BTN_RIGHT, 0)]),
                &mut |frame| {
                    mouse.push(frame);
                    Ok(())
                },
                &mut |event| {
                    touches.push(event);
                    Ok(())
                },
            )
            .unwrap();

        assert!(engine.is_aiming());
        assert_eq!(touches, vec![TouchDragEvent::Down([0.76, 0.55])]);
        assert!(
            mouse
                .iter()
                .any(|frame| frame.action == ACTION_BUTTON_PRESS)
        );
        assert!(
            mouse
                .iter()
                .any(|frame| frame.action == ACTION_BUTTON_RELEASE)
        );
    }

    #[test]
    fn f3_is_the_only_toggle_for_touch_drag_aim() {
        let look = MouseLookConfig {
            mode: MouseLookMode::TouchDrag,
            ..MouseLookConfig::default()
        };
        let mut engine = MouseLookEngine::with_display(
            MouseConfig {
                look,
                ..MouseConfig::default()
            },
            100,
            100,
        );
        let mut touches = Vec::new();
        engine
            .set_manual_lock(true, &mut |event| {
                touches.push(event);
                Ok(())
            })
            .unwrap();
        engine
            .handle_frame_with_touch(
                &input_at(2_000, &[(EV_KEY, BTN_RIGHT, 1)]),
                &mut |_| Ok(()),
                &mut |event| {
                    touches.push(event);
                    Ok(())
                },
            )
            .unwrap();
        engine
            .handle_frame_with_touch(
                &input_at(3_000, &[(EV_KEY, BTN_RIGHT, 0)]),
                &mut |_| Ok(()),
                &mut |event| {
                    touches.push(event);
                    Ok(())
                },
            )
            .unwrap();
        engine
            .handle_frame_with_touch(
                &input_at(4_000, &[(EV_REL, REL_X, 4)]),
                &mut |_| Ok(()),
                &mut |event| {
                    touches.push(event);
                    Ok(())
                },
            )
            .unwrap();
        assert!(engine.is_aiming());
        assert_eq!(touches[0], TouchDragEvent::Down([0.76, 0.55]));
        assert!(matches!(touches[1], TouchDragEvent::Move(_)));
        engine
            .set_manual_lock(false, &mut |event| {
                touches.push(event);
                Ok(())
            })
            .unwrap();
        assert!(!engine.is_aiming());
        assert_eq!(touches.last(), Some(&TouchDragEvent::Up));
    }

    #[test]
    fn touch_drag_recenters_at_edge_and_preserves_overshoot() {
        let look = MouseLookConfig {
            mode: MouseLookMode::TouchDrag,
            touch_anchor: [0.7, 0.5],
            touch_bounds: [0.6, 0.2, 0.8, 0.8],
            edge_recenter: true,
            ..MouseLookConfig::default()
        };
        let mut engine = MouseLookEngine::with_display(
            MouseConfig {
                enabled: false,
                look,
                ..MouseConfig::default()
            },
            100,
            100,
        );
        let mut touches = Vec::new();
        engine
            .set_manual_lock(true, &mut |event| {
                touches.push(event);
                Ok(())
            })
            .unwrap();
        engine
            .handle_frame_with_touch(
                &input_at(9_000, &[(EV_REL, REL_X, 20)]),
                &mut |_| Ok(()),
                &mut |event| {
                    touches.push(event);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(
            &touches[..4],
            &[
                TouchDragEvent::Down([0.7, 0.5]),
                TouchDragEvent::Move([0.8, 0.5]),
                TouchDragEvent::Up,
                TouchDragEvent::Down([0.7, 0.5]),
            ]
        );
        let Some(TouchDragEvent::Move([x, y])) = touches.get(4) else {
            panic!("expected the overflow delta to move the new camera contact");
        };
        assert!((*x - 0.8).abs() < 0.00001);
        assert!((*y - 0.5).abs() < 0.00001);
        engine
            .set_manual_lock(false, &mut |event| {
                touches.push(event);
                Ok(())
            })
            .unwrap();
        assert_eq!(touches.last(), Some(&TouchDragEvent::Up));
    }

    #[test]
    fn touch_drag_clamps_camera_finger_to_configured_screen_area() {
        let look = MouseLookConfig {
            mode: MouseLookMode::TouchDrag,
            touch_anchor: [0.7, 0.5],
            touch_bounds: [0.6, 0.2, 0.8, 0.8],
            ..MouseLookConfig::default()
        };
        let mut engine = MouseLookEngine::with_display(
            MouseConfig {
                enabled: false,
                look,
                ..MouseConfig::default()
            },
            100,
            100,
        );
        let mut events = Vec::new();
        engine
            .set_manual_lock(true, &mut |e| {
                events.push(e);
                Ok(())
            })
            .unwrap();
        engine
            .handle_frame_with_touch(
                &input_at(9_000, &[(EV_REL, REL_X, 1000)]),
                &mut |_| Ok(()),
                &mut |e| {
                    events.push(e);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(events.last(), Some(&TouchDragEvent::Move([0.8, 0.5])));
    }

    #[test]
    fn touch_drag_is_released_after_input_resynchronization() {
        let look = MouseLookConfig {
            mode: MouseLookMode::TouchDrag,
            ..MouseLookConfig::default()
        };
        let mut engine = MouseLookEngine::new(MouseConfig {
            enabled: false,
            look,
            ..MouseConfig::default()
        });
        let mut events = Vec::new();
        engine
            .set_manual_lock(true, &mut |e| {
                events.push(e);
                Ok(())
            })
            .unwrap();
        let mut lost = input_at(10_000, &[]);
        lost.resynchronized = true;
        engine
            .handle_frame_with_touch(&lost, &mut |_| Ok(()), &mut |e| {
                events.push(e);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            events,
            vec![TouchDragEvent::Down([0.76, 0.55]), TouchDragEvent::Up]
        );
    }
    #[test]
    fn live_display_resize_updates_mouse_look_calibration_and_resets_timing_filter() {
        let mut engine = MouseLookEngine::new(MouseConfig::default());
        engine.filtered_x = 12.0;
        engine.filtered_y = -7.0;
        engine.last_motion_micros = Some(123_456);
        engine.set_display_geometry(1080, 1920, 1);

        assert_eq!(engine.display_width, 1080);
        assert_eq!(engine.display_height, 1920);
        assert_eq!(engine.rotation_delta, 1);
        assert_eq!(engine.filtered_x, 0.0);
        assert_eq!(engine.filtered_y, 0.0);
        assert_eq!(engine.last_motion_micros, None);
    }

    #[test]
    fn touch_drag_delta_is_inverse_rotated_to_keep_screen_direction_after_rotation() {
        let (dx, dy) = inverse_rotate_delta(0.1, 0.2, 1);
        assert!((dx - 0.2).abs() < 0.0001);
        assert!((dy + 0.1).abs() < 0.0001);

        let (dx, dy) = inverse_rotate_delta(0.1, 0.2, 2);
        assert!((dx + 0.1).abs() < 0.0001);
        assert!((dy + 0.2).abs() < 0.0001);

        let (dx, dy) = inverse_rotate_delta(0.1, 0.2, 3);
        assert!((dx + 0.2).abs() < 0.0001);
        assert!((dy - 0.1).abs() < 0.0001);
    }
}
