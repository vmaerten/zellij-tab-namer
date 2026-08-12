#!/usr/bin/env bash
# Renders the GitHub Release body for a tag, on stdout. Used by the Release
# workflow and by `task release-notes`.
set -euo pipefail

tag="${1:-}"
if [ -z "$tag" ]; then
  echo "usage: $0 <tag>   e.g. $0 v0.1.0" >&2
  exit 2
fi
ver="${tag#v}"

root="$(cd "$(dirname "$0")/.." && pwd)"
tmpl="$root/.github/release-notes.md.tmpl"

repo_url="${REPO_URL:-$(git -C "$root" remote get-url origin |
  sed -e 's|^git@github\.com:|https://github.com/|' -e 's|\.git$||')}"

zellij_tile="$(grep -A2 'name = "zellij-tile"' "$root/Cargo.lock" |
  grep version | head -n1 | cut -d'"' -f2)"

changes="$(mktemp)"
trap 'rm -f "$changes"' EXIT

awk -v ver="$ver" '
  $0 ~ "^## \\[" ver "\\]"              { found = 1; next }
  found && (/^## \[/ || /^\[[^]]+\]: /) { exit }
  found                                 { print }
' "$root/CHANGELOG.md" >"$changes"

if [ -z "$(tr -d '[:space:]' <"$changes")" ]; then
  echo "$0: CHANGELOG.md has no section for $ver, run 'task changelog NEW=$ver'" >&2
  exit 1
fi

notes="$(sed \
  -e "s|{{ZELLIJ_TILE}}|$zellij_tile|g" \
  -e "s|{{REPO_URL}}|$repo_url|g" \
  -e "s|{{TAG}}|$tag|g" \
  -e "/{{CHANGES}}/r $changes" \
  -e "/{{CHANGES}}/d" \
  "$tmpl")"

if printf '%s\n' "$notes" | grep -q '{{'; then
  echo "$0: unsubstituted placeholder in the rendered notes:" >&2
  printf '%s\n' "$notes" | grep -n '{{' >&2
  exit 1
fi

printf '%s\n' "$notes"
