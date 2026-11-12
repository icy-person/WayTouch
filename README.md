# WayTouch

**Map a Linux keyboard and mouse to touch controls and mouse input in Waydroid.**

[فارسی](README.fa.md) · [Contributing](CONTRIBUTING.md)

WayTouch lets you control Android apps in Waydroid with the keyboard and mouse connected to your Linux computer. The Rust host reads Linux input events and sends mapped actions to an Android service. A Qt desktop app manages devices and profiles; the Android editor lets you arrange the on-screen controls.

WayTouch targets Linux x86_64 and Waydroid. Input support varies between Android apps and games, so mappings and screen positions may need to be adjusted for each game.

## What it does

- Maps keyboard keys to Android touch buttons.
- Provides a virtual movement joystick, with configurable keys and position.
- Sends mouse movement, buttons, and scroll as native Android mouse input.
- Supports **Relative Mouse** and **Touch Drag** camera control.
- Lets you adjust sensitivity, inversion, smoothing, deadzone, cursor speed, and other mouse settings.
- Supports up to 10 simultaneous touch pointers.
- Includes an Android layout editor for moving controls and changing their bindings.

## How it works

```text
Keyboard / mouse
       |
       v
Rust host (Linux evdev)
       |
       | ADB-forwarded connection
       v
WayTouch Android service
       |
       v
Android app or game
```

The host handles physical input and mapping. The Android service injects touch and mouse events. The desktop app and Android editor are used to configure the profile and control layout.

## Project components

| Path | Purpose |
|---|---|
| `rust-host/` | Linux input handling, mapping, mouse control, device discovery, and CLI |
| `android-server/` | Kotlin service that runs in the Waydroid Android environment |
| `android-editor/` | Android app, notification controls, and on-screen layout editor |
| `gui/` | PySide6 and Qt Quick desktop interface |
| `profiles/` | Input bindings, device paths, and control layout settings |
| `scripts/` | Build, setup, installation, startup, and device-test scripts |
| `assets/` | Application artwork and icons |
| `.github/` | GitHub Actions workflow and pull request template |

## Default controls

| Key or input | Action |
|---|---|
| `W`, `A`, `S`, `D` | Move the virtual joystick |
| `F1` | Capture or release the physical mouse |
| `F2` | Enable or disable mouse-button-to-touch mapping |
| `F3` | Toggle camera Aim while mouse capture is active |
| `Ctrl` + `Alt` + `Esc` | Stop the controller and release captured input |

The profile defines the action buttons and their bindings. You can change them in the desktop app or Android editor.

## Requirements

- Linux x86_64
- Rust toolchain `1.99.1`
- Kotlin compiler `2.4.21`
- JDK `21`
- Android SDK platform `37.0`
- Android Build Tools `36.0.0`
- D8/R8 `9.1.41`
- Python `3.10+` and PySide6 `6.12.0`
- `git`, `make`, `curl`, `unzip`, `zip`, a C build toolchain, and Android SDK command-line tools (`sdkmanager`)

Waydroid and ADB are required to install and run WayTouch. They are not needed for the normal local build and device-independent checks.

## Set up the build environment

On CachyOS or Arch Linux, install the system packages:

```bash
sudo pacman -S --needed base-devel git curl unzip zip make \
  jdk21-openjdk python python-pip
```

Install Rust with [rustup](https://rustup.rs/), install the Android SDK command-line tools, and accept the SDK licences:

```bash
sdkmanager --licenses
```

Set the Android SDK path and install the SDK packages used by the project:

```bash
export ANDROID_HOME="$HOME/android/SDK"
export ANDROID_SDK_ROOT="$ANDROID_HOME"

sdkmanager --install \
  'platforms;android-37.0' \
  'build-tools;36.0.0'
```

From the WayTouch repository, prepare the local development tools and check the environment:

```bash
make setup
make doctor
```

`make setup` creates `.venv/`, installs the Python dependencies, and prepares the pinned Kotlin compiler. Rust uses the version in `rust-toolchain.toml`.

## Build

Build the Linux host, Android service, and Android editor, and run the device-independent checks:

```bash
make build
```

Common Makefile targets:

| Command | Purpose |
|---|---|
| `make setup` | Prepare local Python and Kotlin tools |
| `make doctor` | Check the development environment |
| `make build` | Build the project and run automated checks |
| `make test` | Run the full build checks |
| `make rust-test` | Check Rust formatting, run tests, and run Clippy |
| `make fmt` | Format Rust code |
| `make fmt-check` | Check Rust formatting |
| `make lint` | Run Clippy with warnings treated as errors |
| `make android-editor` | Build the debug APK for the Android editor |
| `make clean` | Remove generated build outputs |

The main build outputs are:

```text
rust-host/target/release/waytouch-host
android-server/build/waytouch-server.jar
android-server/build/dex/classes.dex
android-editor/build/waytouch-editor.apk
```

The editor APK uses a local debug signing key. It is intended for development and testing, not public release.

## Run with Waydroid

Start Waydroid and connect ADB:

```bash
waydroid status
waydroid adb connect
adb devices -l
```

Build and install the Android editor:

```bash
make build
./scripts/install-android-editor.sh
```

Start the Android service:

```bash
./scripts/start-server.sh
```

Launch the desktop interface:

```bash
./scripts/waytouch-gui
```

In **Input devices**, discover your keyboard and mouse, select the devices WayTouch should use, and review the active profile before starting the controller. The Android editor is available from the WayTouch notification.

To add a desktop application-menu entry, run `./scripts/install-desktop.sh`.

## Profiles and input devices

`profiles/default.json` contains the display size, keyboard and mouse device paths, joystick settings, mouse-look options, cursor settings, and action bindings.

The device paths in the example profile may not match your computer. Check them in the GUI or run:

```bash
./rust-host/target/release/waytouch-host devices
./rust-host/target/release/waytouch-host discover
```

To inspect a profile and apply discovered device paths:

```bash
./rust-host/target/release/waytouch-host profile-check profiles/default.json
./rust-host/target/release/waytouch-host discover --apply profiles/default.json
```

Review the profile after applying device discovery. WayTouch uses stable `/dev/input/by-id` or `/dev/input/by-path` links where available; do not copy another computer's device paths unchanged.

## Input capture and safety

When the controller is running, selected keyboard and mouse devices are grabbed exclusively through Linux evdev. Other desktop applications will not receive input from those devices until WayTouch releases them. Use `Ctrl` + `Alt` + `Esc` to stop the controller and release captured input.

The Android connections use ADB forwarding and a per-start IPC token. The host stores the token under `$XDG_STATE_HOME/waytouch/` (or `$HOME/.local/state/waytouch/`). Do not share this token or add it to Git.

## Tests

`make build` runs the available Rust, Python, Qt/QML, Android compilation, and packaging checks. Tests that need a running Waydroid device are separate:

```bash
python3 scripts/test-input-pipeline.py
python3 gui/test_backend_live.py
python3 scripts/test-touchdrag-stress.py
```

`gui/test_backend_live.py` takes exclusive access to the configured input devices while it runs. Close apps that need your keyboard or mouse before starting it.

Automated build checks do not confirm that every Android game accepts injected touch or mouse events. Check the target Waydroid image and game on the device you use.

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) for the development setup, build commands, device tests, and pull request checklist. Keep changes focused, add tests for behavior changes, and do not commit build outputs, signing keys, or IPC tokens.

## Current limitations

- The Linux input backend depends on evdev; the host controller is not supported on Windows or macOS.
- WayTouch is designed for Waydroid. Other Android environments are not covered by the current setup scripts.
- Game compatibility depends on how each game handles injected touch and mouse events.
- The default profile is an example. Screen positions and bindings need to match the game and display in use.
- Captured input devices are unavailable to other desktop applications while the controller is active.
