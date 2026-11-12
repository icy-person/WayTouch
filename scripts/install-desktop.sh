#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "$0")/.." && pwd)"
APPLICATIONS="$HOME/.local/share/applications"
ICONS="$HOME/.local/share/icons/hicolor/scalable/apps"

mkdir -p "$APPLICATIONS" "$ICONS"
install -m 0644 "$ROOT/assets/waytouch.svg" "$ICONS/waytouch.svg"

# Remove the legacy optional cursor-lock extension from older installations.
# This is cleanup only; WayTouch never installs, enables or calls a shell extension.
LEGACY_EXTENSION_UUID="waytouch-cursor-lock@local"
LEGACY_EXTENSION="$HOME/.local/share/gnome-shell/extensions/$LEGACY_EXTENSION_UUID"
if [[ -d "$LEGACY_EXTENSION" ]]; then
  if command -v gnome-extensions >/dev/null 2>&1; then
    gnome-extensions disable "$LEGACY_EXTENSION_UUID" >/dev/null 2>&1 || true
  fi
  rm -rf -- "$LEGACY_EXTENSION"
fi

# WayTouch has no GNOME Shell extension or desktop-shell plug-in dependency.

cat > "$APPLICATIONS/waytouch.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=WayTouch
GenericName=Waydroid Input Controller
Comment=Configure keyboard, touch and mouse look for Waydroid with Qt 6
Exec=$ROOT/scripts/waytouch-gui
Icon=waytouch
Terminal=false
StartupNotify=true
Categories=Utility;
Keywords=Waydroid;Mouse;Controller;Input;Touch;
EOF

chmod 0644 "$APPLICATIONS/waytouch.desktop"
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$APPLICATIONS" || true
fi

echo "WayTouch has been added to the desktop application menu."
echo "Desktop entry: $APPLICATIONS/waytouch.desktop"
