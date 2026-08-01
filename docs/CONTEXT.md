# zellij-tab-namer

A zellij plugin that names tabs after the git repository — or, failing that, the directory — of their panes' current working directory, with optional per-tab decorations driven over zellij's pipe mechanism.

## Language

### Naming

**Base name**:
The name computed for a tab from its cwd: the git root's basename, the directory's basename, or `~` for the home directory.
_Avoid_: applied name, title

**Rendered name**:
The full string actually displayed on a tab: decorations wrapped around the base name, plus the pane count suffix.
_Avoid_: full name, composed name

**Decoration**:
A prefix or suffix attached to one tab through the Pipe API, wrapped around its base name.
_Avoid_: badge, marker

**Pane count suffix**:
An optional, format-configurable suffix showing how many terminal panes a tab holds, displayed only above one.

**Pipe API**:
The pipe messages (`set_prefix`, `set_suffix`, `clear_prefix`, `clear_suffix`, `clear_all`) through which external tools decorate tabs.
_Avoid_: pipe protocol

### Resolution

**Git root**:
The toplevel directory of a git repository. A tab whose cwd sits anywhere under a git root takes the root's basename as its base name.
_Avoid_: toplevel, repo root

**Waiter**:
A tab awaiting the git verdict for a cwd whose query is in flight. Every waiter on the same cwd is renamed when the verdict lands.

**Discovery query**:
The cwd lookup issued for a newly discovered pane whose tab has no base name yet — how tabs get named at session start, tab creation and restore, without waiting for a `cd`. The focused pane of the tab's visible layer speaks for it, with fallbacks to another pane of that layer, then any focused pane, then any terminal pane.
_Avoid_: poll, probe

### Architecture

**Effect**:
One intended action of the plugin on zellij (rename a tab, launch a git query, issue a discovery query, unblock a pipe, request permissions, subscribe). The core's only output; the catalogue of the plugin's entire impact on the world.
_Avoid_: command, action, side effect

**Core**:
The pure state machine: zellij events in, effects out. Compiles and is tested natively; contains no host calls.
_Avoid_: engine, business logic

**Adapter**:
The wasm-only shim that feeds zellij events into the core and executes the effects it returns against the zellij host.
_Avoid_: shell, wrapper, boundary
