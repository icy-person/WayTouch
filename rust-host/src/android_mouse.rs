use crate::mouse::MouseFrame;
#[cfg(test)]
use std::io::Read;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const ADDRESS: &str = "127.0.0.1:27186";
const QUEUE_CAPACITY: usize = 64;
const BUTTON_BITS: [u16; 5] = [1, 2, 4, 8, 16];
const ACTION_HOVER_ENTER: u8 = 9;
const ACTION_HOVER_EXIT: u8 = 10;
const MAX_MOTION_IN_FLIGHT: usize = 8;

pub(crate) struct AndroidMouse {
    sender: Option<SyncSender<MouseFrame>>,
    worker: Option<JoinHandle<()>>,
    latest_position: Arc<Mutex<Option<(f32, f32)>>>,
}

fn encode_frame(sequence: u64, frame: MouseFrame) -> String {
    format!(
        "WTM3 {sequence} {} {} {} {:.3} {:.3} {:.3} {:.3}\n",
        frame.action,
        frame.buttons,
        frame.action_button,
        frame.relative_x,
        frame.relative_y,
        frame.horizontal_scroll,
        frame.vertical_scroll
    )
}

fn next_sequence(sequence: u64) -> u64 {
    if sequence >= 0xffff_ffff {
        1
    } else {
        sequence + 1
    }
}

#[cfg(test)]
fn write_frame(connection: &mut TcpStream, sequence: u64, frame: MouseFrame) -> io::Result<()> {
    let message = encode_frame(sequence, frame);
    connection.write_all(message.as_bytes())?;

    // The ACK is sent only after Android accepted the injected MotionEvent.
    let mut response = Vec::with_capacity(24);
    loop {
        let mut byte = [0u8; 1];
        connection.read_exact(&mut byte)?;
        if byte[0] == 10 {
            break;
        }
        if byte[0] != 13 {
            if response.len() >= 32 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "oversized mouse ACK",
                ));
            }
            response.push(byte[0]);
        }
    }
    let line = String::from_utf8_lossy(&response);
    let fields = line.split_ascii_whitespace().collect::<Vec<_>>();
    if fields.len() != 4 || fields[0] != "WTMA3" || fields[1] != sequence.to_string() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "mouse ACK mismatch: expected WTMA3 {sequence} with X/Y, received {:?}",
                line.trim()
            ),
        ));
    }
    let x = fields[2].parse::<f32>().map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid mouse ACK X: {error}"),
        )
    })?;
    let y = fields[3].parse::<f32>().map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid mouse ACK Y: {error}"),
        )
    })?;
    if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid mouse ACK coordinates",
        ));
    }
    Ok(())
}

struct MouseConnection {
    writer: TcpStream,
    reader: BufReader<TcpStream>,
}

impl MouseConnection {
    fn new(writer: TcpStream) -> io::Result<Self> {
        let reader = BufReader::with_capacity(4096, writer.try_clone()?);
        let mut connection = Self { writer, reader };
        // Explicitly reserve the persistent controller slot. Short-lived PING
        // and legacy position probes must never be mistaken for this stream.
        connection.writer.write_all(b"CONTROL\n")?;
        connection.writer.flush()?;
        let mut response = String::new();
        let read = connection.reader.read_line(&mut response)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Android mouse server closed before CONTROL acknowledgement",
            ));
        }
        if response.trim_end_matches(['\r', '\n']) != "OK CONTROL" {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "Android mouse controller registration rejected: {:?}",
                    response.trim()
                ),
            ));
        }
        Ok(connection)
    }

    fn read_ack(&mut self, expected_sequence: u64) -> io::Result<(f32, f32)> {
        let mut response = Vec::with_capacity(48);
        loop {
            let available = self.reader.fill_buf()?;
            if available.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Android mouse ACK stream closed",
                ));
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let take = newline.map_or(available.len(), |position| position + 1);
            if response.len() + take > 64 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "oversized mouse ACK",
                ));
            }
            let has_newline = newline.is_some();
            response.extend_from_slice(&available[..take]);
            self.reader.consume(take);
            if has_newline {
                break;
            }
        }
        let line = String::from_utf8_lossy(&response);
        let fields = line.split_ascii_whitespace().collect::<Vec<_>>();
        if fields.len() != 4 || fields[0] != "WTMA3" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid mouse ACK: {:?}", line.trim()),
            ));
        }
        let received_sequence = fields[1].parse::<u64>().map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid mouse ACK sequence: {error}"),
            )
        })?;
        if received_sequence != expected_sequence {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "mouse ACK mismatch: expected {expected_sequence}, received {received_sequence}"
                ),
            ));
        }
        let x = fields[2].parse::<f32>().map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid Android mouse X: {error}"),
            )
        })?;
        let y = fields[3].parse::<f32>().map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid Android mouse Y: {error}"),
            )
        })?;
        if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Android returned invalid mouse position ({x}, {y})"),
            ));
        }
        Ok((x, y))
    }

    fn send_one(&mut self, sequence: &mut u64, frame: MouseFrame) -> io::Result<(f32, f32)> {
        let current = *sequence;
        self.writer
            .write_all(encode_frame(current, frame).as_bytes())?;
        self.writer.flush()?;
        let position = self.read_ack(current)?;
        *sequence = next_sequence(current);
        Ok(position)
    }

    fn send_motion_batch(
        &mut self,
        sequence: &mut u64,
        frames: &[MouseFrame],
    ) -> io::Result<(f32, f32)> {
        if frames.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot send an empty mouse motion batch",
            ));
        }
        let mut expected = Vec::with_capacity(frames.len());
        let mut next = *sequence;
        for frame in frames {
            if !is_motion_snapshot(*frame) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "non-motion frame entered mouse batch",
                ));
            }
            expected.push(next);
            self.writer
                .write_all(encode_frame(next, *frame).as_bytes())?;
            next = next_sequence(next);
        }
        self.writer.flush()?;
        let mut latest_position = (0.0, 0.0);
        for expected_sequence in expected {
            latest_position = self.read_ack(expected_sequence)?;
        }
        *sequence = next;
        Ok(latest_position)
    }
}

fn is_motion_snapshot(frame: MouseFrame) -> bool {
    frame.action == 7 || frame.action == 9
}

fn cancel_frame() -> MouseFrame {
    MouseFrame {
        action: 3,
        buttons: 0,
        action_button: 0,
        relative_x: 0.0,
        relative_y: 0.0,
        horizontal_scroll: 0.0,
        vertical_scroll: 0.0,
    }
}

fn position_query_frame() -> MouseFrame {
    MouseFrame {
        action: 13,
        buttons: 0,
        action_button: 0,
        relative_x: 0.0,
        relative_y: 0.0,
        horizontal_scroll: 0.0,
        vertical_scroll: 0.0,
    }
}

fn publish_position(target: &Arc<Mutex<Option<(f32, f32)>>>, position: (f32, f32)) {
    if let Ok(mut latest) = target.lock() {
        *latest = Some(position);
    }
}

fn restore_buttons(
    connection: &mut MouseConnection,
    sequence: &mut u64,
    buttons: u16,
) -> io::Result<()> {
    let mut state = 0u16;
    for bit in BUTTON_BITS {
        if buttons & bit == 0 {
            continue;
        }
        state |= bit;
        connection.send_one(
            sequence,
            MouseFrame {
                action: 11,
                buttons: state,
                action_button: bit,
                relative_x: 0.0,
                relative_y: 0.0,
                horizontal_scroll: 0.0,
                vertical_scroll: 0.0,
            },
        )?;
    }
    Ok(())
}

impl AndroidMouse {
    pub(crate) fn new() -> Self {
        let (sender, receiver) = mpsc::sync_channel::<MouseFrame>(QUEUE_CAPACITY);
        let latest_position = Arc::new(Mutex::new(None));
        let worker_position = Arc::clone(&latest_position);
        let worker = thread::Builder::new()
            .name("waytouch-android-mouse".into())
            .spawn(move || {
                let address: SocketAddr = ADDRESS.parse().expect("valid Android mouse address");
                let mut stream: Option<MouseConnection> = None;
                let mut retry_after = Instant::now();
                let mut reported_offline = false;
                let mut next_sequence = 1u64;
                let mut last_buttons: u16;
                let mut pending_frame: Option<MouseFrame> = None;
                let mut pipeline_batches = 0u64;
                let mut batched_motion_frames = 0u64;
                let mut single_motion_frames = 0u64;
                let mut transition_frames = 0u64;
                let mut maximum_batch = 0usize;
                let mut report_pipeline_at = Instant::now() + Duration::from_secs(5);

                loop {
                    let mut frame = match pending_frame.take() {
                        Some(frame) => frame,
                        None => match receiver.recv() {
                            Ok(frame) => frame,
                            Err(_) => break,
                        },
                    };
                    last_buttons = frame.buttons;

                    if stream.is_none() {
                        if Instant::now() < retry_after {
                            continue;
                        }

                        // The connection is down: collapse only the stale backlog
                        // to the latest button state. Old motion deltas are not replayed.
                        while let Ok(pending) = receiver.try_recv() {
                            last_buttons = pending.buttons;
                            frame = pending;
                        }

                        match TcpStream::connect_timeout(&address, Duration::from_millis(180)) {
                            Ok(mut raw_connection) => {
                                let _ = raw_connection.set_nodelay(true);
                                let _ = raw_connection.set_read_timeout(Some(Duration::from_secs(2)));
                                let _ = raw_connection.set_write_timeout(Some(Duration::from_millis(80)));
                                if let Err(error) = crate::auth::authenticate(&mut raw_connection) {
                                    eprintln!("ANDROID_MOUSE_AUTH_ERROR error={error}");
                                    retry_after = Instant::now() + Duration::from_millis(500);
                                    reported_offline = true;
                                    continue;
                                }

                                let mut connection = match MouseConnection::new(raw_connection) {
                                    Ok(connection) => connection,
                                    Err(error) => {
                                        eprintln!("ANDROID_MOUSE_STREAM_SETUP_ERROR error={error}");
                                        retry_after = Instant::now() + Duration::from_millis(250);
                                        reported_offline = true;
                                        continue;
                                    }
                                };

                                let was_offline = reported_offline;
                                if was_offline {
                                    // Re-establish button state before accepting fresh motion.
                                    // These barriers are ordered and never batched with MOVE.
                                    next_sequence = 1;
                                    let resync = connection
                                        .send_one(&mut next_sequence, cancel_frame())
                                        .and_then(|_| {
                                            restore_buttons(
                                                &mut connection,
                                                &mut next_sequence,
                                                last_buttons,
                                            )
                                        });
                                    if let Err(error) = resync {
                                        eprintln!("ANDROID_MOUSE_RESYNC_ERROR error={error}");
                                        retry_after = Instant::now() + Duration::from_millis(250);
                                        reported_offline = true;
                                        continue;
                                    }
                                }

                                // Android owns the absolute pointer. Query it after every
                                // connection so stale relative deltas are never replayed.
                                match connection.send_one(&mut next_sequence, position_query_frame()) {
                                    Ok(position) => publish_position(&worker_position, position),
                                    Err(error) => {
                                        eprintln!("ANDROID_MOUSE_POSITION_SYNC_ERROR error={error}");
                                        retry_after = Instant::now() + Duration::from_millis(250);
                                        reported_offline = true;
                                        continue;
                                    }
                                }

                                stream = Some(connection);
                                if was_offline {
                                    eprintln!(
                                        "ANDROID_MOUSE_RECONNECTED address={ADDRESS} buttons={last_buttons}"
                                    );
                                    reported_offline = false;
                                    continue;
                                }
                                println!("ANDROID_MOUSE_CONNECTED address={ADDRESS}");
                            }
                            Err(error) => {
                                if !reported_offline {
                                    eprintln!(
                                        "ANDROID_MOUSE_OFFLINE address={ADDRESS} error={error}"
                                    );
                                    reported_offline = true;
                                }
                                retry_after = Instant::now() + Duration::from_millis(500);
                                continue;
                            }
                        }
                    }

                    if let Some(connection) = stream.as_mut() {
                        let mut batch = Vec::with_capacity(MAX_MOTION_IN_FLIGHT);
                        if is_motion_snapshot(frame) {
                            batch.push(frame);
                            // Only adjacent hover snapshots may share a window.
                            // The first state-changing event is held for the next
                            // iteration, after every earlier motion ACK is received.
                            while batch.len() < MAX_MOTION_IN_FLIGHT {
                                match receiver.try_recv() {
                                    Ok(next) if is_motion_snapshot(next) => {
                                        batch.push(next);
                                    }
                                    Ok(next) => {
                                        pending_frame = Some(next);
                                        break;
                                    }
                                    Err(_) => break,
                                }
                            }
                        }

                        let delivery = if batch.is_empty() {
                            if is_motion_snapshot(frame) {
                                single_motion_frames = single_motion_frames.saturating_add(1);
                            } else {
                                transition_frames = transition_frames.saturating_add(1);
                            }
                            connection.send_one(&mut next_sequence, frame)
                        } else {
                            if batch.len() > 1 {
                                pipeline_batches = pipeline_batches.saturating_add(1);
                                batched_motion_frames =
                                    batched_motion_frames.saturating_add(batch.len() as u64);
                                maximum_batch = maximum_batch.max(batch.len());
                            } else {
                                single_motion_frames = single_motion_frames.saturating_add(1);
                            }
                            connection.send_motion_batch(&mut next_sequence, &batch)
                        };
                        if Instant::now() >= report_pipeline_at {
                            println!(
                                "ANDROID_MOUSE_PIPELINE_STATS batches={} batched_frames={} max_batch={} single_motion_frames={} transition_frames={}",
                                pipeline_batches,
                                batched_motion_frames,
                                maximum_batch,
                                single_motion_frames,
                                transition_frames
                            );
                            pipeline_batches = 0;
                            batched_motion_frames = 0;
                            maximum_batch = 0;
                            single_motion_frames = 0;
                            transition_frames = 0;
                            report_pipeline_at = Instant::now() + Duration::from_secs(5);
                        }
                        match delivery {
                            Ok(position) => publish_position(&worker_position, position),
                            Err(error) => {
                                eprintln!("ANDROID_MOUSE_DELIVERY_ERROR error={error}; reconnecting");
                                stream = None;
                                next_sequence = 1;
                                retry_after = Instant::now() + Duration::from_millis(100);
                                reported_offline = true;
                            }
                        }
                    }
                }
            })
            .expect("could not create Android mouse transport worker");

        Self {
            sender: Some(sender),
            worker: Some(worker),
            latest_position,
        }
    }

    /// Consume the latest absolute position acknowledged by Android.
    pub(crate) fn take_position(&self) -> Option<(f32, f32)> {
        self.latest_position
            .lock()
            .ok()
            .and_then(|mut latest| latest.take())
    }

    /// Returns true when the frame entered the Android transport queue and false
    /// when high-rate hover motion was intentionally dropped to bound latency.
    pub(crate) fn send(&self, frame: MouseFrame) -> Result<bool, String> {
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| "Android mouse transport is shutting down".to_owned())?;

        match sender.try_send(frame) {
            Ok(()) => Ok(true),
            Err(TrySendError::Full(frame)) if frame.action == 7 || frame.action == 9 => {
                // A bounded queue must not turn a high-rate mouse into seconds of
                // stale motion. The cursor overlay must skip the same dropped delta.
                Ok(false)
            }
            Err(TrySendError::Full(frame)) => sender
                .send(frame)
                .map(|()| true)
                .map_err(|_| "Android mouse transport worker has stopped".to_owned()),
            Err(TrySendError::Disconnected(_)) => {
                Err("Android mouse transport worker has stopped".to_owned())
            }
        }
    }

    pub(crate) fn set_cursor_active(&self, active: bool) {
        let _ = self.send(MouseFrame {
            action: if active {
                ACTION_HOVER_ENTER
            } else {
                ACTION_HOVER_EXIT
            },
            buttons: 0,
            action_button: 0,
            relative_x: 0.0,
            relative_y: 0.0,
            horizontal_scroll: 0.0,
            vertical_scroll: 0.0,
        });
    }

    pub(crate) fn set_aiming(&self, aiming: bool) {
        // F3 camera aim temporarily leaves native mouse-hover mode, but does not
        // alter GNOME capture. F1 remains the only global pointer-capture switch.
        self.set_cursor_active(!aiming);
    }

    pub(crate) fn shutdown(&mut self) {
        let _ = self.send(cancel_frame());
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for AndroidMouse {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_frame() -> MouseFrame {
        MouseFrame {
            action: 7,
            buttons: 1,
            action_button: 0,
            relative_x: 3.5,
            relative_y: -2.0,
            horizontal_scroll: 0.0,
            vertical_scroll: 0.0,
        }
    }

    #[test]
    fn mouse_motion_batch_pipelines_frames_and_matches_ordered_acks() {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(socket.try_clone().unwrap());
            let mut registration = String::new();
            reader.read_line(&mut registration).unwrap();
            assert_eq!(registration, "CONTROL\n");
            socket.write_all(b"OK CONTROL\n").unwrap();
            let mut lines = Vec::new();
            for _ in 0..3 {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                lines.push(line);
            }
            assert_eq!(lines[0], "WTM3 41 7 0 0 1.000 -1.000 0.000 0.000\n");
            assert_eq!(lines[1], "WTM3 42 7 0 0 2.000 -2.000 0.000 0.000\n");
            assert_eq!(lines[2], "WTM3 43 9 0 0 0.000 0.000 0.000 0.000\n");
            socket
                .write_all(b"WTMA3 41 10 20\nWTMA3 42 11 21\nWTMA3 43 12 22\n")
                .unwrap();
        });

        let stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut connection = MouseConnection::new(stream).unwrap();
        let mut sequence = 41;
        let frames = [
            MouseFrame {
                action: 7,
                buttons: 0,
                action_button: 0,
                relative_x: 1.0,
                relative_y: -1.0,
                horizontal_scroll: 0.0,
                vertical_scroll: 0.0,
            },
            MouseFrame {
                action: 7,
                buttons: 0,
                action_button: 0,
                relative_x: 2.0,
                relative_y: -2.0,
                horizontal_scroll: 0.0,
                vertical_scroll: 0.0,
            },
            MouseFrame {
                action: 9,
                buttons: 0,
                action_button: 0,
                relative_x: 0.0,
                relative_y: 0.0,
                horizontal_scroll: 0.0,
                vertical_scroll: 0.0,
            },
        ];
        let final_position = connection
            .send_motion_batch(&mut sequence, &frames)
            .unwrap();
        assert_eq!(final_position, (12.0, 22.0));
        assert_eq!(sequence, 44);
        server.join().unwrap();
        assert_eq!(next_sequence(0xffff_ffff), 1);
    }

    #[test]
    fn wtm2_frame_waits_for_the_matching_android_ack() {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(socket.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            assert_eq!(line, "WTM3 41 7 1 0 3.500 -2.000 0.000 0.000\n");
            socket.write_all(b"WTMA3 41 42 84\n").unwrap();
        });

        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        write_frame(&mut client, 41, sample_frame()).unwrap();
        server.join().unwrap();
    }

    #[test]
    fn wtm2_frame_rejects_an_ack_for_a_different_sequence() {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(socket.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            socket.write_all(b"WTMA3 99 10 20\n").unwrap();
        });

        let mut client = TcpStream::connect(address).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let error = write_frame(&mut client, 41, sample_frame()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        server.join().unwrap();
    }

    #[test]
    fn reconnect_snapshot_replays_each_held_button_in_order() {
        // The protocol uses Android BUTTON_* bit values, so every restored press
        // carries the cumulative state expected by MouseInputServer.
        let mut states = Vec::new();
        let mut state = 0u16;
        for bit in BUTTON_BITS {
            if (1 | 4 | 16) & bit != 0 {
                state |= bit;
                states.push((bit, state));
            }
        }
        assert_eq!(states, vec![(1, 1), (4, 5), (16, 21)]);
    }
}
