mod android_mouse;
mod auth;
mod control_lock;
mod cursor;
mod input;
mod mapping;
mod mouse;
mod protocol;
mod uinput;

use android_mouse::AndroidMouse;
use cursor::AndroidCursor;
use mapping::{LayoutSnapshot, Profile, TouchMapper};
use mouse::{MouseFrame, MouseLookEngine, TouchDragEvent};
use protocol::{ACK_MAGIC, Action, Frame, Pointer};
use serde::Deserialize;
use std::collections::{HashSet, VecDeque};
use std::env;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uinput::UinputMouse;

const ADDRESS: &str = "127.0.0.1:27183";
const LAYOUT_ADDRESS: &str = "127.0.0.1:27184";
const EV_KEY: u16 = 1;
const KEY_F1: u16 = 59;
const KEY_F2: u16 = 60;
const KEY_F3: u16 = 61;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MouseButtonRoute {
    Mapped,
    Native,
    Drop,
}

/// F2 controls only mouse-button touch mapping. Native button presses that
/// began while mapping was disabled keep their matching release even if F2
/// is toggled back on before the physical button is released.
struct MouseButtonMappingState {
    enabled: bool,
    mapped_held: HashSet<u16>,
    cancelled_until_release: HashSet<u16>,
    native_held: HashSet<u16>,
}

impl Default for MouseButtonMappingState {
    fn default() -> Self {
        Self {
            enabled: true,
            mapped_held: HashSet::new(),
            cancelled_until_release: HashSet::new(),
            native_held: HashSet::new(),
        }
    }
}

impl MouseButtonMappingState {
    fn toggle(&mut self) -> bool {
        self.enabled = !self.enabled;
        self.enabled
    }

    fn cancel_mapped_held(&mut self) {
        self.cancelled_until_release
            .extend(self.mapped_held.drain());
    }

    fn clear_held(&mut self) {
        self.mapped_held.clear();
        self.cancelled_until_release.clear();
        self.native_held.clear();
    }

    fn route(&mut self, code: u16, value: i32) -> MouseButtonRoute {
        if value != 0 && value != 1 {
            return MouseButtonRoute::Drop;
        }

        if self.cancelled_until_release.contains(&code) {
            if value == 0 {
                self.cancelled_until_release.remove(&code);
            }
            return MouseButtonRoute::Drop;
        }

        if self.mapped_held.contains(&code) {
            if value == 0 {
                self.mapped_held.remove(&code);
                return if self.enabled {
                    MouseButtonRoute::Mapped
                } else {
                    MouseButtonRoute::Drop
                };
            }
            return MouseButtonRoute::Drop;
        }

        if self.native_held.contains(&code) {
            if value == 0 {
                self.native_held.remove(&code);
            }
            return MouseButtonRoute::Native;
        }

        if value == 0 {
            return MouseButtonRoute::Drop;
        }

        if self.enabled {
            self.mapped_held.insert(code);
            MouseButtonRoute::Mapped
        } else {
            self.native_held.insert(code);
            MouseButtonRoute::Native
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct LayoutEnvelope {
    revision: u64,
    layout: LayoutSnapshot,
}

struct LayoutStream {
    reader: BufReader<TcpStream>,
    last_activity: Instant,
}

impl LayoutStream {
    fn connect() -> io::Result<Self> {
        let mut stream = TcpStream::connect(LAYOUT_ADDRESS)?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        stream.set_write_timeout(Some(Duration::from_secs(3)))?;
        crate::auth::authenticate(&mut stream)?;
        Ok(Self {
            reader: BufReader::new(stream),
            last_activity: Instant::now(),
        })
    }

    fn exchange(&mut self, command: &str) -> io::Result<String> {
        self.reader.get_mut().write_all(command.as_bytes())?;
        self.reader.get_mut().write_all(b"\n")?;
        self.reader.get_mut().flush()?;
        let mut response = String::new();
        let read = self.reader.read_line(&mut response)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Android layout connection closed",
            ));
        }
        self.last_activity = Instant::now();
        Ok(response.trim_end().to_owned())
    }

    fn request(&mut self, command: &str) -> io::Result<String> {
        if self.last_activity.elapsed() >= Duration::from_secs(5) {
            let pong = self.exchange("PING")?;
            if pong != "PONG" {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("Android layout keepalive returned {pong:?}"),
                ));
            }
        }
        self.exchange(command)
    }
}

#[derive(Clone)]
struct LayoutConnection {
    stream: Arc<Mutex<Option<LayoutStream>>>,
}

impl LayoutConnection {
    fn new() -> Self {
        Self {
            stream: Arc::new(Mutex::new(None)),
        }
    }

    fn request(&self, command: &str) -> io::Result<String> {
        let mut slot = self
            .stream
            .lock()
            .map_err(|_| io::Error::other("Android layout connection lock poisoned"))?;
        if slot.is_none() {
            *slot = Some(LayoutStream::connect()?);
        }

        let first = slot
            .as_mut()
            .expect("layout stream initialized")
            .request(command);
        if first.is_ok() {
            return first;
        }

        // Any EOF, timeout, or protocol failure invalidates the stream. Re-authenticate
        // once and retry the same idempotent GET/DISPLAY or revision-guarded mutation.
        *slot = None;
        let mut replacement = LayoutStream::connect()?;
        let response = replacement.request(command)?;
        *slot = Some(replacement);
        Ok(response)
    }
}

type PendingLayout = Arc<Mutex<Option<LayoutEnvelope>>>;
type SharedLayoutState = Arc<Mutex<(u64, LayoutSnapshot)>>;
type LayoutSyncHandle = (
    PendingLayout,
    SharedLayoutState,
    Arc<AtomicBool>,
    JoinHandle<()>,
);

fn start_layout_sync(
    initial_revision: u64,
    initial_layout: LayoutSnapshot,
    connection: LayoutConnection,
) -> LayoutSyncHandle {
    let pending = Arc::new(Mutex::new(None));
    let shared_state = Arc::new(Mutex::new((initial_revision, initial_layout)));
    let stop = Arc::new(AtomicBool::new(false));
    let thread_pending = Arc::clone(&pending);
    let thread_state = Arc::clone(&shared_state);
    let thread_stop = Arc::clone(&stop);
    let worker = thread::Builder::new()
        .name("waytouch-android-layout".into())
        .spawn(move || {
            let mut conflict_revision: Option<u64> = None;
            while !thread_stop.load(Ordering::Acquire) {
                let local = thread_state.lock().ok().map(|state| state.clone());
                if let Some((local_revision, local_layout)) = local
                    && let Ok(response) = connection.request("GET")
                    && !response.is_empty()
                    && response != "NONE"
                {
                    match serde_json::from_str::<LayoutEnvelope>(&response) {
                        Ok(remote) if conflict_revision == Some(remote.revision) => {
                            // A previous compare-and-swap lost to an Android edit.
                            conflict_revision = None;
                            if let Ok(mut slot) = thread_pending.lock() {
                                *slot = Some(remote);
                            }
                        }
                        Ok(remote) if remote.revision < local_revision => {
                            match serde_json::to_string(&local_layout) {
                                Ok(payload) => {
                                    let command = format!(
                                        "SYNC {} {} {}",
                                        remote.revision, local_revision, payload
                                    );
                                    if let Ok(reply) = connection.request(&command) {
                                        if reply.starts_with("OK ") {
                                            eprintln!(
                                                "LAYOUT_SYNC_PUBLISHED source=linux revision={local_revision}"
                                            );
                                        } else if let Some(revision) = reply
                                            .strip_prefix("CONFLICT ")
                                            .and_then(|value| value.parse().ok())
                                        {
                                            conflict_revision = Some(revision);
                                            eprintln!(
                                                "LAYOUT_SYNC_CONFLICT remote_revision={revision}; Android edit will be pulled"
                                            );
                                        }
                                    }
                                }
                                Err(error) => eprintln!("Cannot encode Linux layout: {error}"),
                            }
                        }
                        Ok(remote)
                            if remote.revision > local_revision || remote.layout != local_layout =>
                        {
                            if let Ok(mut slot) = thread_pending.lock() {
                                *slot = Some(remote);
                            }
                        }
                        Ok(_) => {}
                        Err(error) => eprintln!("Android layout envelope rejected: {error}"),
                    }
                }
                thread::sleep(Duration::from_millis(250));
            }
        })
        .expect("could not start Android layout sync thread");
    (pending, shared_state, stop, worker)
}

#[derive(Clone)]
struct OutboundFrame {
    action: Action,
    action_index: u8,
    pointers: Vec<Pointer>,
    source_timestamp_micros: u128,
}

#[derive(Default)]
struct OutboundState {
    frames: VecDeque<OutboundFrame>,
    closed: bool,
}

/// Ordered bounded TCP output. Adjacent MOVE snapshots for the same pointer
/// set may coalesce; transition frames are retained and backpressure the caller.
struct OutboundFrameQueue {
    state: Mutex<OutboundState>,
    readable: Condvar,
    writable: Condvar,
    capacity: usize,
    coalesced_moves: AtomicU64,
    dropped_moves: AtomicU64,
}

impl OutboundFrameQueue {
    fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(OutboundState::default()),
            readable: Condvar::new(),
            writable: Condvar::new(),
            capacity: capacity.max(1),
            coalesced_moves: AtomicU64::new(0),
            dropped_moves: AtomicU64::new(0),
        }
    }

    fn compatible_move(left: &OutboundFrame, right: &OutboundFrame) -> bool {
        left.action == Action::Move
            && right.action == Action::Move
            && left.pointers.len() == right.pointers.len()
            && left
                .pointers
                .iter()
                .zip(&right.pointers)
                .all(|(a, b)| a.id == b.id && a.tool_type == b.tool_type)
    }

    fn push(&self, frame: OutboundFrame) -> bool {
        let is_move = frame.action == Action::Move;
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return false,
        };
        if state.closed {
            return false;
        }

        if is_move
            && let Some(previous) = state.frames.back_mut()
            && Self::compatible_move(previous, &frame)
        {
            previous.pointers = frame.pointers;
            previous.action_index = frame.action_index;
            previous.source_timestamp_micros = frame.source_timestamp_micros;
            self.coalesced_moves.fetch_add(1, Ordering::Relaxed);
            self.readable.notify_one();
            return true;
        }

        while state.frames.len() >= self.capacity {
            if let Some(position) = state
                .frames
                .iter()
                .position(|queued| queued.action == Action::Move)
            {
                state.frames.remove(position);
                self.dropped_moves.fetch_add(1, Ordering::Relaxed);
                break;
            }
            if is_move {
                // Motion is a snapshot; transitions in the queue already define
                // the authoritative pointer lifecycle.
                self.dropped_moves.fetch_add(1, Ordering::Relaxed);
                return true;
            }
            state = match self.writable.wait(state) {
                Ok(next) => next,
                Err(_) => return false,
            };
            if state.closed {
                return false;
            }
        }

        state.frames.push_back(frame);
        self.readable.notify_one();
        true
    }

    fn pop(&self) -> Option<OutboundFrame> {
        let mut state = self.state.lock().ok()?;
        loop {
            if let Some(frame) = state.frames.pop_front() {
                self.writable.notify_one();
                return Some(frame);
            }
            if state.closed {
                return None;
            }
            state = self.readable.wait(state).ok()?;
        }
    }

    fn take_motion_counters(&self) -> (u64, u64) {
        (
            self.coalesced_moves.swap(0, Ordering::Relaxed),
            self.dropped_moves.swap(0, Ordering::Relaxed),
        )
    }

    fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.closed = true;
            self.readable.notify_all();
            self.writable.notify_all();
        }
    }
}

fn unix_time_micros() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros()
}

fn percentile(sorted: &[f64], percent: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = (((sorted.len() - 1) as f64) * percent).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

fn write_frame_and_ack(
    stream: &mut TcpStream,
    sequence: u32,
    outbound: OutboundFrame,
) -> io::Result<usize> {
    let frame = Frame {
        sequence,
        action: outbound.action,
        action_index: outbound.action_index,
        pointers: outbound.pointers,
    };
    let next_pointer_state = frame
        .encode()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    stream.write_all(&next_pointer_state)?;
    stream.flush()?;

    let mut ack = [0u8; 8];
    stream.read_exact(&mut ack).map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("touch ACK receive failed sequence={sequence}: {error}"),
        )
    })?;
    if &ack[0..4] != ACK_MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid server ACK magic: {:02x?}", &ack[0..4]),
        ));
    }
    let ack_sequence =
        u32::from_le_bytes(ack[4..8].try_into().map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "invalid server ACK sequence")
        })?);
    if ack_sequence != sequence {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("ACK mismatch: expected {sequence}, received {ack_sequence}"),
        ));
    }
    Ok(frame.pointers.len())
}

fn outbound_worker(
    mut stream: TcpStream,
    queue: Arc<OutboundFrameQueue>,
    failure: Arc<Mutex<Option<String>>>,
    input_motion_counters: Arc<input::InputMotionCounters>,
) {
    let mut sequence = 1u32;
    let mut samples = VecDeque::<f64>::with_capacity(2048);
    let mut ack_count = 0u64;
    let mut report_at = Instant::now() + Duration::from_secs(5);
    while let Some(frame) = queue.pop() {
        let source_timestamp_micros = frame.source_timestamp_micros;
        match write_frame_and_ack(&mut stream, sequence, frame) {
            Ok(_pointer_count) => {
                let age_micros = unix_time_micros().saturating_sub(source_timestamp_micros);
                samples.push_back(age_micros as f64 / 1000.0);
                if samples.len() > 2048 {
                    samples.pop_front();
                }
                ack_count = ack_count.saturating_add(1);
                sequence = sequence.wrapping_add(1).max(1);

                if Instant::now() >= report_at {
                    let mut sorted = samples.iter().copied().collect::<Vec<_>>();
                    sorted.sort_by(f64::total_cmp);
                    let (input_coalesced, input_dropped) = input_motion_counters.take();
                    let (tcp_coalesced, tcp_dropped) = queue.take_motion_counters();
                    println!(
                        "TOUCH_STATS count={ack_count} input_coalesced={input_coalesced} input_dropped={input_dropped} tcp_coalesced={tcp_coalesced} tcp_dropped={tcp_dropped} evdev_ack_p50_ms={:.2} evdev_ack_p95_ms={:.2} evdev_ack_p99_ms={:.2}",
                        percentile(&sorted, 0.50),
                        percentile(&sorted, 0.95),
                        percentile(&sorted, 0.99),
                    );
                    samples.clear();
                    ack_count = 0;
                    report_at = Instant::now() + Duration::from_secs(5);
                }
            }
            Err(error) => {
                if let Ok(mut slot) = failure.lock() {
                    *slot = Some(error.to_string());
                }
                queue.close();
                eprintln!("ANDROID_TOUCH_TRANSPORT_ERROR error={error}");
                break;
            }
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}

struct Client {
    queue: Arc<OutboundFrameQueue>,
    failure: Arc<Mutex<Option<String>>>,
    worker: Option<JoinHandle<()>>,
    active: Vec<Pointer>,
    source_timestamp_micros: u128,
    input_motion_counters: Arc<input::InputMotionCounters>,
}

impl Client {
    fn connect() -> io::Result<Self> {
        let mut stream = TcpStream::connect(ADDRESS)?;
        crate::auth::authenticate(&mut stream)?;
        Self::from_authenticated_stream(stream)
    }

    fn from_authenticated_stream(stream: TcpStream) -> io::Result<Self> {
        stream.set_nodelay(true)?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;

        let queue = Arc::new(OutboundFrameQueue::new(256));
        let failure = Arc::new(Mutex::new(None));
        let thread_queue = Arc::clone(&queue);
        let thread_failure = Arc::clone(&failure);
        let input_motion_counters = Arc::new(input::InputMotionCounters::default());
        let thread_input_motion_counters = Arc::clone(&input_motion_counters);
        let worker = thread::Builder::new()
            .name("waytouch-android-touch-tcp".into())
            .spawn(move || {
                outbound_worker(
                    stream,
                    thread_queue,
                    thread_failure,
                    thread_input_motion_counters,
                )
            })?;

        Ok(Self {
            queue,
            failure,
            worker: Some(worker),
            active: Vec::new(),
            source_timestamp_micros: unix_time_micros(),
            input_motion_counters,
        })
    }

    fn set_source_timestamp_micros(&mut self, timestamp_micros: u128) {
        self.source_timestamp_micros = timestamp_micros;
    }

    fn check_status(&self) -> io::Result<()> {
        let failure = self
            .failure
            .lock()
            .map_err(|_| io::Error::other("touch transport status lock poisoned"))?;
        if let Some(error) = failure.as_ref() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                format!("Android touch transport failed: {error}"),
            ));
        }
        Ok(())
    }

    fn send(
        &mut self,
        action: Action,
        action_index: u8,
        pointers: Vec<Pointer>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.check_status()?;
        let frame = Frame {
            sequence: 0,
            action,
            action_index,
            pointers: pointers.clone(),
        };
        let next_active = frame.next_active(&self.active)?;
        if !self.queue.push(OutboundFrame {
            action,
            action_index,
            pointers,
            source_timestamp_micros: self.source_timestamp_micros,
        }) {
            return Err("Android touch output queue is closed".into());
        }
        self.active = next_active;
        Ok(())
    }

    fn cancel_if_active(&mut self) {
        if !self.active.is_empty() {
            let pointers = self.active.clone();
            let _ = self.send(Action::Cancel, 0, pointers);
        }
    }

    fn close_worker(&mut self) {
        self.queue.close();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }

    fn disconnect_without_cancel(mut self) {
        self.active.clear();
        self.close_worker();
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.cancel_if_active();
        self.close_worker();
    }
}

fn parse_xy(args: &[String]) -> Result<(f32, f32), Box<dyn std::error::Error>> {
    if args.len() != 2 {
        return Err("expected X and Y coordinates".into());
    }
    let x: f32 = args[0].parse()?;
    let y: f32 = args[1].parse()?;
    if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 {
        return Err("coordinates must be finite and non-negative".into());
    }
    Ok((x, y))
}

fn run_tap(x: f32, y: f32) -> Result<(), Box<dyn std::error::Error>> {
    let mut client = Client::connect()?;
    client.send(Action::Down, 0, vec![Pointer::finger(0, x, y)])?;
    std::thread::sleep(Duration::from_millis(35));
    client.send(Action::Up, 0, vec![Pointer::finger(0, x, y)])?;
    Ok(())
}

fn run_multitouch_test(x: f32, y: f32) -> Result<(), Box<dyn std::error::Error>> {
    let mut client = Client::connect()?;
    let p0 = Pointer::finger(0, x, y);
    let p5 = Pointer::finger(5, x + 12.0, y + 12.0);
    let p9 = Pointer::finger(9, x + 24.0, y + 24.0);

    client.send(Action::Down, 0, vec![p0])?;
    std::thread::sleep(Duration::from_millis(15));
    client.send(Action::PointerDown, 1, vec![p0, p5])?;
    std::thread::sleep(Duration::from_millis(15));
    client.send(Action::PointerDown, 2, vec![p0, p5, p9])?;
    std::thread::sleep(Duration::from_millis(15));

    client.send(
        Action::Move,
        0,
        vec![
            Pointer::finger(0, x + 2.0, y + 2.0),
            Pointer::finger(5, x + 14.0, y + 14.0),
            Pointer::finger(9, x + 26.0, y + 26.0),
        ],
    )?;
    std::thread::sleep(Duration::from_millis(15));

    client.send(Action::PointerUp, 1, client.active.clone())?;
    std::thread::sleep(Duration::from_millis(15));
    client.send(Action::PointerUp, 1, client.active.clone())?;
    std::thread::sleep(Duration::from_millis(15));
    client.send(Action::Up, 0, client.active.clone())?;
    Ok(())
}

fn run_max_test(x: f32, y: f32) -> Result<(), Box<dyn std::error::Error>> {
    let mut client = Client::connect()?;
    let mut active = vec![Pointer::finger(0, x, y)];
    client.send(Action::Down, 0, active.clone())?;

    for id in 1..10u8 {
        active.push(Pointer::finger(
            id,
            x + f32::from(id) * 2.0,
            y + f32::from(id) * 2.0,
        ));
        client.send(
            Action::PointerDown,
            (active.len() - 1) as u8,
            active.clone(),
        )?;
    }

    let moved: Vec<Pointer> = active
        .iter()
        .map(|p| Pointer::finger(p.id, p.x + 1.0, p.y + 1.0))
        .collect();
    client.send(Action::Move, 0, moved)?;
    std::thread::sleep(Duration::from_millis(20));

    while client.active.len() > 1 {
        let index = (client.active.len() - 1) as u8;
        client.send(Action::PointerUp, index, client.active.clone())?;
    }
    client.send(Action::Up, 0, client.active.clone())?;
    Ok(())
}

fn run_disconnect_test(x: f32, y: f32) -> Result<(), Box<dyn std::error::Error>> {
    let mut client = Client::connect()?;
    let p0 = Pointer::finger(0, x, y);
    let p1 = Pointer::finger(7, x + 10.0, y + 10.0);
    client.send(Action::Down, 0, vec![p0])?;
    client.send(Action::PointerDown, 1, vec![p0, p1])?;
    println!("Closing socket without UP/CANCEL; Android server must synthesize ACTION_CANCEL");
    client.disconnect_without_cancel();
    std::thread::sleep(Duration::from_millis(100));
    Ok(())
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
struct AndroidDisplayInfo {
    width: u32,
    height: u32,
    #[serde(default)]
    rotation: u32,
}

fn query_android_display_info(connection: &LayoutConnection) -> io::Result<AndroidDisplayInfo> {
    let response = connection.request("DISPLAY")?;
    let display: AndroidDisplayInfo = serde_json::from_str(&response)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if display.width < 2 || display.height < 2 || display.rotation > 3 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Android display dimensions or rotation are invalid",
        ));
    }
    Ok(display)
}

type PendingDisplay = Arc<Mutex<Option<AndroidDisplayInfo>>>;

fn start_display_monitor(
    initial: AndroidDisplayInfo,
    connection: LayoutConnection,
) -> (PendingDisplay, Arc<AtomicBool>, JoinHandle<()>) {
    let pending: PendingDisplay = Arc::new(Mutex::new(None));
    let stop = Arc::new(AtomicBool::new(false));
    let thread_pending = Arc::clone(&pending);
    let thread_stop = Arc::clone(&stop);
    let worker = thread::Builder::new()
        .name("waytouch-android-display".into())
        .spawn(move || {
            let mut current = initial;
            while !thread_stop.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(400));
                if thread_stop.load(Ordering::Acquire) {
                    break;
                }
                if let Ok(next) = query_android_display_info(&connection)
                    && next != current
                {
                    if let Ok(mut slot) = thread_pending.lock() {
                        *slot = Some(next);
                    }
                    current = next;
                }
            }
        })
        .expect("could not start Android display monitor");
    (pending, stop, worker)
}

fn run_device_discovery(profile_path: Option<PathBuf>) -> Result<(), Box<dyn std::error::Error>> {
    let discovered = input::discover_stable_devices()?;
    println!("DEVICE_DISCOVERY_BEGIN");
    for path in &discovered.keyboard {
        println!("KEYBOARD {}", path.display());
    }
    for path in &discovered.mouse {
        println!("MOUSE {}", path.display());
    }
    for path in &discovered.suppress {
        println!("SUPPRESS {}", path.display());
    }
    println!(
        "DEVICE_DISCOVERY_COUNTS keyboard={} mouse={} suppress={}",
        discovered.keyboard.len(),
        discovered.mouse.len(),
        discovered.suppress.len()
    );

    if let Some(path) = profile_path {
        if discovered.keyboard.is_empty() || discovered.mouse.is_empty() {
            return Err(
                "discovery did not find at least one keyboard and one stable relative mouse link; connect the required devices and retry"
                    .into(),
            );
        }
        let mut profile = Profile::load(&path)?;
        profile.replace_device_paths(discovered.keyboard, discovered.mouse, discovered.suppress)?;
        profile.persist_atomic(&path)?;
        println!("DEVICE_DISCOVERY_APPLIED profile={}", path.display());
    }
    Ok(())
}

fn check_profile(profile_path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let profile = Profile::load(&profile_path)?;
    let paths = profile.device_paths();
    println!(
        "PROFILE_OK path={} devices={} action_buttons={} max_pointers={}",
        profile_path.display(),
        paths.len(),
        profile.buttons.len(),
        protocol::MAX_POINTERS
    );
    for path in paths {
        println!("PROFILE_DEVICE {}", path.display());
    }
    for path in profile.suppressed_device_paths() {
        println!("PROFILE_SUPPRESS_ONLY {}", path.display());
    }
    if !profile.has_required_devices() {
        println!("PROFILE_NEEDS_DISCOVERY run=waytouch-host discover --apply <profile.json>");
    }
    Ok(())
}

fn run_control_session(profile_path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let mut profile = Profile::load(&profile_path)?;
    if !profile.has_required_devices() {
        return Err(
            format!(
                "profile has no discovered keyboard/mouse assignment; run waytouch-host discover --apply {} first",
                profile_path.display()
            )
            .into(),
        );
    }
    let paths = profile.device_paths();
    println!("Loading profile: {}", profile_path.display());
    println!("Configured input devices: {}", paths.len());

    let mut client = Client::connect()?;
    let layout_connection = LayoutConnection::new();
    println!("Connected to Android WTCH v2 at {ADDRESS}");
    let (mut current_display, display_connected) =
        match query_android_display_info(&layout_connection) {
            Ok(display) => (display, true),
            Err(error) => {
                let (width, height) = profile.display_size();
                eprintln!(
                    "ANDROID_DISPLAY fallback profile={}x{} rotation=0 reason={error}",
                    width, height
                );
                (
                    AndroidDisplayInfo {
                        width,
                        height,
                        rotation: 0,
                    },
                    false,
                )
            }
        };
    let mut calibration_rotation = profile.display_rotation();
    let initial_rotation_delta = (current_display.rotation + 4 - calibration_rotation) % 4;
    profile.set_display_size(current_display.width, current_display.height);
    println!(
        "ANDROID_DISPLAY calibrated width={} height={} rotation={} source={}",
        current_display.width,
        current_display.height,
        current_display.rotation,
        if display_connected {
            "android"
        } else {
            "profile-fallback"
        }
    );
    let (display_pending, display_stop, display_worker) =
        start_display_monitor(current_display, layout_connection.clone());
    let mut android_mouse = profile.mouse.enabled.then(AndroidMouse::new);
    if android_mouse.is_some() {
        println!("Android mouse MotionEvent bridge enabled at 127.0.0.1:27186");
    } else {
        println!("Android mouse forwarding is disabled; mapped touch bindings remain active.");
    }

    let mut mapper = TouchMapper::new(profile.clone());
    mapper.set_rotation_delta(initial_rotation_delta);
    let (display_width, display_height) = profile.display_size();
    let mut mouse_look =
        MouseLookEngine::with_display(profile.mouse.clone(), display_width, display_height);
    let mut android_cursor =
        AndroidCursor::new(profile.mouse.cursor.clone(), display_width, display_height);
    // F1 exclusively transfers physical mouse ownership between GNOME and
    // Android. F2 independently toggles only mapped mouse-button actions.
    // F3 retains the separate optional camera-aim lock.
    android_cursor.set_active(false, 0);
    let mut pointer_capture_active = false;
    let mut mouse_button_mapping = MouseButtonMappingState::default();
    println!(
        "MOUSE_CONFIG mode={:?} capture_key=F1 mouse_button_mapping_key=F2 aim_key=F3 host_cursor_lock=disabled",
        profile.mouse.look.mode
    );
    let mut mouse_input_seen = false;
    let mut mouse_motion_seen = false;
    let mut previous_aim_state = false;
    let mut aim_owner: Option<PathBuf> = None;
    let (layout_pending, layout_state, layout_stop, layout_worker) = start_layout_sync(
        mapper.layout_revision(),
        mapper.layout_snapshot(),
        layout_connection.clone(),
    );
    println!(
        "Android live layout channel: {LAYOUT_ADDRESS} (revision={}, poll interval 250 ms)",
        mapper.layout_revision()
    );
    let control_result = input::run_selected(
        &paths,
        Arc::clone(&client.input_motion_counters),
        |message, input_control| {
            let source_timestamp_micros = match &message {
                input::DeviceMessage::Frame(frame) => frame.timestamp_micros,
                _ => unix_time_micros(),
            };
            client.set_source_timestamp_micros(source_timestamp_micros);
            if let Some(mouse) = android_mouse.as_ref()
                && let Some((x, y)) = mouse.take_position()
            {
                android_cursor.set_position_pixels(x, y, source_timestamp_micros);
            }
            client.check_status().map_err(|error| error.to_string())?;
            if let Some(next_display) = display_pending
                .lock()
                .map_err(|_| "Android display state lock was poisoned")?
                .take()
                && next_display != current_display
            {
                let next_rotation_delta = (next_display.rotation + 4 - calibration_rotation) % 4;
                mapper.set_display_geometry(
                    next_display.width,
                    next_display.height,
                    next_rotation_delta,
                    &mut |action, index, pointers| {
                        client
                            .send(action, index, pointers)
                            .map_err(|error| error.to_string())
                    },
                )?;
                profile.set_display_size(next_display.width, next_display.height);
                mouse_look.set_display_geometry(
                    next_display.width,
                    next_display.height,
                    next_rotation_delta,
                );
                android_cursor.set_display_geometry(
                    next_display.width,
                    next_display.height,
                    next_rotation_delta,
                );
                println!(
                    "ANDROID_DISPLAY_CHANGED width={} height={} rotation={} previous_rotation={}",
                    next_display.width,
                    next_display.height,
                    next_display.rotation,
                    current_display.rotation
                );
                current_display = next_display;
            }

            let mut message = message;
            let f1_timestamp = match &mut message {
                input::DeviceMessage::Frame(frame) if profile.is_keyboard_device(&frame.path) => {
                    let mut pressed = false;
                    frame.events.retain(|event| {
                        if event.event_type == EV_KEY && event.code == KEY_F1 {
                            if event.value == 1 {
                                pressed = true;
                            }
                            false
                        } else {
                            true
                        }
                    });
                    pressed.then_some(frame.timestamp_micros)
                }
                _ => None,
            };
            if let Some(timestamp_micros) = f1_timestamp {
                pointer_capture_active = !pointer_capture_active;
                input_control.set_captured(pointer_capture_active);
                android_cursor.set_active(pointer_capture_active, timestamp_micros);
                if let Some(mouse) = android_mouse.as_ref() {
                    mouse.set_cursor_active(pointer_capture_active);
                }

                if !pointer_capture_active {
                    let mut send_mouse = |frame| {
                        if let Some(mouse) = android_mouse.as_ref() {
                            mouse.send(frame).map(|_| ())
                        } else {
                            Ok(())
                        }
                    };
                    let mut send_camera_touch = |event| match event {
                        TouchDragEvent::Down(position) => mapper
                            .camera_drag_start(position, &mut |action, index, pointers| {
                                client
                                    .send(action, index, pointers)
                                    .map_err(|e| e.to_string())
                            })
                            .map(|_| ()),
                        TouchDragEvent::Move(position) => {
                            mapper.camera_drag_move(position, &mut |action, index, pointers| {
                                client
                                    .send(action, index, pointers)
                                    .map_err(|e| e.to_string())
                            })
                        }
                        TouchDragEvent::Up => {
                            mapper.camera_drag_end(&mut |action, index, pointers| {
                                client
                                    .send(action, index, pointers)
                                    .map_err(|e| e.to_string())
                            })
                        }
                    };
                    mouse_look.release_all_with_touch(&mut send_mouse, &mut send_camera_touch)?;
                    previous_aim_state = false;
                    aim_owner = None;
                    android_cursor.set_aiming(false, timestamp_micros);

                    // Release mapped controls belonging to mouse devices. The
                    // F2 mapping preference itself is intentionally preserved.
                    for path in paths.iter().filter(|path| profile.is_mouse_device(path)) {
                        mapper.handle_message(
                            input::DeviceMessage::Removed { path: path.clone() },
                            &mut |action, index, pointers| {
                                client
                                    .send(action, index, pointers)
                                    .map_err(|e| e.to_string())
                            },
                        )?;
                    }
                    mouse_button_mapping.clear_held();
                }

                println!(
                    "MOUSE_CAPTURE state={} source=F1",
                    if pointer_capture_active {
                        "active"
                    } else {
                        "inactive"
                    }
                );
            }

            let f2_timestamp = match &mut message {
                input::DeviceMessage::Frame(frame) if profile.is_keyboard_device(&frame.path) => {
                    let mut pressed = false;
                    frame.events.retain(|event| {
                        if event.event_type == EV_KEY && event.code == KEY_F2 {
                            if event.value == 1 {
                                pressed = true;
                            }
                            false
                        } else {
                            true
                        }
                    });
                    pressed.then_some((frame.timestamp_micros, frame.path.clone()))
                }
                _ => None,
            };
            if f2_timestamp.is_some() {
                let enabled = mouse_button_mapping.toggle();
                if !enabled {
                    // End any mouse-button touch already in progress. F1 mouse
                    // capture, Android cursor, camera aim and keyboard mapping
                    // are deliberately left untouched.
                    for path in paths.iter().filter(|path| profile.is_mouse_device(path)) {
                        mapper.handle_message(
                            input::DeviceMessage::Removed { path: path.clone() },
                            &mut |action, index, pointers| {
                                client
                                    .send(action, index, pointers)
                                    .map_err(|e| e.to_string())
                            },
                        )?;
                    }
                    mouse_button_mapping.cancel_mapped_held();
                }
                println!(
                    "MOUSE_BUTTON_MAPPING state={} source=F2",
                    if enabled { "enabled" } else { "disabled" }
                );
            }

            let f3_timestamp = match &mut message {
                input::DeviceMessage::Frame(frame) if profile.is_keyboard_device(&frame.path) => {
                    let mut pressed = false;
                    frame.events.retain(|event| {
                        if event.event_type == EV_KEY && event.code == KEY_F3 {
                            if event.value == 1 {
                                pressed = true;
                            }
                            false
                        } else {
                            true
                        }
                    });
                    pressed.then_some((frame.timestamp_micros, frame.path.clone()))
                }
                _ => None,
            };
            if let Some((timestamp_micros, source_path)) = f3_timestamp {
                if !pointer_capture_active {
                    println!("HOTKEY_F3_IGNORED reason=mouse_capture_inactive");
                } else {
                    let target = !mouse_look.is_aiming();
                    let mut send_hotkey_touch = |event| {
                        match event {
                            TouchDragEvent::Down(position) => mapper
                                .camera_drag_start(position, &mut |action, index, pointers| {
                                    client.send(action, index, pointers).map_err(|error| error.to_string())
                                })
                                .map(|started| {
                                    if !started {
                                        eprintln!("CAMERA_TOUCH_SKIPPED reason=Android ten-pointer limit is already reached");
                                    }
                                }),
                            TouchDragEvent::Move(position) => mapper.camera_drag_move(
                                position,
                                &mut |action, index, pointers| {
                                    client.send(action, index, pointers).map_err(|error| error.to_string())
                                },
                            ),
                            TouchDragEvent::Up => mapper.camera_drag_end(
                                &mut |action, index, pointers| {
                                    client.send(action, index, pointers).map_err(|error| error.to_string())
                                },
                            ),
                        }
                    };
                    mouse_look.set_manual_lock(target, &mut send_hotkey_touch)?;
                    let aim_state = mouse_look.is_aiming();
                    aim_owner = aim_state.then_some(source_path);
                    if aim_state != previous_aim_state {
                        previous_aim_state = aim_state;
                        if let Some(mouse) = android_mouse.as_ref() {
                            mouse.set_aiming(aim_state);
                        }
                        android_cursor.set_aiming(aim_state, timestamp_micros);
                        println!(
                            "AIM_LOCK state={} source=F3",
                            if aim_state { "locked" } else { "unlocked" }
                        );
                    }
                    println!(
                        "HOTKEY_F3 action={}",
                        if aim_state { "lock" } else { "unlock" }
                    );
                }
            }

            let pending = layout_pending
                .lock()
                .map_err(|_| "Android layout synchronization lock was poisoned")?
                .take();
            if let Some(envelope) = pending {
                calibration_rotation = current_display.rotation;
                mapper.set_display_calibration_rotation(calibration_rotation);
                mapper.apply_layout(envelope.layout, &mut |action, index, pointers| {
                    client
                        .send(action, index, pointers)
                        .map_err(|error| error.to_string())
                })?;
                mapper.set_layout_revision(envelope.revision);
                mapper.refresh_movement(&mut |action, index, pointers| {
                    client
                        .send(action, index, pointers)
                        .map_err(|error| error.to_string())
                })?;
                mapper.refresh_buttons(&mut |action, index, pointers| {
                    client
                        .send(action, index, pointers)
                        .map_err(|error| error.to_string())
                })?;
                mapper
                    .persist_profile(&profile_path)
                    .map_err(|error| format!("could not persist synchronized layout: {error}"))?;
                let snapshot = mapper.layout_snapshot();
                *layout_state
                    .lock()
                    .map_err(|_| "Android layout state lock was poisoned")? =
                    (envelope.revision, snapshot);
                println!(
                    "LAYOUT_SYNC_APPLIED source=android revision={} profile={}",
                    envelope.revision,
                    profile_path.display()
                );
            }
            if let input::DeviceMessage::Removed { path } = &message
                && aim_owner.as_deref() == Some(path.as_path())
            {
                let mut release_camera_touch = |event| match event {
                    TouchDragEvent::Down(position) => mapper
                        .camera_drag_start(position, &mut |action, index, pointers| {
                            client
                                .send(action, index, pointers)
                                .map_err(|error| error.to_string())
                        })
                        .map(|_| ()),
                    TouchDragEvent::Move(position) => {
                        mapper.camera_drag_move(position, &mut |action, index, pointers| {
                            client
                                .send(action, index, pointers)
                                .map_err(|error| error.to_string())
                        })
                    }
                    TouchDragEvent::Up => mapper.camera_drag_end(&mut |action, index, pointers| {
                        client
                            .send(action, index, pointers)
                            .map_err(|error| error.to_string())
                    }),
                };
                mouse_look.set_manual_lock(false, &mut release_camera_touch)?;
                aim_owner = None;
                previous_aim_state = false;
                if let Some(mouse) = android_mouse.as_ref() {
                    mouse.set_aiming(false);
                }
                println!(
                    "AIM_LOCK state=unlocked reason=aim_keyboard_disconnected path={}",
                    path.display()
                );
            }

            let mouse_path = match &message {
                input::DeviceMessage::Frame(frame) => profile.is_mouse_device(&frame.path),
                input::DeviceMessage::Removed { path } => profile.is_mouse_device(path),
                input::DeviceMessage::Tick => false,
            };
            // Compute mapped-button routing once so the native mouse bridge
            // and TouchMapper can never consume the same button edge.
            let mouse_button_routes: Option<Vec<Option<MouseButtonRoute>>> = if mouse_path
                && input_control.is_captured()
            {
                match &message {
                    input::DeviceMessage::Frame(frame) => {
                        if frame.resynchronized {
                            mouse_button_mapping.clear_held();
                        }
                        Some(
                            frame
                                .events
                                .iter()
                                .map(|event| {
                                    if event.event_type == EV_KEY
                                        && mapper.is_mapped_mouse_button(event.code)
                                    {
                                        Some(mouse_button_mapping.route(event.code, event.value))
                                    } else {
                                        None
                                    }
                                })
                                .collect(),
                        )
                    }
                    _ => None,
                }
            } else {
                None
            };

            if mouse_path && !input_control.is_captured() {
                if let input::DeviceMessage::Removed { path } = &message {
                    mapper.handle_message(
                        input::DeviceMessage::Removed { path: path.clone() },
                        &mut |action, index, pointers| {
                            client
                                .send(action, index, pointers)
                                .map_err(|e| e.to_string())
                        },
                    )?;
                    mouse_button_mapping.clear_held();
                }
                return Ok(());
            }

            if mouse_path {
                let mut send_mouse = |mouse_frame: MouseFrame| {
                    if let Some(android_mouse) = android_mouse.as_ref() {
                        android_mouse.send(mouse_frame).map(|_| ())
                    } else {
                        Ok(())
                    }
                };
                let mut send_camera_touch = |event| {
                    match event {
                TouchDragEvent::Down(position) => mapper.camera_drag_start(position, &mut |action, index, pointers| {
                    client.send(action, index, pointers).map_err(|error| error.to_string())
                }).map(|started| {
                    if !started {
                        eprintln!("CAMERA_TOUCH_SKIPPED reason=Android ten-pointer limit is already reached");
                    }
                }),
                TouchDragEvent::Move(position) => mapper.camera_drag_move(position, &mut |action, index, pointers| {
                    client.send(action, index, pointers).map_err(|error| error.to_string())
                }),
                TouchDragEvent::Up => mapper.camera_drag_end(&mut |action, index, pointers| {
                    client.send(action, index, pointers).map_err(|error| error.to_string())
                }),
            }
                };
                match &message {
                    input::DeviceMessage::Frame(frame) => {
                        if !mouse_input_seen {
                            mouse_input_seen = true;
                            println!(
                                "MOUSE_INPUT_ACTIVE path={} name={:?}",
                                frame.path.display(),
                                frame.device_name
                            );
                        }
                        if !mouse_motion_seen {
                            let motion = frame.events.iter().any(|event| {
                                event.event_type == 2 && (event.code == 0 || event.code == 1)
                            });
                            if motion {
                                mouse_motion_seen = true;
                                println!("MOUSE_MOTION_DETECTED first_relative_frame=true");
                            }
                        }
                        let mouse_routes = mouse_button_routes
                            .as_ref()
                            .expect("captured mouse frames have routing decisions");

                        let mut mouse_frame = frame.clone();
                        let mut route_index = 0usize;
                        mouse_frame.events.retain(|_| {
                            let route = mouse_routes[route_index];
                            route_index += 1;
                            matches!(route, None | Some(MouseButtonRoute::Native))
                        });
                        mouse_look.handle_frame_with_touch(
                            &mouse_frame,
                            &mut send_mouse,
                            &mut send_camera_touch,
                        )?;
                        drop(send_camera_touch);
                        if let Some(mouse) = android_mouse.as_ref()
                            && let Some((x, y)) = mouse.take_position()
                        {
                            android_cursor.set_position_pixels(x, y, frame.timestamp_micros);
                        }
                        let aim_state = mouse_look.is_aiming();
                        if aim_state != previous_aim_state {
                            previous_aim_state = aim_state;
                            println!(
                                "AIM_LOCK state={} source=F3",
                                if aim_state { "locked" } else { "unlocked" }
                            );
                            if let Some(mouse) = android_mouse.as_ref() {
                                mouse.set_aiming(aim_state);
                            }
                        }
                        // The cursor follows the exact frames delivered to Android,
                        // so camera sensitivity, Y inversion and TouchDrag suppression
                        // cannot desynchronize the custom overlay from the native pointer.
                        android_cursor.set_aiming(aim_state, frame.timestamp_micros);
                    }
                    input::DeviceMessage::Removed { .. } => {
                        mouse_look
                            .release_all_with_touch(&mut send_mouse, &mut send_camera_touch)?;
                        mouse_button_mapping.clear_held();
                        aim_owner = None;
                        if pointer_capture_active {
                            pointer_capture_active = false;
                            input_control.set_captured(false);
                            if let Some(mouse) = android_mouse.as_ref() {
                                mouse.set_cursor_active(false);
                            }
                            println!("MOUSE_CAPTURE state=inactive reason=mouse_disconnected");
                        }
                        if previous_aim_state {
                            previous_aim_state = false;
                            println!("AIM_LOCK state=unlocked reason=mouse_disconnected");
                        }
                        android_cursor.device_removed();
                    }
                    input::DeviceMessage::Tick => {}
                }
            }
            if let (Some(routes), input::DeviceMessage::Frame(frame)) =
                (&mouse_button_routes, &mut message)
            {
                // TouchMapper sees only Mapped edges. Native and cancelled
                // edges were already consumed or deliberately dropped.
                let mut route_index = 0usize;
                frame.events.retain(|_| {
                    let route = routes[route_index];
                    route_index += 1;
                    matches!(route, None | Some(MouseButtonRoute::Mapped))
                });
            }
            mapper
                .handle_message(message, &mut |action, index, pointers| {
                    client
                        .send(action, index, pointers)
                        .map_err(|error| error.to_string())
                })
                .map_err(|error| {
                    eprintln!("Touch mapping failed: {error}");
                    error
                })
        },
    );
    if let Some(mouse) = android_mouse.as_mut() {
        // Ensure the native Android hover state exits when F1 capture ends.
        mouse.set_cursor_active(false);
        mouse.shutdown();
    }
    android_cursor.hide();
    layout_stop.store(true, Ordering::Release);
    let _ = layout_worker.join();
    display_stop.store(true, Ordering::Release);
    let _ = display_worker.join();
    control_result
}

/// Keep the controller alive across transient Android/ADB socket failures and
/// evdev hot-unplug events. A failed session is fully torn down before the next
/// attempt, so pointer/touch state is cancelled and device grabs are released.
fn run_control(profile_path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    control_lock::arm_parent_death_signal()?;
    let _control_lock = control_lock::ControlLock::acquire()?;

    // Fail fast for a malformed or incomplete profile instead of retrying it.
    let validated_profile = Profile::load(&profile_path)?;
    if !validated_profile.has_required_devices() {
        return Err(
            format!(
                "profile has no discovered keyboard/mouse assignment; run waytouch-host discover --apply {} first",
                profile_path.display()
            )
            .into(),
        );
    }

    let mut retry_delay = Duration::from_millis(250);
    loop {
        match run_control_session(profile_path.clone()) {
            Ok(()) => return Ok(()),
            Err(error) => {
                if error
                    .downcast_ref::<io::Error>()
                    .is_some_and(|io_error| io_error.kind() == io::ErrorKind::AlreadyExists)
                {
                    return Err(error);
                }

                eprintln!(
                    "CONTROL_SESSION_INTERRUPTED error={error}; reconnecting in {}ms",
                    retry_delay.as_millis()
                );
                thread::sleep(retry_delay);
                retry_delay = retry_delay.saturating_mul(2).min(Duration::from_secs(4));
            }
        }
    }
}

fn run_uinput_test() -> Result<(), Box<dyn std::error::Error>> {
    let mut mouse = UinputMouse::create()?;
    println!(
        "UINPUT_CREATED name=WayTouch Virtual Mouse node={}",
        mouse.event_node().display()
    );
    std::thread::sleep(Duration::from_millis(400));
    mouse.send(MouseFrame {
        action: 7,
        buttons: 0,
        action_button: 0,
        relative_x: 4.0,
        relative_y: -3.0,
        horizontal_scroll: 0.0,
        vertical_scroll: 0.0,
    })?;
    mouse.send(MouseFrame {
        action: 11,
        buttons: 2,
        action_button: 2,
        relative_x: 0.0,
        relative_y: 0.0,
        horizontal_scroll: 0.0,
        vertical_scroll: 0.0,
    })?;
    mouse.send(MouseFrame {
        action: 7,
        buttons: 2,
        action_button: 0,
        relative_x: 12.5,
        relative_y: -6.5,
        horizontal_scroll: 0.0,
        vertical_scroll: 0.0,
    })?;
    mouse.send(MouseFrame {
        action: 8,
        buttons: 2,
        action_button: 0,
        relative_x: 0.0,
        relative_y: 0.0,
        horizontal_scroll: 0.0,
        vertical_scroll: 1.0,
    })?;
    mouse.send(MouseFrame {
        action: 12,
        buttons: 0,
        action_button: 2,
        relative_x: 0.0,
        relative_y: 0.0,
        horizontal_scroll: 0.0,
        vertical_scroll: 0.0,
    })?;
    std::thread::sleep(Duration::from_millis(750));
    println!("UINPUT_TEST_EVENTS_SENT; dropping device now");
    Ok(())
}

fn print_usage() {
    eprintln!("Usage:");
    eprintln!("  waytouch-host devices");
    eprintln!("  waytouch-host discover");
    eprintln!("  waytouch-host discover --apply <profile.json>");
    eprintln!("  waytouch-host listen [--device /dev/input/eventX]");
    eprintln!("  waytouch-host profile-check [profile.json]");
    eprintln!("  waytouch-host control [profile.json]");
    eprintln!("  waytouch-host tap <x> <y>");
    eprintln!("  waytouch-host multitouch-test <x> <y>");
    eprintln!("  waytouch-host multitouch-max-test <x> <y>");
    eprintln!("  waytouch-host disconnect-test <x> <y>");
    eprintln!("  waytouch-host protocol-info");
    eprintln!("  waytouch-host uinput-test   (moves pointer slightly and tests buttons/scroll)");
    eprintln!(
        "    Mouse output: /dev/uinput virtual mouse; normal pointer motion + right-button relative look"
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("devices") if args.len() == 1 => input::list_devices().map_err(Into::into),
        Some("discover") if args.len() == 1 => run_device_discovery(None),
        Some("discover") if args.len() == 3 && args[1] == "--apply" => {
            run_device_discovery(Some(PathBuf::from(&args[2])))
        }
        Some("listen") if args.len() == 1 => input::listen(None).map_err(Into::into),
        Some("listen") if args.len() == 3 && args[1] == "--device" => {
            input::listen(Some(PathBuf::from(&args[2]))).map_err(Into::into)
        }
        Some("profile-check") if args.len() <= 2 => {
            let profile = args
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("profiles/default.json"));
            check_profile(profile)
        }
        Some("control") if args.len() <= 2 => {
            let profile = args
                .get(1)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("profiles/default.json"));
            run_control(profile)
        }
        Some("tap") => {
            let (x, y) = parse_xy(&args[1..])?;
            run_tap(x, y)
        }
        Some("multitouch-test") => {
            let (x, y) = parse_xy(&args[1..])?;
            run_multitouch_test(x, y)
        }
        Some("multitouch-max-test") => {
            let (x, y) = parse_xy(&args[1..])?;
            run_max_test(x, y)
        }
        Some("disconnect-test") => {
            let (x, y) = parse_xy(&args[1..])?;
            run_disconnect_test(x, y)
        }
        Some("uinput-test") if args.len() == 1 => run_uinput_test(),
        Some("protocol-info") if args.len() == 1 => {
            println!(
                "WayTouch protocol: WTCH v2 touch (10 pointers); mouse is forwarded through the host uinput virtual device"
            );
            Ok(())
        }
        _ => {
            print_usage();
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod mouse_button_mapping_tests {
    use super::*;

    #[test]
    fn f2_mapping_toggle_is_independent_from_f1_capture() {
        let mut capture_active = false;
        let mut mapping = MouseButtonMappingState::default();

        // F1 only changes capture ownership.
        capture_active = !capture_active;
        assert!(capture_active);
        assert!(mapping.enabled);

        // F2 only changes mouse-button mapping.
        assert!(!mapping.toggle());
        assert!(capture_active);

        // F1 can release/reacquire the mouse without changing F2's preference.
        capture_active = !capture_active;
        assert!(!capture_active);
        assert!(!mapping.enabled);
        capture_active = !capture_active;
        assert!(capture_active);
        assert!(!mapping.enabled);

        // F2 also works while GNOME owns the mouse.
        capture_active = !capture_active;
        assert!(!capture_active);
        assert!(mapping.toggle());
        assert!(!capture_active);
    }

    #[test]
    fn enabled_mapping_routes_button_edges_only_to_touch_mapper() {
        let mut mapping = MouseButtonMappingState::default();
        assert_eq!(mapping.route(272, 1), MouseButtonRoute::Mapped);
        assert_eq!(mapping.route(272, 0), MouseButtonRoute::Mapped);
        assert!(mapping.mapped_held.is_empty());
        assert!(mapping.native_held.is_empty());
    }

    #[test]
    fn disabled_mapping_routes_native_click_and_drains_release_after_reenable() {
        let mut mapping = MouseButtonMappingState::default();
        assert!(!mapping.toggle());
        assert_eq!(mapping.route(272, 1), MouseButtonRoute::Native);
        assert!(mapping.toggle());
        assert_eq!(mapping.route(272, 0), MouseButtonRoute::Native);
        assert_eq!(mapping.route(272, 1), MouseButtonRoute::Mapped);
        assert_eq!(mapping.route(272, 0), MouseButtonRoute::Mapped);
    }

    #[test]
    fn disabling_mapping_cancels_held_touch_and_drops_its_late_release() {
        let mut mapping = MouseButtonMappingState::default();
        assert_eq!(mapping.route(272, 1), MouseButtonRoute::Mapped);

        assert!(!mapping.toggle());
        mapping.cancel_mapped_held();
        // Re-enabling before the physical release must not create an orphan UP.
        assert!(mapping.toggle());
        assert_eq!(mapping.route(272, 0), MouseButtonRoute::Drop);
        assert_eq!(mapping.route(272, 1), MouseButtonRoute::Mapped);
        assert_eq!(mapping.route(272, 0), MouseButtonRoute::Mapped);
    }

    #[test]
    fn unmatched_releases_and_mouse_repeat_edges_are_dropped() {
        let mut mapping = MouseButtonMappingState::default();
        assert_eq!(mapping.route(272, 0), MouseButtonRoute::Drop);
        assert_eq!(mapping.route(272, 2), MouseButtonRoute::Drop);
        assert_eq!(mapping.route(272, 1), MouseButtonRoute::Mapped);
        assert_eq!(mapping.route(272, 2), MouseButtonRoute::Drop);
        assert_eq!(mapping.route(272, 0), MouseButtonRoute::Mapped);
    }
}

#[cfg(test)]
mod display_tests {
    use super::*;

    #[test]
    fn display_info_parses_rotation_and_defaults_legacy_rotation_to_zero() {
        let landscape: AndroidDisplayInfo =
            serde_json::from_str(r#"{"width":1920,"height":1080,"rotation":1}"#).unwrap();
        assert_eq!(
            landscape,
            AndroidDisplayInfo {
                width: 1920,
                height: 1080,
                rotation: 1,
            }
        );

        let legacy: AndroidDisplayInfo =
            serde_json::from_str(r#"{"width":1080,"height":1920}"#).unwrap();
        assert_eq!(legacy.rotation, 0);
    }
}

#[cfg(test)]
mod telemetry_tests {
    use super::*;

    #[test]
    fn latency_percentiles_are_stable_for_sorted_ack_samples() {
        let values = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        assert_eq!(percentile(&values, 0.50), 6.0);
        assert_eq!(percentile(&values, 0.95), 10.0);
        assert_eq!(percentile(&values, 0.99), 10.0);
        assert_eq!(percentile(&[], 0.95), 0.0);
    }
}

#[cfg(test)]
mod outbound_queue_tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::Instant;

    fn movement_frame(x: f32, y: f32) -> OutboundFrame {
        OutboundFrame {
            action: Action::Move,
            action_index: 0,
            pointers: vec![Pointer::finger(0, x, y)],
            source_timestamp_micros: unix_time_micros(),
        }
    }

    #[test]
    fn outbound_queue_coalesces_adjacent_move_snapshots_to_latest_coordinates() {
        let queue = OutboundFrameQueue::new(4);
        assert!(queue.push(movement_frame(1.0, 2.0)));
        assert!(queue.push(movement_frame(10.0, 20.0)));
        assert!(queue.push(movement_frame(100.0, 200.0)));

        let state = queue.state.lock().unwrap();
        assert_eq!(state.frames.len(), 1);
        let frame = state.frames.front().unwrap();
        assert_eq!(frame.pointers[0].x, 100.0);
        assert_eq!(frame.pointers[0].y, 200.0);
    }

    #[test]
    fn outbound_queue_evicts_motion_but_retains_down_up_and_cancel_transitions() {
        let queue = OutboundFrameQueue::new(3);
        assert!(queue.push(OutboundFrame {
            action: Action::Down,
            action_index: 0,
            pointers: vec![Pointer::finger(0, 1.0, 1.0)],
            source_timestamp_micros: unix_time_micros(),
        }));
        assert!(queue.push(movement_frame(2.0, 2.0)));
        assert!(queue.push(OutboundFrame {
            action: Action::Up,
            action_index: 0,
            pointers: vec![Pointer::finger(0, 2.0, 2.0)],
            source_timestamp_micros: unix_time_micros(),
        }));
        assert!(queue.push(OutboundFrame {
            action: Action::Down,
            action_index: 0,
            pointers: vec![Pointer::finger(1, 3.0, 3.0)],
            source_timestamp_micros: unix_time_micros(),
        }));

        // The queue is now full of transitions. A real writer frees one slot
        // before the producer enqueues CANCEL; no critical edge is discarded.
        assert_eq!(queue.pop().unwrap().action, Action::Down);
        assert!(queue.push(OutboundFrame {
            action: Action::Cancel,
            action_index: 0,
            pointers: vec![Pointer::finger(1, 3.0, 3.0)],
            source_timestamp_micros: unix_time_micros(),
        }));

        let actions = (0..3)
            .map(|_| queue.pop().unwrap().action)
            .collect::<Vec<_>>();
        assert_eq!(actions, vec![Action::Up, Action::Down, Action::Cancel]);
        assert!(queue.state.lock().unwrap().frames.is_empty());
        queue.close();
    }

    fn read_test_frame(stream: &mut TcpStream) -> (u32, u8) {
        let mut header = [0u8; 12];
        stream.read_exact(&mut header).unwrap();
        assert_eq!(&header[..4], b"WTCH");
        let count = usize::from(header[6]);
        let mut pointers = vec![0u8; count * 20];
        stream.read_exact(&mut pointers).unwrap();
        (
            u32::from_le_bytes(header[8..12].try_into().unwrap()),
            header[5],
        )
    }

    fn write_test_ack(stream: &mut TcpStream, sequence: u32) {
        let mut ack = Vec::with_capacity(8);
        ack.extend_from_slice(ACK_MAGIC);
        ack.extend_from_slice(&sequence.to_le_bytes());
        stream.write_all(&ack).unwrap();
        stream.flush().unwrap();
    }

    #[test]
    fn async_client_keeps_contiguous_sequences_while_coalescing_under_delayed_ack() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();

            let (first_sequence, first_action) = read_test_frame(&mut stream);
            assert_eq!(first_sequence, 1);
            assert_eq!(first_action, Action::Down as u8);
            std::thread::sleep(Duration::from_millis(80));
            write_test_ack(&mut stream, first_sequence);

            let (second_sequence, second_action) = read_test_frame(&mut stream);
            assert_eq!(second_sequence, 2);
            assert_eq!(second_action, Action::Move as u8);
            write_test_ack(&mut stream, second_sequence);

            let (third_sequence, third_action) = read_test_frame(&mut stream);
            assert_eq!(third_sequence, 3);
            assert_eq!(third_action, Action::Up as u8);
            write_test_ack(&mut stream, third_sequence);
        });

        let stream = TcpStream::connect(address).unwrap();
        let mut client = Client::from_authenticated_stream(stream).unwrap();
        let started = Instant::now();
        client
            .send(Action::Down, 0, vec![Pointer::finger(0, 1.0, 1.0)])
            .unwrap();
        for step in 0..100 {
            client
                .send(
                    Action::Move,
                    0,
                    vec![Pointer::finger(0, step as f32 + 2.0, 3.0)],
                )
                .unwrap();
        }
        client
            .send(Action::Up, 0, vec![Pointer::finger(0, 101.0, 3.0)])
            .unwrap();
        let enqueue_time = started.elapsed();
        assert!(
            enqueue_time < Duration::from_millis(70),
            "input-side enqueue blocked for {enqueue_time:?}"
        );
        client.close_worker();
        server.join().unwrap();
        assert!(client.check_status().is_ok());
    }

    #[test]
    fn outbound_queue_never_exceeds_capacity_during_motion_flood() {
        let queue = OutboundFrameQueue::new(8);
        for index in 0..20_000 {
            queue.push(movement_frame(index as f32, index as f32));
        }
        assert!(queue.state.lock().unwrap().frames.len() <= 8);
    }
}
