#!/bin/sh
# Download and install the complete native Coil release without a compiler.
set -eu
repo=scorredoira/coil
case $(uname -s) in Linux) os=linux ;; Darwin) os=macos ;; *) echo 'Supported: Linux and macOS' >&2; exit 1 ;; esac
case $(uname -m) in x86_64) arch=x86_64 ;; arm64|aarch64) arch=aarch64 ;; *) echo 'Supported: x86_64 and ARM64' >&2; exit 1 ;; esac
version=${COIL_VERSION:-latest}
if [ "$version" = latest ]; then
    base="https://github.com/$repo/releases/latest/download"
else
    case "$version" in *[!a-zA-Z0-9.-]*|'') echo 'Invalid COIL_VERSION' >&2; exit 1 ;; esac
    base="https://github.com/$repo/releases/download/$version"
fi
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT HUP INT TERM
curl -fLsS "$base/SHA256SUMS" -o "$work/SHA256SUMS"
asset=$(awk -v suffix="-$arch-$os.tar.gz" 'substr($2, length($2)-length(suffix)+1)==suffix {print $2}' "$work/SHA256SUMS")
case "$asset" in coil-v*-"$arch"-"$os".tar.gz) ;; *) echo 'Release has no matching archive' >&2; exit 1 ;; esac
case "$asset" in *[!a-zA-Z0-9.-]*) echo 'Invalid asset name' >&2; exit 1 ;; esac
curl -fLsS "$base/$asset" -o "$work/$asset"
(cd "$work"
    awk -v name="$asset" '$2==name' SHA256SUMS > selected.sha256
    if command -v sha256sum >/dev/null 2>&1; then sha256sum -c selected.sha256
    else shasum -a 256 -c selected.sha256; fi
    tar -xzf "$asset"
)
sh "$work/${asset%.tar.gz}/install.sh"
