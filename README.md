<div align="center">

<img alt="" height="128" src="fork/logo.svg">

# Coil

A fork of [Helix](https://github.com/helix-editor/helix)

</div>

Helix's modal editing, with the things you would otherwise leave the editor
for: a sidebar that stays on screen, search and replace across the project,
git's changes, history and blame, Markdown as it reads, and a mouse that works
where you point it.
Everything else is Helix as it is — its keys, its language servers, its
tree-sitter — and Coil follows its `master`.

![The file tree beside two open files](fork/screenshots/tree.png)

## A sidebar file tree

The editor's own keys — `Ctrl-q`, `Ctrl-s`, `F12` — work while the sidebar has
the focus; only its plain keys are its own.

`Space t` shows or hides it, `Space T` focuses it; started on a file
(`coil foo.ts`), the editor opens without it. It follows the file you are
editing, and inside it `j`/`k` move, `Enter` opens, `a` creates, `r` renames,
`d` deletes. A click opens a row, the wheel scrolls, and dragging the line
between the tree and the editor resizes it: the width is remembered.

## What git sees changed

The **Changes** tab (`Tab` inside the sidebar) lists what `git status` names,
each file with its letter: modified, added, deleted, renamed. `Enter` opens
it, and the gutter marks the lines that changed.

![The Changes tab: a file modified, one deleted, one added](fork/screenshots/changes.png)

## The history, and each commit's diff

The **Commits** tab lists the history. A click on a commit shows its whole
diff in the editor; `Enter` or a double click lists the files it touched, the
diff then following whatever the cursor is on: a directory, one file. `Esc`
goes back.

![The history in the sidebar, the diff of the selected commit on the right](fork/screenshots/commits.png)

`Space H` narrows the history to the current file, following renames.

![The history of one file](fork/screenshots/history.png)

## Who changed this line

`Space B` says who last changed the line under the cursor, when, and in which
commit; press it again to open that commit in the sidebar.

![Who changed the line under the cursor, in the status line](fork/screenshots/blame.png)

## Search and replace across the project

`Space Space` (or `Space /`) opens a panel with a replace box and
include/exclude filters (the filters are remembered), switches for case, whole
word, regex and preserving case — the panel's border says their keys — and
each result shows the line it matched. `Alt-a` replaces every match on
screen: the files are changed but left unsaved, and one undo takes it back.

![The search panel with replace and filters](fork/screenshots/search.png)

## Search and replace in this file

`Ctrl-f` opens the same panel on the file you are editing, its name on the
border: the matches listed by line, the file beside them, the same switches.
`Alt-a` replaces every match in it, unsaved, and one undo takes it back.
`Ctrl-g` asks for a line number and goes there, following it as you type.

![The search panel on one file, its matches by line](fork/screenshots/search-file.png)

## Markdown, as it reads

`Ctrl-Shift-m` draws the Markdown file you are editing beside it: headings,
lists and tasks, quotes and GitHub's alerts, tables lined up, code highlighted,
all reflowed to the panel. It redraws as you type and keeps to the part of the
file on screen; the wheel over it scrolls it on its own until the file moves.

![A README and its preview side by side](fork/screenshots/preview.png)

## Tabs, splits and the mouse

Buffers are tabs you can click, each with a cross that closes it. Splits
resize by dragging the line between two side by side, or the status line
between two stacked. In the pickers a click previews a row and a double click
opens it.

![Two files side by side, each a tab you can click](fork/screenshots/splits.png)

## Ready as installed

Coil needs no configuration file. On top of Helix's keys it brings the ones
you already know:

| Key | Does |
|---|---|
| `Ctrl-a` | Select everything |
| `Shift` + an arrow | Select while typing (`Ctrl-Shift` for whole words) |
| `Backspace` | Delete the selection, or the character before the cursor |
| `Delete` | Delete the selection, or the character under the cursor |
| `Ctrl-c` | Copy the selection to the system clipboard |
| `Ctrl-s` | Save |
| `Ctrl-f` | Search and replace in this file |
| `Ctrl-g` | Go to a line |
| `Alt-z` | Wrap long lines, or stop |
| `Ctrl-Shift-m` | Show or hide the Markdown preview |
| `F12` / `Shift-F12` | Go to the definition / to the references |
| `F2` | Rename the symbol |
| `F8` | Next diagnostic |
| `Ctrl-q` | Save everything and quit |
| `Ctrl-,` | Settings |
| `Space Space` | Search and replace across the project (`Space /` too) |

On a Mac `Cmd-c` copies and `Cmd-s` saves as well, and a `Cmd` or `Ctrl` key
nothing is bound to never types its letter.

While typing, a selection behaves as it does in any other editor: `Shift` with
an arrow, `Home` or `End` grows it from the cursor, the mouse drags one, typing
replaces it and `Backspace` or `Delete` removes it. None of that leaves insert
mode, and none of it touches how selections work outside it, where they are what
the commands act on.

Your work is saved for you: leave a file — another tab, another window, or the
terminal itself losing focus — and it is written to disk. Closing a tab writes
it too, and `Ctrl-q` writes everything before it quits. Saving while you type
(`auto-save.after-delay`) stays off.

The only thing that cannot be written for you is a buffer with no file behind
it, and that one asks, in the middle of the screen: arrows or `Tab` walk the
answers, `Enter` takes the one in focus, a click takes the one it lands on, and
`Escape` always answers no.

![The question asked before quitting with something unsaved](fork/screenshots/quit.png)

And the editor starts the way you would set it up: long lines wrap,
indentation guides show, open files are tabs, the cursor is a bar while
typing, the mode colours the status line, the theme follows the terminal's
light or dark background, the file picker shows ignored files too, and
completion offers only what the language server suggests.

Any of it can be changed in `~/.config/coil/config.toml`, which is laid over
these defaults: write only what you want different.

Copying over SSH reaches your own machine's clipboard when the terminal
supports OSC 52: Ghostty, kitty and WezTerm do, iTerm2 once it is allowed in
its settings.

## Settings on the screen

`Ctrl-,` opens the handful of settings a newcomer reaches for, each showing what
it is set to now: wrapping, saving, line numbers, tabs, the mouse. Up and down
walk them, `Space` or a click changes the one in focus, and the change applies at
once and is written to `~/.config/coil/config.toml` as it is made — only the line
it touches, so the rest of the file, comments included, stays as you wrote it.

![The settings on screen, over the file being edited](fork/screenshots/settings.png)

## Coming from Helix

The keys are Helix's, and so is its documentation:
[docs.helix-editor.com](https://docs.helix-editor.com/) applies as it is —
except for the keys above: `Ctrl-c` copies (comment with `Space c`), `Ctrl-a`
selects everything (incrementing a number keeps no key), `Ctrl-s` saves and
`Ctrl-f` searches the file (a page down is `PageDown`) — and that while typing
a selection is replaced by what you type, which Helix leaves alone. Coil keeps
its own files, so the two never mix:

| | Helix | Coil |
|---|---|---|
| Command | `hx` | `coil` |
| Configuration | `~/.config/helix/` | `~/.config/coil/` |
| Per project | `.helix/` | `.coil/` |
| Runtime override | `HELIX_RUNTIME` | `COIL_RUNTIME` |

To bring your configuration along: `cp -r ~/.config/helix ~/.config/coil`.

## Installing

Coil is built from source. It needs [Rust](https://rustup.rs), git and a C
compiler, which builds the tree-sitter grammars (on macOS,
`xcode-select --install`).

```sh
git clone https://github.com/scorredoira/coil
cd coil
cargo install --path helix-term --locked
mkdir -p ~/.config/coil
ln -sfn "$PWD/runtime" ~/.config/coil/runtime
```

The first build fetches and compiles every grammar, so it takes a few
minutes. `coil` lands in `~/.cargo/bin`, which rustup puts on your `PATH`;
`coil --health` says where it reads its configuration from. The `runtime`
link keeps the clone as the source of the grammars and themes, so leave it
where it is.

To update: `git pull` in the clone, then the `cargo install` line again.

## Following Helix

Coil is rebased onto Helix's `master` regularly, so its fixes and features
arrive here too. Problems with Coil belong in
[this repository's issues](https://github.com/scorredoira/coil/issues), not
in Helix's.

## License

[MPL-2.0](LICENSE), like Helix. The editor underneath is the work of
[Helix's contributors](https://github.com/helix-editor/helix/graphs/contributors).
