# Coil binary release

## Install on macOS or Linux

The installer detects your operating system and architecture, verifies the
archive's SHA-256 checksum and installs the complete package under `~/.local`.
No compiler or root access is needed. Run these commands to install or update:

```sh
curl -fsSL https://raw.githubusercontent.com/scorredoira/coil/master/fork/install.sh -o /tmp/install-coil.sh
sh /tmp/install-coil.sh
```

## Add Coil to PATH permanently

Run the block for your shell once. It saves the PATH setting for future terminals
and also makes `coil` available in the current terminal.

**macOS with the default shell (zsh):**

```sh
printf '\nexport PATH="$HOME/.local/bin:$PATH"\n' >> ~/.zshrc
export PATH="$HOME/.local/bin:$PATH"
coil
```

**Linux with bash:**

```sh
printf '\nexport PATH="$HOME/.local/bin:$PATH"\n' >> ~/.bashrc
export PATH="$HOME/.local/bin:$PATH"
coil
```

If you use zsh on Linux, use the zsh block. The single quotes around `printf`'s
argument preserve `$HOME` and `$PATH` so they are evaluated when each new shell
starts. You only need to add the line once, not on every update.

## Install from a downloaded archive

Run `./coil` from this directory, or run `sh install.sh` to install under
`~/.local`. Put `~/.local/bin` on PATH. `COIL_PREFIX` selects another absolute
prefix. The installer keeps the runtime beside the binary and does not modify
your configuration or replace system vim. Running it again installs an update;
older directories under the prefix's `lib/coil` can be removed when no longer used.

Themes, queries and compiled syntax grammars are included. Language servers
and formatters are separate: `coil --health` lists what your machine has.
Git history requires the `git` command. Linux packages require glibc 2.35+
(e.g. Ubuntu 22.04+, Debian 12+); Alpine/musl is not supported by these builds.
macOS builds target macOS 14 or later, for Intel or Apple Silicon respectively.

The Linux `.run` is a single portable file. `chmod +x` it, rename it `coil`
and run it. It needs standard shell utilities, tar, gzip and sha256sum; no
FUSE, root access or compiler. On first use it verifies and extracts itself
under `${XDG_CACHE_HOME:-$HOME/.cache}/coil/portable`. Keep that cache on a
filesystem that permits execution. Delete it to reclaim space; the next run
extracts again. The file is portable between compatible machines of its architecture.

To use Coil for tools that ask for an editor, set `EDITOR=coil` and
`VISUAL=coil` in your shell profile. An optional `alias vim=coil` only affects
your interactive shell; Coil does not implement Vim's command-line interface.
