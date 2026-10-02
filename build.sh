#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -P "$(dirname "$0")" && pwd)

if ! command -v cargo >/dev/null 2>&1; then
    printf '%s\n' 'error: cargo was not found in PATH; install a Rust toolchain and add cargo to PATH.' >&2
    exit 127
fi

exec cargo build --manifest-path "$SCRIPT_DIR/Cargo.toml" --locked --release --bin lyrics-fetcher "$@"
