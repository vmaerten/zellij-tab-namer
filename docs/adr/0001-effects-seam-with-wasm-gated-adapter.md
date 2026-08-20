# Effects seam with a wasm-gated adapter

The plugin's logic is a pure core (`init`/`handle`/`handle_pipe` take zellij events and return `Vec<Effect>`) and the `ZellijPlugin` impl is a thin adapter, gated to `#[cfg(target_arch = "wasm32")]`, that executes those effects against the host. This is what makes the plugin testable at all: the timing races (shared-cwd waiters, stale git results) are exercised as native unit tests instead of inside a live zellij session.

Until `zellij-tile` 0.44, the seam enforced itself: the host functions were extern symbols that only existed on wasm, so a host call inside the core failed to link natively. 0.45 stubs `host_run_plugin_command` out to a no-op on native, and that guarantee is gone: the offending call now compiles, writes JSON to a stdout nobody reads, and the whole suite stays green while the plugin misbehaves on wasm. `clippy.toml` replaces it: the host functions are listed as `disallowed-methods`, `task ci` already runs `clippy -D warnings`, and `drive()` carries the only allow.

The list is nominative, so a host function added in a later `zellij-tile` is not covered until someone adds it. Extend `clippy.toml` when the adapter learns a new host call.

## Consequences

- `.cargo/config.toml` no longer forces `wasm32-wasip1`: native is the default so bare `cargo test`/`cargo check`/IDE tooling work; the plugin is built with the `cargo wasm` alias. Don't "fix" this back.
- Native builds need the empty `#[cfg(not(target_arch = "wasm32"))] fn main()` — the real entrypoint comes from `register_plugin!` on wasm.
- The `Effect::RunGit` variant carries the correlation `context` so the round-trip protocol (build on send, parse on `RunCommandResult`) stays entirely core-side; the adapter only knows the constant command line.
