#!/usr/bin/env bash
# Records the README GIFs. Use this rather than calling vhs directly:
#
#   docs/media/demo/record.sh                      # both tapes
#   docs/media/demo/record.sh docs/media/demo.tape # just one
#
# Two reasons it exists. A take fails roughly half the time — the plugin comes up
# without ever getting its permissions and the tape's Wait+Screen refuses to
# record that — so this retries. And a failed vhs run still writes a truncated
# GIF over the good one, so this keeps a copy and puts it back when a tape gives
# up for good.
set -uo pipefail

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
cd "$root"

command -v vhs >/dev/null || { echo "vhs is not on PATH (brew install vhs)"; exit 1; }
[ -f target/wasm32-wasip1/release/zellij-tab-namer.wasm ] ||
  { echo "no wasm yet — run 'task wasm' first"; exit 1; }

tapes=("$@")
[ ${#tapes[@]} -eq 0 ] && tapes=(docs/media/demo.tape docs/media/demo-pipe.tape)

attempts=3
status=0

for tape in "${tapes[@]}"; do
  gif=$(grep -m1 '^Output ' "$tape" | awk '{print $2}')
  backup=""
  if [ -f "$gif" ]; then
    backup=$(mktemp)
    cp "$gif" "$backup"
  fi

  recorded=0
  for attempt in $(seq 1 $attempts); do
    if vhs "$tape" >/dev/null 2>&1; then
      echo "$gif — recorded ($(du -h "$gif" | cut -f1), attempt $attempt/$attempts)"
      recorded=1
      break
    fi
    echo "$gif — take $attempt/$attempts failed the tape's own check"
  done

  if [ $recorded -eq 0 ]; then
    status=1
    if [ -n "$backup" ]; then
      cp "$backup" "$gif"
      echo "$gif — gave up, previous GIF restored"
    else
      # nothing to restore, so don't leave the truncated GIF vhs wrote behind
      rm -f "$gif"
      echo "$gif — gave up, no GIF written"
    fi
  fi
  [ -n "$backup" ] && rm -f "$backup"
done

exit $status
