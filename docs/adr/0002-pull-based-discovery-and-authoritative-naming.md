# Pull-based discovery; the plugin owns tab names

`CwdChanged` only fires on the first `cd`, which left new sessions, new tabs and restored sessions unnamed. When a `PaneUpdate` reveals a pane whose tab has no base name, the core now emits a discovery query (`Effect::QueryPaneCwd`, backed by `get_pane_cwd`); the adapter runs it synchronously and feeds the result back into the core as a synthetic `CwdChanged`, so the query round-trip stays inside the effects seam and under test.

## Consequences

- The plugin is authoritative: every tab is renamed at first discovery, including restored tabs and manually renamed ones. Persistent per-tab customisation belongs to the Pipe API, not manual renames. The "only rename default-named tabs" alternative was rejected: stale saved names (ghost decorations, obsolete pane counts) would survive restores, and default-name detection is fragile.
- The former "inherit the previous tab's cwd" heuristic is deleted — don't reintroduce it. The query returns the pane's actual cwd; a guess can only agree with it or be wrong.
- A failed query (pane exited, cwd inaccessible) is silent; the first `CwdChanged` takes over.
