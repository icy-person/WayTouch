#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=build-versions.sh
source "$ROOT/scripts/build-versions.sh"

PYTHON="${PYTHON:-python3}"
command -v "$PYTHON" >/dev/null 2>&1 || {
  echo "Python 3 is required. Install Python 3.10 or newer first." >&2
  exit 1
}

"$PYTHON" -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 10) else "Python 3.10 or newer is required.")'
"$PYTHON" -m venv "$ROOT/.venv"
"$ROOT/.venv/bin/python" -m pip install --disable-pip-version-check \
  -r "$ROOT/requirements-dev.txt"

"$ROOT/scripts/install-kotlin.sh"

ANDROID_HOME="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/android/SDK}}"
if [[ ! -f "$ANDROID_HOME/platforms/android-$WAYTOUCH_ANDROID_PLATFORM/android.jar" ]] ||
   [[ ! -x "$ANDROID_HOME/build-tools/$WAYTOUCH_BUILD_TOOLS/aapt" ]] ||
   [[ ! -x "$ANDROID_HOME/build-tools/$WAYTOUCH_BUILD_TOOLS/apksigner" ]]; then
  echo
  echo "Android SDK packages are not complete under: $ANDROID_HOME"
  echo "Install the exact packages after accepting the Android SDK licenses:"
  echo "  sdkmanager --install \\"
  echo "    'platforms;android-$WAYTOUCH_ANDROID_PLATFORM' \\"
  echo "    'build-tools;$WAYTOUCH_BUILD_TOOLS'"
else
  echo "Android SDK platform and build-tools are present."
fi

echo
echo "WayTouch developer tools are prepared."
echo "Run: make doctor"
echo "Then: make build"
echo "Python dependencies are isolated in .venv; Kotlin is installed locally only when needed."
