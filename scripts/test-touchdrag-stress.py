#!/usr/bin/env python3
"""Live Waydroid stress test for independent F1 capture, F2 mouse mapping and F3 aim."""

from __future__ import annotations

import fcntl
import json
import os
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


def cursor_visible() -> bool:
    output = run(["adb", "shell", "dumpsys", "window", "windows"]).stdout
    return "WayTouch Cursor" in output


def wait_for(predicate, expected: bool, description: str, timeout: float = 4.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if bool(predicate()) == expected:
            return
        time.sleep(0.05)
    raise RuntimeError(f"Timed out waiting for {description}={expected}.")


def mouse_is_grabbed(path: str) -> bool:
    fd = os.open(path, os.O_RDONLY | os.O_NONBLOCK)
    try:
        try:
            fcntl.ioctl(fd, EVIOCGRAB, 1)
        except OSError as error:
            if error.errno == 16:
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
    """Do not emit synthetic hotkeys until the controller has opened its evdev nodes."""
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


def remote_log_lines() -> int:
    return int(run(["adb", "shell", "wc", "-l", REMOTE_LOG]).stdout.split()[0])


def main() -> int:
    if not HOST.is_file():
        raise SystemExit("Build the release host first: ./scripts/build.sh")
    run(["adb", "get-state"])

    keyboard = UInput(
        {e.EV_KEY: [e.KEY_F1, e.KEY_F2, e.KEY_F3, e.KEY_W, e.KEY_SPACE,
                    e.KEY_LEFTCTRL, e.KEY_LEFTALT, e.KEY_ESC]},
        name="WayTouch TouchDrag stress keyboard",
        bustype=e.BUS_USB,
    )
    mouse = UInput(
        {e.EV_KEY: [e.BTN_LEFT, e.BTN_RIGHT, e.BTN_MIDDLE],
         e.EV_REL: [e.REL_X, e.REL_Y, e.REL_WHEEL]},
        name="WayTouch TouchDrag stress mouse",
        bustype=e.BUS_USB,
    )
    profile = json.loads(PROFILE.read_text(encoding="utf-8"))
    profile["devices"] = {
        "keyboard": [keyboard.device.path],
        "mouse": [mouse.device.path],
        "suppress": [],
    }
    profile["mouse"]["enabled"] = True
    profile["mouse"]["native_buttons"] = True
    profile["mouse"]["look"]["mode"] = "touch_drag"
    profile["mouse"]["cursor"]["enabled"] = True
    profile["mouse"]["cursor"]["hide_while_aiming"] = False
    # Deterministic bindings: the assertions below count MAP_DOWN events for
    # exactly one mapped mouse button ("action_3" on BTN_LEFT), independent of
    # how default.json evolves.
    profile["buttons"] = [
        {"name": "action_3", "source": "mouse", "code": 272, "position": [0.86, 0.62]},
    ]
    profile_file = tempfile.NamedTemporaryFile(
        mode="w", encoding="utf-8", suffix=".json",
        prefix="waytouch-touchdrag-stress-", delete=False,
    )
    profile_path = Path(profile_file.name)
    json.dump(profile, profile_file, ensure_ascii=False, indent=2)
    profile_file.write("\n")
    profile_file.close()

    controller: subprocess.Popen[str] | None = None
    controller_output = ""
    before = remote_log_lines()
    try:
        checked = run([str(HOST), "profile-check", str(profile_path)], check=False)
        if checked.returncode:
            raise RuntimeError("Temporary TouchDrag profile is invalid: " + checked.stdout + checked.stderr)

        controller = subprocess.Popen(
            [str(HOST), "control", str(profile_path)],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
        wait_for_controller_devices(controller, [keyboard.device.path, mouse.device.path])
        if controller.poll() is not None:
            raise RuntimeError("Controller exited during startup.")

        send_key(keyboard, e.KEY_F1, 1)
        send_key(keyboard, e.KEY_F1, 0)
        wait_for_mouse_grabbed(mouse.device.path, True)
        wait_for(cursor_visible, True, "Android custom cursor")
        send_key(keyboard, e.KEY_F3, 1)
        send_key(keyboard, e.KEY_F3, 0)
        time.sleep(0.15)

        movement_frames = 240
        mapped_clicks = 0
        for index in range(movement_frames):
            mouse.write(e.EV_REL, e.REL_X, (index % 7) - 3)
            mouse.write(e.EV_REL, e.REL_Y, ((index * 3) % 7) - 3)
            mouse.syn()
            if index % 7 == 0:
                send_key(mouse, e.BTN_LEFT, 1)
                time.sleep(0.003)
                send_key(mouse, e.BTN_LEFT, 0)
                mapped_clicks += 1
            if index % 40 == 0 and controller.poll() is not None:
                raise RuntimeError("Controller exited during TouchDrag motion burst.")

        for _ in range(24):
            send_key(mouse, e.BTN_LEFT, 1)
            time.sleep(0.003)
            send_key(mouse, e.BTN_LEFT, 0)
            mapped_clicks += 1
        # Let the ordered Android touch queue drain before toggling mapping.
        time.sleep(0.4)

        # F2 disables only mouse-button mapping; F1 capture and F3 aim remain.
        send_key(keyboard, e.KEY_F2, 1)
        send_key(keyboard, e.KEY_F2, 0)
        time.sleep(0.2)
        if not mouse_is_grabbed(mouse.device.path) or not cursor_visible():
            raise RuntimeError("F2 mapping disable changed F1 capture or Android cursor.")
        suspended_clicks = 8
        for _ in range(suspended_clicks):
            send_key(mouse, e.BTN_LEFT, 1)
            time.sleep(0.003)
            send_key(mouse, e.BTN_LEFT, 0)
        time.sleep(0.15)

        # Re-enable mapped mouse buttons without changing mouse capture.
        send_key(keyboard, e.KEY_F2, 1)
        send_key(keyboard, e.KEY_F2, 0)
        time.sleep(0.2)
        if not mouse_is_grabbed(mouse.device.path) or not cursor_visible():
            raise RuntimeError("F2 mapping enable changed F1 capture or Android cursor.")
        for _ in range(4):
            send_key(mouse, e.BTN_LEFT, 1)
            time.sleep(0.003)
            send_key(mouse, e.BTN_LEFT, 0)
            mapped_clicks += 1
        time.sleep(0.2)

        # F3 unlocks camera aim; F1 alone returns the physical mouse to the host.
        send_key(keyboard, e.KEY_F3, 1)
        send_key(keyboard, e.KEY_F3, 0)
        time.sleep(0.2)
        if not mouse_is_grabbed(mouse.device.path):
            raise RuntimeError("F3 aim unlock unexpectedly released F1 capture.")

        send_key(keyboard, e.KEY_F1, 1)
        send_key(keyboard, e.KEY_F1, 0)
        wait_for_mouse_grabbed(mouse.device.path, False)
        wait_for(cursor_visible, False, "Android cursor hide")
        if controller.poll() is not None:
            raise RuntimeError("Controller exited after F1 release.")

    finally:
        if controller is not None and controller.poll() is None:
            controller.terminate()
            try:
                controller_output, _ = controller.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                controller.kill()
                controller_output, _ = controller.communicate(timeout=2)
        elif controller is not None and controller.stdout is not None:
            controller_output = controller.stdout.read()

        keyboard.close()
        mouse.close()
        profile_file.close()
        profile_path.unlink(missing_ok=True)

    after = remote_log_lines()
    delta = max(1, after - before)
    android_log = run(["adb", "shell", "tail", "-n", str(delta), REMOTE_LOG]).stdout
    if "WTCH protocol/client error" in android_log:
        raise SystemExit("Android WTCH protocol failure:\n" + android_log[-5000:])
    if "WTCH mouse client error" in android_log:
        raise SystemExit("Android WTM3 mouse failure:\n" + android_log[-5000:])
    if "ANDROID_TOUCH_TRANSPORT_ERROR" in controller_output:
        raise SystemExit("Host touch transport failed:\n" + controller_output[-5000:])
    if "ANDROID_MOUSE_DELIVERY_ERROR" in controller_output:
        raise SystemExit("Host mouse transport failed:\n" + controller_output[-5000:])

    accepted_touches = sum("WTCH accepted seq=" in line for line in android_log.splitlines())
    mapped_downs = controller_output.count("MAP_DOWN source=Mouse control=action_3")
    if accepted_touches < 20 or mapped_downs < 55:
        raise SystemExit(
            f"Stress traffic was too low: accepted_touches={accepted_touches}, mapped_downs={mapped_downs}."
        )
    if mapped_downs != mapped_clicks:
        raise SystemExit(
            "Suspended clicks must not create mapped touches: "
            f"mapped_downs={mapped_downs} != active mapped_clicks={mapped_clicks}."
        )
    native_primary_presses = android_log.count(
        "WTCH_MOUSE_BUTTON_ACCEPTED action=11 actionButton=1")
    if native_primary_presses != suspended_clicks:
        raise SystemExit(
            "Suspended BTN_LEFT clicks must reach Android natively: "
            f"expected={suspended_clicks}, observed={native_primary_presses}."
        )

    print(f"TOUCHDRAG_STRESS_OK movement_frames={movement_frames} mapped_clicks={mapped_clicks}")
    print(f"MAPPED_ACTION_DOWNS={mapped_downs}")
    print(f"ANDROID_TOUCH_FRAMES_ACCEPTED={accepted_touches}")
    print(f"ANDROID_MOVE_DROPS={sum('WTCH_MOVE_DROPPED' in x for x in android_log.splitlines())}")
    print(f"ANDROID_MOUSE_FRAME_DROPS={sum('WTCH_MOUSE_FRAME_DROPPED' in x for x in android_log.splitlines())}")
    print("ANDROID_PROTOCOL_ERRORS=0")
    print("ANDROID_MOUSE_ERRORS=0")
    print("F1 capture, F2 mouse mapping and F3 aim verified; controller stopped; temporary devices/profile removed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
