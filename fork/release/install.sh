#!/bin/sh
set -eu
# Install an extracted release, keeping runtime beside the real executable.
source_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
prefix=${COIL_PREFIX:-"$HOME/.local"}
case "$prefix" in /*) ;; *) echo 'COIL_PREFIX must be absolute' >&2; exit 1 ;; esac
mkdir -p "$prefix/lib/coil" "$prefix/bin"
install_dir=$(mktemp -d "$prefix/lib/coil/release.XXXXXXXX")
trap 'rm -rf "$install_dir"' EXIT HUP INT TERM
cp "$source_dir/coil" "$install_dir/coil"
cp -R "$source_dir/runtime" "$install_dir/runtime"
chmod +x "$install_dir/coil"
"$install_dir/coil" --version
if [ -d "$prefix/bin/coil" ]; then
    echo "Refusing to replace directory $prefix/bin/coil" >&2
    exit 1
fi
ln -sfn "$install_dir/coil" "$prefix/bin/coil"
trap - EXIT HUP INT TERM
printf '\nInstalled: %s/bin/coil\nAdd %s/bin to PATH if needed.\n' "$prefix" "$prefix"
