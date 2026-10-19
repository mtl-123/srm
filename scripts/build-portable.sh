#!/usr/bin/env bash
set -euo pipefail
project="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
# Install this target with rustup target add x86_64-unknown-linux-musl.
# A matching rustc/standard library and a musl linker are required.
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="${CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER:-musl-gcc}"
export RUSTFLAGS='-C target-cpu=x86-64 -C target-feature=+crt-static'
cargo build --manifest-path "$project/Cargo.toml" --release --locked --target x86_64-unknown-linux-musl
