<div align="center">

<h1>
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="logo_dark.svg">
  <source media="(prefers-color-scheme: light)" srcset="logo_light.svg">
  <img alt="Helix" height="128" src="logo_light.svg">
</picture>
</h1>

[![Build status](https://github.com/helix-editor/helix/actions/workflows/build.yml/badge.svg)](https://github.com/helix-editor/helix/actions)
[![GitHub Release](https://img.shields.io/github/v/release/helix-editor/helix)](https://github.com/helix-editor/helix/releases/latest)
[![Documentation](https://shields.io/badge/-documentation-452859)](https://docs.helix-editor.com/)
[![GitHub contributors](https://img.shields.io/github/contributors/helix-editor/helix)](https://github.com/helix-editor/helix/graphs/contributors)
[![Matrix Space](https://img.shields.io/matrix/helix-community:matrix.org)](https://matrix.to/#/#helix-community:matrix.org)

</div>

# This fork

Helix with the things you would otherwise leave the editor for: a sidebar that
stays on screen, search and replace across the project, git's changes, history
and blame, and a mouse that works where you point it. Everything else is helix
as it is; this fork sits on top of its `master`.

### A sidebar file tree

`Space t` shows or hides it, `Space T` focuses it. It follows the file you are
editing, and inside it `j`/`k` move, `Enter` opens, `a` creates, `r` renames,
`d` deletes. A click opens a row, the wheel scrolls, and dragging the line
between the tree and the editor resizes it: the width is remembered. Buffers
are tabs you can click, each with a cross that closes it.

![The file tree beside two open files](fork/screenshots/tree.png)

### Search and replace across the project

`Space /` opens a panel with a replace box and include/exclude filters (the
filters are remembered), switches for case, whole word, regex and preserving
case, and each result shows the line it matched. `Alt-a` replaces every match
on screen: the files are changed but left unsaved, and one undo takes it back.

![The global search panel with replace and filters](fork/screenshots/search.png)

### Git in the sidebar

The **Changes** tab (`Tab` inside the tree) lists what `git status` names, each
file with its letter: modified, added, deleted, renamed. The **Commits** tab
lists the history; `Enter` on a commit lists the files it touched, and the
editor shows the diff of whatever the cursor is on: the whole commit, a
directory, one file. `Esc` goes back. `Space H` narrows it to the current
file's history, following renames.

![Browsing a commit: its files in the sidebar, the diff of the one under the cursor on the right](fork/screenshots/commits.png)

### Who changed this line

`Space B` says who last changed the line under the cursor, when, and in which
commit; press it again to open that commit in the sidebar.

![Who changed the line under the cursor, in the status line](fork/screenshots/blame.png)

### Installing

```sh
git clone https://github.com/scorredoira/helix
cd helix
cargo install --path helix-term --locked
ln -Tsf $PWD/runtime ~/.config/helix/runtime
```

<br>

---
---

<div align="center">

**What follows is helix's own README.** The project is
[helix-editor/helix](https://github.com/helix-editor/helix).

</div>

---
---

![Screenshot](./screenshot.png)

A [Kakoune](https://github.com/mawww/kakoune) / [Neovim](https://github.com/neovim/neovim) inspired editor, written in Rust.

The editing model is very heavily based on Kakoune; during development I found
myself agreeing with most of Kakoune's design decisions.

For more information, see the [website](https://helix-editor.com) or
[documentation](https://docs.helix-editor.com/).

All shortcuts/keymaps can be found [in the documentation on the website](https://docs.helix-editor.com/keymap.html).

[Troubleshooting](https://github.com/helix-editor/helix/wiki/Troubleshooting)

# Features

- Vim-like modal editing
- Multiple selections
- Built-in language server support
- Smart, incremental syntax highlighting and code editing via tree-sitter

Although it's primarily a terminal-based editor, I am interested in exploring
a custom renderer (similar to Emacs) using wgpu.

Note: Only certain languages have indentation definitions at the moment. Check
`runtime/queries/<lang>/` for `indents.scm`.

# Installation

[Installation documentation](https://docs.helix-editor.com/install.html).

[![Packaging status](https://repology.org/badge/vertical-allrepos/helix-editor.svg?exclude_unsupported=1)](https://repology.org/project/helix-editor/versions)

# Contributing

Contributing guidelines can be found [here](./docs/CONTRIBUTING.md).

# Getting help

Your question might already be answered on the [FAQ](https://github.com/helix-editor/helix/wiki/FAQ).

Discuss the project on the community [Matrix Space](https://matrix.to/#/#helix-community:matrix.org) (make sure to join `#helix-editor:matrix.org` if you're on a client that doesn't support Matrix Spaces yet).

# Credits

Thanks to [@jakenvac](https://github.com/jakenvac) for designing the logo!
