#!/usr/bin/env python3
"""End-to-end WayTouch F1 mouse capture + F2 mouse mapping + F3 camera test.

Requires: python-evdev, /dev/uinput access, a running WayTouch Android server,
ADB forwarding on 27183/27184/27186, and the GNOME WayTouch cursor extension.
The test uses temporary synthetic input devices and never edits the default profile.
"""

from __future__ import annotations

import fcntl
import json
import os
import re
import subprocess
import tempfile
import time
from pathlib import Path

from evdev import UInput, ecodes as e

ROOT = Path(__file__).resolve().parents[1]
HOST = ROOT / "rust-host" / "target" / "release" / "waytouch-host"
PROFILE = ROOT / "profiles" / "default.json"
REMOTE_LOG = "/data/local/tmp/waytouch.log"
EVIOCGRAB = 0x40044590


def run(args: list[str], *, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(args, capture_output=True, text=True, check=check)


def android_cursor_frame() -> tuple[int, int, int, int] | None:
    output = run(["adb", "shell", "dumpsys", "window", "windows"]).stdout
    marker = output.find("WayTouch Cursor")
    if marker < 0:
        return None
    match = re.search(
        r"\bframe=\[(-?\d+),(-?\d+)\]\[(-?\d+),(-?\d+)\]",
        output[marker:marker + 3000],
    )
    return tuple(map(int, match.groups())) if match else None


def wait_for_cursor_frame(
    *, changed_from: tuple[int, int, int, int] | None = None, timeout: float = 4.0
) -> tuple[int, int, int, int]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        frame = android_cursor_frame()
        if frame is not None and (changed_from is None or frame != changed_from):
            return frame
        time.sleep(0.08)
    raise RuntimeError("Android custom mouse cursor did not appear/move in the expected time.")


def wait_for_cursor_hidden(timeout: float = 4.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if android_cursor_frame() is None:
            return
        time.sleep(0.08)
    raise RuntimeError("Android custom cursor remained visible after F1 was released.")


def mouse_is_grabbed(path: str) -> bool:
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK)
    try:
        try:
            fcntl.ioctl(fd, EVIOCGRAB, 1)
        except OSError as error:
            if error.errno == 16:  # EBUSY: WayTouch currently owns EVIOCGRAB.
                return True
            raise
        else:
            fcntl.ioctl(fd, EVIOCGRAB, 0)
            return False
    finally:
        os.close(fd)


def wait_for_mouse_grabbed(path: str, expected: bool, timeout: float = 3.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if mouse_is_grabbed(path) == expected:
            return
        time.sleep(0.05)
    raise RuntimeError(f"Physical mouse EVIOCGRAB state did not become {expected}.")


def wait_for_controller_devices(
    controller: subprocess.Popen[str], paths: list[str], timeout: float = 5.0
) -> None:
    """Wait until evdev descriptors are open before sending F1 or gameplay input."""
    deadline = time.monotonic() + timeout
    expected = set(paths)
    while time.monotonic() < deadline:
        if controller.poll() is not None:
            raise RuntimeError("Controller exited before opening the selected input devices.")
        try:
            descriptors = Path(f"/proc/{controller.pid}/fd").iterdir()
            opened = {
                os.readlink(descriptor)
                for descriptor in descriptors
                if descriptor.is_symlink()
            }
        except (FileNotFoundError, PermissionError, ProcessLookupError):
            opened = set()
        if expected.issubset(opened):
            return
        time.sleep(0.025)
    raise RuntimeError(
        "Controller did not open all selected evdev devices: "
        + ", ".join(sorted(expected - opened))
    )


def send_key(device: UInput, code: int, value: int) -> None:
    # Give the evdev reader a distinct press frame before the matching release.
    if value == 0 and code in {e.KEY_F1, e.KEY_F2, e.KEY_F3}:
        time.sleep(0.025)
    device.write(e.EV_KEY, code, value)
    device.syn()


def main() -> int:
    if not HOST.is_file():
        raise SystemExit("Build the Rust release host first: ./scripts/build.sh")
    run(["adb", "get-state"])
    forwards = run(["adb", "forward", "--list"]).stdout
    for port in (27183, 27184, 27186):
        if f"tcp:{port} tcp:{port}" not in forwards:
            raise SystemExit(f"ADB forward tcp:{port} is missing; run scripts/start-server.sh first.")

    keyboard_caps = {
        e.EV_KEY: [
            e.KEY_ESC, e.KEY_1, e.KEY_2, e.KEY_3, e.KEY_4, e.KEY_5,
            e.KEY_Q, e.KEY_W, e.KEY_E, e.KEY_R, e.KEY_T,
            e.KEY_A, e.KEY_S, e.KEY_D, e.KEY_F, e.KEY_G,
            e.KEY_Z, e.KEY_X, e.KEY_C, e.KEY_V, e.KEY_B,
            e.KEY_SPACE, e.KEY_LEFTCTRL, e.KEY_LEFTALT, e.KEY_F1, e.KEY_F2, e.KEY_F3,
            e.KEY_ENTER, e.KEY_BACKSPACE, e.KEY_TAB, e.KEY_LEFTSHIFT,
        ]
    }
    mouse_caps = {
        e.EV_KEY: [e.BTN_LEFT, e.BTN_RIGHT, e.BTN_MIDDLE, e.BTN_SIDE, e.BTN_EXTRA],
        e.EV_REL: [e.REL_X, e.REL_Y, e.REL_WHEEL, e.REL_HWHEEL],
    }

    keyboard = UInput(keyboard_caps, name="WayTouch E2E Source Keyboard", bustype=e.BUS_USB)
    mouse = UInput(mouse_caps, name="WayTouch E2E Source Mouse", bustype=e.BUS_USB)
    profile_file = tempfile.NamedTemporaryFile(
        mode="w", encoding="utf-8", suffix=".json", prefix="waytouch-e2e-", delete=False
    )
    profile_path = Path(profile_file.name)
    device_paths = [keyboard.device.path, mouse.device.path]
    controller: subprocess.Popen[str] | None = None
    controller_output = ""
    grab_errors: list[str] = []
    try:
        profile = json.loads(PROFILE.read_text(encoding="utf-8"))
        profile["devices"]["keyboard"] = [keyboard.device.path]
        profile["devices"]["mouse"] = [mouse.device.path]
        profile["devices"]["suppress"] = []
        # Relative mode prevents the test from placing gameplay touchscreen
        # contacts while validating independent F1/F2/F3 input behavior.
        profile["mouse"]["look"]["mode"] = "relative_mouse"
        profile["mouse"]["cursor"]["enabled"] = True
        profile["mouse"]["cursor"]["hide_while_aiming"] = False
        # In locked RelativeMouse mode Android receives this transformed motion.
        # The overlay must use the same deltas, not raw evdev values.
        profile["mouse"]["look"]["sensitivity_x"] = 1.5
        profile["mouse"]["look"]["sensitivity_y"] = 2.0
        profile["mouse"]["look"]["invert_y"] = True
        # Deterministic bindings: the F2 enable/disable assertions below
        # depend on exactly BTN_LEFT being mapped and BTN_RIGHT/BTN_MIDDLE
        # staying native, independent of how default.json evolves.
        profile["buttons"] = [
            {"name": "fire", "source": "mouse", "code": 272, "position": [0.94, 0.42]},
        ]
        json.dump(profile, profile_file, ensure_ascii=False, indent=2)
        profile_file.write("\n")
        profile_file.close()

        checked = run([str(HOST), "profile-check", str(profile_path)], check=False)
        if checked.returncode:
            raise RuntimeError("Synthetic profile validation failed: " + checked.stdout + checked.stderr)

        before = int(run(["adb", "shell", "wc", "-l", REMOTE_LOG]).stdout.split()[0])
        controller = subprocess.Popen(
            [str(HOST), "control", str(profile_path)],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
        wait_for_controller_devices(controller, device_paths)
        if controller.poll() is not None:
            raise RuntimeError("WayTouch controller exited before test input.")
        if android_cursor_frame() is not None:
            raise RuntimeError("Android custom cursor appeared before F1 was pressed.")
        if mouse_is_grabbed(mouse.device.path):
            raise RuntimeError("Starting the key mapper unexpectedly grabbed the physical mouse.")

        # While F1 is off the physical mouse remains owned by the host; no mouse
        # MotionEvent or Android cursor overlay should be produced.
        mouse.write(e.EV_REL, e.REL_X, 7)
        mouse.write(e.EV_REL, e.REL_Y, 4)
        mouse.syn()
        time.sleep(0.25)
        if android_cursor_frame() is not None:
            raise RuntimeError("Android overlay appeared while F1 capture was disabled.")
        pre_capture_lines = run(["adb", "shell", "tail", "-n",             str(max(1, int(run(["adb", "shell", "wc", "-l", REMOTE_LOG]).stdout.split()[0]) - before)),
            REMOTE_LOG]).stdout
        if "WTCH_MOUSE_ACTIVE" in pre_capture_lines:
            raise RuntimeError("Mouse motion reached Android before F1 capture was enabled.")

        # F1 captures the physical mouse and shows Android's cursor.
        send_key(keyboard, e.KEY_F1, 1)
        send_key(keyboard, e.KEY_F1, 0)
        try:
            wait_for_mouse_grabbed(mouse.device.path, True)
        except RuntimeError as failure:
            if controller is not None and controller.poll() is None:
                controller.terminate()
                try:
                    controller_output, _ = controller.communicate(timeout=5)
                except subprocess.TimeoutExpired:
                    controller.kill()
                    controller_output, _ = controller.communicate(timeout=2)
            raise RuntimeError(
                str(failure) + "\\n--- controller output ---\\n" + controller_output[-8000:]
            ) from failure
        cursor_initial = wait_for_cursor_frame()

        # F3 controls camera aim only; it must not change F1 capture or F2 mapping.
        send_key(keyboard, e.KEY_F3, 1)
        send_key(keyboard, e.KEY_F3, 0)
        time.sleep(0.25)
        if not mouse_is_grabbed(mouse.device.path):
            raise RuntimeError("F3 camera aim incorrectly changed F1 mouse capture.")

        # Native relative movement is transformed by the active camera-look sensitivity.
        mouse.write(e.EV_REL, e.REL_X, 24)
        mouse.write(e.EV_REL, e.REL_Y, -11)
        mouse.syn()
        time.sleep(0.3)
        cursor_after_motion = wait_for_cursor_frame(changed_from=cursor_initial)
        cursor_dx = cursor_after_motion[0] - cursor_initial[0]
        cursor_dy = cursor_after_motion[1] - cursor_initial[1]
        if abs(cursor_dx - 36) > 2 or abs(cursor_dy - 22) > 2:
            raise RuntimeError(
                "Android cursor diverged from delivered MotionEvent deltas: "
                f"expected (+36,+22), observed ({cursor_dx:+d},{cursor_dy:+d})."
            )
        send_key(keyboard, e.KEY_F3, 1)
        send_key(keyboard, e.KEY_F3, 0)
        time.sleep(0.2)
        if not mouse_is_grabbed(mouse.device.path):
            raise RuntimeError("F3 aim unlock incorrectly changed F1 mouse capture.")

        # F2 disables only mapped mouse buttons. Capture and Android cursor stay active.
        send_key(keyboard, e.KEY_F2, 1)
        send_key(keyboard, e.KEY_F2, 0)
        time.sleep(0.2)
        if not mouse_is_grabbed(mouse.device.path):
            raise RuntimeError("F2 mapping disable changed F1 mouse capture.")
        if android_cursor_frame() is None:
            raise RuntimeError("F2 mapping disable hid the Android cursor.")

        # The mapped BTN_LEFT now passes through as a native Android click.
        send_key(mouse, e.BTN_LEFT, 1)
        time.sleep(0.08)
        send_key(mouse, e.BTN_LEFT, 0)
        time.sleep(0.08)

        # Unmapped secondary/tertiary buttons must remain native Android clicks.
        for button in (e.BTN_RIGHT, e.BTN_MIDDLE):
            send_key(mouse, button, 1)
            time.sleep(0.08)
            send_key(mouse, button, 0)
            time.sleep(0.08)

        # F2 re-enables the mouse-button touch mapping without touching capture.
        send_key(keyboard, e.KEY_F2, 1)
        send_key(keyboard, e.KEY_F2, 0)
        time.sleep(0.2)
        if not mouse_is_grabbed(mouse.device.path):
            raise RuntimeError("F2 mapping enable changed F1 mouse capture.")

        # A mapped press is a touch, never a native mouse click.
        send_key(mouse, e.BTN_LEFT, 1)
        time.sleep(0.08)
        send_key(mouse, e.BTN_LEFT, 0)
        time.sleep(0.08)

        # F1 only releases the mouse to the host; the enabled mapping preference remains.
        send_key(keyboard, e.KEY_F1, 1)
        send_key(keyboard, e.KEY_F1, 0)
        wait_for_mouse_grabbed(mouse.device.path, False)
        wait_for_cursor_hidden()

        # Re-capture without touching F2: mouse-button mapping must still be enabled.
        send_key(keyboard, e.KEY_F1, 1)
        send_key(keyboard, e.KEY_F1, 0)
        try:
            wait_for_mouse_grabbed(mouse.device.path, True)
        except RuntimeError as failure:
            if controller is not None and controller.poll() is None:
                controller.terminate()
                try:
                    controller_output, _ = controller.communicate(timeout=5)
                except subprocess.TimeoutExpired:
                    controller.kill()
                    controller_output, _ = controller.communicate(timeout=2)
            raise RuntimeError(
                str(failure) + "\\n--- controller output ---\\n" + controller_output[-8000:]
            ) from failure
        wait_for_cursor_frame()

        send_key(mouse, e.BTN_LEFT, 1)
        time.sleep(0.08)
        send_key(mouse, e.BTN_LEFT, 0)
        time.sleep(0.08)

        # Release capture again, then toggle F2 while the host owns the mouse.
        send_key(keyboard, e.KEY_F1, 1)
        send_key(keyboard, e.KEY_F1, 0)
        wait_for_mouse_grabbed(mouse.device.path, False)
        wait_for_cursor_hidden()

        send_key(keyboard, e.KEY_F2, 1)
        send_key(keyboard, e.KEY_F2, 0)
        time.sleep(0.2)
        if mouse_is_grabbed(mouse.device.path):
            raise RuntimeError("F2 mapping toggle captured the physical mouse from GNOME.")
        if android_cursor_frame() is not None:
            raise RuntimeError("F2 mapping toggle showed the Android cursor.")

        # F1 recaptures but preserves the now-disabled mouse-button mapping.
        send_key(keyboard, e.KEY_F1, 1)
        send_key(keyboard, e.KEY_F1, 0)
        try:
            wait_for_mouse_grabbed(mouse.device.path, True)
        except RuntimeError as failure:
            if controller is not None and controller.poll() is None:
                controller.terminate()
                try:
                    controller_output, _ = controller.communicate(timeout=5)
                except subprocess.TimeoutExpired:
                    controller.kill()
                    controller_output, _ = controller.communicate(timeout=2)
            raise RuntimeError(
                str(failure) + "\\n--- controller output ---\\n" + controller_output[-8000:]
            ) from failure
        wait_for_cursor_frame()
        send_key(mouse, e.BTN_LEFT, 1)
        time.sleep(0.08)
        send_key(mouse, e.BTN_LEFT, 0)
        time.sleep(0.08)

        # F2 can re-enable mapping while captured and leaves capture untouched.
        send_key(keyboard, e.KEY_F2, 1)
        send_key(keyboard, e.KEY_F2, 0)
        time.sleep(0.2)
        if not mouse_is_grabbed(mouse.device.path):
            raise RuntimeError("F2 re-enable changed F1 capture.")

        send_key(keyboard, e.KEY_F1, 1)
        send_key(keyboard, e.KEY_F1, 0)
        wait_for_mouse_grabbed(mouse.device.path, False)
        wait_for_cursor_hidden()
        if controller.poll() is not None:
            raise RuntimeError("Controller exited during independent F1/F2 state testing.")

    finally:
        if controller is not None and controller.poll() is None:
            controller.terminate()
            try:
                controller_output, _ = controller.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                controller.kill()
                controller_output, _ = controller.communicate(timeout=2)
        elif controller is not None and controller.stdout is not None and not controller.stdout.closed:
            controller_output = controller.stdout.read()

        # The uinput nodes still exist here; verify the controller released
        # EVIOCGRAB before closing the synthetic devices themselves.
        for path in device_paths:
            try:
                fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK)
                try:
                    fcntl.ioctl(fd, EVIOCGRAB, 1)
                    fcntl.ioctl(fd, EVIOCGRAB, 0)
                finally:
                    os.close(fd)
            except OSError as error:
                grab_errors.append(f"{path}: {error}")
        for device in (keyboard, mouse):
            device.close()
        profile_file.close()
        profile_path.unlink(missing_ok=True)

    tags = (
        "MOUSE_CONFIG", "ANDROID_MOUSE_CONNECTED", "MOUSE_INPUT_ACTIVE",
        "MOUSE_MOTION_DETECTED", "MOUSE_CAPTURE", "MOUSE_EVDEV_CAPTURE",
        "AIM_LOCK", "HOTKEY_F3", "MOUSE_BUTTON_MAPPING",
        "MAP_DOWN",
        "ERROR", "error",
    )
    print("\n".join(line for line in controller_output.splitlines() if any(tag in line for tag in tags)))

    required = (
        "MOUSE_CAPTURE state=active source=F1",
        "MOUSE_CAPTURE state=inactive source=F1",
        "MOUSE_EVDEV_CAPTURE state=grabbed",
        "MOUSE_EVDEV_CAPTURE state=released",
        "MOUSE_INPUT_ACTIVE",
        "MOUSE_MOTION_DETECTED",
        "ANDROID_MOUSE_CONNECTED",
        "AIM_LOCK state=locked source=F3",
        "AIM_LOCK state=unlocked source=F3",
        "HOTKEY_F3 action=lock",
        "HOTKEY_F3 action=unlock",
        "MOUSE_BUTTON_MAPPING state=disabled source=F2",
        "MOUSE_BUTTON_MAPPING state=enabled source=F2",
        "MAP_DOWN source=Mouse control=fire",
    )
    missing = [tag for tag in required if tag not in controller_output]
    if missing:
        raise SystemExit("Controller did not produce required events: " + ", ".join(missing))

    after = int(run(["adb", "shell", "wc", "-l", REMOTE_LOG]).stdout.split()[0])
    delta = max(1, after - before)
    android_lines = run(["adb", "shell", "tail", "-n", str(delta), REMOTE_LOG]).stdout
    if "WTCH_MOUSE_ACTIVE" not in android_lines:
        raise SystemExit("Android did not accept the initial SOURCE_MOUSE hover/motion event.")
    for button in (2, 4):  # Android BUTTON_SECONDARY / BUTTON_TERTIARY
        if f"WTCH_MOUSE_BUTTON_ACCEPTED action=11 actionButton={button}" not in android_lines:
            raise SystemExit(f"Android did not accept mouse button press for button mask {button}.")
        if f"WTCH_MOUSE_BUTTON_ACCEPTED action=12 actionButton={button}" not in android_lines:
            raise SystemExit(f"Android did not accept mouse button release for button mask {button}.")
    primary_presses = android_lines.count(
        "WTCH_MOUSE_BUTTON_ACCEPTED action=11 actionButton=1")
    primary_releases = android_lines.count(
        "WTCH_MOUSE_BUTTON_ACCEPTED action=12 actionButton=1")
    if primary_presses != 2 or primary_releases != 2:
        raise SystemExit(
            "BTN_LEFT must reach Android as native primary clicks only while "
            f"F2 mapping is disabled: presses={primary_presses}, releases={primary_releases}."
        )
    if "InputManager rejected mouse" in android_lines or "WTCH mouse client error" in android_lines:
        raise SystemExit("Android reported a native mouse injection failure:\n" + android_lines)

    if grab_errors:
        raise SystemExit("Controller did not release synthetic evdev grabs: " + "; ".join(grab_errors))

    print("INPUT_PIPELINE_E2E_OK")
    print("evdev REL_X/REL_Y -> Rust -> Android SOURCE_MOUSE accepted.")
    print("Starting the key mapper did not grab the physical mouse or show the Android overlay.")
    print("F1 toggled EVIOCGRAB and the Android custom cursor without a desktop extension.")
    print("F1 only transferred mouse ownership between the host and Android.")
    print("F2 only toggled mapped mouse buttons, both while captured and uncaptured.")
    print("F3 camera aim did not change F1 capture or F2 mapping.")
    print("The mapped BTN_LEFT fired a touch while active and a native click while suspended.")
    print("RMB/MMB down/up accepted by Android and did not change pointer capture.")
    print("Android overlay followed the transformed MotionEvent delta.")
    print("All exclusive grabs released after controller shutdown.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
