#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
DEX="$ROOT/android-server/build/dex/classes.dex"
REMOTE_DEX="/data/local/tmp/waytouch-server.dex"
REMOTE_LOG="/data/local/tmp/waytouch.log"
REMOTE_LAYOUT="/data/local/tmp/waytouch-layout.json"
REMOTE_TOKEN_DIR="/data/local/tmp/waytouch-private"
REMOTE_TOKEN="$REMOTE_TOKEN_DIR/waytouch-ipc.token"
LOCAL_LAYOUT="$ROOT/android-server/build/default-layout.json"
STATE_HOME="${XDG_STATE_HOME:-$HOME/.local/state}"
LOCAL_TOKEN="$STATE_HOME/waytouch/ipc-token"

[[ -f "$DEX" ]] || { echo "Build first: ./scripts/build.sh" >&2; exit 1; }
command -v adb >/dev/null || { echo "adb is not installed" >&2; exit 1; }
command -v openssl >/dev/null || { echo "openssl is required to generate a session token" >&2; exit 1; }
command -v python3 >/dev/null || { echo "python3 is required for layout migration" >&2; exit 1; }
adb get-state >/dev/null 2>&1 || {
  echo "No ADB device. Start Waydroid, then run: waydroid adb connect" >&2
  exit 1
}

echo "[1/6] Creating a private per-start Android IPC token"
mkdir -p "$(dirname "$LOCAL_TOKEN")"
chmod 700 "$(dirname "$LOCAL_TOKEN")"
umask 077
openssl rand -hex 32 > "$LOCAL_TOKEN"
chmod 600 "$LOCAL_TOKEN"
adb shell "mkdir -p $REMOTE_TOKEN_DIR && chmod 700 $REMOTE_TOKEN_DIR"
adb push "$LOCAL_TOKEN" "$REMOTE_TOKEN" >/dev/null
adb shell chmod 600 "$REMOTE_TOKEN"

echo "[2/6] Stopping only an existing WayTouch app_process"
old_pids="$(adb shell 'ps -A -o PID,ARGS | grep "[a]pp_process /system/bin com.waytouch.WayTouchServer" | awk "{print $1}"' | tr -d '\r' || true)"
for pid in $old_pids; do
  if [[ "$pid" =~ ^[0-9]+$ ]]; then
    adb shell kill "$pid" 2>/dev/null || true
  fi
done

echo "[3/6] Installing authenticated Android DEX"
adb push "$DEX" "$REMOTE_DEX" >/dev/null

# Prepare the profile-derived default layout on every startup. If the existing
# Android file is corrupt, the server backs it up and the authenticated SEED
# request below restores this validated default without overwriting healthy edits.
python3 - "$ROOT/profiles/default.json" "$LOCAL_LAYOUT" <<'PYLAYOUT'
import json
import sys
with open(sys.argv[1], encoding="utf-8") as source:
    profile = json.load(source)
layout = {
    "movement": {
        "center": profile["movement"]["center"],
        "radius": profile["movement"]["radius"],
        "keys": profile["movement"]["keys"],
    },
    "buttons": [
        {
            "name": item["name"],
            "position": item["position"],
            "source": item["source"],
            "code": item["code"],
        }
        for item in profile.get("buttons", [])
    ],
}
envelope = {
    "revision": int(profile.get("layout_revision", 0)),
    "layout": layout,
}
with open(sys.argv[2], "w", encoding="utf-8") as output:
    json.dump(envelope, output, ensure_ascii=False, separators=(",", ":"), allow_nan=False)
PYLAYOUT
if ! adb shell "test -f $REMOTE_LAYOUT" >/dev/null 2>&1; then
  adb push "$LOCAL_LAYOUT" "$REMOTE_LAYOUT" >/dev/null
fi
adb shell "test -f $REMOTE_TOKEN" >/dev/null 2>&1 || {
  echo "Android IPC token was not installed." >&2
  exit 1
}

echo "[4/6] Configuring ADB forwarding"
adb forward tcp:27183 tcp:27183 >/dev/null
adb forward tcp:27184 tcp:27184 >/dev/null
adb forward tcp:27185 tcp:27185 >/dev/null
adb forward tcp:27186 tcp:27186 >/dev/null

echo "[5/6] Starting authenticated WayTouch Android touch server (WTCH v2 + layout channel)"
adb shell "echo -n '' > $REMOTE_LOG"
adb shell "CLASSPATH=$REMOTE_DEX nohup app_process /system/bin com.waytouch.WayTouchServer >$REMOTE_LOG 2>&1 </dev/null &"

# Do not trust a stale log or a fixed one-second sleep: wait for this process
# to publish the READY marker, and fail with its actual log if it does not.
ready=0
server_log=""
for _ in {1..20}; do
  server_log="$(adb shell cat "$REMOTE_LOG" 2>/dev/null || true)"
  if grep -q 'WTCH_READY version=2' <<<"$server_log"; then
    ready=1
    break
  fi
  sleep 0.25
done

printf '%s\n' "$server_log"
if [[ "$ready" -ne 1 ]]; then
  echo "WayTouch v2 did not become ready within 5 seconds." >&2
  echo "Inspect Android server log: adb shell cat $REMOTE_LOG" >&2
  exit 1
fi
if ! grep -q "WTCH_LAYOUT_READY port=27184" <<<"$server_log"; then
  for _ in {1..20}; do
    server_log="$(adb shell cat "$REMOTE_LOG" 2>/dev/null || true)"
    grep -q "WTCH_LAYOUT_READY port=27184" <<<"$server_log" && break
    sleep 0.1
  done
fi
if ! grep -q "WTCH_LAYOUT_READY port=27184" <<<"$server_log"; then
  echo "WayTouch touch server started but the layout editor channel did not become ready." >&2
  exit 1
fi
if ! grep -q "WTCH_MOUSE_READY port=27186" <<<"$server_log"; then
  for _ in {1..20}; do
    server_log="$(adb shell cat "$REMOTE_LOG" 2>/dev/null || true)"
    grep -q "WTCH_MOUSE_READY port=27186" <<<"$server_log" && break
    sleep 0.1
  done
fi
if ! grep -q "WTCH_MOUSE_READY port=27186" <<<"$server_log"; then
  echo "WayTouch touch server started but Android SOURCE_MOUSE channel 27186 did not become ready." >&2
  exit 1
fi

# BootstrapActivity is exported but protected by android.permission.DUMP
# (available to adb shell). It forwards the token to the private EditorService,
# keeping UI and notification actions on the app's own UID.
if ! adb shell pm path com.waytouch.editor >/dev/null 2>&1; then
  echo "WayTouch Editor APK is missing. Install it first: ./scripts/install-android-editor.sh" >&2
  exit 1
fi
if ! adb shell am start -n com.waytouch.editor/.BootstrapActivity \
    --es com.waytouch.editor.IPC_TOKEN "$(<"$LOCAL_TOKEN")" >/dev/null 2>&1; then
  echo "Cannot bootstrap the authenticated WayTouch EditorService. Reinstall the updated editor APK and retry." >&2
  exit 1
fi

cursor_ready=0
for _ in {1..20}; do
  if python3 - "$LOCAL_TOKEN" <<'PYCURSOR'
import socket
import sys
token = open(sys.argv[1], encoding="ascii").read().strip()
with socket.create_connection(("127.0.0.1", 27185), timeout=0.4) as connection:
    connection.settimeout(0.4)
    connection.sendall(("AUTH " + token + "\n").encode("ascii"))
    with connection.makefile("r", encoding="ascii") as reader:
        if reader.readline().strip() != "OK AUTH":
            raise SystemExit("cursor authentication failed")
        # The health probe is deliberately a one-shot connection. PING must
        # return PONG without acquiring the exclusive persistent cursor slot.
        connection.sendall(b"PING\n")
        if reader.readline().strip() != "PONG":
            raise SystemExit("cursor PING health check failed")
PYCURSOR
  then
    cursor_ready=1
    break
  fi
  sleep 0.2
done
if [[ "$cursor_ready" -ne 1 ]]; then
  echo "Authenticated cursor service tcp:27185 did not become ready. Check EditorService startup and Android logcat." >&2
  exit 1
fi
echo "WTCH_CURSOR_READY port=27185 authenticated=true"

mouse_ready=0
for _ in {1..20}; do
  if python3 - "$LOCAL_TOKEN" <<'PYMOUSE'
import math
import socket
import sys
token = open(sys.argv[1], encoding="ascii").read().strip()
with socket.create_connection(("127.0.0.1", 27186), timeout=0.5) as connection:
    connection.settimeout(0.5)
    connection.sendall(("AUTH " + token + "\n").encode("ascii"))
    with connection.makefile("r", encoding="ascii") as reader:
        if reader.readline().strip() != "OK AUTH":
            raise SystemExit("mouse authentication failed")
        # PING is a read-only health command and never enters the controller's
        # persistent mouse-event stream or competes for its exclusive slot.
        connection.sendall(b"PING\n")
        if reader.readline().strip() != "PONG":
            raise SystemExit("mouse channel PING health check failed")
PYMOUSE
  then
    mouse_ready=1
    break
  fi
  sleep 0.2
done
if [[ "$mouse_ready" -ne 1 ]]; then
  echo "Authenticated SOURCE_MOUSE channel tcp:27186 did not acknowledge its health-check frame." >&2
  exit 1
fi
echo "WTCH_MOUSE_HEALTH_OK port=27186 ack=true"

# Upgrade old position-only Android layouts in place. The Android coordinates
# remain authoritative; binding metadata is joined by stable action name from
# the validated Linux profile. Never replace a custom/unmatched action silently.
python3 - "$ROOT/profiles/default.json" "$LOCAL_LAYOUT" "$LOCAL_TOKEN" <<'PY'
import json
import socket
import sys
import time

profile = json.load(open(sys.argv[1], encoding="utf-8"))
default_envelope = json.load(open(sys.argv[2], encoding="utf-8"))
token = open(sys.argv[3], encoding="ascii").read().strip()
address = ("127.0.0.1", 27184)

def request(command):
    with socket.create_connection(address, timeout=1.5) as connection:
        connection.settimeout(1.5)
        connection.sendall(("AUTH " + token + "\n").encode("ascii"))
        with connection.makefile("r", encoding="utf-8") as reader:
            if reader.readline().strip() != "OK AUTH":
                raise SystemExit("Android rejected layout IPC authentication")
            connection.sendall((command + "\n").encode("utf-8"))
            return reader.readline().strip()

raw = request("GET")
if raw == "NONE":
    default_payload = json.dumps(default_envelope["layout"], ensure_ascii=False,
                                 separators=(",", ":"), allow_nan=False)
    seeded = request("SEED " + default_payload)
    if not seeded.startswith("OK "):
        raise SystemExit("Could not recover an empty/corrupt Android layout: " + seeded)
    print("Corrupt or missing Android layout recovered from validated default profile: " + seeded)
    raw = request("GET")
if not raw or raw.startswith("ERR"):
    raise SystemExit("Could not read Android layout for schema migration: " + raw)
envelope = json.loads(raw)
layout = envelope["layout"]
movement = layout["movement"]
buttons = layout["buttons"]
local_buttons = {item["name"]: item for item in profile.get("buttons", [])}
needs_migration = (
    "keys" not in movement
    or movement.get("keys") != profile["movement"]["keys"]
    or any("source" not in item or "code" not in item for item in buttons)
)
if not needs_migration:
    print("Android layout schema and movement keys are current; revision=" + str(envelope["revision"]))
else:
    # Keep Android's existing positions and any existing custom bindings. Only
    # fill missing source/code metadata for action names known to the profile.
    movement["keys"] = dict(profile["movement"]["keys"])
    for item in buttons:
        local = local_buttons.get(item["name"])
        if local is not None and ("source" not in item or "code" not in item):
            item["source"] = local["source"]
            item["code"] = int(local["code"])
    current = int(envelope["revision"])
    next_revision = max(current + 1, time.time_ns() // 1_000_000)
    payload = json.dumps(layout, ensure_ascii=False, separators=(",", ":"), allow_nan=False)
    reply = request(f"SYNC {current} {next_revision} {payload}")
    if not reply.startswith("OK "):
        raise SystemExit("Android layout migration failed: " + reply)
    print("Android movement/binding schema migrated; all existing control coordinates and custom bindings preserved: " + reply)
PY

printf "%s\n" "$server_log"
echo "[6/6] Android IPC authenticated; local token stored at $LOCAL_TOKEN"
echo "WayTouch ready: authenticated WTCH v2 touch (27183), Layout (27184), Cursor (27185) and Mouse (27186)."
