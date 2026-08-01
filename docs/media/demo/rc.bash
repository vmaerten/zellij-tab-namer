# Shell rcfile for the demo panes: a starship prompt and no history, so the
# capture shows the commands and nothing else. Copied into the throwaway $HOME
# by run.sh, so panes split open mid-recording get it too.
unset PROMPT_COMMAND
export HISTFILE=/dev/null
eval "$(starship init bash)"
