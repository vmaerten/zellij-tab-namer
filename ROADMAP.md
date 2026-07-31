# Roadmap

`zellij-tab-namer` is functional and released on `main` (README, LICENSE, ADRs,
23 tests), with CI and a tag-driven release pipeline. What's left is polish and
optional reach — nothing blocking.

## Polish

- [ ] **Demo media** — add a screenshot/GIF to the README (`docs/media/demo.png`).
      It's a visual plugin whose README shows nothing.
- [ ] **Two accepted review findings** (shipped as-is, decide whether to fix):
  - *Tab-reorder race* — a `PaneUpdate` that reorders tabs *before* its matching
    `TabUpdate` can briefly map a pane to the wrong `tab_id` (the deferral only
    catches an absent tab position, not a reordered one). Verify against zellij's
    event-ordering guarantees; add deferral-on-reorder if it isn't guaranteed.
  - *Stranded git query* — an in-flight `git rev-parse` whose `RunCommandResult`
    is dropped blocks naming for that cwd forever (no timeout/fallback). Very low
    probability; add a fallback only if it ever bites in practice.
- [ ] **`feat/configurable-format`** — the branch still carries two unmerged
  commits: a format system with `{name}` / `{process}` / `{pane_count}`
  placeholders. PR #2 was closed as superseded by the `pane_count` option that
  landed in #4, but the wider idea wasn't. Revive it or drop the branch.

## Repo hygiene

- [x] **CI** — fmt, clippy `-D warnings`, `cargo test` and the wasm build, on
  every push to `main` and every PR.
- [x] **Release pipeline** — pushing a `v*` tag verifies the tag against
  `Cargo.toml`, builds the wasm and publishes it as a Release asset, with notes
  assembled from the git-cliff changelog. `task release` drives the whole thing.
- [x] **Renovate** — non-major updates land in one weekly PR; `zellij-tile` is
  carved out and reviewed on its own, since it pins the plugin ABI.
- [x] **PR #2** closed.

## Reach (optional)

- [ ] **awesome-zellij** entry — for discoverability. Note how it differs from
  `zellij-tabula` (same idea) and that it pairs with `zellij-agent-activity`.

See [`docs/adr/`](docs/adr) for the design rationale and [`CONTEXT.md`](CONTEXT.md)
for the glossary.
