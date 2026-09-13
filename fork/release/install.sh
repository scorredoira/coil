#!/bin/sh
set -eu
# Install an extracted release, keeping runtime beside the real executable.
source_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
prefix=${SID_PREFIX:-"$HOME/.local"}
case "$prefix" in /*) ;; *) echo 'SID_PREFIX must be absolute' >&2; exit 1 ;; esac
mkdir -p "$prefix/lib/sid" "$prefix/bin"
install_dir=$(mktemp -d "$prefix/lib/sid/release.XXXXXXXX")
trap 'rm -rf "$install_dir"' EXIT HUP INT TERM
cp "$source_dir/sid" "$install_dir/sid"
cp -R "$source_dir/runtime" "$install_dir/runtime"
chmod +x "$install_dir/sid"
"$install_dir/sid" --version
if [ -d "$prefix/bin/sid" ]; then
    echo "Refusing to replace directory $prefix/bin/sid" >&2
    exit 1
fi
ln -sfn "$install_dir/sid" "$prefix/bin/sid"
trap - EXIT HUP INT TERM
printf '\nInstalled: %s/bin/sid\nAdd %s/bin to PATH if needed.\n' "$prefix" "$prefix"
