# Building Coil for another machine

Coil travels as **two things**: the `coil` binary and a `runtime/` directory beside
it. There is nothing else to install — no shared library, no interpreter, no
package manager. What the binary links is glibc and nothing more:

```
$ ldd target/opt/coil
libgcc_s.so.1   libm.so.6   libc.so.6
```

The runtime is **not** optional. The tree-sitter grammars are `.so` files opened
with `dlopen` the moment a file is loaded (`get_language`, in
`helix-loader/src/grammar.rs`), and the queries and themes are read from disk.
Without them the editor starts and shows plain text in one colour.

`runtime/` is looked for in this order (`helix-loader/src/lib.rs:42`):

1. `$CARGO_MANIFEST_DIR/../runtime` — only under `cargo run`
2. `~/.config/coil/runtime`
3. `$COIL_RUNTIME`
4. `<directory of the executable>/runtime`

The last one is what a deployed copy lands on, so a tree like
`/opt/coil/{coil,runtime}` needs no environment variable and no symlink.

## One rule: build on the platform you ship to

The grammars are compiled by `helix-term/build.rs`, which calls `cc` for the
cargo target. A mac `.dylib` therefore needs the macOS SDK and a linux `.so`
needs a linux toolchain: cross-compiling means dragging the other platform's SDK
onto this one. Two machines running the same two commands is less to maintain
than one machine running a cross toolchain, so that is what this doc does.

## Linux x86_64

```sh
cargo build --profile opt --locked
```

`opt` (`Cargo.toml:27`) is `release` plus fat LTO, one codegen unit and `strip`:
**21 MB**, against the 34 MB of `--release`. The binary lands in `target/opt/coil`.

The first build on a machine fetches and compiles every grammar and takes
minutes; later ones reuse `runtime/grammars`.

### The glibc floor

The binary and every grammar `.so` are dynamic against the glibc of the machine
that built them, so the server needs **glibc ≥ the builder's**. Ask it with
`ldd --version`:

| distro         | glibc |
| -------------- | ----- |
| Ubuntu 22.04   | 2.35  |
| Debian 12      | 2.36  |
| Ubuntu 24.04   | 2.39  |
| Debian 13      | 2.41  |

Built here (Ubuntu 24.04, 2.39) the tarball runs on Ubuntu 24.04+ and Debian 13,
and **not** on Debian 12 or Ubuntu 22.04. If an older server turns up, build
against an older glibc — either way the fork itself changes nothing:

```sh
# a container whose glibc is the floor you want (2.36 here); pin the image's
# rust to rust-toolchain.toml's channel
docker run --rm -v "$PWD":/src -w /src --user "$(id -u):$(id -g)" \
    -e CARGO_HOME=/src/target/.cargo-container -e CARGO_TARGET_DIR=target/container \
    rust:1.90-bookworm cargo build --profile opt --locked

# or cargo-zigbuild, which takes the floor as part of the target (zig is already
# on this box): cargo install cargo-zigbuild
cargo zigbuild --profile opt --locked --target x86_64-unknown-linux-gnu.2.28
```

A static musl build is **not** an option: a fully static binary cannot `dlopen`,
which is how every grammar is loaded.

An arm64 server is the same recipe run on an arm64 linux box
(`aarch64-unknown-linux-gnu`).

## macOS, Apple Silicon

On the Mac, once:

```sh
xcode-select --install
```

then the same build — native is already `aarch64-apple-darwin`, so there is no
target to pass:

```sh
cargo build --profile opt --locked
```

There is no glibc question here, but the SDK plays the same role: build on the
oldest macOS you need to serve, because a newer SDK can leave the grammars
asking for that newer macOS.

A tarball **downloaded** onto the Mac instead of built there is quarantined by
Gatekeeper — the build is unsigned and unnotarized, and that is fine for one's
own editor:

```sh
xattr -dr com.apple.quarantine /opt/coil
```

## Packaging it

`runtime/grammars/sources` is 2,6 GB of grammar repos the build cloned; only the
built `.so` beside them ship. All 302 of those are 248 MB, and the languages
actually opened are 34 of them:

```sh
GRAMMARS="bash comment css diff dockerfile git-config git-rebase gitattributes
gitcommit gitignore go gomod gotmpl html javascript jsdoc json lua make markdown
markdown_inline nginx nix python regex rust scss sql toml tsx typescript vim xml
yaml"

D=target/dist          # under target/, so git never sees it
rm -rf $D && mkdir -p $D/coil/runtime/grammars
cp target/opt/coil $D/coil/
cp -r runtime/queries runtime/themes runtime/tutor $D/coil/runtime/
for g in $GRAMMARS; do cp "runtime/grammars/$g.so" $D/coil/runtime/grammars/; done

tar -C $D -cf - coil | zstd -19 -T0 \
    -o "$D/coil-$(git describe --tags)-linux-x86_64.tar.zst"
```

That is **44 MB** unpacked and **8,2 MB** in the tarball. Queries (16 MB) and
themes (3,5 MB) go whole because they are text and compress to nothing, so
adding a language later is just its `.so`; a grammar name is the file's name, and
it is not always the language's — git commits highlight through `gitcommit.so`.
To ship everything instead, copy `runtime/grammars/*.so` and pay 248 MB on disk
for languages nobody opens.

## Installing it there

```sh
curl -sL <url>/coil-linux-x86_64.tar.zst | tar -x --zstd -C /opt
ln -sf /opt/coil/coil /usr/local/bin/coil

coil --health | grep -i runtime   # the runtime dirs it found, best first
coil --health go                  # "Tree-sitter parser ✓" = that grammar shipped
```

Updating is those same lines over the top — nothing under `/opt/coil` holds
state. `~/.config/coil/config.toml` is the user's and never travels in the
tarball.

One trap: a `~/.config/coil/runtime` on that machine (the symlink the README's
from-source install makes) **wins** over `/opt/coil/runtime`, being priority 2
against 4. On a box that once had a clone, remove it or the old grammars are the
ones that load.

## What the machine still needs

- **`git` on `PATH`** — the sidebar's Changes, History and blame tabs run it.
- **A language server per language**, installed separately; `coil --health <lang>`
  says what it looked for and found. Nothing in the tarball needs one.

## Gotchas

- `HELIX_DISABLE_AUTO_GRAMMAR_BUILD=1` skips the grammar fetch and compile
  (`helix-term/build.rs:6`) — handy for a quick rebuild, but the first build on a
  machine has to run without it.
- `.github/workflows/release.yml` is still upstream's: it builds `hx`, names its
  artifacts `helix-*`, and its AppImage wrapper exports `HELIX_RUNTIME`, which
  this fork's loader does not read (it reads `COIL_RUNTIME`). Nothing here
  depends on that workflow.
- `--locked` keeps `Cargo.lock` as committed; without it a build can quietly move
  a dependency.
