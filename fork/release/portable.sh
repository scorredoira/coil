#!/bin/sh
set -eu
# One transferable file; its verified runtime is cached once per release.
digest=@PAYLOAD_SHA256@
cache_root=${XDG_CACHE_HOME:-"$HOME/.cache"}/coil/portable
cache="$cache_root/$digest"
if [ ! -f "$cache/.ready" ]; then
    umask 077
    mkdir -p "$cache_root"
    staging=$(mktemp -d "$cache_root/.unpack.XXXXXXXX")
    trap 'rm -rf "$staging"' EXIT HUP INT TERM
    line=$(awk '/^__COIL_PAYLOAD__$/ { print NR + 1; exit }' "$0")
    tail -n +"$line" "$0" > "$staging/payload.tar.gz"
    printf '%s  %s\n' "$digest" "$staging/payload.tar.gz" | sha256sum -c - >/dev/null
    mkdir "$staging/app"
    tar -xzf "$staging/payload.tar.gz" -C "$staging/app"
    touch "$staging/app/.ready"
    # Another process may have installed the identical cache while we unpacked.
    if ! mv -T "$staging/app" "$cache" 2>/dev/null; then
        [ -f "$cache/.ready" ] || exit 1
    fi
    rm -rf "$staging"
    trap - EXIT HUP INT TERM
fi
exec "$cache/coil" "$@"
__COIL_PAYLOAD__
