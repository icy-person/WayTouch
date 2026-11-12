SHELL := /bin/bash
.DEFAULT_GOAL := help

.PHONY: help setup doctor build test rust-test fmt fmt-check lint clean android-editor

help:
	@printf '%s\n' \
	  'WayTouch developer targets:' \
	  '  make setup         Create .venv, install pinned PySide6, prepare Kotlin' \
	  '  make doctor        Check all required versions and Android SDK packages' \
	  '  make build         Run the complete host + Android + GUI build and tests' \
	  '  make test          Alias for the full verified build' \
	  '  make rust-test     Run Rust formatting, unit tests, and strict Clippy' \
	  '  make fmt           Apply Rust formatting' \
	  '  make fmt-check     Verify Rust formatting without changing files' \
	  '  make lint          Run Clippy with warnings denied' \
	  '  make android-editor Build and sign the local debug editor APK' \
	  '  make clean         Remove generated build outputs'

setup:
	./scripts/setup-dev.sh

doctor:
	./scripts/doctor.sh

build: doctor
	./scripts/build.sh

test: build

rust-test:
	cargo fmt --manifest-path rust-host/Cargo.toml -- --check
	cargo test --locked --manifest-path rust-host/Cargo.toml
	cargo clippy --locked --manifest-path rust-host/Cargo.toml --all-targets -- -D warnings

fmt:
	cargo fmt --manifest-path rust-host/Cargo.toml

fmt-check:
	cargo fmt --manifest-path rust-host/Cargo.toml -- --check

lint:
	cargo clippy --locked --manifest-path rust-host/Cargo.toml --all-targets -- -D warnings

android-editor:
	./scripts/build-android-editor.sh

clean:
	cargo clean --manifest-path rust-host/Cargo.toml
	rm -rf android-server/build android-server/build-dex android-editor/build
