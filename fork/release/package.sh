#!/usr/bin/env bash
set -euo pipefail
# Run at the repository root after a native release build.
platform=${1:?platform required}
version=${2:?version required}
[[ "$version" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || exit 1
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
name="coil-$version-$platform"
mkdir -p "$stage/$name/runtime" dist
cp target/release/coil LICENSE "$stage/$name/"
cp -R runtime/queries runtime/themes runtime/grammars "$stage/$name/runtime/"
rm -rf "$stage/$name/runtime/grammars/sources"
cp fork/release/install.sh "$stage/$name/"
cp fork/release/README.md "$stage/$name/README.md"
tar -czf "dist/$name.tar.gz" -C "$stage" "$name"
if [[ "$platform" == *-linux ]]; then
    payload="$stage/payload.tar.gz"
    tar -czf "$payload" -C "$stage/$name" .
    digest=$(sha256sum "$payload" | cut -d' ' -f1)
    sed "s/@PAYLOAD_SHA256@/$digest/g" fork/release/portable.sh > "dist/$name.run"
    cat "$payload" >> "dist/$name.run"
    chmod +x "dist/$name.run"
fi
