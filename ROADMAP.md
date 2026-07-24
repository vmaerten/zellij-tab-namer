# Roadmap

`zellij-tab-namer` is functional and released on `main` (README, LICENSE, ADRs,
21+ tests). What's left is polish and optional reach — nothing blocking.

## Polish

- [ ] **Demo media** — add a screenshot/GIF to the README (`docs/media/demo.png`).
- [ ] **Two accepted review findings** (shipped as-is, decide whether to fix):
  - *Tab-reorder race* — a `PaneUpdate` that reorders tabs *before* its matching
    `TabUpdate` can briefly map a pane to the wrong `tab_id` (the deferral only
    catches an absent tab position, not a reordered one). Verify against zellij's
    event-ordering guarantees; add deferral-on-reorder if it isn't guaranteed.
  - *Stranded git query* — an in-flight `git rev-parse` whose `RunCommandResult`
    is dropped blocks naming for that cwd forever (no timeout/fallback). Very low
    probability; add a fallback only if it ever bites in practice.

## Repo hygiene

- [ ] **PR #2 (`feat/configurable-format`)** — the `pane_count` config already
  landed on `main` via #4; close the PR (or rebase if anything is still unmerged).
- [ ] **CI** — run `cargo test` + `cargo wasm` on push.

## Reach (optional)

- [ ] **awesome-zellij** entry — for discoverability. Note how it differs from
  `zellij-tabula` (same idea) and that it pairs with `zellij-agent-activity`.
- [ ] **Prebuilt release** — ship the `.wasm` as a GitHub release asset so users
  can load it by URL without building.

See [`docs/adr/`](docs/adr) for the design rationale and [`CONTEXT.md`](CONTEXT.md)
for the glossary.
