#!/usr/bin/env python3
"""WayTouch Qt 6 controller backend.

The UI is QML/Qt Quick with the Material style. Process ownership and profile
validation stay in this Python QObject; no shell command is assembled from
profile values.
"""

from __future__ import annotations

import copy
import json
import math
import os
import re
import shutil
import socket
import stat
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any

from PySide6.QtCore import (
    QObject, Property, QProcess, QThread, QTimer, Signal, Slot
)

ROOT = Path(__file__).resolve().parents[1]
HOST = ROOT / "rust-host" / "target" / "release" / "waytouch-host"
PROFILE_DIR = ROOT / "profiles"
SERVER_SCRIPT = ROOT / "scripts" / "start-server.sh"
BUILD_SCRIPT = ROOT / "scripts" / "build.sh"
REMOTE_LOG = "/data/local/tmp/waytouch.log"
MAX_LOG_LINES = 700


def _ipc_token_file() -> Path:
    if os.environ.get("WAYTOUCH_IPC_TOKEN_FILE"):
        return Path(os.environ["WAYTOUCH_IPC_TOKEN_FILE"])
    state_home = Path(os.environ.get("XDG_STATE_HOME", Path.home() / ".local/state"))
    return state_home / "waytouch" / "ipc-token"


def _authenticate_socket(connection: socket.socket) -> None:
    token_path = _ipc_token_file()
    mode = stat.S_IMODE(token_path.stat().st_mode)
    if mode & 0o077:
        raise PermissionError(f"IPC token permissions must be 0600: {token_path}")
    token = token_path.read_text(encoding="ascii").strip().lower()
    if len(token) != 64 or any(char not in "0123456789abcdef" for char in token):
        raise ValueError(f"IPC token is missing or malformed: {token_path}")
    connection.sendall(f"AUTH {token}\n".encode("ascii"))
    response = bytearray()
    while len(response) <= 64:
        byte = connection.recv(1)
        if not byte:
            break
        if byte == b"\n":
            break
        if byte != b"\r":
            response.extend(byte)
    if bytes(response) != b"OK AUTH":
        raise PermissionError("Android rejected WayTouch IPC authentication")


def _run_capture(args: list[str], timeout: float = 2.0) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        cwd=ROOT,
        text=True,
        encoding="utf-8",
        errors="replace",
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        timeout=timeout,
        check=False,
    )


def _set_nested(node: dict[str, Any], path: str, value: Any) -> None:
    parts = path.split(".")
    current: Any = node
    for part in parts[:-1]:
        key: Any = int(part) if isinstance(current, list) and part.isdigit() else part
        current = current[key]
    final: Any = int(parts[-1]) if isinstance(current, list) and parts[-1].isdigit() else parts[-1]
    current[final] = value


def _get_nested(node: dict[str, Any], path: str, default: Any = None) -> Any:
    current: Any = node
    for part in path.split("."):
        key: Any = int(part) if isinstance(current, list) and part.isdigit() else part
        try:
            current = current[key]
        except (KeyError, IndexError, TypeError):
            return default
    return current


class _PreflightWorker(QThread):
    resultReady = Signal(bool, str)

    def __init__(self, backend: "WayTouchBackend", profile: dict[str, Any]) -> None:
        super().__init__()
        self._backend = backend
        self._profile = profile

    def run(self) -> None:
        try:
            okay, details = self._backend._preflight(self._profile)
        except Exception as exc:  # Keep worker failures visible to the UI.
            okay, details = False, f"Preflight failed unexpectedly: {exc}"
        self.resultReady.emit(okay, details)


class _StatusProbeWorker(QThread):
    resultReady = Signal(str, bool, str)

    def __init__(
        self,
        backend: "WayTouchBackend",
        mouse_enabled: bool,
        cursor_enabled: bool,
    ) -> None:
        super().__init__()
        self._backend = backend
        self._mouse_enabled = mouse_enabled
        self._cursor_enabled = cursor_enabled

    def run(self) -> None:
        try:
            # The main touch socket is a single-controller stream. Probing it
            # while the Rust controller owns it creates noisy rejected clients;
            # server readiness is checked from its process log instead.

            if self._mouse_enabled:
                with socket.create_connection(("127.0.0.1", 27186), timeout=0.75) as connection:
                    connection.settimeout(0.75)
                    _authenticate_socket(connection)
                    connection.sendall(b"PING\n")
                    with connection.makefile("r", encoding="ascii") as reader:
                        if reader.readline().strip() != "PONG":
                            raise ConnectionError("Android mouse health probe did not receive PONG")

            if self._cursor_enabled:
                with socket.create_connection(("127.0.0.1", 27185), timeout=0.75) as connection:
                    connection.settimeout(0.75)
                    _authenticate_socket(connection)
                    connection.sendall(b"PING\n")
                    with connection.makefile("r", encoding="ascii") as reader:
                        if reader.readline().strip() != "PONG":
                            raise ConnectionError("Android cursor channel did not answer PING")
        except OSError as exc:
            self.resultReady.emit(
                "TCP CHANNEL OFFLINE", False,
                f"Android IPC authentication/forward check failed: {exc}",
            )
            return

        layout_state = self._backend._read_android_layout_envelope()
        if layout_state is None:
            self.resultReady.emit(
                "LAYOUT CHANNEL OFFLINE", False,
                "WTCH v2 is ready, but revisioned layout channel 27184 is unavailable.",
            )
            return
        self.resultReady.emit(
            "ANDROID READY", True,
            f"ADB + WTCH v2 + layout sync are ready (revision {layout_state['revision']}).",
        )


class WayTouchBackend(QObject):
    profileChanged = Signal()
    profileFilesChanged = Signal()
    dirtyChanged = Signal()
    controllerStateChanged = Signal()
    serverStateChanged = Signal()
    deviceSummaryChanged = Signal()
    logChanged = Signal()
    statusMessageChanged = Signal()
    latencyStatsChanged = Signal()
    quitRequested = Signal()

    def __init__(self, auto_probe: bool = True) -> None:
        super().__init__()
        self._profile_path = PROFILE_DIR / "default.json"
        self._profile: dict[str, Any] = {}
        self._profile_files: list[dict[str, str]] = []
        self._dirty = False
        self._layout_dirty = False
        self._controller: QProcess | None = None
        self._preflight_thread: _PreflightWorker | None = None
        self._preflight_running = False
        self._preflight_cancelled = False
        self._server_process: QProcess | None = None
        self._server_stage = ""
        self._next_adb_recovery_at = 0.0
        self._next_server_recovery_at = 0.0
        self._build_process: QProcess | None = None
        self._probe_process: QProcess | None = None
        self._status_probe_thread: _StatusProbeWorker | None = None
        self._controller_state = "Stopped"
        self._server_state = "Checking"
        self._server_ready = False
        self._device_summary = "Input devices have not been scanned."
        self._log_text = ""
        self._status_message = "Ready."
        self._latency_summary = "Latency: waiting for controller statistics."
        self._quit_after_stop = False
        self._scan_process: QProcess | None = None
        self._discovery_process: QProcess | None = None
        self._test_process: QProcess | None = None

        self._load_initial_profile()
        self._refresh_profile_files()

        self._status_timer = QTimer(self)
        self._status_timer.setInterval(5000)
        self._status_timer.timeout.connect(self.refreshServerStatus)
        if auto_probe:
            self._status_timer.start()
            QTimer.singleShot(0, self.refreshServerStatus)

    @Property("QVariantMap", notify=profileChanged)
    def profile(self) -> dict[str, Any]:
        return self._profile

    @Property("QVariantList", notify=profileFilesChanged)
    def profileFiles(self) -> list[dict[str, str]]:
        return self._profile_files

    @Property(bool, notify=dirtyChanged)
    def dirty(self) -> bool:
        return self._dirty

    @Property(str, notify=controllerStateChanged)
    def controllerState(self) -> str:
        return self._controller_state

    @Property(bool, notify=controllerStateChanged)
    def controllerRunning(self) -> bool:
        return self._preflight_running or (
            self._controller is not None and self._controller.state() != QProcess.NotRunning
        )

    @Property(str, notify=serverStateChanged)
    def serverState(self) -> str:
        return self._server_state

    @Property(bool, notify=serverStateChanged)
    def serverReady(self) -> bool:
        return self._server_ready

    @Property(str, notify=deviceSummaryChanged)
    def deviceSummary(self) -> str:
        return self._device_summary

    @Property(str, notify=logChanged)
    def logText(self) -> str:
        return self._log_text

    @Property(str, notify=statusMessageChanged)
    def statusMessage(self) -> str:
        return self._status_message

    @Property(str, notify=latencyStatsChanged)
    def latencySummary(self) -> str:
        return self._latency_summary

    @Property(str, notify=profileChanged)
    def profileName(self) -> str:
        return self._profile_path.name

    @Property(str, notify=profileChanged)
    def profilePath(self) -> str:
        return str(self._profile_path)

    def _set_status(self, message: str) -> None:
        if message != self._status_message:
            self._status_message = message
            self.statusMessageChanged.emit()

    def _set_dirty(self, value: bool) -> None:
        if value != self._dirty:
            self._dirty = value
            self.dirtyChanged.emit()

    def _append_log(self, message: str) -> None:
        message = message.replace("\r\n", "\n").replace("\r", "\n")
        if not message:
            return
        stats_pattern = re.compile(
            r"TOUCH_STATS count=(\d+) input_coalesced=(\d+) input_dropped=(\d+) "
            r"tcp_coalesced=(\d+) tcp_dropped=(\d+) "
            r"evdev_ack_p50_ms=([0-9.]+) evdev_ack_p95_ms=([0-9.]+) evdev_ack_p99_ms=([0-9.]+)"
        )
        for match in stats_pattern.finditer(message):
            count, input_coalesced, input_dropped, tcp_coalesced, tcp_dropped = (
                int(match.group(i)) for i in range(1, 6)
            )
            p50, p95, p99 = (float(match.group(i)) for i in range(6, 9))
            self._latency_summary = (
                f"evdev→ACK p50 {p50:.2f} ms · p95 {p95:.2f} ms · p99 {p99:.2f} ms"
                f"   |   frames {count} · input drop {input_dropped} · TCP drop {tcp_dropped}"
                f" · coalesced input/TCP {input_coalesced}/{tcp_coalesced}"
            )
            self.latencyStatsChanged.emit()
        self._log_text += message
        if not self._log_text.endswith("\n"):
            self._log_text += "\n"
        lines = self._log_text.splitlines()
        if len(lines) > MAX_LOG_LINES:
            self._log_text = "\n".join(lines[-MAX_LOG_LINES:]) + "\n"
        self.logChanged.emit()

    @staticmethod
    def _ensure_profile_defaults(data: dict[str, Any]) -> None:
        """Add optional professional mouse/cursor defaults without replacing user values."""
        mouse = data.setdefault("mouse", {})
        cursor = mouse.setdefault("cursor", {})
        for key, value in {
            "enabled": True,
            "style": "crosshair",
            "size": 28.0,
            "color": "#ff3158",
            "hide_while_aiming": True,
        }.items():
            cursor.setdefault(key, value)

        look = mouse.setdefault("look", {})
        for key, value in {
            "enabled": True,
            "mode": "touch_drag",
            "sensitivity_x": 1.0,
            "sensitivity_y": 1.0,
            "invert_y": False,
            "smoothing": 0.0,
            "deadzone": 0.0,
            "rate_cap_hz": 240,
            "touch_anchor": [0.76, 0.55],
            "touch_bounds": [0.52, 0.06, 0.98, 0.94],
            "touch_scale": 1.0,
            "edge_recenter": True,
        }.items():
            look.setdefault(key, value)
        acceleration = look.setdefault("acceleration", {})
        for key, value in {
            "enabled": False,
            "threshold": 1.2,
            "max_gain": 2.5,
            "exponent": 1.35,
        }.items():
            acceleration.setdefault(key, value)

    def _load_initial_profile(self) -> None:
        try:
            data = json.loads(self._profile_path.read_text(encoding="utf-8"))
            if not isinstance(data, dict):
                raise ValueError("Profile root must be a JSON object.")
            self._ensure_profile_defaults(data)
            self._profile = data
            self._profile.setdefault("layout_revision", 0)
            self._append_log(f"Profile loaded: {self._profile_path}")
        except (OSError, json.JSONDecodeError, ValueError) as exc:
            self._profile = {}
            self._append_log(f"PROFILE LOAD ERROR: {exc}")

    def _refresh_profile_files(self) -> None:
        PROFILE_DIR.mkdir(parents=True, exist_ok=True)
        profiles = [path for path in PROFILE_DIR.glob("*.json") if path.is_file()]
        if self._profile_path.is_file() and self._profile_path not in profiles:
            profiles.append(self._profile_path)
        profiles.sort(key=lambda path: path.name.lower())
        self._profile_files = [{"name": p.name, "path": str(p)} for p in profiles]
        self.profileFilesChanged.emit()

    def _resolve_profile_path(self, value: str) -> Path:
        path = Path(value).expanduser()
        if not path.is_absolute():
            path = (ROOT / path).resolve()
        return path

    @Slot(str, result=bool)
    def loadProfile(self, value: str) -> bool:
        if self._controller is not None or self._preflight_running:
            self._set_status("Stop controller before switching profiles.")
            return False
        if self._dirty:
            self._set_status("Save current changes before switching profiles.")
            return False
        try:
            path = self._resolve_profile_path(value)
            data = json.loads(path.read_text(encoding="utf-8"))
            if not isinstance(data, dict):
                raise ValueError("Profile root must be a JSON object.")
        except (OSError, json.JSONDecodeError, ValueError) as exc:
            self._set_status(f"Profile load failed: {exc}")
            self._append_log(f"PROFILE LOAD ERROR: {exc}")
            return False

        self._profile_path = path
        self._ensure_profile_defaults(data)
        data.setdefault("layout_revision", 0)
        self._profile = data
        self._set_dirty(False)
        self._layout_dirty = False
        self.profileChanged.emit()
        self._refresh_profile_files()
        self._set_status(f"Loaded {path.name}. Validate before starting control.")
        self._append_log(f"Loaded profile: {path}")
        return True

    @Slot(str, result="QVariant")
    def value(self, path: str) -> Any:
        return _get_nested(self._profile, path)

    @Slot(str, "QVariant")
    def setValue(self, path: str, value: Any) -> None:
        if self._controller is not None or self._preflight_running:
            self._set_status("Stop the controller before editing profile values.")
            return
        try:
            _set_nested(self._profile, path, value)
            if path.startswith("movement."):
                self._layout_dirty = True
            self._set_dirty(True)
            self.profileChanged.emit()
        except (KeyError, IndexError, TypeError, ValueError) as exc:
            self._set_status(f"Setting update failed ({path}): {exc}")

    @Slot(int, str, "QVariant")
    def updateAction(self, index: int, key: str, value: Any) -> None:
        if self._controller is not None or self._preflight_running:
            self._set_status("Stop the controller before editing action bindings.")
            return
        buttons = self._profile.get("buttons", [])
        if not isinstance(buttons, list) or not (0 <= index < len(buttons)):
            return
        if key not in {"name", "source", "code", "position"}:
            return
        reserved_keys = {1, 29, 56, 59, 60, 61, 97, 100}
        movement = self._profile.get("movement", {}).get("keys", {})
        movement_codes = {
            int(movement.get(direction, -1))
            for direction in ("up", "left", "down", "right")
        }
        if key == "name":
            name = str(value).strip()
            if not name or len(name) > 40 or any(
                i != index and str(button.get("name", "")).strip() == name
                for i, button in enumerate(buttons)
            ):
                self._set_status("Action names must be non-empty, unique and at most 40 characters.")
                self.profileChanged.emit()
                return
            buttons[index]["name"] = name
        elif key == "code":
            try:
                code = int(value)
            except (TypeError, ValueError, OverflowError):
                self._set_status("Input code must be an integer.")
                return
            source = buttons[index].get("source")
            valid_range = 272 <= code <= 287 if source == "mouse" else 0 <= code <= 767
            duplicate = any(
                i != index and button.get("source") == source
                and int(button.get("code", -1)) == code
                for i, button in enumerate(buttons)
            )
            reserved = source == "key" and (code in reserved_keys or code in movement_codes)
            if not valid_range or duplicate or reserved:
                reason = (
                    "F1/F2/F3, Ctrl/Alt/Esc and movement keys are reserved."
                    if reserved else "This input code is invalid or already assigned."
                )
                self._set_status(reason)
                self.profileChanged.emit()
                return
            buttons[index]["code"] = code
        elif key == "source":
            if value not in {"key", "mouse"}:
                self._set_status("Input source must be key or mouse.")
                return
            used = {
                int(button.get("code", -1))
                for i, button in enumerate(buttons)
                if i != index and button.get("source") == value
            }
            movement = self._profile.get("movement", {}).get("keys", {})
            movement_codes = {
                int(movement.get(direction, -1))
                for direction in ("up", "left", "down", "right")
            }
            if value == "mouse":
                candidates = tuple(range(272, 288))
            else:
                preferred = (33, 18, 16, 19, 34, 57, 42, 15, 46, 47, 45, 44, 2, 3, 4, 5)
                candidates = preferred + tuple(
                    code for code in range(1, 768)
                    if code not in preferred and code not in range(272, 288)
                )
            current_code = int(buttons[index].get("code", -1))
            allowed = set(range(272, 288)) if value == "mouse" else (set(range(1, 768)) - set(range(272, 288)))
            if current_code not in allowed or current_code in used or (
                value == "key" and (current_code in movement_codes or current_code in reserved_keys)
            ):
                replacement = next(
                    (
                        code for code in candidates
                        if code not in used and (
                            value == "mouse" or (code not in movement_codes and code not in reserved_keys)
                        )
                    ),
                    None,
                )
                if replacement is None:
                    self._set_status(f"No unused {value} trigger is available.")
                    return
                buttons[index]["code"] = replacement
            buttons[index]["source"] = value
        elif key == "position":
            if (
                not isinstance(value, (list, tuple)) or len(value) != 2
                or any(not isinstance(part, (int, float)) or not math.isfinite(float(part))
                       or not 0.0 <= float(part) <= 1.0 for part in value)
            ):
                self._set_status("Action positions must be two finite normalized coordinates.")
                return
            buttons[index][key] = [float(value[0]), float(value[1])]
        if key in {"name", "source", "code", "position"}:
            self._layout_dirty = True
        self._set_dirty(True)
        self.profileChanged.emit()

    @Slot()
    def addAction(self) -> None:
        if self._controller is not None or self._preflight_running:
            self._set_status("Stop the controller before adding action bindings.")
            return
        buttons = self._profile.setdefault("buttons", [])
        mouse_codes = {
            int(button.get("code", -1))
            for button in buttons
            if button.get("source") == "mouse"
        }
        if (code := next((item for item in range(272, 288) if item not in mouse_codes), None)) is not None:
            source = "mouse"
        else:
            key_codes = {
                int(button.get("code", -1))
                for button in buttons
                if button.get("source") == "key"
            }
            movement = self._profile.get("movement", {}).get("keys", {})
            reserved = {int(movement.get(direction, -1)) for direction in ("up", "left", "down", "right")}
            reserved_keys = {1, 29, 56, 59, 60, 61, 97, 100}
            preferred = (33, 18, 16, 19, 34, 57, 42, 15, 46, 47, 45, 44, 2, 3, 4, 5)
            candidates = preferred + tuple(
                item for item in range(1, 768)
                if item not in preferred and item not in range(272, 288)
            )
            code = next((item for item in candidates if item not in key_codes and item not in reserved and item not in reserved_keys), None)
            source = "key"
        if code is None:
            self._set_status("No unused keyboard or mouse trigger is available.")
            return
        used_names = {str(button.get("name", "")) for button in buttons}
        suffix = 1
        while f"action_{suffix}" in used_names:
            suffix += 1
        buttons.append({
            "name": f"action_{suffix}",
            "source": source,
            "code": code,
            "position": [0.5, 0.5],
        })
        self._layout_dirty = True
        self._set_dirty(True)
        self.profileChanged.emit()

    @Slot(int, result=bool)
    def setMovementPreset(self, preset_index: int) -> bool:
        if self._controller is not None or self._preflight_running:
            self._set_status("Stop the controller before changing movement keys.")
            return False
        presets = (
            {"up": 17, "left": 30, "down": 31, "right": 32},
            {"up": 103, "left": 105, "down": 108, "right": 106},
            {"up": 72, "left": 75, "down": 80, "right": 77},
        )
        if preset_index not in range(len(presets)):
            return False
        selected = presets[preset_index]
        selected_codes = set(selected.values())
        action_codes = {
            int(item["code"])
            for item in self._profile.get("buttons", [])
            if item.get("source") == "key"
        }
        if selected_codes & action_codes:
            self._set_status("Movement preset conflicts with an action key. Rebind that action first.")
            return False
        self._profile.setdefault("movement", {})["keys"] = dict(selected)
        self._set_dirty(True)
        self._layout_dirty = True
        self.profileChanged.emit()
        self._set_status(("WASD", "Arrow keys", "Numpad")[preset_index] + " movement preset selected.")
        return True

    @Slot(str, result=bool)
    def setLookPreset(self, preset: str) -> bool:
        if self._controller is not None or self._preflight_running:
            self._set_status("Stop the controller before changing camera response presets.")
            return False
        presets = {
            "fps": {
                "sensitivity_x": 1.5, "sensitivity_y": 1.5,
                "smoothing": 0.5, "deadzone": 1.0, "rate_cap_hz": 240,
            },
            "rpg": {
                "sensitivity_x": 1.0, "sensitivity_y": 1.0,
                "smoothing": 0.70, "deadzone": 0.5, "rate_cap_hz": 120,
            },
            "moba": {
                "sensitivity_x": 1.2, "sensitivity_y": 1.2,
                "smoothing": 0.60, "deadzone": 0.5, "rate_cap_hz": 180,
            },
        }
        selected = presets.get(preset.lower())
        if selected is None:
            self._set_status("Unknown camera response preset.")
            return False
        self._profile.setdefault("mouse", {}).setdefault("look", {}).update(selected)
        self._set_dirty(True)
        self.profileChanged.emit()
        self._set_status(
            f"{preset.upper()} camera response selected "
            f"(rate cap {selected['rate_cap_hz']} Hz). Save the profile to keep it."
        )
        return True

    @Slot(str, result=bool)
    def setMousePreset(self, preset: str) -> bool:
        if self._controller is not None or self._preflight_running:
            self._set_status("Stop the controller before changing mouse presets.")
            return False
        mouse = self._profile.setdefault("mouse", {})
        look = mouse.setdefault("look", {})
        if preset == "mobile_shooter":
            mouse["enabled"] = True
            mouse.setdefault("cursor", {})["enabled"] = True
            mouse["cursor"]["hide_while_aiming"] = True
            look.update({
                "enabled": True,
                "mode": "touch_drag",
                "smoothing": 0.0,
                "deadzone": 0.0,
                "edge_recenter": True,
            })
            look.setdefault("acceleration", {})["enabled"] = False
            self._set_status("F1 switches mouse ownership between the host and Android; F2 toggles mouse-button mapping; F3 controls camera aim.")
        elif preset == "native_relative":
            mouse["enabled"] = True
            mouse.setdefault("cursor", {})["enabled"] = True
            mouse["cursor"]["hide_while_aiming"] = True
            look.update({
                "enabled": True,
                "mode": "relative_mouse",
            })
            self._set_status("Native relative preset selected. Use only with games that capture relative mouse input.")
        else:
            self._set_status("Unknown mouse preset.")
            return False
        self._set_dirty(True)
        self.profileChanged.emit()
        return True

    @Slot(int)
    def removeAction(self, index: int) -> None:
        if self._controller is not None or self._preflight_running:
            self._set_status("Stop the controller before removing action bindings.")
            return
        buttons = self._profile.get("buttons", [])
        if isinstance(buttons, list) and 0 <= index < len(buttons):
            buttons.pop(index)
            self._layout_dirty = True
            self._set_dirty(True)
            self.profileChanged.emit()

    @Slot(str, str)
    def setDeviceList(self, role: str, text: str) -> None:
        if self._controller is not None or self._preflight_running:
            self._set_status("Stop the controller before changing captured devices.")
            return
        if role not in {"keyboard", "mouse", "suppress"}:
            return
        paths = [part.strip() for part in text.replace(";", "\n").splitlines() if part.strip()]
        self._profile.setdefault("devices", {})[role] = paths
        self._set_dirty(True)
        self.profileChanged.emit()

    def _validate_profile_data(self, data: dict[str, Any]) -> tuple[bool, str, Path | None]:
        if not HOST.is_file():
            return False, "Rust host binary is missing. Run Build / verify first.", None
        temp_path: Path | None = None
        try:
            PROFILE_DIR.mkdir(parents=True, exist_ok=True)
            with tempfile.NamedTemporaryFile(
                mode="w",
                encoding="utf-8",
                dir=PROFILE_DIR,
                prefix=".waytouch-check-",
                suffix=".json",
                delete=False,
            ) as handle:
                temp_path = Path(handle.name)
                json.dump(data, handle, ensure_ascii=False, indent=2, allow_nan=False)
                handle.write("\n")
                handle.flush()
                os.fsync(handle.fileno())
            result = _run_capture([str(HOST), "profile-check", str(temp_path)], timeout=5.0)
            return result.returncode == 0, result.stdout.strip(), temp_path
        except (OSError, TypeError, ValueError, subprocess.TimeoutExpired) as exc:
            if temp_path is not None:
                temp_path.unlink(missing_ok=True)
            return False, str(exc), None

    def _read_android_layout_envelope(self) -> dict[str, Any] | None:
        try:
            with socket.create_connection(("127.0.0.1", 27184), timeout=0.5) as connection:
                connection.settimeout(0.5)
                _authenticate_socket(connection)
                connection.sendall(b"GET\n")
                with connection.makefile("r", encoding="utf-8") as reader:
                    line = reader.readline()
            if not line or line.strip() in {"NONE"} or line.startswith("ERR"):
                return None
            envelope = json.loads(line)
            revision = envelope.get("revision")
            layout = envelope.get("layout")
            if not isinstance(revision, int) or revision < 0 or not isinstance(layout, dict):
                return None
            if not isinstance(layout.get("movement"), dict) or not isinstance(layout.get("buttons"), list):
                return None
            return envelope
        except (OSError, ValueError, TypeError, json.JSONDecodeError):
            return None

    def _android_layout_payload(self) -> dict[str, Any]:
        movement = self._profile["movement"]
        return {
            "movement": {
                "center": list(movement["center"]),
                "radius": movement["radius"],
                "keys": {
                    "up": int(movement["keys"]["up"]),
                    "left": int(movement["keys"]["left"]),
                    "down": int(movement["keys"]["down"]),
                    "right": int(movement["keys"]["right"]),
                },
            },
            "buttons": [
                {
                    "name": button["name"],
                    "position": list(button["position"]),
                    "source": button["source"],
                    "code": int(button["code"]),
                }
                for button in self._profile.get("buttons", [])
            ],
        }

    def _publish_android_layout(self, expected_revision: int, revision: int) -> str:
        payload = json.dumps(
            self._android_layout_payload(),
            ensure_ascii=False,
            separators=(",", ":"),
            allow_nan=False,
        )
        command = f"SYNC {expected_revision} {revision} {payload}\n"
        try:
            with socket.create_connection(("127.0.0.1", 27184), timeout=0.7) as connection:
                connection.settimeout(0.7)
                _authenticate_socket(connection)
                connection.sendall(command.encode("utf-8"))
                with connection.makefile("r", encoding="utf-8") as reader:
                    return reader.readline().strip()
        except OSError as exc:
            return f"ERR {exc}"

    def _write_profile_snapshot(self, target: Path) -> None:
        temporary: Path | None = None
        try:
            old_mode = stat.S_IMODE(target.stat().st_mode) if target.exists() else 0o600
            with tempfile.NamedTemporaryFile(
                mode="w", encoding="utf-8", dir=target.parent,
                prefix=f".{target.stem}.sync-", suffix=".tmp", delete=False,
            ) as handle:
                temporary = Path(handle.name)
                json.dump(self._profile, handle, ensure_ascii=False, indent=2, allow_nan=False)
                handle.write("\n")
                handle.flush()
                os.fsync(handle.fileno())
            temporary.chmod(old_mode)
            os.replace(temporary, target)
            temporary = None
        finally:
            if temporary is not None:
                temporary.unlink(missing_ok=True)

    def _merge_android_layout(self, envelope: dict[str, Any], target: Path) -> bool:
        try:
            layout = envelope["layout"]
            movement = layout["movement"]
            buttons = layout["buttons"]
            local_buttons = self._profile.get("buttons", [])
            local_by_name = {item["name"]: item for item in local_buttons}
            next_buttons = []
            for item in buttons:
                previous = local_by_name.get(item["name"])
                source = item.get("source", previous.get("source") if previous else None)
                code = item.get("code", previous.get("code") if previous else None)
                if source not in {"key", "mouse"} or not isinstance(code, int):
                    return False
                next_buttons.append({
                    "name": item["name"],
                    "source": source,
                    "code": code,
                    "position": list(item["position"]),
                })
            self._profile["movement"]["center"] = list(movement["center"])
            self._profile["movement"]["radius"] = float(movement["radius"])
            if isinstance(movement.get("keys"), dict):
                for direction in ("up", "left", "down", "right"):
                    self._profile["movement"]["keys"][direction] = int(movement["keys"][direction])
            self._profile["buttons"] = next_buttons
            self._profile["layout_revision"] = int(envelope["revision"])
            self._write_profile_snapshot(target)
            self._layout_dirty = False
            self._set_dirty(False)
            self.profileChanged.emit()
            return True
        except (KeyError, TypeError, ValueError, OSError):
            return False

    def _save_to(self, target: Path) -> bool:
        if self._controller is not None or self._preflight_running:
            self._set_status("Stop controller before editing or saving a profile.")
            return False
        target = target.expanduser().resolve()
        if not target.parent.is_dir():
            self._set_status(f"Profile directory does not exist: {target.parent}")
            return False

        layout_was_dirty = self._layout_dirty
        android_envelope = self._read_android_layout_envelope() if layout_was_dirty else None
        android_revision = int(android_envelope["revision"]) if android_envelope is not None else None
        layout_revision = int(self._profile.get("layout_revision", 0))
        if layout_was_dirty:
            self._profile["layout_revision"] = max(
                layout_revision,
                android_revision or 0,
                time.time_ns() // 1_000_000,
            ) + 1

        temp_path: Path | None = None
        try:
            with tempfile.NamedTemporaryFile(
                mode="w",
                encoding="utf-8",
                dir=target.parent,
                prefix=f".{target.stem}.",
                suffix=".json.tmp",
                delete=False,
            ) as handle:
                temp_path = Path(handle.name)
                json.dump(self._profile, handle, ensure_ascii=False, indent=2, allow_nan=False)
                handle.write("\n")
                handle.flush()
                os.fsync(handle.fileno())

            check = _run_capture([str(HOST), "profile-check", str(temp_path)], timeout=5.0)
            if check.returncode != 0:
                self._append_log("PROFILE SAVE REJECTED:\n" + check.stdout)
                self._set_status("Save blocked: Rust validation rejected this profile.")
                return False

            old_mode = stat.S_IMODE(target.stat().st_mode) if target.exists() else 0o600
            temp_path.chmod(old_mode)
            backup = target.with_suffix(target.suffix + ".bak")
            if target.exists() and not backup.exists():
                shutil.copy2(target, backup)
            os.replace(temp_path, target)
            temp_path = None
            self._profile_path = target
            self._set_dirty(False)
            self.profileChanged.emit()
            self._refresh_profile_files()
            layout_conflict_path: Path | None = None
            if layout_was_dirty:
                revision = int(self._profile["layout_revision"])
                if android_revision is None:
                    self._append_log(
                        f"LAYOUT_SYNC_PENDING revision={revision}: Android layout channel is offline; "
                        "Rust will publish this layout when the controller reconnects."
                    )
                else:
                    reply = self._publish_android_layout(android_revision, revision)
                    if reply.startswith("OK "):
                        self._append_log(f"LAYOUT_SYNC_OK source=linux revision={revision}")
                    elif reply.startswith("CONFLICT"):
                        latest = self._read_android_layout_envelope()
                        if latest is not None:
                            conflict_dir = target.parent / "conflicts"
                            conflict_dir.mkdir(parents=True, exist_ok=True)
                            layout_conflict_path = conflict_dir / (
                                f"{target.stem}.android-layout-r{latest['revision']}-"
                                f"{time.time_ns()}.json"
                            )
                            with layout_conflict_path.open("x", encoding="utf-8") as handle:
                                json.dump(latest, handle, ensure_ascii=False, indent=2, allow_nan=False)
                                handle.write("\n")
                                handle.flush()
                                os.fsync(handle.fileno())
                            layout_conflict_path.chmod(0o600)
                            self._set_dirty(True)
                            self._append_log(
                                "LAYOUT_SYNC_CONFLICT: local layout was preserved; the newer Android "
                                f"snapshot was saved to {layout_conflict_path}. Press Save again to retry "
                                "against the latest Android revision."
                            )
                        else:
                            self._set_dirty(True)
                            self._append_log(
                                f"LAYOUT_SYNC_CONFLICT: {reply}; local layout was preserved and sync is pending."
                            )
                    else:
                        self._append_log(
                            f"LAYOUT_SYNC_PENDING revision={revision}: {reply}; "
                            "Rust will retry synchronization when the controller starts."
                        )
                self._layout_dirty = layout_was_dirty and (
                    layout_conflict_path is not None or self._dirty
                )
            if layout_conflict_path is not None:
                self._set_status(
                    f"Saved {target.name}; Android conflict backed up. Local layout kept. "
                    "Press Save again to retry synchronization."
                )
            else:
                self._set_status(f"Saved {target.name}.")
            return True
        except (OSError, TypeError, ValueError, subprocess.TimeoutExpired) as exc:
            self._append_log(f"PROFILE SAVE ERROR: {exc}")
            self._set_status(f"Save failed: {exc}")
            return False
        finally:
            if temp_path is not None:
                temp_path.unlink(missing_ok=True)

    @Slot(result=bool)
    def saveProfile(self) -> bool:
        return self._save_to(self._profile_path)

    @Slot(str, result=bool)
    def saveProfileAs(self, value: str) -> bool:
        path = self._resolve_profile_path(value)
        if path.suffix.lower() != ".json":
            path = path.with_suffix(".json")
        return self._save_to(path)

    @Slot(result=bool)
    def validateProfile(self) -> bool:
        okay, details, temp_path = self._validate_profile_data(self._profile)
        if temp_path is not None:
            temp_path.unlink(missing_ok=True)
        self._append_log(details if details else "No validator output.")
        self._set_status("Profile validation passed." if okay else f"Profile invalid: {details}")
        return okay

    def _preflight(self, profile: dict[str, Any] | None = None) -> tuple[bool, str]:
        candidate = profile if profile is not None else self._profile
        if not HOST.is_file():
            return False, "Host binary is missing. Run Build / verify."
        okay, details, temp_path = self._validate_profile_data(candidate)
        if temp_path is not None:
            temp_path.unlink(missing_ok=True)
        if not okay:
            return False, f"Profile validation failed:\n{details}"

        devices = candidate.get("devices", {})
        if not devices.get("keyboard") or not devices.get("mouse"):
            return False, "No stable keyboard/mouse assignment. Open Input devices and run Discover & configure."
        paths = (
            list(devices.get("keyboard", []))
            + list(devices.get("mouse", []))
            + list(devices.get("suppress", []))
        )
        missing = [path for path in paths if not Path(path).exists()]
        if missing:
            return False, "Configured input devices are missing:\n" + "\n".join(missing)
        unreadable = [path for path in paths if not os.access(path, os.R_OK)]
        if unreadable:
            return False, "No read permission for input devices:\n" + "\n".join(unreadable)

        mouse_config = candidate.get("mouse", {})
        cursor_config = mouse_config.get("cursor", {})
        look_config = mouse_config.get("look", {})
        try:
            state = _run_capture(["adb", "get-state"], timeout=1.5)
            if state.returncode != 0 or state.stdout.strip() != "device":
                return False, "Waydroid ADB is offline. Connect Android first."
            log = _run_capture(["adb", "shell", "cat", REMOTE_LOG], timeout=1.5)
            if log.returncode != 0 or "WTCH_READY version=2" not in log.stdout:
                return False, "Android WTCH v2 is not ready. Start/restart the Android server."
            if mouse_config.get("enabled", True) and "WTCH_MOUSE_READY port=27186" not in log.stdout:
                return False, "Android SOURCE_MOUSE channel 27186 is not ready. Run scripts/start-server.sh again."
            with socket.create_connection(("127.0.0.1", 27183), timeout=0.5) as connection:
                _authenticate_socket(connection)
            if mouse_config.get("enabled", True):
                with socket.create_connection(("127.0.0.1", 27186), timeout=0.75) as connection:
                    connection.settimeout(0.75)
                    _authenticate_socket(connection)
                    connection.sendall(b"PING\n")
                    with connection.makefile("r", encoding="ascii") as reader:
                        if reader.readline().strip() != "PONG":
                            raise ConnectionError("Android SOURCE_MOUSE did not answer PING")
            if cursor_config.get("enabled", True):
                with socket.create_connection(("127.0.0.1", 27185), timeout=0.75) as connection:
                    connection.settimeout(0.75)
                    _authenticate_socket(connection)
                    connection.sendall(b"PING\n")
                    with connection.makefile("r", encoding="ascii") as reader:
                        if reader.readline().strip() != "PONG":
                            raise ConnectionError("Android cursor control channel did not answer PING")
            if self._read_android_layout_envelope() is None:
                return False, "Android layout sync channel 27184 is not ready or returned an invalid revisioned profile."
        except (OSError, subprocess.TimeoutExpired) as exc:
            return False, f"Android connection preflight failed: {exc}"
        return True, "Preflight checks passed."

    def _configure_process(self, process: QProcess, label: str, stream_output: bool = True) -> None:
        process.setWorkingDirectory(str(ROOT))
        process.setProcessChannelMode(QProcess.MergedChannels)
        if stream_output:
            process.readyReadStandardOutput.connect(
                lambda p=process, name=label: self._read_process_output(p, name)
            )
        process.errorOccurred.connect(
            lambda error, p=process, name=label: self._process_error(p, name, error)
        )

    def _read_process_output(self, process: QProcess, label: str) -> None:
        data = bytes(process.readAllStandardOutput()).decode("utf-8", errors="replace")
        if data:
            self._append_log(data)
            if label == "controller":
                if "CONTROL_SESSION_INTERRUPTED" in data:
                    self._set_controller_state("Reconnecting")
                    self._set_status(
                        "Android/input link interrupted; WayTouch is reconnecting automatically…"
                    )
                if "Connected to Android WTCH v2" in data and self._controller_state == "Reconnecting":
                    self._set_controller_state("Connected")
                    self._set_status("Android connection restored; press F1 to capture the mouse.")
                if "MOUSE_CAPTURE state=active" in data or "exclusive input capture active" in data.lower():
                    self._set_controller_state("Capturing")
                    self._set_status("Mouse captured by Android.")
                elif "MOUSE_CAPTURE state=inactive" in data:
                    self._set_controller_state("Connected")
                    self._set_status("Mouse released to the host; Android link remains connected.")

    def _process_error(self, process: QProcess, label: str, error: QProcess.ProcessError) -> None:
        self._append_log(f"{label.upper()} PROCESS ERROR ({error.name}): {process.errorString()}")

    def _set_controller_state(self, value: str) -> None:
        if self._controller_state != value:
            self._controller_state = value
            self.controllerStateChanged.emit()

    @Slot()
    def startControl(self) -> None:
        if self._controller is not None or self._preflight_running:
            return
        if self._dirty and not self.saveProfile():
            return

        # Run profile validation, ADB probes and all four IPC checks off
        # the GUI event loop. The immutable snapshot prevents concurrent edits.
        self._preflight_running = True
        self._preflight_cancelled = False
        self._set_controller_state("Checking")
        self._set_status("Checking profile, input devices and Android channels…")
        self._append_log("Preflight started in a background worker.")
        worker = _PreflightWorker(self, copy.deepcopy(self._profile))
        self._preflight_thread = worker
        worker.resultReady.connect(self._preflight_finished)
        worker.finished.connect(self._preflight_worker_finished)
        worker.start()

    @Slot(bool, str)
    def _preflight_finished(self, okay: bool, details: str) -> None:
        if not self._preflight_running:
            return
        self._preflight_running = False
        if self._preflight_cancelled:
            self._set_controller_state("Stopped")
            self._set_status("Controller start cancelled.")
            self._append_log("Preflight completed after cancellation; controller was not started.")
            if self._quit_after_stop:
                self._quit_after_stop = False
                self.quitRequested.emit()
            return
        if not okay:
            self._set_controller_state("Stopped")
            self._set_status(details)
            self._append_log("START BLOCKED: " + details)
            if self._quit_after_stop:
                self._quit_after_stop = False
                self.quitRequested.emit()
            return
        self._launch_control_process(details)

    @Slot()
    def _preflight_worker_finished(self) -> None:
        worker = self.sender()
        if self._preflight_thread is worker:
            self._preflight_thread = None
        if worker is not None:
            worker.deleteLater()

    def _launch_control_process(self, details: str) -> None:
        process = QProcess(self)
        self._configure_process(process, "controller")
        process.setProgram(str(HOST))
        process.setArguments(["control", str(self._profile_path)])
        self._append_log("PREFLIGHT: " + details)
        process.started.connect(lambda: self._set_controller_state("Starting"))
        process.finished.connect(self._controller_finished)
        self._controller = process
        self._set_controller_state("Starting")
        self._append_log(f"$ {HOST} control {self._profile_path}")
        process.start()
        if not process.waitForStarted(1500):
            self._append_log("Controller failed to start: " + process.errorString())
            self._controller = None
            self._set_controller_state("Stopped")
            self._set_status("Controller could not start.")
            process.deleteLater()

    def _controller_finished(self, code: int, status: QProcess.ExitStatus) -> None:
        process = self._controller
        if process is not None:
            self._read_process_output(process, "controller")
            self._append_log(f"Controller exited: code={code}, status={status.name}. Input handles released.")
            process.deleteLater()
        self._controller = None
        if not self._dirty:
            try:
                latest = json.loads(self._profile_path.read_text(encoding="utf-8"))
                if isinstance(latest, dict) and latest != self._profile:
                    latest.setdefault("layout_revision", 0)
                    self._profile = latest
                    self._layout_dirty = False
                    self.profileChanged.emit()
                    self._append_log(
                        f"Reloaded synchronized layout revision {latest.get('layout_revision', 0)} from disk."
                    )
            except (OSError, json.JSONDecodeError, ValueError) as exc:
                self._append_log(f"PROFILE RELOAD WARNING: {exc}")
        self._set_controller_state("Stopped")
        self._set_status("Controller stopped; physical input devices released.")
        if self._quit_after_stop:
            self._quit_after_stop = False
            self.quitRequested.emit()

    @Slot(bool)
    def stopControl(self, quit_after: bool = False) -> None:
        self._quit_after_stop = self._quit_after_stop or quit_after
        if self._preflight_running:
            self._preflight_cancelled = True
            self._set_controller_state("Stopping")
            self._set_status("Cancelling controller start after current checks finish.")
            return
        process = self._controller
        if process is None or process.state() == QProcess.NotRunning:
            if self._quit_after_stop:
                self._quit_after_stop = False
                self.quitRequested.emit()
            return
        if process.state() != QProcess.NotRunning:
            self._set_controller_state("Stopping")
            self._append_log("Stop requested; sending SIGTERM to controller.")
            process.terminate()
            QTimer.singleShot(2500, lambda p=process: self._kill_controller_if_needed(p))

    def _kill_controller_if_needed(self, process: QProcess) -> None:
        # _controller_finished schedules deleteLater(); only touch the wrapper
        # while it is still the currently-owned controller process.
        if self._controller is process and process.state() != QProcess.NotRunning:
            process.kill()

    def _set_server_state(self, value: str, ready: bool) -> None:
        changed = self._server_state != value or self._server_ready != ready
        self._server_state = value
        self._server_ready = ready
        if changed:
            self.serverStateChanged.emit()

    @Slot()
    def refreshServerStatus(self) -> None:
        if self._probe_process is not None and self._probe_process.state() != QProcess.NotRunning:
            return
        if self._status_probe_thread is not None and self._status_probe_thread.isRunning():
            return
        process = QProcess(self)
        self._configure_process(process, "adb probe", stream_output=False)
        process.setProgram("adb")
        process.setArguments(["get-state"])
        process.finished.connect(lambda code, _status, p=process: self._probe_adb_finished(p, code))
        self._probe_process = process
        process.start()

    def _probe_adb_finished(self, process: QProcess, code: int) -> None:
        output = bytes(process.readAllStandardOutput()).decode("utf-8", errors="replace").strip()
        process.deleteLater()
        if self._probe_process is process:
            self._probe_process = None
        if code != 0 or output != "device":
            self._set_server_state("ADB OFFLINE", False)
            self._set_status("Waydroid ADB is offline; attempting automatic reconnection…")
            self._schedule_adb_recovery()
            return

        log_proc = QProcess(self)
        self._configure_process(log_proc, "android status", stream_output=False)
        log_proc.setProgram("adb")
        log_proc.setArguments(["shell", "cat", REMOTE_LOG])
        log_proc.finished.connect(
            lambda result, _status, p=log_proc: self._probe_log_finished(p, result)
        )
        self._probe_process = log_proc
        log_proc.start()

    def _probe_log_finished(self, process: QProcess, code: int) -> None:
        output = bytes(process.readAllStandardOutput()).decode("utf-8", errors="replace")
        process.deleteLater()
        if self._probe_process is process:
            self._probe_process = None
        if code != 0 or "WTCH_READY version=2" not in output:
            self._set_server_state("ANDROID SERVER NOT READY", False)
            self._set_status("ADB is connected, but WTCH v2 has not reported READY; restarting Android IPC.")
            self._schedule_server_recovery()
            return

        mouse_config = self._profile.get("mouse", {})
        cursor_config = mouse_config.get("cursor", {})
        mouse_enabled = bool(mouse_config.get("enabled", True))
        if mouse_enabled and "WTCH_MOUSE_READY port=27186" not in output:
            self._set_server_state("MOUSE CHANNEL OFFLINE", False)
            self._set_status("Android SOURCE_MOUSE has not reported READY; restarting Android IPC.")
            self._schedule_server_recovery()
            return

        worker = _StatusProbeWorker(
            self,
            mouse_enabled,
            bool(cursor_config.get("enabled", True)),
        )
        self._status_probe_thread = worker
        worker.resultReady.connect(self._status_probe_finished)
        worker.finished.connect(self._status_probe_worker_finished)
        worker.start()

    @Slot(str, bool, str)
    def _status_probe_finished(self, state: str, ready: bool, message: str) -> None:
        self._set_server_state(state, ready)
        self._set_status(message)
        if not ready:
            self._schedule_server_recovery()

    @Slot()
    def _status_probe_worker_finished(self) -> None:
        worker = self.sender()
        if self._status_probe_thread is worker:
            self._status_probe_thread = None
        if worker is not None:
            worker.deleteLater()

    def _schedule_server_recovery(self) -> None:
        # ADB may be online while one TCP forward or Android server process has
        # disappeared. Re-run the authoritative bootstrap to rebuild all channels.
        if self._server_process is not None and self._server_process.state() != QProcess.NotRunning:
            return
        now = time.monotonic()
        if now < self._next_server_recovery_at:
            return
        self._next_server_recovery_at = now + 15.0
        self._append_log("ANDROID_IPC_RECOVERY: channel probe failed; restarting scripts/start-server.sh")
        self._launch_server_script()

    def _schedule_adb_recovery(self) -> None:
        # The status timer fires every five seconds. Keep recovery single-flight
        # and rate-limited so a stopped container cannot spawn an adb process storm.
        if self._server_process is not None and self._server_process.state() != QProcess.NotRunning:
            return
        now = time.monotonic()
        if now < self._next_adb_recovery_at:
            return
        self._next_adb_recovery_at = now + 15.0

        process = QProcess(self)
        self._configure_process(process, "waydroid adb auto-reconnect")
        process.setProgram("waydroid")
        process.setArguments(["adb", "connect"])
        process.finished.connect(self._adb_connect_finished)
        self._server_process = process
        self._server_stage = "auto-connect"
        self._append_log("$ waydroid adb connect (automatic recovery)")
        process.start()

    def _launch_server_script(self) -> None:
        process = QProcess(self)
        self._configure_process(process, "android server")
        process.setProgram("/usr/bin/bash")
        process.setArguments([str(SERVER_SCRIPT)])
        process.finished.connect(self._server_script_finished)
        self._server_process = process
        self._server_stage = "server"
        self._append_log("$ scripts/start-server.sh")
        process.start()

    @Slot()
    def startAndroidServer(self) -> None:
        if self._controller is not None or self._preflight_running:
            self._set_status("Stop input capture before restarting Android server.")
            return
        if self._server_process is not None and self._server_process.state() != QProcess.NotRunning:
            return

        try:
            result = _run_capture(["adb", "get-state"], timeout=1.0)
            connected = result.returncode == 0 and result.stdout.strip() == "device"
        except (OSError, subprocess.TimeoutExpired):
            connected = False

        if connected:
            self._launch_server_script()
            return

        process = QProcess(self)
        self._configure_process(process, "waydroid adb connect")
        process.setProgram("waydroid")
        process.setArguments(["adb", "connect"])
        process.finished.connect(self._adb_connect_finished)
        self._server_process = process
        self._server_stage = "connect"
        self._append_log("$ waydroid adb connect")
        process.start()

    def _adb_connect_finished(self, code: int, _status: QProcess.ExitStatus) -> None:
        process = self._server_process
        if process is not None:
            self._read_process_output(process, "waydroid adb connect")
            process.deleteLater()
        self._server_process = None
        if code != 0:
            self._set_server_state("ADB CONNECT FAILED", False)
            self._set_status("waydroid adb connect failed; automatic retry is rate-limited.")
            return
        self._launch_server_script()

    def _server_script_finished(self, code: int, _status: QProcess.ExitStatus) -> None:
        process = self._server_process
        if process is not None:
            self._read_process_output(process, "android server")
            process.deleteLater()
        self._server_process = None
        if code != 0:
            self._set_server_state("SERVER START FAILED", False)
            self._set_status("Android server launch failed; inspect the activity log.")
        else:
            self._set_status("Android server launch script completed; checking readiness…")
            QTimer.singleShot(350, self.refreshServerStatus)

    @Slot()
    def buildProject(self) -> None:
        if self._build_process is not None and self._build_process.state() != QProcess.NotRunning:
            self._set_status("Build is already running.")
            return
        if self._controller is not None or self._preflight_running:
            self._set_status("Stop input capture before rebuilding.")
            return
        process = QProcess(self)
        self._configure_process(process, "build")
        process.setProgram("/usr/bin/bash")
        process.setArguments([str(BUILD_SCRIPT)])
        process.finished.connect(self._build_finished)
        self._build_process = process
        self._append_log("$ scripts/build.sh")
        process.start()

    def _build_finished(self, code: int, _status: QProcess.ExitStatus) -> None:
        process = self._build_process
        if process is not None:
            self._read_process_output(process, "build")
            process.deleteLater()
        self._build_process = None
        if code == 0:
            self._set_status("Build and all configured checks passed.")
        else:
            self._set_status(f"Build failed with exit code {code}; inspect the log.")

    @Slot()
    def scanInputDevices(self) -> None:
        if not HOST.is_file():
            self._set_status("Build the Rust host before scanning input devices.")
            return
        if self._scan_process is not None and self._scan_process.state() != QProcess.NotRunning:
            return
        process = QProcess(self)
        self._configure_process(process, "device scan")
        process.setProgram(str(HOST))
        process.setArguments(["devices"])
        process.finished.connect(self._scan_finished)
        self._scan_process = process
        process.start()

    def _scan_finished(self, code: int, _status: QProcess.ExitStatus) -> None:
        process = self._scan_process
        output = ""
        if process is not None:
            output = bytes(process.readAllStandardOutput()).decode("utf-8", errors="replace")
            process.deleteLater()
        self._scan_process = None
        self._device_summary = output.strip() or "No input-device output."
        self.deviceSummaryChanged.emit()
        self._set_status("Input device scan completed." if code == 0 else "Device scan failed.")

    @Slot()
    def discoverAndApplyInputDevices(self) -> None:
        if self._controller is not None or self._preflight_running:
            self._set_status("Stop the controller before discovering or changing input devices.")
            return
        if self._dirty:
            self._set_status("Save or discard profile edits before applying device discovery.")
            return
        if not HOST.is_file():
            self._set_status("Build the Rust host before discovering input devices.")
            return
        if self._discovery_process is not None and self._discovery_process.state() != QProcess.NotRunning:
            return

        process = QProcess(self)
        self._configure_process(process, "device discovery", stream_output=False)
        process.setProgram(str(HOST))
        process.setArguments(["discover", "--apply", str(self._profile_path)])
        process.finished.connect(self._discovery_finished)
        self._discovery_process = process
        self._set_status("Discovering stable keyboard, mouse and touchpad links…")
        self._append_log(f"$ waytouch-host discover --apply {self._profile_path}")
        process.start()

    def _discovery_finished(self, code: int, _status: QProcess.ExitStatus) -> None:
        process = self._discovery_process
        output = ""
        if process is not None:
            output = bytes(process.readAllStandardOutput()).decode("utf-8", errors="replace")
            process.deleteLater()
        self._discovery_process = None
        self._device_summary = output.strip() or "Device discovery returned no output."
        self.deviceSummaryChanged.emit()
        if output.strip():
            self._append_log(output)

        if code != 0:
            self._set_status("Device discovery failed. Connect a keyboard and relative mouse with stable udev links; inspect the log.")
            return
        try:
            data = json.loads(self._profile_path.read_text(encoding="utf-8"))
            devices = data["devices"]
            keyboard = devices.get("keyboard", [])
            mouse = devices.get("mouse", [])
            if not keyboard or not mouse:
                raise ValueError("discovery did not configure both keyboard and mouse roles")
            self._profile["devices"] = devices
            self.profileChanged.emit()
            self._set_dirty(False)
            self._set_status(
                f"Discovery applied: {len(keyboard)} keyboard, {len(mouse)} mouse and "
                f"{len(devices.get('suppress', []))} suppress-only link(s). Validate the profile before starting."
            )
        except (OSError, json.JSONDecodeError, KeyError, TypeError, ValueError) as exc:
            self._set_status(f"Discovery completed, but profile reload failed: {exc}")
            self._append_log(f"DEVICE DISCOVERY RELOAD ERROR: {exc}")

    @Slot()
    def testVirtualMouse(self) -> None:
        if self._controller is not None or self._preflight_running:
            self._set_status("Stop controller before the standalone virtual mouse test.")
            return
        if self._test_process is not None and self._test_process.state() != QProcess.NotRunning:
            return
        if not HOST.is_file():
            self._set_status("Build the Rust host first.")
            return
        process = QProcess(self)
        self._configure_process(process, "uinput test")
        process.setProgram(str(HOST))
        process.setArguments(["uinput-test"])
        process.finished.connect(self._test_finished)
        self._test_process = process
        self._append_log("$ waytouch-host uinput-test")
        process.start()

    def _test_finished(self, code: int, _status: QProcess.ExitStatus) -> None:
        process = self._test_process
        if process is not None:
            self._read_process_output(process, "uinput test")
            process.deleteLater()
        self._test_process = None
        self._set_status("Virtual mouse test completed." if code == 0 else "Virtual mouse test failed; inspect log.")

    @Slot()
    def clearLog(self) -> None:
        self._log_text = ""
        self.logChanged.emit()

    @Slot()
    def openProjectFolder(self) -> None:
        QProcess.startDetached("xdg-open", [str(ROOT)])

    @Slot()
    def stopForQuit(self) -> None:
        if self._controller is not None or self._preflight_running:
            self.stopControl(True)
        else:
            self.quitRequested.emit()


def main() -> int:
    from PySide6.QtGui import QGuiApplication
    from PySide6.QtQml import QQmlApplicationEngine
    from PySide6.QtQuickControls2 import QQuickStyle

    QQuickStyle.setStyle("Material")
    app = QGuiApplication([])
    app.setApplicationName("WayTouch")
    app.setOrganizationName("WayTouch")
    app.setOrganizationDomain("waytouch.local")
    backend = WayTouchBackend()
    engine = QQmlApplicationEngine()
    engine.rootContext().setContextProperty("wayTouchApi", backend)
    engine.load(str(ROOT / "gui" / "main.qml"))
    if not engine.rootObjects():
        return 2
    backend.quitRequested.connect(app.quit)
    return app.exec()


if __name__ == "__main__":
    raise SystemExit(main())
