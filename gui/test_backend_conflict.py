#!/usr/bin/env python3
"""Regression test: profile save must preserve local edits on Android layout conflict."""

from __future__ import annotations

import copy
import json
import sys
import tempfile
from pathlib import Path

from PySide6.QtCore import QCoreApplication

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "gui"))
from waytouch import WayTouchBackend  # noqa: E402


app = QCoreApplication([])
backend = WayTouchBackend(auto_probe=False)

with tempfile.TemporaryDirectory(prefix="waytouch-conflict-test-") as temp:
    target = Path(temp) / "profile.json"
    backend._profile = copy.deepcopy(backend._profile)
    backend._profile_path = target
    local_button = backend._profile["buttons"][0]
    local_button["position"] = [0.23, 0.67]
    backend._layout_dirty = True
    backend._set_dirty(True)

    remote = {
        "revision": 999,
        "layout": {
            "movement": {
                "center": [0.4, 0.55],
                "radius": 0.22,
                "keys": dict(backend._profile["movement"]["keys"]),
            },
            "buttons": [
                {
                    "name": local_button["name"],
                    "position": [0.81, 0.19],
                    "source": local_button["source"],
                    "code": local_button["code"],
                }
            ],
        },
    }
    publish_calls: list[tuple[int, int]] = []
    backend._read_android_layout_envelope = lambda: copy.deepcopy(remote)

    def publish(expected_revision: int, new_revision: int) -> str:
        publish_calls.append((expected_revision, new_revision))
        return "CONFLICT revision changed" if len(publish_calls) == 1 else "OK revision accepted"

    backend._publish_android_layout = publish

    assert backend._save_to(target), backend.statusMessage
    saved = json.loads(target.read_text(encoding="utf-8"))
    assert saved["buttons"][0]["position"] == [0.23, 0.67], "local button position was overwritten"
    assert backend._profile["buttons"][0]["position"] == [0.23, 0.67]
    assert backend.dirty is True, "conflict should keep an explicit save/retry action available"
    assert backend._layout_dirty is True, "conflicted layout must remain pending"
    conflict_files = list((Path(temp) / "conflicts").glob("*.json"))
    assert len(conflict_files) == 1, "Android conflict snapshot was not backed up"
    assert json.loads(conflict_files[0].read_text(encoding="utf-8")) == remote

    # Explicit second Save retries using the newest Android revision. The local
    # layout remains the source of truth and is not silently replaced.
    assert backend._save_to(target), backend.statusMessage
    assert publish_calls[0][0] == 999
    assert publish_calls[1][0] == 999
    assert publish_calls[1][1] > 999
    saved_again = json.loads(target.read_text(encoding="utf-8"))
    assert saved_again["buttons"][0]["position"] == [0.23, 0.67]
    assert backend.dirty is False
    assert backend._layout_dirty is False

    original_look = dict(backend._profile["mouse"]["look"])
    assert backend.setLookPreset("fps")
    assert backend._profile["mouse"]["look"]["rate_cap_hz"] == 240
    assert backend._profile["mouse"]["look"]["smoothing"] == 0.5
    assert backend._profile["mouse"]["look"]["deadzone"] == 1.0
    assert backend.setLookPreset("rpg")
    assert backend._profile["mouse"]["look"]["rate_cap_hz"] == 120
    assert backend._profile["mouse"]["look"]["smoothing"] == 0.70
    assert backend.setLookPreset("moba")
    assert backend._profile["mouse"]["look"]["rate_cap_hz"] == 180
    assert backend._profile["mouse"]["look"]["smoothing"] == 0.60
    backend._profile["mouse"]["look"] = original_look

    backend._append_log(
        "TOUCH_STATS count=1200 input_coalesced=10 input_dropped=2 "
        "tcp_coalesced=34 tcp_dropped=1 "
        "evdev_ack_p50_ms=1.20 evdev_ack_p95_ms=3.45 evdev_ack_p99_ms=6.78"
    )
    assert "p95 3.45 ms" in backend.latencySummary
    assert "input drop 2" in backend.latencySummary
    assert "TCP drop 1" in backend.latencySummary
    assert "coalesced input/TCP 10/34" in backend.latencySummary

    invalid_name = backend._profile["buttons"][0]["name"]
    signal_count = {"profile": 0}
    backend.profileChanged.connect(lambda: signal_count.__setitem__("profile", signal_count["profile"] + 1))
    backend.updateAction(0, "name", "")
    assert backend._profile["buttons"][0]["name"] == invalid_name
    assert signal_count["profile"] > 0, "invalid text edit did not notify QML to restore its binding"

print("QT_LAYOUT_CONFLICT_REGRESSION_OK local_profile_preserved=true android_snapshot_saved=true retry=true")
