# Shared opening for the README tapes: settings, then a session started and
# granted off-camera. Sourced by demo.tape and demo-pipe.tape, which stay in Hide
# when it returns — each one sets its own scene before calling Show.

Set Shell bash
Set FontFamily "JetBrainsMono Nerd Font"
Set FontSize 20
Set Width 1240
Set Height 400
Set Padding 20
Set Theme "Catppuccin Frappe"
Set Framerate 15
Set PlaybackSpeed 1.0
Set TypingSpeed 60ms

Hide
Type "docs/media/demo/run.sh" Enter

# Grant the plugin's permissions. The throwaway $HOME means zellij's permission
# cache is always empty, so the grant prompt always comes up — but it renders
# nothing on screen, so there is no way to wait for it and no way to tell how
# long the session takes to get there on this machine. Hence several `y`: the one
# that falls inside the window grants, the others land in the shell and are wiped
# by the Ctrl+u below.
Sleep 4s
Type "y"
Sleep 2s
Type "y"
Sleep 2s
Type "y"
Sleep 2s
Type "y"
Sleep 2s
Type "y"
Sleep 2s
Type "y"
Sleep 2s
Type "y"

# Did it take? The tab bar answers: `Documents` only appears there, and only once
# the plugin is naming tabs. Waiting on it means a failed grant fails the
# recording here instead of quietly producing a GIF full of `Tab #1`.
Wait+Screen@30s /Documents/

Ctrl+u
Type "clear" Enter
Sleep 1s
