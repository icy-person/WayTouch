# Contributing to WayTouch

Thanks for helping develop WayTouch. The project is a Linux desktop controller with three buildable parts: a Rust/evdev host, a Kotlin Android shell-side service and editor, and a PySide6 + Qt Quick desktop interface.

## Supported development environment

The supported build target is **Linux x86_64**. The controller reads Linux evdev devices, so building the host application on Windows or macOS is not supported. Android APK/DEX outputs are produced from the Linux build.

Toolchain versions are pinned in the repository:

| Component | Required version |
| --- | --- |
| Rust | 1.99.1 (rustup reads rust-toolchain.toml) |
| Kotlin compiler | 2.4.21 |
| JDK | 21 |
| Android SDK platform | 37.0 |
| Android Build Tools | 36.0.0 |
| D8/R8 | 9.1.41 |
| Python | 3.10 or newer |
| PySide6 | 6.12.0 |

You also need Git, curl, unzip, zip, make, a C build toolchain, and Android SDK command-line tools (sdkmanager). The Android platform and build-tools packages are installed separately because Google requires accepting their SDK licenses.

## First-time setup

1. Install the system prerequisites. On CachyOS/Arch, the usual base packages include base-devel, git, curl, unzip, zip, make, jdk21-openjdk, python, and python-pip. Install Rust through rustup. On Debian/Ubuntu, install the equivalent compiler/build tools, JDK 21, Python 3 with venv/pip, make, curl, unzip, and zip.
2. Install the Android command-line tools and accept the SDK licenses:

   ```bash
   sdkmanager --licenses
   ```

3. Set ANDROID_HOME (or ANDROID_SDK_ROOT) to your SDK directory, then install the exact SDK packages:

   ```bash
   sdkmanager --install \\
     'platforms;android-37.0' \\
     'build-tools;36.0.0'
   ```

4. From the repository root, prepare the pinned developer tools and Python environment:

   ```bash
   make setup
   ```

   This creates the ignored .venv/, installs requirements-dev.txt, and downloads the official Kotlin compiler only when the pinned version is not already available. The Kotlin archive is SHA-256 verified before installation.

5. Check the environment and build:

   ```bash
   make doctor
   make build
   ```

The build scripts respect ANDROID_HOME, ANDROID_SDK_ROOT, KOTLIN_HOME, KOTLINC, and PYTHON. The project-local Kotlin compiler in .tools/ and Python virtual environment in .venv/ are detected automatically.

## Standard development commands

| Command | Purpose |
| --- | --- |
| make doctor | Verify versions, tools, and Android SDK paths |
| make build | Run the complete build and all device-independent checks |
| make test | Same full verification as make build |
| make rust-test | Rust format check, unit tests, and strict Clippy |
| make fmt | Format Rust sources |
| make fmt-check | Check formatting without modifying files |
| make lint | Run Clippy with warnings treated as errors |
| make android-editor | Build the debug-signed Android editor APK |
| make clean | Remove generated Rust, DEX, JAR, and APK build outputs |

The full build performs these checks:

- Rust formatting, unit tests, strict Clippy, and an optimized host build.
- Kotlin compilation of the Android service and conversion to DEX with the pinned D8/R8 tool.
- Python syntax checks, Android bootstrap contract tests, headless Qt/QML loading, and GUI backend conflict regression tests.
- Android editor resource processing, Kotlin compilation, APK packaging, signing, and signature verification.

## Build outputs

Successful builds produce:

- rust-host/target/release/waytouch-host — Linux host controller.
- android-server/build/waytouch-server.jar — Android shell-side service JAR.
- android-server/build/dex/classes.dex — service DEX for app_process.
- android-editor/build/waytouch-editor.apk — locally debug-signed Android editor APK.

Generated outputs, local keys, .venv/, .tools/, and SDK configuration are not source files and must not be committed.

## Device-only tests

The normal build and GitHub Actions CI do **not** need a running Waydroid instance and do not grab the developer's physical input devices.

Run these only when the relevant device and permissions are available:

- python3 scripts/test-input-pipeline.py — end-to-end input forwarding with Waydroid.
- python3 gui/test_backend_live.py — briefly acquires exclusive evdev access to configured input devices; do not run while you need the mouse/keyboard.
- python3 scripts/test-touchdrag-stress.py — connected-device touch-drag stress validation.

A passing CI run proves that sources compile and the host-side automated checks pass. It does not prove that a particular Android build, Waydroid image, compositor, or game accepts injected touch/mouse events. Those still require real device/game testing.

## Android release signing

Normal builds create a local debug key and are not release builds. Release APK signing must use a separately managed keystore and password files. Provide these environment variables only in a trusted release environment:

- WAYTOUCH_RELEASE_KEYSTORE
- WAYTOUCH_RELEASE_KEY_ALIAS
- WAYTOUCH_RELEASE_STORE_PASSWORD_FILE
- WAYTOUCH_RELEASE_KEY_PASSWORD_FILE
- WAYTOUCH_BUILD_MODE=release

Never commit a keystore, signing password, token, or .env file. Do not place release signing secrets in pull-request CI.

## Pull request checklist

- Keep changes scoped to the component being modified.
- Run make doctor and make build.
- Add or update regression tests for behavior changes.
- Note any device/game tests that were run, or explain why they were not applicable.
- Update the README or relevant documentation when a command, dependency, protocol, or developer workflow changes.
