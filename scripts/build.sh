#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=build-versions.sh
source "$ROOT/scripts/build-versions.sh"

if [[ -x "$ROOT/.tools/kotlin-$WAYTOUCH_KOTLIN_VERSION/bin/kotlinc" ]]; then
  PATH="$ROOT/.tools/kotlin-$WAYTOUCH_KOTLIN_VERSION/bin:$PATH"
  export PATH
elif [[ -n "${KOTLIN_HOME:-}" && -x "$KOTLIN_HOME/bin/kotlinc" ]]; then
  PATH="$KOTLIN_HOME/bin:$PATH"
  export PATH
fi
KOTLINC="${KOTLINC:-kotlinc}"
PYTHON_BIN="${PYTHON:-}"
if [[ -z "$PYTHON_BIN" && -x "$ROOT/.venv/bin/python" ]]; then
  PYTHON_BIN="$ROOT/.venv/bin/python"
elif [[ -z "$PYTHON_BIN" ]]; then
  PYTHON_BIN="python3"
fi

ANDROID_HOME="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/android/SDK}}"
export ANDROID_HOME
ANDROID_JAR="$ANDROID_HOME/platforms/android-$WAYTOUCH_ANDROID_PLATFORM/android.jar"
R8_VERSION="$WAYTOUCH_R8_VERSION"
R8_JAR="${XDG_CACHE_HOME:-$HOME/.cache}/waytouch/tools/r8-$R8_VERSION.jar"
RUST_MANIFEST="$ROOT/rust-host/Cargo.toml"
ANDROID_BUILD="$ROOT/android-server/build"

for tool in cargo "$KOTLINC" "$PYTHON_BIN" java curl; do
  command -v "$tool" >/dev/null || { echo "Missing tool: $tool" >&2; exit 1; }
done
[[ -f "$ANDROID_JAR" ]] || { echo "Missing Android API jar: $ANDROID_JAR" >&2; exit 1; }
if [[ ! -s "$R8_JAR" ]]; then
  echo "Downloading D8/R8 $R8_VERSION (required for Kotlin 2.4 metadata)"
  mkdir -p "$(dirname "$R8_JAR")"
  temporary="$R8_JAR.tmp.$$"
  trap 'rm -f "$temporary"' EXIT
  curl --fail --location --retry 2 \
    "https://storage.googleapis.com/r8-releases/raw/$R8_VERSION/r8lib.jar" \
    --output "$temporary"
  mv "$temporary" "$R8_JAR"
  trap - EXIT
fi
D8_VERSION="$(java -cp "$R8_JAR" com.android.tools.r8.D8 --version 2>&1)"
grep -F "$R8_VERSION" <<<"$D8_VERSION" >/dev/null || {
  echo "Unexpected D8 version. Expected $R8_VERSION, got: $D8_VERSION" >&2
  exit 1
}

echo "[1/7] Checking Rust formatting"
cargo fmt --manifest-path "$RUST_MANIFEST" -- --check

echo "[2/7] Running Rust protocol/state-machine tests"
cargo test --locked --manifest-path "$RUST_MANIFEST"
echo "[2/7] Running strict Rust lint checks"
cargo clippy --locked --manifest-path "$RUST_MANIFEST" --all-targets -- -D warnings

echo "[3/7] Building optimized Rust host"
cargo build --locked --release --manifest-path "$RUST_MANIFEST"

echo "[4/7] Compiling Kotlin Android server"
mkdir -p "$ANDROID_BUILD"
"$KOTLINC" "$ROOT"/android-server/src/main/kotlin/com/waytouch/*.kt \
  -classpath "$ANDROID_JAR" -include-runtime \
  -d "$ANDROID_BUILD/waytouch-server.jar"

echo "[5/7] Converting Android server JAR to DEX"
rm -rf "$ANDROID_BUILD/dex"
mkdir -p "$ANDROID_BUILD/dex"
java -cp "$R8_JAR" com.android.tools.r8.D8 --min-api 28 --lib "$ANDROID_JAR" \
  --output "$ANDROID_BUILD/dex" \
  "$ANDROID_BUILD/waytouch-server.jar" \
  >"$ANDROID_BUILD/d8.log" 2>&1

echo "[6/7] Checking Qt 6 / Material interface"
"$PYTHON_BIN" -c 'import PySide6; print(PySide6.__version__)'
"$PYTHON_BIN" -m py_compile "$ROOT/gui/waytouch.py" "$ROOT/gui/test_qml.py" "$ROOT/gui/test_backend_live.py" "$ROOT/gui/test_backend_conflict.py" "$ROOT/scripts/test-input-pipeline.py" "$ROOT/scripts/test-android-bootstrap.py" "$ROOT/scripts/test-touchdrag-stress.py"
"$PYTHON_BIN" "$ROOT/scripts/test-android-bootstrap.py"
QT_QPA_PLATFORM=offscreen "$PYTHON_BIN" "$ROOT/gui/test_qml.py"
QT_QPA_PLATFORM=offscreen "$PYTHON_BIN" "$ROOT/gui/test_backend_conflict.py"
bash -n "$ROOT/scripts/start-server.sh" "$ROOT/scripts/waytouch-gui" "$ROOT/scripts/install-desktop.sh" "$ROOT/scripts/build-android-editor.sh" "$ROOT/scripts/install-android-editor.sh" "$ROOT/scripts/install-kotlin.sh" "$ROOT/scripts/setup-dev.sh" "$ROOT/scripts/doctor.sh"

echo "[7/7] Building the Android notification layout editor APK"
bash "$ROOT/scripts/build-android-editor.sh"

echo "Build and tests completed:"
ls -lh "$ROOT/rust-host/target/release/waytouch-host" \
  "$ANDROID_BUILD/dex/classes.dex" "$ROOT/android-editor/build/waytouch-editor.apk"
