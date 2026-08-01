# README demo, the decoration half. Regenerate with: docs/media/demo/record.sh
# Needs the wasm built first:                        task wasm
#
# Same staged session as demo.tape (docs/media/demo/boot.tape opens it); this one
# only shows the pipe API: a prefix put around the name the plugin computed, and
# what happens to it on a cd and on a split. Same caveat as demo.tape — takes fail
# often, so record through record.sh, which retries and protects the old GIF.

Output docs/media/demo-pipe.gif

Source docs/media/demo/boot.tape

Show

Sleep 1500ms
Type@25ms "# another tool can decorate this name" Enter
Sleep 1s

Type 'zellij pipe --name set_prefix --args "value=⚡ "' Enter
Sleep 2500ms

Type@25ms "# the name underneath is still the plugin's" Enter
Sleep 1s
Type "cd ~/Downloads" Enter
Sleep 2500ms
Type@25ms "# …so the prefix survives the cd" Enter
Sleep 2s

# Ctrl+p n splits a pane, Escape leaves the mode: the pane-count suffix appears
# and the prefix survives that too.
Ctrl+p
Sleep 500ms
Type "n"
Escape
Sleep 3s
