#!/usr/bin/env python3
"""Live integration check for the Qt backend and the Rust controller."""

from __future__ import annotations

import fcntl
import hashlib
import json
import os
import sys
from pathlib import Path

from PySide6.QtCore import QCoreApplication, QTimer

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "gui"))
from waytouch import WayTouchBackend  # noqa: E402

PROFILE = ROOT / "profiles" / "default.json"
INPUTS = [
    "/dev/input/by-path/platform-i8042-serio-0-event-kbd",
    "/dev/input/by-id/usb-15ca_USB_Optical_Mouse-event-mouse",
    "/dev/input/event5",
    "/dev/input/by-path/platform-AMDI0010:03-event-mouse",
]
EVIOCGRAB = 0x40044590

app = QCoreApplication([])
backend = WayTouchBackend(auto_probe=True)
default_before = hashlib.sha256(PROFILE.read_bytes()).hexdigest()
state = {"started": False, "stopped": False, "failed": False, "attempts": 0, "checking": False}


def finish(code: int, message: str) -> None:
    if code:
        print("QT_BACKEND_TEST_FAIL:", message, file=sys.stderr)
        state["failed"] = True
    else:
        print(message)
    app.exit(code)


def assert_server_and_profiles() -> None:
    state["attempts"] += 1
    if not backend.serverReady:
        if state["attempts"] < 12:
            QTimer.singleShot(250, assert_server_and_profiles)
            return
        finish(1, "Android bridge not ready: " + backend.serverState)
        return

    if not backend.validateProfile():
        finish(1, "Default profile failed Rust validation:\n" + backend.logText[-2000:])
        return

    # Validate that invalid numeric values are rejected, then restore the live
    # value before creating a temporary profile copy.
    original = float(backend.value("mouse.cursor_speed"))
    backend.setValue("mouse.cursor_speed", 0.0)
    if backend.validateProfile():
        finish(1, "Invalid cursor speed was unexpectedly accepted.")
        return
    backend.setValue("mouse.cursor_speed", original)
    if not backend.validateProfile():
        finish(1, "Profile did not validate after restoring its original value.")
        return

    # Exercise the named mouse presets without changing the default profile.
    original_mouse = json.loads(json.dumps(backend._profile.get("mouse", {})))
    if not backend.setMousePreset("mobile_shooter"):
        finish(1, "Mobile shooter mouse preset was rejected.")
        return
    mobile = backend._profile["mouse"]
    if (
        mobile["look"]["mode"] != "touch_drag"
        or mobile["cursor"]["enabled"] is not True
        or mobile["look"]["acceleration"]["enabled"] is not False
    ):
        finish(1, "Mobile shooter mouse preset did not apply its complete configuration.")
        return
    if not backend.setMousePreset("native_relative"):
        finish(1, "Native relative mouse preset was rejected.")
        return
    native = backend._profile["mouse"]["look"]
    if native["mode"] != "relative_mouse":
        finish(1, "Native relative mouse preset did not apply its complete configuration.")
        return
    backend._profile["mouse"] = original_mouse
    backend.profileChanged.emit()
    print("Mouse presets: mobile Touch Drag and native relative selection passed.")

    test_profile = Path("/tmp/waytouch-qt-profile-test.json")
    test_profile.unlink(missing_ok=True)
    if not backend.saveProfileAs(str(test_profile)):
        finish(1, "Atomic save-as of validated test profile failed: " + backend.statusMessage)
        return
    saved = json.loads(test_profile.read_text(encoding="utf-8"))
    if float(saved["mouse"]["cursor_speed"]) != original:
        finish(1, "Saved test profile did not preserve its expected mouse setting.")
        return
    test_profile.unlink(missing_ok=True)

    if not backend.loadProfile(str(PROFILE)):
        finish(1, "Could not restore default profile in backend.")
        return

    backend.controllerStateChanged.connect(on_controller_state)
    backend.startControl()
    state["checking"] = backend.controllerState == "Checking"
    if not backend.controllerRunning or not state["checking"]:
        finish(1, "Background preflight did not enter Checking state.")
        return
    QTimer.singleShot(12000, lambda: finish(1, "Controller did not complete background preflight/start/stop."))


def on_controller_state() -> None:
    current = backend.controllerState
    if current in {"Starting", "Capturing"} and not state["started"]:
        state["started"] = True
        # Always stop after the process has actually started, not while the
        # background preflight is still probing adb and Android channels.
        QTimer.singleShot(900, lambda: backend.stopControl(False))
    elif current == "Stopped" and state["checking"]:
        if state["started"]:
            state["stopped"] = True
            app.quit()
        else:
            finish(1, "Background preflight rejected the live test: " + backend.statusMessage)


QTimer.singleShot(0, assert_server_and_profiles)
exit_code = app.exec()

if exit_code != 0 or state["failed"] or not state["started"] or not state["stopped"]:
    print("Controller log:\n", backend.logText[-5000:], file=sys.stderr)
    raise SystemExit(1)

# The original profile must remain byte-for-byte unchanged by the test.
default_after = hashlib.sha256(PROFILE.read_bytes()).hexdigest()
if default_before != default_after:
    raise SystemExit("Default profile unexpectedly changed during integration test.")

# Verify that each exclusive evdev grab is available again after stopping.
for path in INPUTS:
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK)
    try:
        fcntl.ioctl(fd, EVIOCGRAB, 1)
        fcntl.ioctl(fd, EVIOCGRAB, 0)
    finally:
        os.close(fd)

# The kernel uinput output node must disappear when the Rust controller exits.
for event in Path("/sys/class/input").glob("event*"):
    try:
        device_name = (event / "device" / "name").read_text().strip()
    except OSError:
        continue
    if device_name == "WayTouch Virtual Mouse":
        raise SystemExit("WayTouch Virtual Mouse remained after controller shutdown.")

print("QT_BACKEND_INTEGRATION_OK")
print("Profile validation/rejection, save-as, controller start/stop, EVIOCGRAB release and uinput cleanup passed.")
