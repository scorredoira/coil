#!/usr/bin/env bash
# Takes the screenshots in fork/screenshots from the built editor
# (target/release/hx), driving it inside tmux over a throwaway clone of this
# repository, so each picture shows the fork exactly as it is.
#
# Needs tmux, git and python3 with Pillow. Build first: cargo build --release
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(dirname "$here")
hx="$repo/target/release/hx"
out="$here/screenshots"
socket=fork-screenshots
work=$(mktemp -d)
trap 'tmux -L "$socket" kill-server 2>/dev/null || true; rm -rf "$work"' EXIT

if [[ ! -x "$hx" ]]; then
	echo "$hx is not built: cargo build --release" >&2
	exit 1
fi

git clone --quiet --shared "$repo" "$work/demo"
mkdir -p "$work/data" "$work/config/helix" "$out"
cp "$here/demo-config.toml" "$work/config/helix/config.toml"
# The tree as wide as a drag of its separator would leave it, remembered.
mkdir -p "$work/data/helix"
echo 'width = 40' >"$work/data/helix/file-tree.toml"

# Something for the Changes tab to list: a file edited, one added, one deleted.
demo="$work/demo"
sed -i 's|/// Rows the last render had room for; what paging and scrolling are measured against.|/// How many rows fit on screen, measured again on every render.|' \
	"$demo/helix-term/src/ui/file_tree.rs"
printf '\n## Notes\n\nA line added to the readme.\n' >>"$demo/README.md"
printf 'pub struct Minimap {\n    pub width: u16,\n}\n' >"$demo/helix-term/src/ui/minimap.rs"
rm "$demo/theme.toml"

# Starts the editor on the given files, alone on a fresh screen.
start() {
	tmux -L "$socket" kill-server 2>/dev/null || true
	sleep 0.3
	tmux -L "$socket" -f /dev/null new-session -d -s shot -x 132 -y 36 -c "$demo" \
		"env XDG_DATA_HOME=$work/data XDG_CONFIG_HOME=$work/config \
		HELIX_RUNTIME=$repo/runtime COLORTERM=truecolor $hx $*; sleep 600"
	sleep 2
}

# Sends keys as tmux names them (Space, Enter, M-h...).
keys() {
	tmux -L "$socket" send-keys -t shot "$@"
	sleep 0.4
}

# Types text as it is.
typed() {
	tmux -L "$socket" send-keys -t shot -l "$1"
	sleep 0.4
}

# Captures the screen, once whatever it waits on (git, the search) has answered.
shoot() {
	sleep "${2:-1.5}"
	tmux -L "$socket" capture-pane -t shot -p -e -N >"$work/$1.ansi"
	python3 "$here/render.py" "$work/$1.ansi" "$out/$1.png"
	echo "$out/$1.png"
}

# The position of a commit in the history list, found by its subject.
commit_row() {
	git -C "$demo" log --format=%s | grep -n -m1 -F "$1" | cut -d: -f1
}

# Moves the tree's cursor down n rows.
down() {
	for ((i = 1; i < $1; i++)); do
		tmux -L "$socket" send-keys -t shot j
	done
	sleep 0.4
}

start helix-term/src/ui/editor.rs helix-term/src/ui/file_tree.rs
keys Space T
shoot tree

keys Tab
keys Home
down 2
keys Enter
keys 3 2 G
shoot changes

keys Space T
keys Tab
keys Home
down "$(commit_row "bufferline: each tab carries a cross")"
keys Enter
keys j j
shoot commits 2

start helix-term/src/ui/file_tree.rs
keys Space H
shoot history 2

start helix-term/src/ui/file_tree.rs
keys 6 8 G
keys Space B
shoot blame 2

start helix-term/src/ui/editor.rs
keys Space /
typed set_status
keys M-h M-i
keys Tab
typed set_message
keys Tab
typed 'helix-term/**'
shoot search 2
