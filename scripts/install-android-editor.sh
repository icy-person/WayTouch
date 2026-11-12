#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "$0")/.." && pwd)"
APK="$ROOT/android-editor/build/waytouch-editor.apk"
command -v adb >/dev/null || { echo "adb is not installed" >&2; exit 1; }
[[ -f "$APK" ]] || { echo "Build first: ./scripts/build-android-editor.sh" >&2; exit 1; }
adb get-state >/dev/null 2>&1 || {
  echo "No connected Waydroid device. Run: waydroid adb connect" >&2
  exit 1
}
adb install -r "$APK"
adb shell appops set com.waytouch.editor SYSTEM_ALERT_WINDOW allow
adb shell pm grant com.waytouch.editor android.permission.POST_NOTIFICATIONS >/dev/null 2>&1 || true
adb shell am force-stop com.waytouch.editor
adb shell am start -n com.waytouch.editor/.MainActivity
echo "The app will enable its notification service on launch. Open/close the editor from the persistent WayTouch notification."
