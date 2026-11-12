use evdev::{
    AbsoluteAxisCode, Device, EventType, InputEvent, InputId, KeyCode, PropType, RelativeAxisCode,
    SynchronizationCode,
};
use nix::errno::Errno;
use nix::poll::{PollFd, PollFlags, poll};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::io;
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SCAN_INTERVAL: Duration = Duration::from_millis(750);
const RETRY_DELAY: Duration = Duration::from_secs(2);
const EV_SYN: u16 = EventType::SYNCHRONIZATION.0;
const EV_KEY: u16 = EventType::KEY.0;
const EV_REL: u16 = EventType::RELATIVE.0;
const EV_ABS: u16 = EventType::ABSOLUTE.0;
const SYN_REPORT: u16 = SynchronizationCode::SYN_REPORT.0;
const SYN_DROPPED: u16 = SynchronizationCode::SYN_DROPPED.0;

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
struct DeviceRoles {
    keyboard: bool,
    relative_pointer: bool,
    touchpad: bool,
}

impl DeviceRoles {
    fn is_supported(&self) -> bool {
        self.keyboard || self.relative_pointer || self.touchpad
    }

    fn label(&self) -> String {
        let mut labels = Vec::new();
        if self.keyboard {
            labels.push("keyboard");
        }
        if self.relative_pointer {
            labels.push("mouse");
        }
        if self.touchpad {
            labels.push("touchpad");
        }
        labels.join("+")
    }
}

#[derive(Clone, Debug)]
struct DeviceInfo {
    path: PathBuf,
    name: String,
    physical_path: Option<String>,
    unique_name: Option<String>,
    input_id: InputId,
    roles: DeviceRoles,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct DeviceIdentity {
    name: String,
    physical_path: Option<String>,
    unique_name: Option<String>,
    input_id: InputId,
    roles: DeviceRoles,
}

impl DeviceIdentity {
    fn from_info(info: &DeviceInfo) -> Self {
        Self {
            name: info.name.clone(),
            physical_path: info.physical_path.clone(),
            unique_name: info.unique_name.clone(),
            input_id: info.input_id.clone(),
            roles: info.roles.clone(),
        }
    }

    fn is_strong(&self) -> bool {
        self.physical_path
            .as_deref()
            .is_some_and(|path| !path.is_empty())
            || self
                .unique_name
                .as_deref()
                .is_some_and(|name| !name.is_empty())
    }
}

impl DeviceInfo {
    fn display_line(&self) -> String {
        format!(
            "{}  [{}]  {}{}",
            self.path.display(),
            self.roles.label(),
            self.name,
            self.physical_path
                .as_deref()
                .map(|path| format!("  phys={path}"))
                .unwrap_or_default()
        )
    }
}

fn inspect_device(path: PathBuf, device: &Device) -> DeviceInfo {
    let keys = device.supported_keys();
    let has_key = |code| keys.is_some_and(|supported| supported.contains(code));
    let relative = device.supported_relative_axes();
    let absolute = device.supported_absolute_axes();

    let keyboard = has_key(KeyCode::KEY_A) && has_key(KeyCode::KEY_SPACE);
    let relative_pointer = relative.is_some_and(|axes| {
        axes.contains(RelativeAxisCode::REL_X) && axes.contains(RelativeAxisCode::REL_Y)
    }) && has_key(KeyCode::BTN_LEFT);

    let has_absolute_xy = absolute.is_some_and(|axes| {
        (axes.contains(AbsoluteAxisCode::ABS_X) && axes.contains(AbsoluteAxisCode::ABS_Y))
            || (axes.contains(AbsoluteAxisCode::ABS_MT_POSITION_X)
                && axes.contains(AbsoluteAxisCode::ABS_MT_POSITION_Y))
    });
    let has_touch_button = has_key(KeyCode::BTN_TOUCH) || has_key(KeyCode::BTN_TOOL_FINGER);
    let touchpad =
        has_absolute_xy && has_touch_button && device.properties().contains(PropType::POINTER);

    DeviceInfo {
        path,
        name: device.name().unwrap_or("Unnamed input device").to_owned(),
        physical_path: device.physical_path().map(str::to_owned),
        unique_name: device.unique_name().map(str::to_owned),
        input_id: device.input_id(),
        roles: DeviceRoles {
            keyboard,
            relative_pointer,
            touchpad,
        },
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DiscoveredDevicePaths {
    pub(crate) keyboard: Vec<PathBuf>,
    pub(crate) mouse: Vec<PathBuf>,
    pub(crate) suppress: Vec<PathBuf>,
}

fn stable_aliases_for(input_root: &Path, event_node: &Path) -> Vec<PathBuf> {
    let Ok(target) = fs::canonicalize(event_node) else {
        return Vec::new();
    };
    let mut aliases = Vec::new();
    for group in ["by-id", "by-path"] {
        let directory = input_root.join(group);
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let alias = entry.path();
            let name = alias
                .file_name()
                .and_then(|item| item.to_str())
                .unwrap_or_default();
            if !name.contains("-event") {
                continue;
            }
            if fs::canonicalize(&alias).is_ok_and(|resolved| resolved == target) {
                aliases.push(alias);
            }
        }
    }
    aliases.sort();
    aliases.dedup();
    aliases
}

fn preferred_alias(aliases: &[PathBuf], role: &str) -> Option<PathBuf> {
    aliases
        .iter()
        .min_by_key(|path| {
            let by_id = path
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "by-id");
            let prefer_id = role == "mouse";
            (by_id != prefer_id, (*path).clone())
        })
        .cloned()
}

pub(crate) fn discover_stable_devices() -> io::Result<DiscoveredDevicePaths> {
    let mut result = DiscoveredDevicePaths::default();
    let mut devices = evdev::enumerate().collect::<Vec<_>>();
    devices.sort_by(|left, right| left.0.cmp(&right.0));

    for (event_node, device) in devices {
        let info = inspect_device(event_node.clone(), &device);
        if !info.roles.is_supported() {
            continue;
        }
        let aliases = stable_aliases_for(Path::new("/dev/input"), &event_node);
        if aliases.is_empty() {
            eprintln!(
                "DISCOVERY_NO_STABLE_LINK path={} name={:?}; this device will not be stored by eventN",
                event_node.display(),
                info.name
            );
            continue;
        }
        if info.roles.keyboard && info.roles.relative_pointer {
            eprintln!(
                "DISCOVERY_AMBIGUOUS path={} name={:?}; composite keyboard/mouse node skipped",
                event_node.display(),
                info.name
            );
            continue;
        }
        if info.roles.keyboard
            && let Some(path) = preferred_alias(&aliases, "keyboard")
        {
            result.keyboard.push(path);
        }
        if info.roles.relative_pointer
            && !info.roles.touchpad
            && let Some(path) = preferred_alias(&aliases, "mouse")
        {
            result.mouse.push(path);
        }
        if info.roles.touchpad
            && let Some(path) = preferred_alias(&aliases, "suppress")
        {
            result.suppress.push(path);
        }
    }

    for paths in [
        &mut result.keyboard,
        &mut result.mouse,
        &mut result.suppress,
    ] {
        paths.sort();
        paths.dedup();
    }
    Ok(result)
}

pub fn list_devices() -> io::Result<()> {
    let mut devices = evdev::enumerate()
        .filter_map(|(path, device)| {
            let info = inspect_device(path, &device);
            info.roles.is_supported().then_some(info)
        })
        .collect::<Vec<_>>();
    devices.sort_by(|left, right| left.path.cmp(&right.path));

    if devices.is_empty() {
        println!("No readable keyboard, relative mouse, or touchpad devices found.");
        println!("Check membership in the 'input' group and /dev/input permissions.");
        return Ok(());
    }

    println!("WayTouch evdev devices (read-only; devices are never grabbed):");
    for info in &devices {
        println!("DEVICE {}", info.display_line());
        let aliases = stable_aliases_for(Path::new("/dev/input"), &info.path);
        if aliases.is_empty() {
            println!("  STABLE_ALIAS none");
        } else {
            for alias in aliases {
                println!("  STABLE_ALIAS {}", alias.display());
            }
        }
    }
    println!("{} supported device(s).", devices.len());
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RawInputEvent {
    pub(crate) event_type: u16,
    pub(crate) code: u16,
    pub(crate) value: i32,
}

#[derive(Clone, Debug)]
pub(crate) struct InputFrame {
    pub(crate) path: PathBuf,
    pub(crate) device_name: String,
    pub(crate) timestamp_micros: u128,
    pub(crate) resynchronized: bool,
    pub(crate) events: Vec<RawInputEvent>,
}

#[derive(Default)]
struct FrameBuilder {
    events: Vec<RawInputEvent>,
    timestamp_micros: u128,
    saw_drop: bool,
}

impl FrameBuilder {
    fn push(&mut self, event: InputEvent) -> Option<(Vec<RawInputEvent>, u128, bool)> {
        let event_type = event.event_type().0;
        let code = event.code();
        let value = event.value();

        if event_type == EV_SYN && code == SYN_DROPPED {
            self.events.clear();
            self.saw_drop = true;
            self.timestamp_micros = timestamp_micros(event.timestamp());
            return None;
        }

        if event_type == EV_SYN && code == SYN_REPORT {
            let resynchronized = std::mem::take(&mut self.saw_drop);
            if self.events.is_empty() && !resynchronized {
                return None;
            }
            let events = std::mem::take(&mut self.events);
            let timestamp = self.timestamp_micros;
            self.timestamp_micros = timestamp_micros(event.timestamp());
            return Some((events, timestamp, resynchronized));
        }

        self.timestamp_micros = timestamp_micros(event.timestamp());
        self.events.push(RawInputEvent {
            event_type,
            code,
            value,
        });
        None
    }
}

fn timestamp_micros(time: SystemTime) -> u128 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros()
}

enum WorkerMessage {
    Frame(InputFrame),
    Stopped {
        path: PathBuf,
        name: String,
        reason: String,
    },
}

#[derive(Default)]
pub(crate) struct InputMotionCounters {
    pub(crate) coalesced: AtomicU64,
    pub(crate) dropped: AtomicU64,
}

impl InputMotionCounters {
    pub(crate) fn take(&self) -> (u64, u64) {
        (
            self.coalesced.swap(0, Ordering::Relaxed),
            self.dropped.swap(0, Ordering::Relaxed),
        )
    }
}

#[derive(Default)]
struct PriorityQueueState {
    messages: VecDeque<WorkerMessage>,
    closed: bool,
}

/// Bounded, ordered input queue. Only pure X/Y motion is coalesced or evicted;
/// key/button transitions, wheel events, resync frames and disconnects are kept.
struct PriorityInputQueue {
    state: Mutex<PriorityQueueState>,
    not_empty: Condvar,
    not_full: Condvar,
    capacity: usize,
    motion_counters: Arc<InputMotionCounters>,
}

impl PriorityInputQueue {
    fn new(capacity: usize, motion_counters: Arc<InputMotionCounters>) -> Self {
        Self {
            state: Mutex::new(PriorityQueueState::default()),
            not_empty: Condvar::new(),
            not_full: Condvar::new(),
            capacity: capacity.max(1),
            motion_counters,
        }
    }

    fn is_motion(frame: &InputFrame) -> bool {
        !frame.resynchronized
            && !frame.events.is_empty()
            && frame.events.iter().all(|event| {
                event.event_type == EV_REL
                    && (event.code == RelativeAxisCode::REL_X.0
                        || event.code == RelativeAxisCode::REL_Y.0)
            })
    }

    fn is_low_priority(message: &WorkerMessage) -> bool {
        matches!(message, WorkerMessage::Frame(frame) if Self::is_motion(frame))
    }

    fn coalesce(existing: &mut InputFrame, incoming: &InputFrame) -> bool {
        if existing.path != incoming.path
            || !Self::is_motion(existing)
            || !Self::is_motion(incoming)
        {
            return false;
        }
        for event in &incoming.events {
            if let Some(previous) = existing
                .events
                .iter_mut()
                .find(|item| item.event_type == event.event_type && item.code == event.code)
            {
                previous.value = previous.value.saturating_add(event.value);
            } else {
                existing.events.push(event.clone());
            }
        }
        existing.timestamp_micros = incoming.timestamp_micros;
        true
    }

    fn push(&self, message: WorkerMessage) -> bool {
        let incoming_is_motion = Self::is_low_priority(&message);
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return false,
        };
        if state.closed {
            return false;
        }

        if incoming_is_motion
            && let Some(WorkerMessage::Frame(previous)) = state.messages.back_mut()
            && let WorkerMessage::Frame(incoming) = &message
            && Self::coalesce(previous, incoming)
        {
            self.motion_counters
                .coalesced
                .fetch_add(1, Ordering::Relaxed);
            self.not_empty.notify_one();
            return true;
        }

        while state.messages.len() >= self.capacity {
            if let Some(position) = state.messages.iter().position(Self::is_low_priority) {
                state.messages.remove(position);
                self.motion_counters.dropped.fetch_add(1, Ordering::Relaxed);
                break;
            }
            if incoming_is_motion {
                // Do not stall evdev readers when only critical transitions remain.
                self.motion_counters.dropped.fetch_add(1, Ordering::Relaxed);
                return true;
            }
            // Critical events are lossless: wait for the consumer, unless shutting down.
            state = match self.not_full.wait(state) {
                Ok(next) => next,
                Err(_) => return false,
            };
            if state.closed {
                return false;
            }
        }

        state.messages.push_back(message);
        self.not_empty.notify_one();
        true
    }

    fn recv_timeout(&self, timeout: Duration) -> Result<WorkerMessage, RecvTimeoutError> {
        let deadline = Instant::now() + timeout;
        let mut state = self
            .state
            .lock()
            .map_err(|_| RecvTimeoutError::Disconnected)?;
        loop {
            if let Some(message) = state.messages.pop_front() {
                self.not_full.notify_one();
                return Ok(message);
            }
            if state.closed {
                return Err(RecvTimeoutError::Disconnected);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(RecvTimeoutError::Timeout);
            }
            let (next, result) = self
                .not_empty
                .wait_timeout(state, remaining)
                .map_err(|_| RecvTimeoutError::Disconnected)?;
            state = next;
            if result.timed_out() && state.messages.is_empty() {
                return Err(RecvTimeoutError::Timeout);
            }
        }
    }

    fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
            self.not_empty.notify_all();
            self.not_full.notify_all();
        }
    }
}

/// Runtime switch for exclusive mouse capture. Keyboard devices stay grabbed
/// for key mapping; the physical mouse is shared with GNOME until F1 is pressed.
#[derive(Clone, Default)]
pub(crate) struct MouseCaptureControl {
    requested: Arc<AtomicBool>,
}

impl MouseCaptureControl {
    pub(crate) fn set_captured(&self, captured: bool) {
        self.requested.store(captured, Ordering::Release);
    }

    pub(crate) fn is_captured(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }

    fn shared_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.requested)
    }
}

fn is_deferred_mouse_capture(info: &DeviceInfo) -> bool {
    info.roles.relative_pointer && !info.roles.touchpad && !info.roles.keyboard
}

#[derive(Clone)]
enum WorkerSink {
    Channel(SyncSender<WorkerMessage>),
    Priority(Arc<PriorityInputQueue>),
}

impl WorkerSink {
    fn send(&self, message: WorkerMessage) -> bool {
        match self {
            Self::Channel(sender) => sender.send(message).is_ok(),
            Self::Priority(queue) => queue.push(message),
        }
    }
}

fn spawn_reader(
    info: DeviceInfo,
    mut device: Device,
    sender: WorkerSink,
    stop: Arc<AtomicBool>,
    mouse_capture: Option<Arc<AtomicBool>>,
) -> io::Result<thread::JoinHandle<()>> {
    device.set_nonblocking(true)?;
    let thread_name = format!(
        "waytouch-{}",
        info.path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("input")
    );
    thread::Builder::new().name(thread_name).spawn(move || {
        let mut builder = FrameBuilder::default();
        let mut mouse_grabbed = mouse_capture.is_none();
        loop {
            if stop.load(Ordering::Acquire) {
                return;
            }

            if let Some(requested) = mouse_capture.as_ref() {
                let should_grab = requested.load(Ordering::Acquire);
                if should_grab != mouse_grabbed {
                    let result = if should_grab {
                        device.grab()
                    } else {
                        device.ungrab()
                    };
                    if let Err(error) = result {
                        let _ = sender.send(WorkerMessage::Stopped {
                            path: info.path.clone(),
                            name: info.name.clone(),
                            reason: format!("mouse capture transition failed: {error}"),
                        });
                        return;
                    }
                    builder = FrameBuilder::default();
                    if should_grab {
                        // Discard motion and button state accumulated while GNOME
                        // owned the mouse; the Android cursor starts from a clean edge.
                        loop {
                            match device.fetch_events() {
                                Ok(events) => {
                                    let mut drained = false;
                                    for _event in events {
                                        drained = true;
                                    }
                                    if drained {
                                        continue;
                                    }
                                    break;
                                }
                                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                                Err(error) => {
                                    let _ = sender.send(WorkerMessage::Stopped {
                                        path: info.path.clone(),
                                        name: info.name.clone(),
                                        reason: format!(
                                            "could not flush shared mouse events: {error}"
                                        ),
                                    });
                                    return;
                                }
                            }
                        }
                    }
                    mouse_grabbed = should_grab;
                    println!(
                        "MOUSE_EVDEV_CAPTURE state={} path={}",
                        if mouse_grabbed { "grabbed" } else { "released" },
                        info.path.display()
                    );
                }
            }

            // A short poll interval makes F1 capture/release feel immediate. Mouse
            // readers need to observe the shared toggle even while the user is idle.
            // which closes the evdev fd and releases EVIOCGRAB deterministically.
            let ready = {
                let borrowed = device.as_fd();
                let mut descriptors = [PollFd::new(borrowed, PollFlags::POLLIN)];
                let poll_timeout = if mouse_capture.is_some() {
                    25u16
                } else {
                    250u16
                };
                match poll(&mut descriptors, poll_timeout) {
                    Ok(count) => count > 0,
                    Err(Errno::EINTR) => false,
                    Err(error) => {
                        let _ = sender.send(WorkerMessage::Stopped {
                            path: info.path.clone(),
                            name: info.name.clone(),
                            reason: error.to_string(),
                        });
                        return;
                    }
                }
            };

            if stop.load(Ordering::Acquire) {
                return;
            }
            if !ready {
                continue;
            }

            match device.fetch_events() {
                Ok(events) => {
                    for event in events {
                        if let Some((events, timestamp_micros, resynchronized)) =
                            builder.push(event)
                            && (mouse_capture.is_none() || mouse_grabbed)
                            && !sender.send(WorkerMessage::Frame(InputFrame {
                                path: info.path.clone(),
                                device_name: info.name.clone(),
                                timestamp_micros,
                                resynchronized,
                                events,
                            }))
                        {
                            return;
                        }
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => {
                    let _ = sender.send(WorkerMessage::Stopped {
                        path: info.path.clone(),
                        name: info.name.clone(),
                        reason: error.to_string(),
                    });
                    return;
                }
            }
        }
    })
}

fn event_description(event: &RawInputEvent) -> String {
    match event.event_type {
        EV_KEY => {
            let state = match event.value {
                0 => "up".to_owned(),
                1 => "down".to_owned(),
                2 => "repeat".to_owned(),
                value => format!("value={value}"),
            };
            format!("KEY({:?})={state}", KeyCode(event.code))
        }
        EV_REL => format!("REL({:?})={}", RelativeAxisCode(event.code), event.value),
        EV_ABS => format!("ABS({:?})={}", AbsoluteAxisCode(event.code), event.value),
        EV_SYN => format!("SYN({:?})={}", SynchronizationCode(event.code), event.value),
        other => format!("EV{other}(code={})={}", event.code, event.value),
    }
}

fn print_frame(frame: InputFrame) {
    let events = frame
        .events
        .iter()
        .map(event_description)
        .collect::<Vec<_>>()
        .join(", ");
    let recovery = if frame.resynchronized {
        " resynchronized=true"
    } else {
        ""
    };
    println!(
        "INPUT ts_us={} device={} name={:?}{} [{}]",
        frame.timestamp_micros,
        frame.path.display(),
        frame.device_name,
        recovery,
        events
    );
}

fn should_include(path: &Path, requested: Option<&Path>) -> bool {
    requested.is_none_or(|requested_path| requested_path == path)
}

pub fn listen(requested_path: Option<PathBuf>) -> io::Result<()> {
    if let Some(path) = requested_path.as_deref()
        && !(path.parent() == Some(Path::new("/dev/input"))
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix("event"))
                .is_some_and(|suffix| {
                    !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit())
                }))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "device path must be a numeric evdev node such as /dev/input/event5",
        ));
    }

    if let Some(path) = requested_path.as_deref() {
        let device = Device::open(path)?;
        let info = inspect_device(path.to_path_buf(), &device);
        println!("Selected {}", info.display_line());
    }

    let (sender, receiver): (SyncSender<WorkerMessage>, Receiver<WorkerMessage>) =
        mpsc::sync_channel(256);
    let mut active = HashSet::<PathBuf>::new();
    let mut retry_after = HashMap::<PathBuf, Instant>::new();
    let stop = Arc::new(AtomicBool::new(false));
    let mut next_scan = Instant::now();

    println!("WayTouch input listener started (passive mode; no EVIOCGRAB).");
    println!("Press Ctrl+C to stop. Normal desktop input delivery remains enabled.");

    loop {
        let now = Instant::now();
        if now >= next_scan {
            for (path, device) in evdev::enumerate() {
                if !should_include(&path, requested_path.as_deref()) || active.contains(&path) {
                    continue;
                }

                if retry_after.get(&path).is_some_and(|retry| *retry > now) {
                    continue;
                }

                let info = inspect_device(path.clone(), &device);
                if requested_path.is_none() && !info.roles.is_supported() {
                    continue;
                }

                if requested_path.is_some() && !info.roles.is_supported() {
                    eprintln!(
                        "Opening explicitly selected device {} ({}) despite unknown role.",
                        path.display(),
                        info.name
                    );
                }

                println!("DEVICE_ADDED {}", info.display_line());
                match spawn_reader(
                    info,
                    device,
                    WorkerSink::Channel(sender.clone()),
                    Arc::clone(&stop),
                    None,
                ) {
                    Ok(_worker) => {
                        active.insert(path.clone());
                    }
                    Err(error) => {
                        eprintln!("Cannot start reader for {}: {error}", path.display());
                        retry_after.insert(path, now + RETRY_DELAY);
                    }
                }
            }

            next_scan = now + SCAN_INTERVAL;
        }

        let timeout = next_scan.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(timeout) {
            Ok(WorkerMessage::Frame(frame)) => print_frame(frame),
            Ok(WorkerMessage::Stopped { path, name, reason }) => {
                active.remove(&path);
                retry_after.insert(path.clone(), Instant::now() + RETRY_DELAY);
                eprintln!(
                    "DEVICE_REMOVED path={} name={:?} reason={}",
                    path.display(),
                    name,
                    reason
                );
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "all evdev event channels disconnected",
                ));
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum DeviceMessage {
    Frame(InputFrame),
    Removed {
        path: PathBuf,
    },
    /// Periodic wake-up so runtime configuration can be applied without input.
    Tick,
}

fn selected_path_for_event_node<'a>(
    selected_paths: &'a [PathBuf],
    event_node: &Path,
) -> Option<&'a PathBuf> {
    selected_paths.iter().find(|selected| {
        std::fs::symlink_metadata(selected).is_ok_and(|metadata| metadata.file_type().is_symlink())
            && std::fs::canonicalize(selected).is_ok_and(|resolved| resolved == event_node)
    })
}

fn is_direct_event_node(path: &Path) -> bool {
    if path.parent() != Some(Path::new("/dev/input")) {
        return false;
    }
    path.file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("event"))
        .is_some_and(|suffix| {
            !suffix.is_empty() && suffix.chars().all(|character| character.is_ascii_digit())
        })
}

fn selected_path_for_device<'a>(
    selected_paths: &'a [PathBuf],
    selected_identities: &HashMap<PathBuf, DeviceIdentity>,
    identity_counts: &HashMap<DeviceIdentity, usize>,
    event_node: &Path,
    info: &DeviceInfo,
) -> Option<&'a PathBuf> {
    if let Some(stable_path) = selected_path_for_event_node(selected_paths, event_node) {
        let expected = selected_identities.get(stable_path)?;
        return (expected == &DeviceIdentity::from_info(info)).then_some(stable_path);
    }

    // Legacy /dev/input/eventN selections are allowed to follow a replug only
    // when a strong physical identity is present and exactly one live node has
    // that identity. An unrelated device that inherits the old event number
    // must never be grabbed merely because its pathname happens to match.
    let identity = DeviceIdentity::from_info(info);
    if !identity.is_strong() || identity_counts.get(&identity) != Some(&1) {
        return None;
    }

    let mut matches = selected_paths.iter().filter(|selected| {
        is_direct_event_node(selected) && selected_identities.get(*selected) == Some(&identity)
    });
    let selected = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(selected)
}

pub(crate) fn is_event_node(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if path.parent() == Some(Path::new("/dev/input")) {
        return name.strip_prefix("event").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit())
        });
    }

    matches!(
        path.parent(),
        Some(parent)
            if parent == Path::new("/dev/input/by-id")
                || parent == Path::new("/dev/input/by-path")
    ) && name.contains("-event")
}

fn update_emergency_chord(
    frame: &InputFrame,
    held_by_device: &mut HashMap<PathBuf, HashSet<u16>>,
) -> bool {
    if frame.resynchronized {
        // SYN_DROPPED invalidates this device's prior key-down history.
        held_by_device.remove(&frame.path);
    }

    {
        let held = held_by_device.entry(frame.path.clone()).or_default();
        for event in &frame.events {
            if event.event_type != EV_KEY {
                continue;
            }
            match event.value {
                1 => {
                    held.insert(event.code);
                }
                0 => {
                    held.remove(&event.code);
                }
                _ => {}
            }
        }
    }

    let contains = |code| held_by_device.values().any(|keys| keys.contains(&code));
    let ctrl = contains(KeyCode::KEY_LEFTCTRL.0) || contains(KeyCode::KEY_RIGHTCTRL.0);
    let alt = contains(KeyCode::KEY_LEFTALT.0) || contains(KeyCode::KEY_RIGHTALT.0);
    ctrl && alt && contains(KeyCode::KEY_ESC.0)
}

/// Captures selected keyboard devices for key mapping. The selected physical
/// mouse remains shared with the desktop until MouseCaptureControl is toggled by F1.
pub(crate) fn run_selected<F>(
    paths: &[PathBuf],
    motion_counters: Arc<InputMotionCounters>,
    mut handler: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnMut(DeviceMessage, &MouseCaptureControl) -> Result<(), String>,
{
    if paths.is_empty() {
        return Err("the profile must select at least one input device".into());
    }

    let mut unique = HashSet::new();
    let mut initial_devices = Vec::<(DeviceInfo, Device)>::new();
    for path in paths {
        if !is_event_node(path) {
            return Err(format!("invalid evdev node path: {}", path.display()).into());
        }
        let resolved = std::fs::canonicalize(path)?;
        if !unique.insert(resolved) {
            return Err(format!("duplicate input device target: {}", path.display()).into());
        }
        let device = Device::open(path)?;
        let info = inspect_device(path.clone(), &device);
        println!("CONTROL_DEVICE {}", info.display_line());
        initial_devices.push((info, device));
    }

    let selected_identities = initial_devices
        .iter()
        .map(|(info, _)| (info.path.clone(), DeviceIdentity::from_info(info)))
        .collect::<HashMap<_, _>>();

    // Keyboard and suppress-only devices are captured for the key mapper. The
    // physical mouse remains visible/usable in GNOME until the user presses F1.
    for (info, device) in &mut initial_devices {
        if !is_deferred_mouse_capture(info) {
            device.grab()?;
        }
    }

    let mouse_capture = MouseCaptureControl::default();
    let queue = Arc::new(PriorityInputQueue::new(256, motion_counters));
    let stop = Arc::new(AtomicBool::new(false));
    let mut active = HashSet::<PathBuf>::new();
    let mut workers = HashMap::<PathBuf, thread::JoinHandle<()>>::new();
    let mut retry_after = HashMap::<PathBuf, Instant>::new();
    let mut held_keys = HashMap::<PathBuf, HashSet<u16>>::new();
    let mut next_scan = Instant::now();

    println!("WayTouch key capture active; the physical mouse remains shared with GNOME until F1.");
    println!(
        "F1 toggles exclusive mouse capture and the Android custom cursor. F1 again restores GNOME."
    );
    println!(
        "F1: capture/release mouse. Emergency stop: Ctrl+Alt+Esc releases all touches and devices."
    );

    for (info, device) in initial_devices {
        let path = info.path.clone();
        let deferred_mouse = is_deferred_mouse_capture(&info);
        match spawn_reader(
            info,
            device,
            WorkerSink::Priority(Arc::clone(&queue)),
            Arc::clone(&stop),
            deferred_mouse.then(|| mouse_capture.shared_flag()),
        ) {
            Ok(worker) => {
                active.insert(path.clone());
                workers.insert(path, worker);
            }
            Err(error) => {
                stop.store(true, Ordering::Release);
                queue.close();
                for (_, worker) in workers.drain() {
                    let _ = worker.join();
                }
                return Err(error.into());
            }
        }
    }

    let mut emergency_stop = false;
    let mut terminal_error: Option<String> = None;

    'control: loop {
        let now = Instant::now();
        if now >= next_scan {
            let enumerated = evdev::enumerate().collect::<Vec<_>>();
            let mut observed = Vec::with_capacity(enumerated.len());
            let mut identity_counts = HashMap::<DeviceIdentity, usize>::new();
            for (event_node, device) in &enumerated {
                let info = inspect_device(event_node.clone(), device);
                *identity_counts
                    .entry(DeviceIdentity::from_info(&info))
                    .or_default() += 1;
                observed.push(info);
            }

            for ((event_node, mut device), observed_info) in enumerated.into_iter().zip(observed) {
                // Stable udev links are resolved every scan. Legacy eventN profile
                // entries fall back to a strong, unique hardware identity captured
                // at startup, never to pathname equality alone.
                let Some(stable_path) = selected_path_for_device(
                    paths,
                    &selected_identities,
                    &identity_counts,
                    &event_node,
                    &observed_info,
                ) else {
                    continue;
                };
                if active.contains(stable_path) {
                    continue;
                }
                if retry_after
                    .get(stable_path)
                    .is_some_and(|retry| *retry > now)
                {
                    continue;
                }

                let mut info = observed_info;
                info.path = stable_path.clone();
                let deferred_mouse = is_deferred_mouse_capture(&info);
                if !deferred_mouse && let Err(error) = device.grab() {
                    eprintln!(
                        "Cannot exclusively capture {} ({}): {error}",
                        stable_path.display(),
                        event_node.display()
                    );
                    retry_after.insert(stable_path.clone(), now + RETRY_DELAY);
                    continue;
                }

                println!(
                    "DEVICE_ADDED {} capture={}",
                    info.display_line(),
                    if deferred_mouse { "F1" } else { "exclusive" }
                );
                match spawn_reader(
                    info,
                    device,
                    WorkerSink::Priority(Arc::clone(&queue)),
                    Arc::clone(&stop),
                    deferred_mouse.then(|| mouse_capture.shared_flag()),
                ) {
                    Ok(worker) => {
                        active.insert(stable_path.clone());
                        workers.insert(stable_path.clone(), worker);
                    }
                    Err(error) => {
                        eprintln!("Cannot start reader for {}: {error}", stable_path.display());
                        retry_after.insert(stable_path.clone(), now + RETRY_DELAY);
                    }
                }
            }
            next_scan = now + SCAN_INTERVAL;
        }

        // Wake periodically even when the user is idle; the controller uses this
        // tick to apply Android layout changes immediately without waiting for a key.
        let timeout = next_scan
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(100));
        match queue.recv_timeout(timeout) {
            Ok(WorkerMessage::Frame(frame)) => {
                if update_emergency_chord(&frame, &mut held_keys) {
                    emergency_stop = true;
                    break 'control;
                }
                if let Err(error) = handler(DeviceMessage::Frame(frame), &mouse_capture) {
                    terminal_error = Some(error);
                    break 'control;
                }
            }
            Ok(WorkerMessage::Stopped { path, name, reason }) => {
                active.remove(&path);
                held_keys.remove(&path);
                if let Some(worker) = workers.remove(&path) {
                    let _ = worker.join();
                }
                retry_after.insert(path.clone(), Instant::now() + RETRY_DELAY);
                eprintln!(
                    "DEVICE_REMOVED path={} name={:?} reason={}",
                    path.display(),
                    name,
                    reason
                );
                if let Err(error) = handler(DeviceMessage::Removed { path }, &mouse_capture) {
                    terminal_error = Some(error);
                    break 'control;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if let Err(error) = handler(DeviceMessage::Tick, &mouse_capture) {
                    terminal_error = Some(error);
                    break 'control;
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                terminal_error = Some("all evdev reader channels disconnected".into());
                break 'control;
            }
        }
    }

    // Stop readers and drop the receiver first so a worker blocked on the bounded
    // queue can exit. Joining closes every evdev fd and releases every EVIOCGRAB.
    mouse_capture.set_captured(false);
    stop.store(true, Ordering::Release);
    queue.close();
    for (_, worker) in workers.drain() {
        let _ = worker.join();
    }

    // Release any virtual contacts still held by the mapping engine before closing
    // the TCP connection to Android.
    for path in paths {
        if let Err(error) = handler(
            DeviceMessage::Removed { path: path.clone() },
            &mouse_capture,
        ) {
            terminal_error.get_or_insert(error);
        }
    }

    if emergency_stop {
        println!("Emergency stop complete: all virtual touches released; input devices ungrabbed.");
    }

    if let Some(error) = terminal_error {
        return Err(io::Error::other(error).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue_frame(path: &str, timestamp_micros: u128, events: Vec<RawInputEvent>) -> InputFrame {
        InputFrame {
            path: PathBuf::from(path),
            device_name: "queue-test".into(),
            timestamp_micros,
            resynchronized: false,
            events,
        }
    }

    #[test]
    fn input_queue_coalesces_adjacent_motion_without_losing_total_delta() {
        let queue = PriorityInputQueue::new(4, Arc::new(InputMotionCounters::default()));
        queue.push(WorkerMessage::Frame(queue_frame(
            "/dev/input/mouse",
            10,
            vec![
                RawInputEvent {
                    event_type: EV_REL,
                    code: RelativeAxisCode::REL_X.0,
                    value: 4,
                },
                RawInputEvent {
                    event_type: EV_REL,
                    code: RelativeAxisCode::REL_Y.0,
                    value: -2,
                },
            ],
        )));
        queue.push(WorkerMessage::Frame(queue_frame(
            "/dev/input/mouse",
            20,
            vec![
                RawInputEvent {
                    event_type: EV_REL,
                    code: RelativeAxisCode::REL_X.0,
                    value: 7,
                },
                RawInputEvent {
                    event_type: EV_REL,
                    code: RelativeAxisCode::REL_Y.0,
                    value: 5,
                },
            ],
        )));

        let WorkerMessage::Frame(frame) = queue.recv_timeout(Duration::ZERO).unwrap() else {
            panic!("expected a motion frame");
        };
        assert_eq!(frame.timestamp_micros, 20);
        assert_eq!(frame.events[0].value, 11);
        assert_eq!(frame.events[1].value, 3);
        assert_eq!(queue.state.lock().unwrap().messages.len(), 0);
    }

    #[test]
    fn input_queue_evicts_motion_before_any_critical_transition() {
        let queue = PriorityInputQueue::new(2, Arc::new(InputMotionCounters::default()));
        queue.push(WorkerMessage::Frame(queue_frame(
            "/dev/input/mouse",
            1,
            vec![RawInputEvent {
                event_type: EV_REL,
                code: RelativeAxisCode::REL_X.0,
                value: 9,
            }],
        )));
        queue.push(WorkerMessage::Frame(queue_frame(
            "/dev/input/keyboard",
            2,
            vec![RawInputEvent {
                event_type: EV_KEY,
                code: KeyCode::KEY_A.0,
                value: 1,
            }],
        )));
        queue.push(WorkerMessage::Frame(queue_frame(
            "/dev/input/keyboard",
            3,
            vec![RawInputEvent {
                event_type: EV_KEY,
                code: KeyCode::KEY_A.0,
                value: 0,
            }],
        )));

        let WorkerMessage::Frame(down) = queue.recv_timeout(Duration::ZERO).unwrap() else {
            panic!("expected key down");
        };
        let WorkerMessage::Frame(up) = queue.recv_timeout(Duration::ZERO).unwrap() else {
            panic!("expected key up");
        };
        assert_eq!(down.events[0].value, 1);
        assert_eq!(up.events[0].value, 0);
    }

    #[test]
    fn input_queue_motion_counters_track_coalescing_eviction_and_incoming_drop() {
        let counters = Arc::new(InputMotionCounters::default());
        let queue = PriorityInputQueue::new(2, Arc::clone(&counters));
        assert!(queue.push(WorkerMessage::Frame(queue_frame(
            "/dev/input/event1",
            1,
            vec![RawInputEvent {
                event_type: EV_REL,
                code: RelativeAxisCode::REL_X.0,
                value: 1,
            }]
        ))));
        assert!(queue.push(WorkerMessage::Frame(queue_frame(
            "/dev/input/event1",
            2,
            vec![RawInputEvent {
                event_type: EV_REL,
                code: RelativeAxisCode::REL_X.0,
                value: 2,
            }]
        ))));
        assert_eq!(counters.coalesced.load(Ordering::Relaxed), 1);

        // The queue currently contains one coalesced MOVE. A disconnect fills
        // the second slot; a subsequent critical key frame evicts that MOVE.
        assert!(queue.push(WorkerMessage::Stopped {
            path: PathBuf::from("/dev/input/event2"),
            name: "test".into(),
            reason: "test".into(),
        }));
        assert!(queue.push(WorkerMessage::Frame(queue_frame(
            "/dev/input/event3",
            3,
            vec![RawInputEvent {
                event_type: EV_KEY,
                code: 30,
                value: 1,
            }]
        ))));
        assert_eq!(counters.dropped.load(Ordering::Relaxed), 1);

        // Now only critical messages remain, so the incoming MOVE itself is
        // dropped rather than stalling the evdev reader.
        assert!(queue.push(WorkerMessage::Frame(queue_frame(
            "/dev/input/event5",
            5,
            vec![RawInputEvent {
                event_type: EV_REL,
                code: RelativeAxisCode::REL_X.0,
                value: 1,
            }]
        ))));
        assert_eq!(counters.dropped.load(Ordering::Relaxed), 2);
        assert_eq!(counters.take(), (1, 2));
    }

    #[test]
    fn input_queue_backpressures_critical_events_instead_of_dropping_key_up() {
        let queue = Arc::new(PriorityInputQueue::new(
            1,
            Arc::new(InputMotionCounters::default()),
        ));
        queue.push(WorkerMessage::Frame(queue_frame(
            "/dev/input/keyboard",
            1,
            vec![RawInputEvent {
                event_type: EV_KEY,
                code: KeyCode::KEY_SPACE.0,
                value: 1,
            }],
        )));
        let writer_queue = Arc::clone(&queue);
        let writer = std::thread::spawn(move || {
            writer_queue.push(WorkerMessage::Frame(queue_frame(
                "/dev/input/keyboard",
                2,
                vec![RawInputEvent {
                    event_type: EV_KEY,
                    code: KeyCode::KEY_SPACE.0,
                    value: 0,
                }],
            )))
        });

        let WorkerMessage::Frame(down) = queue.recv_timeout(Duration::from_secs(1)).unwrap() else {
            panic!("expected key down");
        };
        assert_eq!(down.events[0].value, 1);
        assert!(writer.join().unwrap());
        let WorkerMessage::Frame(up) = queue.recv_timeout(Duration::from_secs(1)).unwrap() else {
            panic!("expected key up");
        };
        assert_eq!(up.events[0].value, 0);
    }

    #[test]
    fn mouse_capture_is_shared_and_starts_released_until_f1() {
        let control = MouseCaptureControl::default();
        let reader_flag = control.shared_flag();
        assert!(!control.is_captured());
        assert!(!reader_flag.load(Ordering::Acquire));
        control.set_captured(true);
        assert!(reader_flag.load(Ordering::Acquire));
        control.set_captured(false);
        assert!(!reader_flag.load(Ordering::Acquire));
    }

    #[test]
    fn input_queue_is_bounded_under_motion_flood_and_preserves_button_edges() {
        let queue = PriorityInputQueue::new(8, Arc::new(InputMotionCounters::default()));
        for timestamp in 0..10_000 {
            queue.push(WorkerMessage::Frame(queue_frame(
                "/dev/input/mouse",
                timestamp,
                vec![RawInputEvent {
                    event_type: EV_REL,
                    code: RelativeAxisCode::REL_X.0,
                    value: 1,
                }],
            )));
        }
        queue.push(WorkerMessage::Frame(queue_frame(
            "/dev/input/mouse",
            10_001,
            vec![RawInputEvent {
                event_type: EV_KEY,
                code: KeyCode::BTN_LEFT.0,
                value: 1,
            }],
        )));
        queue.push(WorkerMessage::Frame(queue_frame(
            "/dev/input/mouse",
            10_002,
            vec![RawInputEvent {
                event_type: EV_KEY,
                code: KeyCode::BTN_LEFT.0,
                value: 0,
            }],
        )));

        let state = queue.state.lock().unwrap();
        assert!(state.messages.len() <= 8);
        assert_eq!(
            state.messages.iter().filter(|message| {
                matches!(message, WorkerMessage::Frame(frame) if frame.events.iter().any(|event| event.event_type == EV_KEY))
            }).count(),
            2
        );
    }

    #[test]
    fn stable_symlink_tracks_a_reconnected_event_node_with_a_new_number() {
        use std::os::unix::fs::symlink;
        use std::time::{SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("waytouch-hotplug-{nonce}"));
        std::fs::create_dir_all(&root).unwrap();
        let first = root.join("event2");
        let second = root.join("event13");
        let stable = root.join("by-id-mouse-event-mouse");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        symlink(&first, &stable).unwrap();

        assert_eq!(
            selected_path_for_event_node(std::slice::from_ref(&stable), &first),
            Some(&stable)
        );
        std::fs::remove_file(&stable).unwrap();
        symlink(&second, &stable).unwrap();
        assert_eq!(
            selected_path_for_event_node(std::slice::from_ref(&stable), &second),
            Some(&stable)
        );
        assert_eq!(
            selected_path_for_event_node(std::slice::from_ref(&stable), &first),
            None
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stable_udev_link_must_still_resolve_to_the_selected_hardware() {
        use std::os::unix::fs::symlink;
        use std::time::{SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("waytouch-udev-identity-{nonce}"));
        std::fs::create_dir_all(&root).unwrap();
        let event = root.join("event8");
        let stable = root.join("mouse-by-id-event-mouse");
        std::fs::write(&event, b"device").unwrap();
        symlink(&event, &stable).unwrap();

        let make_info = |name: &str, product: u16| DeviceInfo {
            path: event.clone(),
            name: name.to_owned(),
            physical_path: Some("usb-1-2/input0".to_owned()),
            unique_name: Some("serial-123".to_owned()),
            input_id: InputId::new(evdev::BusType::BUS_USB, 0x1234, product, 1),
            roles: DeviceRoles {
                keyboard: false,
                relative_pointer: true,
                touchpad: false,
            },
        };
        let selected = make_info("Trusted USB mouse", 0x0001);
        let selected_identity = DeviceIdentity::from_info(&selected);
        let selected_identities = HashMap::from([(stable.clone(), selected_identity)]);
        let counts = HashMap::new();

        assert_eq!(
            selected_path_for_device(
                std::slice::from_ref(&stable),
                &selected_identities,
                &counts,
                &event,
                &selected,
            ),
            Some(&stable)
        );

        let replacement = make_info("Replacement USB mouse", 0x0002);
        assert_eq!(
            selected_path_for_device(
                std::slice::from_ref(&stable),
                &selected_identities,
                &counts,
                &event,
                &replacement,
            ),
            None
        );

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn direct_event_selection_rebinds_only_to_unique_matching_hardware() {
        let selected_path = PathBuf::from("/dev/input/event5");
        let replugged_node = PathBuf::from("/dev/input/event13");
        let make_info = |path: PathBuf, name: &str, product: u16| DeviceInfo {
            path,
            name: name.to_owned(),
            physical_path: Some("i2c-ELAN1206:00".to_owned()),
            unique_name: None,
            input_id: InputId::new(evdev::BusType::BUS_I2C, 0x04f3, product, 1),
            roles: DeviceRoles {
                keyboard: false,
                relative_pointer: true,
                touchpad: false,
            },
        };
        let original = make_info(selected_path.clone(), "ELAN1206:00 Mouse", 0x30f1);
        let replacement = make_info(replugged_node.clone(), "ELAN1206:00 Mouse", 0x30f1);
        let identity = DeviceIdentity::from_info(&original);
        let selected_identities = HashMap::from([(selected_path.clone(), identity.clone())]);
        let identity_counts = HashMap::from([(identity, 1usize)]);

        assert_eq!(
            selected_path_for_device(
                std::slice::from_ref(&selected_path),
                &selected_identities,
                &identity_counts,
                &replugged_node,
                &replacement,
            ),
            Some(&selected_path)
        );

        let unrelated = make_info(replugged_node.clone(), "Unrelated USB Mouse", 0x1234);
        assert_eq!(
            selected_path_for_device(
                std::slice::from_ref(&selected_path),
                &selected_identities,
                &identity_counts,
                &replugged_node,
                &unrelated,
            ),
            None
        );

        let ambiguous_counts = HashMap::from([(DeviceIdentity::from_info(&replacement), 2usize)]);
        assert_eq!(
            selected_path_for_device(
                std::slice::from_ref(&selected_path),
                &selected_identities,
                &ambiguous_counts,
                &replugged_node,
                &replacement,
            ),
            None
        );
    }

    #[test]
    fn emergency_chord_requires_ctrl_alt_and_escape_held_together() {
        let mut held = HashMap::new();
        let frame = |codes: &[(u16, i32)]| InputFrame {
            path: PathBuf::from("/dev/input/event2"),
            device_name: "test keyboard".into(),
            timestamp_micros: 0,
            resynchronized: false,
            events: codes
                .iter()
                .map(|(code, value)| RawInputEvent {
                    event_type: EV_KEY,
                    code: *code,
                    value: *value,
                })
                .collect(),
        };

        assert!(!update_emergency_chord(
            &frame(&[(KeyCode::KEY_LEFTCTRL.0, 1)]),
            &mut held
        ));
        assert!(!update_emergency_chord(
            &frame(&[(KeyCode::KEY_LEFTALT.0, 1)]),
            &mut held
        ));
        assert!(update_emergency_chord(
            &frame(&[(KeyCode::KEY_ESC.0, 1)]),
            &mut held
        ));

        assert!(!update_emergency_chord(
            &frame(&[(KeyCode::KEY_ESC.0, 0)]),
            &mut held
        ));
        assert!(!update_emergency_chord(
            &frame(&[(KeyCode::KEY_LEFTCTRL.0, 0)]),
            &mut held
        ));
    }

    #[test]
    fn emergency_state_is_per_device_and_resync_clears_phantom_modifiers() {
        let first = PathBuf::from("/dev/input/event-kbd-a");
        let second = PathBuf::from("/dev/input/event-kbd-b");
        let mut held = HashMap::<PathBuf, HashSet<u16>>::new();

        let make_frame = |path: &PathBuf, resynchronized: bool, events: &[(u16, i32)]| InputFrame {
            path: path.clone(),
            device_name: "test keyboard".into(),
            timestamp_micros: 0,
            resynchronized,
            events: events
                .iter()
                .map(|(code, value)| RawInputEvent {
                    event_type: EV_KEY,
                    code: *code,
                    value: *value,
                })
                .collect(),
        };

        assert!(!update_emergency_chord(
            &make_frame(&first, false, &[(KeyCode::KEY_LEFTCTRL.0, 1)]),
            &mut held
        ));
        assert!(!update_emergency_chord(
            &make_frame(&second, false, &[(KeyCode::KEY_LEFTALT.0, 1)]),
            &mut held
        ));
        assert!(update_emergency_chord(
            &make_frame(&second, false, &[(KeyCode::KEY_ESC.0, 1)]),
            &mut held
        ));

        // A dropped-event recovery on keyboard A must clear only A's stale Ctrl.
        assert!(!update_emergency_chord(
            &make_frame(&first, true, &[]),
            &mut held
        ));
        assert!(held.get(&first).is_none_or(HashSet::is_empty));

        // Disconnecting keyboard B clears its own Alt/Esc without affecting A.
        held.remove(&second);
        assert!(held.values().all(HashSet::is_empty));
    }

    #[test]
    fn key_event_names_are_decoded() {
        let event = RawInputEvent {
            event_type: EV_KEY,
            code: KeyCode::KEY_W.0,
            value: 1,
        };
        assert_eq!(event_description(&event), "KEY(KEY_W)=down");
    }

    #[test]
    fn relative_mouse_axes_are_decoded() {
        let event = RawInputEvent {
            event_type: EV_REL,
            code: RelativeAxisCode::REL_X.0,
            value: -7,
        };
        assert_eq!(event_description(&event), "REL(REL_X)=-7");
    }

    #[test]
    fn frame_builder_emits_only_at_syn_report() {
        let mut builder = FrameBuilder::default();
        let key = InputEvent::new(EV_KEY, KeyCode::KEY_W.0, 1);
        assert!(builder.push(key).is_none());

        let report = InputEvent::new(EV_SYN, SYN_REPORT, 0);
        let (events, _, recovered) = builder.push(report).unwrap();
        assert_eq!(
            events,
            vec![RawInputEvent {
                event_type: EV_KEY,
                code: KeyCode::KEY_W.0,
                value: 1,
            }]
        );
        assert!(!recovered);
    }

    #[test]
    fn frame_builder_discards_partial_frame_on_syn_dropped() {
        let mut builder = FrameBuilder::default();
        assert!(
            builder
                .push(InputEvent::new(EV_KEY, KeyCode::KEY_W.0, 1))
                .is_none()
        );
        assert!(
            builder
                .push(InputEvent::new(EV_SYN, SYN_DROPPED, 0))
                .is_none()
        );
        let (events, _, recovered) = builder
            .push(InputEvent::new(EV_SYN, SYN_REPORT, 0))
            .unwrap();
        assert!(events.is_empty());
        assert!(recovered);
    }
    #[test]
    fn discovery_returns_only_persistent_aliases_that_resolve_to_the_device() {
        use std::os::unix::fs::symlink;
        use std::time::{SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("waytouch-discovery-{nonce}"));
        let by_id = root.join("by-id");
        let by_path = root.join("by-path");
        std::fs::create_dir_all(&by_id).unwrap();
        std::fs::create_dir_all(&by_path).unwrap();
        let event = root.join("event7");
        let other = root.join("event8");
        std::fs::write(&event, b"mouse").unwrap();
        std::fs::write(&other, b"other").unwrap();
        let mouse_alias = by_id.join("usb-test-event-mouse");
        let topology_alias = by_path.join("platform-test-event-mouse");
        let wrong_alias = by_id.join("usb-other-event-mouse");
        symlink(&event, &mouse_alias).unwrap();
        symlink(&event, &topology_alias).unwrap();
        symlink(&other, &wrong_alias).unwrap();

        let aliases = stable_aliases_for(&root, &event);
        assert_eq!(aliases, vec![mouse_alias.clone(), topology_alias.clone()]);
        assert_eq!(preferred_alias(&aliases, "mouse"), Some(mouse_alias));
        assert_eq!(preferred_alias(&aliases, "keyboard"), Some(topology_alias));
        assert!(stable_aliases_for(&root, &other).contains(&wrong_alias));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn discovery_never_substitutes_a_numeric_event_node_for_missing_stable_link() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("waytouch-no-alias-{nonce}"));
        std::fs::create_dir_all(&root).unwrap();
        let event = root.join("event5");
        std::fs::write(&event, b"built-in mouse").unwrap();

        assert!(stable_aliases_for(&root, &event).is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }
}
