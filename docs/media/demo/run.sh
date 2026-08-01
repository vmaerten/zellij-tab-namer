#!/usr/bin/env bash
# Launches the demo session. Called from demo.tape — running it by hand just
# shows you what the GIF records.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
wasm="$root/target/wasm32-wasip1/release/zellij-tab-namer.wasm"

[ -f "$wasm" ] || { echo "no wasm at $wasm — run 'task wasm' first"; exit 1; }
for tool in starship eza; do
  command -v "$tool" >/dev/null ||
    { echo "$tool is not on PATH — the demo prompt and listing need it (brew install $tool)"; exit 1; }
done

# The session runs in a throwaway $HOME: nothing personal reaches the screen and
# it replays the same on any machine.
#
# `pwd -P` matters: on macOS mktemp hands back /var/folders/…, a symlink to
# /private/var/folders/…. Zellij reports the resolved cwd, so an unresolved $HOME
# never compares equal to it and the home tab would read `home` instead of `~`.
tmp=$(cd "$(mktemp -d)" && pwd -P)
home="$tmp/home"
mkdir -p "$home/code" "$home/Downloads" "$home/Documents" \
         "$home/code/dotfiles/nvim" "$home/.config"

# The repo the demo works in is this repo, cloned: the git log, the tree and the
# branch in the prompt are all real, and the tab is named after a real checkout.
git clone --quiet --local --branch main "$root" "$home/code/zellij-tab-namer"
git -C "$home/code/zellij-tab-namer" remote set-url origin \
  https://github.com/vmaerten/zellij-tab-namer.git
git -C "$home/code/dotfiles" -c init.defaultBranch=main init --quiet

# Every pane in the demo is bash, including the ones split open mid-recording, so
# the prompt lives in the fake home rather than in the layout.
cp "$here/rc.bash" "$home/.bashrc"
cp "$here/starship.toml" "$home/.config/starship.toml"

# Note: pre-writing zellij's permission cache here to skip the grant prompt looks
# tempting, and it does work — outside vhs. Started in a plain pty it named tabs
# 10 times out of 10; recorded through vhs, 0 takes out of 3, every tab stuck on
# `Tab #1`. Something about the vhs terminal makes an already-granted plugin come
# up silent, so the recording goes through the prompt instead: the tape answers it
# with a few `y` in its Hide block.

# The config and layout carry absolute paths, so they are rendered into a temp
# dir that doubles as the layout_dir.
sed -e "s|@WASM@|$wasm|" -e "s|@HERE@|$tmp|" "$here/config.kdl.in" > "$tmp/config.kdl"
sed -e "s|@HOME@|$home|" "$here/layout.kdl" > "$tmp/layout.kdl"

# Its own socket dir, not just its own $HOME. Zellij keeps session sockets under
# $TMPDIR, outside the fake home, and a demo server outlives the recording: when
# vhs closes the terminal the client is logged out and the server tears its
# plugins down ("Bye from plugin") but keeps running. A shared socket dir lets the
# next run re-attach to that one and come up plugin-less — a recording of
# `Tab #1`. A per-run dir keeps every take on a server of its own; the pkill just
# stops those servers from piling up across takes.
# -9: a demo server ignores SIGTERM here, and a plain pkill leaves one behind per
# take until a dozen of them are running.
pkill -9 -f "zellij --server .*/demo\$" >/dev/null 2>&1 || true

exec env HOME="$home" ZELLIJ_SOCKET_DIR="$tmp/sockets" \
  zellij --config "$tmp/config.kdl" -s demo
