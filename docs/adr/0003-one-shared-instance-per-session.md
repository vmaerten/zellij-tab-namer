# One shared instance per session

zellij runs a regular plugin once per connected client: a client attaching gets a fresh instance, and a client detaching (closing the terminal, switching session) has its instance destroyed. The plugin's state lives in memory, so every session switch threw away the decorations and the instance of the next client renamed every tab without them. With two clients attached, two instances also competed for the same tab names.

The plugin is registered with `register_shared_plugin!` (zellij 0.46): one instance per session, kept alive across attaches and detaches, which receives the events of every client. Its state is the session's state.

The alternative, persisting decorations to disk and reloading them on start, was rejected: `/data` is per instance and deleted with it, `/cache` is shared across sessions and would need keying by session name plus garbage collection, and it would only hide the duplicate instances.

## Consequences

- Don't switch back to `register_plugin!`: decorations would again vanish on every session switch.
- Events like `TabUpdate` arrive once per connected client. The core already treats them as idempotent snapshots; with several clients on different tabs, `tab_id: "active"` resolves to the active tab of the last client that reported.
- Requires zellij 0.46, already the floor since 0.3.0.
