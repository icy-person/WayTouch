#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "$0")/.." && pwd)"
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

ANDROID_HOME="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/android/SDK}}"
export ANDROID_HOME
ANDROID_JAR="$ANDROID_HOME/platforms/android-$WAYTOUCH_ANDROID_PLATFORM/android.jar"
R8_VERSION="$WAYTOUCH_R8_VERSION"
R8_JAR="${XDG_CACHE_HOME:-$HOME/.cache}/waytouch/tools/r8-$R8_VERSION.jar"
WORK="$ROOT/android-editor/build"
BUILD_MODE="${WAYTOUCH_BUILD_MODE:-debug}"
[[ "$BUILD_MODE" == "debug" || "$BUILD_MODE" == "release" ]] || {
  echo "WAYTOUCH_BUILD_MODE must be debug or release." >&2
  exit 1
}
DEBUG_KEYSTORE="$HOME/.local/share/waytouch/android-editor-debug.keystore"
APK="$WORK/waytouch-editor.apk"

for tool in "$KOTLINC" javac keytool zip unzip java curl; do
  command -v "$tool" >/dev/null || { echo "Missing tool: $tool" >&2; exit 1; }
done
[[ -f "$ANDROID_JAR" ]] || { echo "Missing Android API jar: $ANDROID_JAR" >&2; exit 1; }
BUILD_TOOLS="$ANDROID_HOME/build-tools/$WAYTOUCH_BUILD_TOOLS"
[[ -x "$BUILD_TOOLS/aapt" && -x "$BUILD_TOOLS/apksigner" ]] || {
  echo "Android build-tools $WAYTOUCH_BUILD_TOOLS (aapt + apksigner) are required." >&2; exit 1;
}
if [[ ! -s "$R8_JAR" ]]; then
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

rm -rf "$WORK/classes" "$WORK/dex" "$WORK/generated"
mkdir -p "$WORK/classes" "$WORK/dex" "$WORK/generated"
# Generate the Android resource symbol table before compiling Kotlin sources.
"$BUILD_TOOLS/aapt" package -f -m -J "$WORK/generated" \
  -M "$ROOT/android-editor/src/main/AndroidManifest.xml" \
  -S "$ROOT/android-editor/src/main/res" -I "$ANDROID_JAR" -F "$WORK/resources.ap_"
javac -classpath "$ANDROID_JAR" -d "$WORK/classes" \
  "$WORK/generated/com/waytouch/editor/R.java"
if [[ "$BUILD_MODE" == "release" ]]; then
  : "${WAYTOUCH_RELEASE_KEYSTORE:?Set WAYTOUCH_RELEASE_KEYSTORE to a separately managed release keystore}"
  : "${WAYTOUCH_RELEASE_KEY_ALIAS:?Set WAYTOUCH_RELEASE_KEY_ALIAS}"
  : "${WAYTOUCH_RELEASE_STORE_PASSWORD_FILE:?Set password-file path for the release keystore}"
  : "${WAYTOUCH_RELEASE_KEY_PASSWORD_FILE:?Set password-file path for the release key}"
  [[ -f "$WAYTOUCH_RELEASE_KEYSTORE" && -f "$WAYTOUCH_RELEASE_STORE_PASSWORD_FILE" && -f "$WAYTOUCH_RELEASE_KEY_PASSWORD_FILE" ]] || {
    echo "Release keystore or password file does not exist." >&2; exit 1;
  }
  KEYSTORE="$WAYTOUCH_RELEASE_KEYSTORE"
  KEY_ALIAS="$WAYTOUCH_RELEASE_KEY_ALIAS"
  KS_PASS_ARG="file:$WAYTOUCH_RELEASE_STORE_PASSWORD_FILE"
  KEY_PASS_ARG="file:$WAYTOUCH_RELEASE_KEY_PASSWORD_FILE"
else
  mkdir -p "$(dirname "$DEBUG_KEYSTORE")"
  KEYSTORE="$DEBUG_KEYSTORE"
  KEY_ALIAS="waytouch"
  KS_PASS_ARG="pass:changeit"
  KEY_PASS_ARG="pass:changeit"
fi

echo "[1/5] Compiling Android editor (Kotlin)"
"$KOTLINC" "$ROOT"/android-editor/src/main/kotlin/com/waytouch/editor/*.kt \
  -classpath "$ANDROID_JAR:$WORK/classes" -include-runtime -d "$WORK/waytouch-editor.jar"

echo "[2/5] Converting editor to DEX"
java -cp "$R8_JAR" com.android.tools.r8.D8 --min-api 28 --lib "$ANDROID_JAR" \
  --output "$WORK/dex" "$WORK/waytouch-editor.jar" >"$WORK/d8.log" 2>&1

echo "[3/5] Packaging Android manifest"
"$BUILD_TOOLS/aapt" package -f -M "$ROOT/android-editor/src/main/AndroidManifest.xml" \
  -S "$ROOT/android-editor/src/main/res" -I "$ANDROID_JAR" -F "$WORK/base.apk"

echo "[4/5] Adding DEX and signing key"
zip -q -j "$WORK/base.apk" "$WORK/dex/classes.dex"
if [[ "$BUILD_MODE" == "debug" && ! -f "$KEYSTORE" ]]; then
  keytool -genkeypair -noprompt -keystore "$KEYSTORE" \
    -storepass changeit -keypass changeit -alias "$KEY_ALIAS" \
    -dname "CN=WayTouch Local Debug" -keyalg RSA -keysize 2048 -validity 10000
fi
"$BUILD_TOOLS/apksigner" sign --ks "$KEYSTORE" --ks-key-alias "$KEY_ALIAS" \
  --ks-pass "$KS_PASS_ARG" --key-pass "$KEY_PASS_ARG" --out "$APK" "$WORK/base.apk"

echo "[5/5] Verifying APK"
"$BUILD_TOOLS/apksigner" verify --verbose "$APK"
unzip -t "$APK" >/dev/null
"$BUILD_TOOLS/aapt" dump badging "$APK" | sed -n '1,8p'
ls -lh "$APK"
