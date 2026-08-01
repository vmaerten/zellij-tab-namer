# README demo, the naming half. Regenerate with:  docs/media/demo/record.sh
# Needs the wasm built first:                      task wasm
#
# The session runs in a throwaway $HOME seeded by docs/media/demo/run.sh — which
# clones this repo into it, so the tree and the prompt are real. Only the
# keystrokes are scripted.
#
# Record through record.sh rather than calling vhs directly: takes fail often (the
# plugin comes up without its permissions and the Wait+Screen in boot.tape refuses
# to record that), and a failed vhs run still writes a truncated GIF over the good
# one. record.sh retries and puts the old GIF back.
#
# The narration is typed as shell comments. `cd src` leaving the tab name alone is
# the rule worth seeing here, and it is a non-event: without a line saying so, it
# reads as nothing happening at all.

Output docs/media/demo.gif

Source docs/media/demo/boot.tape

# Still hidden: put something on screen first, so the very first frame is a
# working session with four named tabs rather than an empty pane being typed into.
Type "eza --tree src docs/adr" Enter
Sleep 1500ms
Show

Sleep 2s
Type@25ms "# this tab is named after the repo, not the folder" Enter
Sleep 1200ms

Type "cd src" Enter
Sleep 2s
Type@25ms "# …and it did not move — the repo wins" Enter
Sleep 2500ms

Type "cd ~/Downloads" Enter
Sleep 2s
Type@25ms "# no repo here, so the folder names the tab" Enter
Sleep 3s
