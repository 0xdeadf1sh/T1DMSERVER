set shell := ["bash", "-cu"]

# aarch64 target triple for the Raspberry Pi Zero 2 W.
pi_target := "aarch64-unknown-linux-gnu"

# List recipes.
default:
    @just --list

# Run the appliance (config path optional; defaults to ./config.toml).
run config="config.toml":
    cargo run --release -- {{config}}

# Build the workspace for the native host (the local compile gate).
build:
    cargo build --release

# Cross-compile for the Raspberry Pi Zero 2 W (aarch64) via `cross`, which
# supplies the aarch64 C toolchain that rusqlite's bundled SQLite needs. For a
# rustup + aarch64-linux-gnu-gcc alternative, see README.md.
build-pi:
    cross build --release --target {{pi_target}}

# Format all crates.
fmt:
    cargo fmt --all

# Lint all crates and targets, denying warnings.
clippy:
    cargo clippy --workspace --all-targets -- -D warnings
