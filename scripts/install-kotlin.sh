#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=build-versions.sh
source "$ROOT/scripts/build-versions.sh"

VERSION="$WAYTOUCH_KOTLIN_VERSION"
INSTALL_DIR="${KOTLIN_HOME:-$ROOT/.tools/kotlin-$VERSION}"

if [[ -x "$INSTALL_DIR/bin/kotlinc" ]]; then
  echo "Kotlin $VERSION already installed at $INSTALL_DIR"
  exit 0
fi

if command -v kotlinc >/dev/null 2>&1; then
  INSTALLED_VERSION="$(kotlinc -version 2>&1 || true)"
  if [[ "$INSTALLED_VERSION" == *" $VERSION "* ]]; then
    echo "Kotlin $VERSION is already available on PATH"
    exit 0
  fi
fi

for tool in curl unzip sha256sum; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "Missing required tool: $tool" >&2
    exit 1
  }
done

# SHA-256 published with the official JetBrains Kotlin compiler release.
EXPECTED_SHA256="7cc140e76daf416a0424a5557f99afd7ca89ebd463ea1101261e3e01c33e75cd"
URL="https://github.com/JetBrains/kotlin/releases/download/v${VERSION}/kotlin-compiler-${VERSION}.zip"
INSTALL_PARENT="$(dirname -- "$INSTALL_DIR")"
mkdir -p "$INSTALL_PARENT"
TEMP_DIR="$(mktemp -d "$INSTALL_PARENT/.kotlin-$VERSION.XXXXXX")"
trap 'rm -rf "$TEMP_DIR"' EXIT

echo "Downloading official Kotlin compiler $VERSION"
curl --fail --location --retry 3 --retry-delay 2 \
  --proto '=https' --tlsv1.2 "$URL" \
  --output "$TEMP_DIR/compiler.zip"

printf '%s  %s\n' "$EXPECTED_SHA256" "$TEMP_DIR/compiler.zip" | sha256sum --check

mkdir -p "$TEMP_DIR/unpacked"
unzip -q "$TEMP_DIR/compiler.zip" -d "$TEMP_DIR/unpacked"
[[ -x "$TEMP_DIR/unpacked/kotlinc/bin/kotlinc" ]] || {
  echo "Unexpected Kotlin archive layout; expected kotlinc/bin/kotlinc." >&2
  exit 1
}

mv "$TEMP_DIR/unpacked/kotlinc" "$INSTALL_DIR"
echo "Installed Kotlin $VERSION at $INSTALL_DIR"
echo "The WayTouch build scripts will detect this project-local compiler automatically."
