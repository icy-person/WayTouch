use crate::mouse::{MouseCursorConfig, MouseCursorStyle};
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const CURSOR_ADDRESS: &str = "127.0.0.1:27185";
const RECONNECT_INTERVAL: Duration = Duration::from_millis(250);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
const CURSOR_POSITION_INTERVAL: Duration = Duration::from_millis(8);
const MAX_PENDING_CURSOR_EVENTS: usize = 256;

#[derive(Clone, Debug)]
struct CursorEvent {
    generation: u64,
    message: String,
    visibility_transition: bool,
}

#[derive(Default)]
struct MailboxState {
    pending: std::collections::VecDeque<CursorEvent>,
    latest: Option<CursorEvent>,
    generation: u64,
    last_visibility: Option<bool>,
    closed: bool,
}

type SharedMailbox = Arc<(Mutex<MailboxState>, Condvar)>;

/// Ordered visibility delivery with coalesced positions. Visibility transitions
/// stay ordered while queued; a saturated backlog collapses to the newest edge.
/// `publish` never blocks, keeping the input event loop responsive even while
/// the Android endpoint is unreachable.
#[derive(Clone)]
struct CursorPublisher {
    shared: SharedMailbox,
    #[cfg(test)]
    worker: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
}

impl CursorPublisher {
    fn new() -> Self {
        Self::with_endpoint(CURSOR_ADDRESS.parse().expect("valid cursor address"), None)
    }

    fn with_endpoint(address: SocketAddr, token: Option<String>) -> Self {
        let shared = Arc::new((Mutex::new(MailboxState::default()), Condvar::new()));
        let worker_state = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("waytouch-android-cursor".into())
            .spawn(move || cursor_writer_worker(worker_state, address, token))
            .expect("cursor sender thread can be created");
        #[cfg(test)]
        let worker = Arc::new(Mutex::new(Some(worker)));
        #[cfg(not(test))]
        drop(worker);
        Self {
            shared,
            #[cfg(test)]
            worker,
        }
    }

    #[cfg(test)]
    fn take_worker(&self) -> Option<thread::JoinHandle<()>> {
        self.worker.lock().ok()?.take()
    }

    fn publish(&self, message: &str) {
        let (lock, changed) = &*self.shared;
        if let Ok(mut state) = lock.lock()
            && !state.closed
        {
            let visible = message.starts_with("SHOW ");
            let visibility_transition = state.last_visibility != Some(visible);
            if visibility_transition && state.pending.len() >= MAX_PENDING_CURSOR_EVENTS {
                // Backpressure collapse. These queued transitions have not been
                // acknowledged. An event already in flight may still complete,
                // but this newest edge establishes the final requested state.
                // Keep the input event loop non-blocking while Android is offline.
                state.pending.clear();
            }

            state.generation = state.generation.wrapping_add(1).max(1);
            let event = CursorEvent {
                generation: state.generation,
                message: message.to_owned(),
                visibility_transition,
            };
            state.latest = Some(event.clone());

            if event.visibility_transition {
                state.last_visibility = Some(visible);
                state.pending.push_back(event);
            } else if visible {
                // Positions are snapshots, unlike visibility edges. Coalesce
                // only with the immediately preceding position; never cross HIDE.
                if let Some(previous) = state.pending.back_mut()
                    && !previous.visibility_transition
                {
                    *previous = event;
                } else if state.pending.len() < MAX_PENDING_CURSOR_EVENTS {
                    state.pending.push_back(event);
                }
                // If all slots hold transitions, latest position is sent after
                // those transitions have been acknowledged.
            }
            changed.notify_one();
        }
    }

    fn close(&self) {
        let (lock, changed) = &*self.shared;
        if let Ok(mut state) = lock.lock()
            && !state.closed
        {
            state.generation = state.generation.wrapping_add(1).max(1);
            let off = CursorEvent {
                generation: state.generation,
                message: "OFF\n".to_owned(),
                visibility_transition: true,
            };
            state.latest = Some(off.clone());
            // Shutdown supersedes every queued visibility/position update.
            // Retain only OFF so stale events cannot delay or undo cleanup.
            state.pending.clear();
            state.pending.push_back(off);
            state.last_visibility = Some(false);
            state.closed = true;
            changed.notify_all();
        }
    }
}

fn read_cursor_reply(stream: &mut TcpStream, expected: &[u8]) -> std::io::Result<()> {
    use std::io::Read;
    let mut response = Vec::with_capacity(24);
    loop {
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte)?;
        if byte[0] == b'\n' {
            break;
        }
        if byte[0] != b'\r' {
            if response.len() >= 64 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "oversized cursor acknowledgement",
                ));
            }
            response.push(byte[0]);
        }
    }
    if response == expected {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "unexpected cursor acknowledgement: {:?}",
                String::from_utf8_lossy(&response)
            ),
        ))
    }
}

/// One best-effort attempt to deliver `OFF` on a fresh connection while the
/// worker is shutting down. Failures are ignored; the overlay cleanup then
/// relies on the Android-side disconnect cancellation.
fn try_deliver_terminal_off(address: SocketAddr, token: Option<&str>) -> bool {
    let Ok(mut connection) = TcpStream::connect_timeout(&address, Duration::from_millis(250))
    else {
        return false;
    };
    let _ = connection.set_nodelay(true);
    let _ = connection.set_read_timeout(Some(Duration::from_millis(700)));
    let _ = connection.set_write_timeout(Some(Duration::from_millis(200)));
    let authentication = match token {
        Some(token) => crate::auth::authenticate_with_token(&mut connection, token),
        None => crate::auth::authenticate(&mut connection),
    };
    if authentication.is_err() {
        return false;
    }
    connection
        .write_all(b"OFF\n")
        .and_then(|()| connection.flush())
        .and_then(|()| read_cursor_reply(&mut connection, b"OK CURSOR"))
        .is_ok()
}

fn cursor_writer_worker(shared: SharedMailbox, address: SocketAddr, token: Option<String>) {
    let (lock, changed) = &*shared;
    let mut stream: Option<TcpStream> = None;
    let mut sent_generation: Option<u64> = None;
    let mut last_write = Instant::now();

    loop {
        let next = {
            let Ok(mut state) = lock.lock() else {
                break;
            };
            loop {
                if let Some(event) = state.pending.front() {
                    break Some(event.clone());
                }
                if let Some(latest) = state.latest.as_ref()
                    && sent_generation != Some(latest.generation)
                {
                    break Some(latest.clone());
                }
                if state.closed {
                    break None;
                }

                let wait = if stream.is_some() {
                    HEARTBEAT_INTERVAL.saturating_sub(last_write.elapsed())
                } else {
                    RECONNECT_INTERVAL
                };
                if wait.is_zero() {
                    break None;
                }
                let Ok((next, result)) = changed.wait_timeout(state, wait) else {
                    break None;
                };
                state = next;
                if result.timed_out() {
                    break None;
                }
            }
        };

        let Some(event) = next else {
            let closed = lock.lock().map(|state| state.closed).unwrap_or(true);
            if closed {
                break;
            }
            if let Some(connection) = stream.as_mut()
                && last_write.elapsed() >= HEARTBEAT_INTERVAL
            {
                if connection.write_all(b"PING\n").is_err()
                    || read_cursor_reply(connection, b"PONG").is_err()
                {
                    stream = None;
                    sent_generation = None;
                } else {
                    last_write = Instant::now();
                }
            }
            continue;
        };

        if stream.is_none() {
            match TcpStream::connect_timeout(&address, Duration::from_millis(250)) {
                Ok(mut connection) => {
                    let _ = connection.set_nodelay(true);
                    let _ = connection.set_read_timeout(Some(Duration::from_millis(700)));
                    let _ = connection.set_write_timeout(Some(Duration::from_millis(200)));
                    let authentication = match token.as_deref() {
                        Some(token) => crate::auth::authenticate_with_token(&mut connection, token),
                        None => crate::auth::authenticate(&mut connection),
                    };
                    if authentication.is_ok() {
                        stream = Some(connection);
                    } else {
                        let closed = lock.lock().map(|state| state.closed).unwrap_or(true);
                        if closed {
                            // Best effort: still try to hide the overlay before
                            // the worker exits so the Android cursor does not
                            // outlive the host when the endpoint comes back
                            // only after shutdown started.
                            try_deliver_terminal_off(address, token.as_deref());
                            break;
                        }
                        thread::sleep(RECONNECT_INTERVAL);
                        continue;
                    }
                }
                Err(_) => {
                    let closed = lock.lock().map(|state| state.closed).unwrap_or(true);
                    if closed {
                        try_deliver_terminal_off(address, token.as_deref());
                        break;
                    }
                    thread::sleep(RECONNECT_INTERVAL);
                    continue;
                }
            }
        }

        // Position snapshots are replaceable. Limit WindowManager updateViewLayout
        // calls to about 60 Hz; while sleeping, the mailbox coalesces to the newest
        // position. Visibility transitions bypass this throttle.
        if !event.visibility_transition {
            let elapsed = last_write.elapsed();
            if elapsed < CURSOR_POSITION_INTERVAL {
                thread::sleep(CURSOR_POSITION_INTERVAL - elapsed);
            }
        }

        let connection = stream.as_mut().expect("connected cursor stream");
        let delivered = connection
            .write_all(event.message.as_bytes())
            .and_then(|()| connection.flush())
            .and_then(|()| {
                read_cursor_reply(connection, b"OK CURSOR").map_err(|error| {
                    std::io::Error::new(
                        error.kind(),
                        format!(
                            "cursor command ACK generation={}: {error}",
                            event.generation
                        ),
                    )
                })
            });
        if let Err(error) = delivered {
            eprintln!(
                "ANDROID_CURSOR_DELIVERY_RETRY generation={} error={error}",
                event.generation
            );
            stream = None;
            sent_generation = None;
            thread::sleep(RECONNECT_INTERVAL);
            continue;
        }

        sent_generation = Some(event.generation);
        last_write = Instant::now();
        if let Ok(mut state) = lock.lock()
            && state
                .pending
                .front()
                .is_some_and(|pending| pending.generation == event.generation)
        {
            state.pending.pop_front();
            changed.notify_all();
        }
    }

    if let Some(connection) = stream {
        let _ = connection.shutdown(std::net::Shutdown::Both);
    }
}

pub(crate) struct AndroidCursor {
    config: MouseCursorConfig,
    width: u32,
    height: u32,
    rotation_delta: u32,
    x: f32,
    y: f32,
    active: bool,
    aiming: bool,
    position_ready: bool,
    last_visible: Option<bool>,
    last_position: [f32; 2],
    sender: Option<CursorPublisher>,
}

impl AndroidCursor {
    pub(crate) fn new(config: MouseCursorConfig, width: u32, height: u32) -> Self {
        Self {
            config,
            width: width.max(2),
            height: height.max(2),
            rotation_delta: 0,
            x: 0.5,
            y: 0.5,
            active: false,
            aiming: false,
            position_ready: false,
            last_visible: None,
            last_position: [f32::NAN, f32::NAN],
            sender: Some(CursorPublisher::new()),
        }
    }

    pub(crate) fn set_display_geometry(&mut self, width: u32, height: u32, rotation_delta: u32) {
        let next_rotation = rotation_delta % 4;
        let turns = (next_rotation + 4 - self.rotation_delta) % 4;
        for _ in 0..turns {
            let previous_x = self.x;
            self.x = 1.0 - self.y;
            self.y = previous_x;
        }
        self.width = width.max(2);
        self.height = height.max(2);
        self.rotation_delta = next_rotation;
        self.x = self.x.clamp(0.0, 1.0);
        self.y = self.y.clamp(0.0, 1.0);
    }

    /// Set the exact Android SOURCE_MOUSE position returned in the transport ACK.
    /// The Android injector is authoritative; this overlay never integrates raw deltas.
    pub(crate) fn set_position_pixels(&mut self, x: f32, y: f32, timestamp_micros: u128) {
        if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 {
            return;
        }
        self.x = (x / (self.width - 1) as f32).clamp(0.0, 1.0);
        self.y = (y / (self.height - 1) as f32).clamp(0.0, 1.0);
        self.position_ready = true;
        self.publish_if_changed(timestamp_micros);
    }

    /// F1 alone owns pointer capture and the Android overlay visibility.
    pub(crate) fn set_active(&mut self, active: bool, timestamp_micros: u128) {
        if self.active != active {
            self.active = active;
            if !active {
                self.aiming = false;
            }
            self.publish_if_changed(timestamp_micros);
        } else if !active && self.last_visible.is_none() {
            self.publish_if_changed(timestamp_micros);
        }
    }

    pub(crate) fn set_aiming(&mut self, aiming: bool, timestamp_micros: u128) {
        if self.aiming != aiming {
            self.aiming = aiming;
            self.publish_if_changed(timestamp_micros);
        }
    }

    pub(crate) fn device_removed(&mut self) {
        self.active = false;
        self.aiming = false;
        self.position_ready = false;
        self.last_visible = Some(false);
        self.publish("OFF\n");
    }

    pub(crate) fn hide(&mut self) {
        if let Some(sender) = self.sender.take() {
            sender.close();
        }
    }

    fn visible(&self) -> bool {
        self.config.enabled
            && self.active
            && self.position_ready
            && !(self.aiming && self.config.hide_while_aiming)
    }

    fn publish_if_changed(&mut self, _timestamp_micros: u128) {
        let visible = self.visible();
        let visibility_changed = self.last_visible != Some(visible);
        let position_changed = visible
            && ((self.x - self.last_position[0]).abs() > 0.00001
                || (self.y - self.last_position[1]).abs() > 0.00001);
        if !visibility_changed && !position_changed {
            return;
        }
        // Position updates come from Android ACKs. CursorPublisher coalesces
        // superseded positions, while visibility transitions remain ordered.

        if visible {
            let style = match self.config.style {
                MouseCursorStyle::Crosshair => "crosshair",
                MouseCursorStyle::Pointer => "pointer",
                MouseCursorStyle::Dot => "dot",
            };
            let line = format!(
                "SHOW {:.6} {:.6} {} {:.1} {}\n",
                self.x, self.y, style, self.config.size, self.config.color
            );
            self.publish(&line);
            self.last_position = [self.x, self.y];
        } else {
            self.publish("HIDE\n");
        }
        self.last_visible = Some(visible);
    }

    fn publish(&self, message: &str) {
        if let Some(sender) = self.sender.as_ref() {
            sender.publish(message);
        }
    }
}

impl Drop for AndroidCursor {
    fn drop(&mut self) {
        self.hide();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_position_rotates_with_live_display_geometry_and_rotates_back() {
        let mut cursor = AndroidCursor::new(MouseCursorConfig::default(), 1920, 1080);
        cursor.set_active(true, 0);
        cursor.x = 0.2;
        cursor.y = 0.3;

        cursor.set_display_geometry(1080, 1920, 1);
        assert!((cursor.x - 0.7).abs() < 0.0001);
        assert!((cursor.y - 0.2).abs() < 0.0001);
        assert_eq!(cursor.width, 1080);
        assert_eq!(cursor.height, 1920);

        cursor.set_display_geometry(1920, 1080, 0);
        assert!((cursor.x - 0.2).abs() < 0.0001);
        assert!((cursor.y - 0.3).abs() < 0.0001);
        cursor.hide();
    }

    #[test]
    fn cursor_position_is_clamped_and_aiming_hides_custom_cursor() {
        let config = MouseCursorConfig {
            hide_while_aiming: true,
            ..MouseCursorConfig::default()
        };
        let mut cursor = AndroidCursor::new(config, 100, 100);
        cursor.set_position_pixels(99.0, 50.0, 0);
        cursor.set_active(true, 1);
        assert!(cursor.visible());
        cursor.aiming = true;
        assert!(!cursor.visible());
        cursor.aiming = false;
        assert!(cursor.visible());
        cursor.hide();
    }

    #[test]
    fn cursor_stays_hidden_until_android_authoritative_position_is_synchronized() {
        let mut cursor = AndroidCursor::new(MouseCursorConfig::default(), 1920, 1080);
        cursor.set_active(true, 0);
        assert!(
            !cursor.visible(),
            "F1 must not show the placeholder center before Android reports its real pointer"
        );

        cursor.set_position_pixels(1067.5, 586.5, 16_000);
        assert!(cursor.visible());
        assert!((cursor.x - 1067.5 / 1919.0).abs() < 0.00001);
        assert!((cursor.y - 586.5 / 1079.0).abs() < 0.00001);
        cursor.hide();
    }

    #[test]
    fn cursor_uses_absolute_coordinates_acknowledged_by_android() {
        let mut cursor = AndroidCursor::new(MouseCursorConfig::default(), 1920, 1080);
        cursor.set_position_pixels(36.0, 22.0, 16_000);
        assert!((cursor.x - (36.0 / 1919.0)).abs() < 0.00001);
        assert!((cursor.y - (22.0 / 1079.0)).abs() < 0.00001);

        let acknowledged = [cursor.x, cursor.y];
        cursor.set_position_pixels(300.0, -200.0, 32_000);
        assert_eq!([cursor.x, cursor.y], acknowledged);
        cursor.set_position_pixels(300.0, 200.0, 48_000);
        assert!((cursor.x - (300.0 / 1919.0)).abs() < 0.00001);
        assert!((cursor.y - (200.0 / 1079.0)).abs() < 0.00001);
        cursor.hide();
    }

    #[test]
    fn manual_hotkey_lock_hides_and_restores_android_cursor() {
        let mut cursor = AndroidCursor::new(MouseCursorConfig::default(), 1920, 1080);
        cursor.set_position_pixels(960.0, 540.0, 0);
        cursor.set_active(true, 1);
        assert!(cursor.visible());
        cursor.set_aiming(true, 1_000);
        assert!(!cursor.visible());
        cursor.set_aiming(false, 2_000);
        assert!(cursor.visible());
        cursor.hide();
    }

    #[test]
    fn cursor_protocol_message_contains_valid_rendering_attributes() {
        let config = MouseCursorConfig {
            style: MouseCursorStyle::Dot,
            size: 34.0,
            color: "#12aBcD".into(),
            ..MouseCursorConfig::default()
        };
        let mut cursor = AndroidCursor::new(config, 1920, 1080);
        cursor.set_position_pixels(960.0, 540.0, 0);
        cursor.set_active(true, 1);
        let style = match cursor.config.style {
            MouseCursorStyle::Crosshair => "crosshair",
            MouseCursorStyle::Pointer => "pointer",
            MouseCursorStyle::Dot => "dot",
        };
        assert_eq!(style, "dot");
        assert_eq!(cursor.config.size, 34.0);
        assert_eq!(cursor.config.color, "#12aBcD");
        assert!(cursor.visible());
        cursor.hide();
    }

    #[test]
    fn cursor_mailbox_preserves_visibility_edges_and_coalesces_only_positions() {
        let publisher =
            CursorPublisher::with_endpoint("127.0.0.1:9".parse().unwrap(), Some("c".repeat(64)));
        publisher.publish("SHOW 0.1 0.2 pointer 28.0 #ff3158\n");
        publisher.publish("SHOW 0.4 0.5 pointer 28.0 #ff3158\n");
        publisher.publish("SHOW 0.6 0.7 pointer 28.0 #ff3158\n");
        publisher.publish("HIDE\n");
        publisher.publish("SHOW 0.9 0.8 dot 32.0 #ffffff\n");

        let (lock, _) = &*publisher.shared;
        let state = lock.lock().unwrap();
        assert_eq!(
            state.latest.as_ref().map(|event| event.message.as_str()),
            Some("SHOW 0.9 0.8 dot 32.0 #ffffff\n")
        );
        assert_eq!(state.generation, 5);
        assert_eq!(state.pending.len(), 4);
        assert_eq!(
            state.pending[0].message,
            "SHOW 0.1 0.2 pointer 28.0 #ff3158\n"
        );
        assert_eq!(
            state.pending[1].message,
            "SHOW 0.6 0.7 pointer 28.0 #ff3158\n"
        );
        assert_eq!(state.pending[2].message, "HIDE\n");
        assert_eq!(state.pending[3].message, "SHOW 0.9 0.8 dot 32.0 #ffffff\n");
        drop(state);
        publisher.close();
    }

    #[test]
    fn cursor_mailbox_close_forces_terminal_hidden_state() {
        let publisher =
            CursorPublisher::with_endpoint("127.0.0.1:9".parse().unwrap(), Some("d".repeat(64)));
        publisher.publish("SHOW 0.5 0.5 pointer 28.0 #ff3158\n");
        for index in 0..MAX_PENDING_CURSOR_EVENTS {
            if index % 2 == 0 {
                publisher.publish("HIDE\n");
            } else {
                publisher.publish("SHOW 0.6 0.6 pointer 28.0 #ff3158\n");
            }
        }

        publisher.close();
        let (lock, _) = &*publisher.shared;
        let state = lock.lock().unwrap();
        assert!(state.closed);
        assert_eq!(state.pending.len(), 1, "shutdown must discard stale events");
        assert_eq!(state.pending.front().unwrap().message, "OFF\n");
        assert_eq!(state.last_visibility, Some(false));
        assert_eq!(
            state.latest.as_ref().map(|event| event.message.as_str()),
            Some("OFF\n")
        );
    }

    fn read_cursor_line(stream: &mut std::net::TcpStream) -> String {
        use std::io::{BufRead, BufReader};
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        line
    }

    fn authenticate_cursor_test_peer(stream: &mut std::net::TcpStream, token: &str) {
        use std::io::Write;
        let auth = read_cursor_line(stream);
        assert_eq!(auth.trim(), format!("AUTH {token}"));
        stream.write_all(b"OK AUTH\n").unwrap();
        stream.flush().unwrap();
    }

    #[test]
    fn cursor_tcp_delivery_reconnects_and_replays_latest_visibility_state() {
        use std::net::TcpListener;
        use std::sync::mpsc;
        use std::time::Instant;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let token = "a".repeat(64);
        let server_token = token.clone();
        let (first_seen_tx, first_seen_rx) = mpsc::channel();

        let server = thread::spawn(move || {
            let (mut first, _) = listener.accept().unwrap();
            first
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            authenticate_cursor_test_peer(&mut first, &server_token);
            let first_state = read_cursor_line(&mut first);
            assert!(
                first_state.starts_with("SHOW "),
                "first state: {first_state:?}"
            );
            first.write_all(b"OK CURSOR\n").unwrap();
            first.flush().unwrap();
            first_seen_tx.send(()).unwrap();

            let _ = first.shutdown(std::net::Shutdown::Both);
            drop(first);

            let (mut second, _) = listener.accept().unwrap();
            second
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            authenticate_cursor_test_peer(&mut second, &server_token);
            let started = Instant::now();
            let hide = read_cursor_line(&mut second);
            assert_eq!(hide.trim(), "HIDE");
            second.write_all(b"OK CURSOR\n").unwrap();
            second.flush().unwrap();

            let show = read_cursor_line(&mut second);
            assert!(show.starts_with("SHOW "), "replayed state: {show:?}");
            second.write_all(b"OK CURSOR\n").unwrap();
            second.flush().unwrap();
            assert!(started.elapsed() < Duration::from_secs(5));
        });

        let publisher = CursorPublisher::with_endpoint(address, Some(token));
        publisher.publish("SHOW 0.2 0.3 pointer 28.0 #ff3158\n");
        first_seen_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        publisher.publish("HIDE\n");
        publisher.publish("SHOW 0.8 0.7 dot 32.0 #ffffff\n");
        publisher.publish("SHOW 0.8 0.7 dot 32.0 #ffffff\n");
        server.join().unwrap();
        publisher.close();
    }

    #[test]
    fn cursor_visibility_stream_stays_ordered_and_terminates_when_acknowledged() {
        use std::io::Write;
        use std::net::TcpListener;
        use std::sync::mpsc;

        const TRANSITIONS: usize = 1_000;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let token = "e".repeat(64);
        let server_token = token.clone();
        let (first_show_tx, first_show_rx) = mpsc::channel();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            authenticate_cursor_test_peer(&mut stream, &server_token);

            // Synchronize with the producer before it starts the transition
            // burst. close() may supersede queued events with OFF, so an
            // unacknowledged initial SHOW is not guaranteed to survive.
            let first = read_cursor_line(&mut stream);
            assert!(first.starts_with("SHOW "), "first state: {first:?}");
            stream.write_all(b"OK CURSOR\n").unwrap();
            stream.flush().unwrap();
            first_show_tx.send(()).unwrap();

            // The producer must never block, so the number of delivered lines
            // depends on how the backlog coalesces while the worker drains it.
            // Validate every delivered line and acknowledge OFF as well.
            loop {
                let line = read_cursor_line(&mut stream);
                let terminal = line.trim() == "OFF";
                if !terminal && !line.starts_with("SHOW ") {
                    assert_eq!(line.trim(), "HIDE", "unexpected line: {line:?}");
                }
                stream.write_all(b"OK CURSOR\n").unwrap();
                stream.flush().unwrap();
                if terminal {
                    break;
                }
            }
        });

        let publisher = CursorPublisher::with_endpoint(address, Some(token));
        let started = std::time::Instant::now();
        publisher.publish("SHOW 0.5 0.5 pointer 28.0 #ff3158\n");
        first_show_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        for index in 0..(TRANSITIONS / 2) {
            publisher.publish("HIDE\n");
            publisher.publish(&format!(
                "SHOW {:.6} {:.6} pointer 28.0 #ff3158\n",
                (index % 100) as f32 / 99.0,
                (index % 97) as f32 / 96.0
            ));
        }
        publisher.close();
        server.join().unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "publishing 1000 transitions must not block the caller"
        );
    }

    #[test]
    fn cursor_state_publication_remains_nonblocking_under_high_rate_updates() {
        use std::time::Instant;
        let publisher =
            CursorPublisher::with_endpoint("127.0.0.1:9".parse().unwrap(), Some("b".repeat(64)));
        let started = Instant::now();
        for index in 0..20_000 {
            publisher.publish(&format!(
                "SHOW {:.6} {:.6} pointer 28.0 #ff3158\n",
                (index % 1000) as f32 / 999.0,
                (index % 997) as f32 / 996.0
            ));
        }
        assert!(started.elapsed() < Duration::from_secs(2));
        let (lock, _) = &*publisher.shared;
        let state = lock.lock().unwrap();
        assert_eq!(state.generation, 20_000);
        assert!(state.latest.as_ref().unwrap().message.starts_with("SHOW "));
        drop(state);
        publisher.close();
    }
    #[test]
    fn cursor_transition_queue_is_bounded_and_collapses_without_blocking_the_caller() {
        use std::sync::mpsc;

        let publisher =
            CursorPublisher::with_endpoint("127.0.0.1:9".parse().unwrap(), Some("f".repeat(64)));
        for index in 0..MAX_PENDING_CURSOR_EVENTS {
            if index % 2 == 0 {
                publisher.publish("SHOW 0.5 0.5 pointer 28.0 #ff3158\\n");
            } else {
                publisher.publish("HIDE\\n");
            }
        }
        {
            let (lock, _) = &*publisher.shared;
            let state = lock.lock().unwrap();
            assert_eq!(state.pending.len(), MAX_PENDING_CURSOR_EVENTS);
            assert!(
                state
                    .pending
                    .iter()
                    .all(|event| event.visibility_transition)
            );
        }

        // Publishing one more visibility edge must return immediately: the
        // not-yet-acknowledged queue collapses to the newest edge instead of
        // stalling the input event loop while Android is unreachable.
        let cloned = publisher.clone();
        let (done_tx, done_rx) = mpsc::channel();
        let producer = thread::spawn(move || {
            cloned.publish("SHOW 0.7 0.8 dot 32.0 #ffffff\\n");
            done_tx.send(()).unwrap();
        });
        assert!(
            done_rx.recv_timeout(Duration::from_millis(500)).is_ok(),
            "publish must never block the caller, even with a full backlog"
        );
        producer.join().unwrap();

        {
            let (lock, _) = &*publisher.shared;
            let state = lock.lock().unwrap();
            assert_eq!(
                state.pending.len(),
                1,
                "a full backlog collapses to the newest un-applied edge"
            );
            assert_eq!(
                state.pending.back().unwrap().message,
                "SHOW 0.7 0.8 dot 32.0 #ffffff\\n"
            );
            assert_eq!(state.last_visibility, Some(true));
            assert!(
                state
                    .latest
                    .as_ref()
                    .is_some_and(|event| event.message == "SHOW 0.7 0.8 dot 32.0 #ffffff\\n")
            );
        }

        // A same-visibility SHOW is a position snapshot: it is appended after
        // the trailing edge, and a further snapshot coalesces into it instead
        // of growing the queue.
        publisher.publish("SHOW 0.9 0.9 dot 32.0 #ffffff\\n");
        publisher.publish("SHOW 0.8 0.8 dot 32.0 #ffffff\\n");
        {
            let (lock, _) = &*publisher.shared;
            let state = lock.lock().unwrap();
            assert_eq!(state.pending.len(), 2);
            assert_eq!(
                state.pending.back().unwrap().message,
                "SHOW 0.8 0.8 dot 32.0 #ffffff\\n"
            );
            assert!(!state.pending.back().unwrap().visibility_transition);
        }
        publisher.close();
    }

    #[test]
    fn cursor_worker_exits_promptly_after_terminal_delivery_attempt_when_endpoint_rejects() {
        use std::net::TcpListener;
        use std::time::Instant;

        // Accept then immediately drop every connection: authentication always
        // fails, so the worker exercises the closed-endpoint shutdown path.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            // Reject the worker's first connection, then release the listener
            // so the terminal best-effort reconnect fails promptly as well.
            let _ = listener.accept();
        });

        let publisher = CursorPublisher::with_endpoint(address, Some("f".repeat(64)));
        publisher.publish("SHOW 0.5 0.5 pointer 28.0 #ff3158\\n");
        publisher.close();
        let handle = publisher.take_worker().expect("worker handle");
        let started = Instant::now();
        loop {
            if handle.is_finished() {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the cursor worker must exit promptly once the mailbox is closed"
            );
            thread::sleep(Duration::from_millis(50));
        }
        handle.join().unwrap();
    }
}
