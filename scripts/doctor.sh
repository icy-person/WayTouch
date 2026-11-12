#!/usr/bin/env bash
set -uo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=build-versions.sh
source "$ROOT/scripts/build-versions.sh"
cd "$ROOT"

FAILURES=0
ok()   { printf '  [OK]   %s\n' "$1"; }
fail() { printf '  [FAIL] %s\n' "$1" >&2; FAILURES=$((FAILURES + 1)); }

if [[ -x "$ROOT/.tools/kotlin-$WAYTOUCH_KOTLIN_VERSION/bin/kotlinc" ]]; then
  PATH="$ROOT/.tools/kotlin-$WAYTOUCH_KOTLIN_VERSION/bin:$PATH"
  export PATH
elif [[ -n "${KOTLIN_HOME:-}" && -x "$KOTLIN_HOME/bin/kotlinc" ]]; then
  PATH="$KOTLIN_HOME/bin:$PATH"
  export PATH
fi

PYTHON_BIN="${PYTHON:-}"
if [[ -z "$PYTHON_BIN" && -x "$ROOT/.venv/bin/python" ]]; then
  PYTHON_BIN="$ROOT/.venv/bin/python"
elif [[ -z "$PYTHON_BIN" ]]; then
  PYTHON_BIN="python3"
fi

ANDROID_HOME="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/android/SDK}}"

echo "WayTouch build environment"
echo "Repository: $ROOT"
echo "Android SDK: $ANDROID_HOME"
echo

for tool in git cargo rustc java javac curl unzip zip keytool; do
  if command -v "$tool" >/dev/null 2>&1; then
    ok "$tool"
  else
    fail "$tool is not installed or not on PATH"
  fi
done

KOTLINC_BIN="${KOTLINC:-kotlinc}"
if command -v "$KOTLINC_BIN" >/dev/null 2>&1 || [[ -x "$KOTLINC_BIN" ]]; then
  ok "Kotlin compiler command: $KOTLINC_BIN"
else
  fail "Kotlin compiler is not installed or not executable: $KOTLINC_BIN"
fi

if command -v rustc >/dev/null 2>&1; then
  RUST_VERSION="$(rustc --version 2>&1)"
  if [[ "$RUST_VERSION" == *" $WAYTOUCH_RUST_VERSION "* ]]; then
    ok "$RUST_VERSION"
  else
    fail "Expected Rust $WAYTOUCH_RUST_VERSION; found: $RUST_VERSION"
  fi
fi
if cargo fmt --version >/dev/null 2>&1; then
  ok "rustfmt component"
else
  fail "rustfmt component missing (rustup component add rustfmt)"
fi
if cargo clippy --version >/dev/null 2>&1; then
  ok "clippy component"
else
  fail "clippy component missing (rustup component add clippy)"
fi

if command -v java >/dev/null 2>&1; then
  JAVA_MAJOR="$(java -version 2>&1 | awk -F'"' '/version/ { split($2, v, "."); print (v[1] == "1" ? v[2] : v[1]); exit }')"
  if [[ "$JAVA_MAJOR" == "21" ]]; then
    ok "JDK 21"
  else
    fail "Expected JDK 21; found Java major version ${JAVA_MAJOR:-unknown}"
  fi
fi

if command -v "$KOTLINC_BIN" >/dev/null 2>&1 || [[ -x "$KOTLINC_BIN" ]]; then
  KOTLIN_VERSION_LINE="$("$KOTLINC_BIN" -version 2>&1 || true)"
  if [[ "$KOTLIN_VERSION_LINE" == *" $WAYTOUCH_KOTLIN_VERSION "* ]]; then
    ok "Kotlin $WAYTOUCH_KOTLIN_VERSION"
  else
    fail "Expected Kotlin $WAYTOUCH_KOTLIN_VERSION; found: $KOTLIN_VERSION_LINE"
  fi
fi

if [[ -x "$PYTHON_BIN" ]] || command -v "$PYTHON_BIN" >/dev/null 2>&1; then
  PYTHON_VERSION="$("$PYTHON_BIN" -c 'import sys; print(".".join(map(str, sys.version_info[:3])))' 2>/dev/null || true)"
  if "$PYTHON_BIN" -c 'import sys; raise SystemExit(0 if sys.version_info >= (3, 10) else 1)' 2>/dev/null; then
    ok "Python $PYTHON_VERSION"
  else
    fail "Python 3.10 or newer is required"
  fi
  PYSIDE_VERSION="$("$PYTHON_BIN" -c 'import PySide6; print(PySide6.__version__)' 2>/dev/null || true)"
  if [[ "$PYSIDE_VERSION" == "$WAYTOUCH_PYSIDE6_VERSION" ]]; then
    ok "PySide6 $PYSIDE_VERSION"
  else
    fail "Expected PySide6 $WAYTOUCH_PYSIDE6_VERSION; found: ${PYSIDE_VERSION:-not installed}. Run make setup."
  fi
else
  fail "Python interpreter not found: $PYTHON_BIN"
fi

if [[ -f "$ANDROID_HOME/platforms/android-$WAYTOUCH_ANDROID_PLATFORM/android.jar" ]]; then
  ok "Android API $WAYTOUCH_ANDROID_PLATFORM platform"
else
  fail "Missing $ANDROID_HOME/platforms/android-$WAYTOUCH_ANDROID_PLATFORM/android.jar"
fi
BUILD_TOOLS="$ANDROID_HOME/build-tools/$WAYTOUCH_BUILD_TOOLS"
for tool in aapt apksigner; do
  if [[ -x "$BUILD_TOOLS/$tool" ]]; then
    ok "Android build-tools $WAYTOUCH_BUILD_TOOLS: $tool"
  else
    fail "Missing executable: $BUILD_TOOLS/$tool"
  fi
done

echo
if (( FAILURES == 0 )); then
  echo "All required build tools are available."
  exit 0
fi

echo "$FAILURES requirement(s) are not satisfied." >&2
if command -v sdkmanager >/dev/null 2>&1; then
  echo "Android SDK command to install the missing packages:" >&2
  echo "  sdkmanager --install 'platforms;android-$WAYTOUCH_ANDROID_PLATFORM' 'build-tools;$WAYTOUCH_BUILD_TOOLS'" >&2
fi
echo "See CONTRIBUTING.md for setup instructions." >&2
exit 1
