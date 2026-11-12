use crate::mouse::MouseFrame;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::mem::size_of;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const UI_DEV_CREATE: u64 = 0x0000_5501;
const UI_DEV_DESTROY: u64 = 0x0000_5502;
const UI_DEV_SETUP: u64 = 0x405c_5503;
const UI_SET_EVBIT: u64 = 0x4004_5564;
const UI_SET_KEYBIT: u64 = 0x4004_5565;
const UI_SET_RELBIT: u64 = 0x4004_5566;
const UI_SET_PROPBIT: u64 = 0x4004_556e;

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_REL: u16 = 2;
const SYN_REPORT: u16 = 0;

const REL_X: u16 = 0;
const REL_Y: u16 = 1;
const REL_HWHEEL: u16 = 6;
const REL_WHEEL: u16 = 8;

const BTN_LEFT: u16 = 272;
const BTN_RIGHT: u16 = 273;
const BTN_MIDDLE: u16 = 274;
const BTN_SIDE: u16 = 275;
const BTN_EXTRA: u16 = 276;

const INPUT_PROP_POINTER: i32 = 0;
const DEVICE_NAME: &str = "WayTouch Virtual Mouse";

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

#[repr(C)]
struct UinputSetup {
    id: InputId,
    name: [u8; 80],
    ff_effects_max: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct InputEvent {
    // Linux timeval uses two native longs. CachyOS is a 64-bit target.
    tv_sec: i64,
    tv_usec: i64,
    event_type: u16,
    code: u16,
    value: i32,
}

unsafe extern "C" {
    fn ioctl(fd: i32, request: u64, ...) -> i32;
}

fn ioctl_int(fd: RawFd, request: u64, value: i32) -> io::Result<()> {
    // SAFETY: The ioctl request is one of Linux uinput's integer-valued setup
    // requests; the third argument is passed by value as required by the ABI.
    let result = unsafe { ioctl(fd, request, value) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn ioctl_no_arg(fd: RawFd, request: u64) -> io::Result<()> {
    // SAFETY: UI_DEV_CREATE and UI_DEV_DESTROY have no third argument.
    let result = unsafe { ioctl(fd, request) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn ioctl_setup(fd: RawFd, request: u64, setup: &UinputSetup) -> io::Result<()> {
    // SAFETY: setup is a correctly laid out repr(C) uinput_setup structure
    // and remains alive for the duration of the ioctl call.
    let result = unsafe { ioctl(fd, request, setup as *const UinputSetup) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn accumulate_relative(delta: f32, remainder: f32) -> (i32, f32) {
    let total = delta + remainder;
    let whole = total.trunc().clamp(i32::MIN as f32, i32::MAX as f32) as i32;
    (whole, total - whole as f32)
}

fn wait_for_udev_node() -> io::Result<PathBuf> {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if let Ok(entries) = fs::read_dir("/sys/class/input") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if !name.starts_with("event") {
                    continue;
                }
                let sysfs_name = entry.path().join("device/name");
                if fs::read_to_string(sysfs_name).is_ok_and(|value| value.trim() == DEVICE_NAME) {
                    let node = PathBuf::from("/dev/input").join(name.as_ref());
                    if let Ok(metadata) = fs::metadata(&node) {
                        let mode = metadata.permissions().mode();
                        // CachyOS's standard input udev rule assigns root:input 0660.
                        // Wait until the rule has run so libinput can open the node.
                        if mode & 0o660 == 0o660 {
                            return Ok(node);
                        }
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "udev did not expose WayTouch Virtual Mouse as an input-group 0660 event node",
    ))
}

pub(crate) struct UinputMouse {
    file: std::fs::File,
    active_buttons: u16,
    created: bool,
    event_node: PathBuf,
    fractional_x: f32,
    fractional_y: f32,
}

impl UinputMouse {
    pub(crate) fn create() -> io::Result<Self> {
        let file = OpenOptions::new().write(true).open("/dev/uinput")?;
        let fd = file.as_raw_fd();

        // EV_SYN is implicit for uinput. Only advertise the classes actually used.
        ioctl_int(fd, UI_SET_EVBIT, i32::from(EV_KEY))?;
        ioctl_int(fd, UI_SET_EVBIT, i32::from(EV_REL))?;
        for axis in [REL_X, REL_Y, REL_HWHEEL, REL_WHEEL] {
            ioctl_int(fd, UI_SET_RELBIT, i32::from(axis))?;
        }
        for button in [BTN_LEFT, BTN_RIGHT, BTN_MIDDLE, BTN_SIDE, BTN_EXTRA] {
            ioctl_int(fd, UI_SET_KEYBIT, i32::from(button))?;
        }
        ioctl_int(fd, UI_SET_PROPBIT, INPUT_PROP_POINTER)?;

        let mut name = [0u8; 80];
        let label = b"WayTouch Virtual Mouse";
        name[..label.len()].copy_from_slice(label);
        let setup = UinputSetup {
            id: InputId {
                bustype: 0x06, // BUS_VIRTUAL
                vendor: 0x1209,
                product: 0x5754,
                version: 1,
            },
            name,
            ff_effects_max: 0,
        };
        ioctl_setup(fd, UI_DEV_SETUP, &setup)?;
        ioctl_no_arg(fd, UI_DEV_CREATE)?;
        let event_node = wait_for_udev_node()?;

        Ok(Self {
            file,
            active_buttons: 0,
            created: true,
            event_node,
            fractional_x: 0.0,
            fractional_y: 0.0,
        })
    }

    pub(crate) fn event_node(&self) -> &std::path::Path {
        &self.event_node
    }

    pub(crate) fn send(&mut self, frame: MouseFrame) -> io::Result<()> {
        let mut events = Vec::with_capacity(8);
        let button_map = [
            (1u16, BTN_LEFT),
            (2u16, BTN_RIGHT),
            (4u16, BTN_MIDDLE),
            (8u16, BTN_SIDE),
            (16u16, BTN_EXTRA),
        ];

        for (bit, code) in button_map {
            let was_pressed = self.active_buttons & bit != 0;
            let is_pressed = frame.buttons & bit != 0;
            if was_pressed != is_pressed {
                events.push(InputEvent {
                    event_type: EV_KEY,
                    code,
                    value: i32::from(is_pressed),
                    ..InputEvent::default()
                });
            }
        }

        let (delta_x, fractional_x) = accumulate_relative(frame.relative_x, self.fractional_x);
        let (delta_y, fractional_y) = accumulate_relative(frame.relative_y, self.fractional_y);
        if delta_x != 0 {
            events.push(InputEvent {
                event_type: EV_REL,
                code: REL_X,
                value: delta_x,
                ..InputEvent::default()
            });
        }
        if delta_y != 0 {
            events.push(InputEvent {
                event_type: EV_REL,
                code: REL_Y,
                value: delta_y,
                ..InputEvent::default()
            });
        }
        if frame.horizontal_scroll != 0.0 {
            events.push(InputEvent {
                event_type: EV_REL,
                code: REL_HWHEEL,
                value: frame.horizontal_scroll.round() as i32,
                ..InputEvent::default()
            });
        }
        if frame.vertical_scroll != 0.0 {
            events.push(InputEvent {
                event_type: EV_REL,
                code: REL_WHEEL,
                value: frame.vertical_scroll.round() as i32,
                ..InputEvent::default()
            });
        }

        if events.is_empty() {
            self.fractional_x = fractional_x;
            self.fractional_y = fractional_y;
            return Ok(());
        }
        events.push(InputEvent {
            event_type: EV_SYN,
            code: SYN_REPORT,
            value: 0,
            ..InputEvent::default()
        });

        let bytes = unsafe {
            // SAFETY: InputEvent is repr(C), Copy, and events is a contiguous
            // array. The byte slice is used only for the synchronous write_all.
            std::slice::from_raw_parts(
                events.as_ptr().cast::<u8>(),
                events.len() * size_of::<InputEvent>(),
            )
        };
        self.file.write_all(bytes)?;
        self.active_buttons = frame.buttons;
        self.fractional_x = fractional_x;
        self.fractional_y = fractional_y;
        Ok(())
    }

    fn release_buttons(&mut self) {
        if self.active_buttons == 0 {
            return;
        }
        let mut events = Vec::with_capacity(6);
        for (bit, code) in [
            (1u16, BTN_LEFT),
            (2u16, BTN_RIGHT),
            (4u16, BTN_MIDDLE),
            (8u16, BTN_SIDE),
            (16u16, BTN_EXTRA),
        ] {
            if self.active_buttons & bit != 0 {
                events.push(InputEvent {
                    event_type: EV_KEY,
                    code,
                    value: 0,
                    ..InputEvent::default()
                });
            }
        }
        events.push(InputEvent {
            event_type: EV_SYN,
            code: SYN_REPORT,
            value: 0,
            ..InputEvent::default()
        });
        let bytes = unsafe {
            // SAFETY: See send(); the backing array remains alive for write_all.
            std::slice::from_raw_parts(
                events.as_ptr().cast::<u8>(),
                events.len() * size_of::<InputEvent>(),
            )
        };
        let _ = self.file.write_all(bytes);
        self.active_buttons = 0;
    }
}

impl Drop for UinputMouse {
    fn drop(&mut self) {
        self.release_buttons();
        if self.created {
            let _ = ioctl_no_arg(self.file.as_raw_fd(), UI_DEV_DESTROY);
            self.created = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_uinput_event_layout_matches_64_bit_kernel_abi() {
        assert_eq!(size_of::<InputEvent>(), 24);
        assert_eq!(size_of::<UinputSetup>(), 92);
    }

    #[test]
    fn fractional_sensitivity_is_accumulated_without_losing_small_motion() {
        let (a, ra) = accumulate_relative(0.4, 0.0);
        let (b, rb) = accumulate_relative(0.4, ra);
        let (c, rc) = accumulate_relative(0.4, rb);
        assert_eq!([a, b, c], [0, 0, 1]);
        assert!((rc - 0.2).abs() < 0.0001);
        let (reverse, _) = accumulate_relative(-0.4, rc);
        assert_eq!(reverse, 0);
    }
}
