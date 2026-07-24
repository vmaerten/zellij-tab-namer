# Effects seam with a wasm-gated adapter

The plugin's logic is a pure core — `init`/`handle`/`handle_pipe` take zellij events and return `Vec<Effect>` — and the `ZellijPlugin` impl is a thin adapter, gated to `#[cfg(target_arch = "wasm32")]`, that executes those effects against the host. The zellij host functions are extern symbols that only exist on wasm, so the gate makes the seam linker-enforced: a host call inside the core fails the native build. This is what makes the plugin testable at all — the timing races (shared-cwd waiters, stale git results) are exercised as native unit tests instead of inside a live zellij session.

## Consequences

- `.cargo/config.toml` no longer forces `wasm32-wasip1`: native is the default so bare `cargo test`/`cargo check`/IDE tooling work; the plugin is built with the `cargo wasm` alias. Don't "fix" this back.
- Native builds need the empty `#[cfg(not(target_arch = "wasm32"))] fn main()` — the real entrypoint comes from `register_plugin!` on wasm.
- The `Effect::RunGit` variant carries the correlation `context` so the round-trip protocol (build on send, parse on `RunCommandResult`) stays entirely core-side; the adapter only knows the constant command line.
