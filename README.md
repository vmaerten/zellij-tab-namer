# zellij-tab-namer

A [Zellij](https://zellij.dev) plugin that **names each tab after the git repository — or the
folder — its panes are in**, automatically, as you `cd` around. No more `Tab #1`, `Tab #2`: your
tabs read `myrepo`, `dotfiles`, `~`. Plus a small **decoration Pipe API** so other tools can add a
prefix or suffix (a status symbol, a badge) *around* that name — without ever fighting the plugin
for it.

<p align="center">
  <img alt="Zellij plugin" src="https://img.shields.io/badge/zellij-plugin-8A2BE2">
  <img alt="Built with Rust" src="https://img.shields.io/badge/built%20with-Rust-000000?logo=rust">
  <img alt="Tests" src="https://img.shields.io/badge/tests-21%20passing-brightgreen">
  <img alt="License" src="https://img.shields.io/badge/license-MIT-blue">
</p>

<p align="center">
  <a href="#what-is-it">What is it?</a> ·
  <a href="#install">Install</a> ·
  <a href="#configuration">Configuration</a> ·
  <a href="#decoration-pipe-api">Decoration Pipe API</a> ·
  <a href="#how-naming-works">How naming works</a>
</p>

<!-- Drop a screenshot here: docs/media/demo.png -->

## What is it?

In a multi-tab Zellij session, the default `Tab #N` names tell you nothing. This plugin watches each
tab's active pane and renames the tab after where it actually is:

- inside a git repo → the **repo name** (`myrepo`),
- otherwise → the **folder name** (`Downloads`),
- your home directory → `~`.

It names tabs **at session start, on new tabs, and on restore** — not only after you `cd` — so a
freshly opened session is already labelled. And it exposes a **decoration Pipe API** (`set_prefix` /
`set_suffix` / `clear_*`) so other tools can wrap a symbol around the name.
[`claude-tab-indicator`](https://github.com/vmaerten/claude-tab-indicator) uses it to show live
Claude Code activity (`⚡ myrepo`).

## Highlights

- **Automatic, cwd-driven names** — git repo, else folder, else `~`.
- **Named from the first frame** — session start, new tab, and layout restore, via a pull-based
  discovery query, not just on `cd`.
- **A decoration seam for other plugins** — one plugin owns the tab name; everyone else decorates it
  over a pipe. No rename wars.
- **Optional pane-count suffix** — e.g. `myrepo (3)` when a tab holds several panes.
- **Correct under churn** — handles symlinked cwds, tabs opened/closed/moved, and out-of-order
  Zellij events; de-duplicates in-flight `git` queries.
- **Pure, tested core** — the naming logic is a host-free state machine with 21 native unit tests
  (see [Architecture](#architecture)).

## Requirements

- **Zellij ≥ 0.44.3** (`zellij --version`).
- **`git`** on `PATH` — optional; without it, tabs fall back to folder names (or set
  `git_detection false`).

## Install

```sh
rustup target add wasm32-wasip1
cargo wasm   # -> target/wasm32-wasip1/release/zellij-tab-namer.wasm
cp target/wasm32-wasip1/release/zellij-tab-namer.wasm ~/.config/zellij/plugins/
```

Load it in `~/.config/zellij/config.kdl`:

```kdl
load_plugins {
    "file:~/.config/zellij/plugins/zellij-tab-namer.wasm";
}
```

Restart Zellij and grant the plugin's permissions when prompted. Tabs start renaming themselves.

## Configuration

Pass options as a config block on the plugin. Both are optional:

```kdl
load_plugins {
    "file:~/.config/zellij/plugins/zellij-tab-namer.wasm" {
        git_detection true            // name tabs after the git repo (default: true)
        pane_count " ({pane_count})"  // suffix shown only when a tab has >1 pane (default: off)
    }
}
```

| Option | Default | Effect |
|---|---|---|
| `git_detection` | `true` | Name a tab after its git repo. Set `false` to always use the folder name and skip running `git`. |
| `pane_count` | *(unset)* | A format string appended when a tab holds more than one pane. `{pane_count}` is replaced with the count. Unset = no suffix. |

## Decoration Pipe API

Other tools decorate a tab by sending a pipe message. The rendered tab name is always composed as:

```
{prefix}{base name}{suffix}{pane-count suffix}
```

Decorations are stored per tab and re-applied whenever the base name is recomputed, so they survive
a `cd`.

| Pipe name | Args | Effect |
|---|---|---|
| `set_prefix` | `value`, `tab_id` | Set the tab's prefix to `value`. |
| `set_suffix` | `value`, `tab_id` | Set the tab's suffix to `value`. |
| `clear_prefix` | `tab_id` | Remove the prefix. |
| `clear_suffix` | `tab_id` | Remove the suffix. |
| `clear_all` | `tab_id` | Remove both. |

`tab_id` is a **stable Zellij tab id**; omit it or pass `active` to target the currently active tab.

```sh
# Prefix the active tab
zellij pipe --name set_prefix --args "value=🔨 "

# Suffix a specific tab by id
zellij pipe --name set_suffix --args "tab_id=3,value= [build]"

# Clear it
zellij pipe --name clear_all --args "tab_id=3"
```

## How naming works

The **base name** of a tab comes from the cwd of the pane that speaks for it (the focused pane of
the visible layer, with sensible fallbacks): the git repository's top-level folder name, else the
directory's own name, else `~` for `$HOME`.

Tabs are named without waiting for a `cd`: on session start, new-tab, and restore, the plugin issues
a one-shot **discovery query** for the tab's cwd. Afterward it follows `cd` via Zellij's
`CwdChanged`, and resolves git roots with a cached `git rev-parse` (in-flight queries for the same
cwd are de-duplicated; symlinked cwds resolve via a root-alias cache).

Known, deliberate limitation: a `git init` in a folder the session has already visited isn't noticed
until the session restarts (the "not a repo" verdict is cached and never invalidated). See the ADRs.

## Architecture

The plugin is split into a **pure core** and a thin **wasm-gated adapter**:

- the core (`init` / `handle` / `handle_pipe`) takes Zellij events and returns a `Vec<Effect>` — no
  host calls;
- the `#[cfg(target_arch = "wasm32")]` adapter is the only place effects touch the Zellij host.

Because the host functions only exist on wasm, the split is **linker-enforced**: a host call in the
core fails the native build. That's what makes the timing-sensitive naming logic testable as
ordinary `cargo test` unit tests instead of in a live session.

- [`CONTEXT.md`](CONTEXT.md) — the domain glossary (base name vs rendered name, decoration, waiter,
  discovery query…).
- [`docs/adr/`](docs/adr) — the architecture decision records.

## Development

```sh
cargo test    # pure core, host-native, no zellij needed (21 tests)
cargo wasm    # release build -> target/wasm32-wasip1/release/zellij-tab-namer.wasm
```

## License

MIT — see [`LICENSE`](LICENSE).
