#!/bin/sh
# Build the sid you run from this clone and put it on PATH.
#
#   ./build.sh            release build, linked from ~/.local/bin/sid
#   SID_BIN_DIR=DIR ./build.sh   link it from DIR instead
#
# Every change to the source needs this to reach the editor you run: sid is a
# compiled binary, so nothing takes effect until it is rebuilt and restarted.
set -eu
cd "$(dirname "$0")"

# rustup's cargo may not be on PATH outside an interactive shell.
command -v cargo >/dev/null 2>&1 || PATH="$HOME/.cargo/bin:$PATH"
command -v cargo >/dev/null 2>&1 || { echo 'cargo not found: install Rust from https://rustup.rs' >&2; exit 1; }

cargo build --release --locked -p helix-term --bin sid

bin_dir=${SID_BIN_DIR:-"$HOME/.local/bin"}
mkdir -p "$bin_dir" "$HOME/.config/sid"
ln -sfn "$PWD/target/release/sid" "$bin_dir/sid"
# Grammars, queries and themes come from the clone.
[ -e "$HOME/.config/sid/runtime" ] || ln -s "$PWD/runtime" "$HOME/.config/sid/runtime"

echo "sid built: $("$PWD/target/release/sid" --version)"
echo "linked from $bin_dir/sid; restart any running sid to use it"
case ":$PATH:" in *":$bin_dir:"*) ;; *) echo "note: $bin_dir is not on PATH" ;; esac
