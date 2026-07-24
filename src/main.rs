// On native (cargo test) only the pure core is compiled; without the wasm
// entrypoint most items look unused to rustc.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use zellij_tile::prelude::*;

const CTX_CWD: &str = "cwd";

/// Upper bound on the total entries across the never-invalidated git caches
/// (`git_roots` / `not_git` / `root_aliases` / `resolved_cwds`). Crossing it
/// clears them — a cleared entry just costs one cheap async re-query. Keeps
/// memory bounded without a timer, matching the plugin's event-driven model.
const MAX_GIT_CACHE_ENTRIES: usize = 4096;

/// Everything the plugin can do to the world. The core only emits these;
/// `drive` in the ZellijPlugin adapter is the sole place they are executed.
#[derive(Debug, Clone, PartialEq)]
enum Effect {
    RequestPermissions(Vec<PermissionType>),
    Subscribe(Vec<EventType>),
    RenameTab {
        tab_id: usize,
        name: String,
    },
    /// Launch `git rev-parse --show-toplevel` in `cwd`. The context is echoed
    /// back by zellij in RunCommandResult — the correlation protocol (build
    /// and parse) lives entirely in the core.
    RunGit {
        cwd: PathBuf,
        context: BTreeMap<String, String>,
    },
    /// Discovery query (ADR-0002): ask for a terminal pane's current cwd; the
    /// adapter feeds the answer back into the core as a synthetic CwdChanged.
    /// Responses never emit further queries, keeping the adapter loop bounded.
    QueryPaneCwd {
        pane_id: u32,
    },
    UnblockCliPipe(String),
}

enum Decoration {
    Prefix,
    Suffix,
}

#[derive(Default)]
struct State {
    /// Effects queued by the current `init`/`handle`/`handle_pipe` call,
    /// drained into its return value.
    effects: Vec<Effect>,
    got_permissions: bool,
    /// Latest state snapshots seen before permissions were granted, replayed on
    /// grant. TabUpdate/PaneUpdate are full snapshots, so only the last of each
    /// kind matters — this keeps pre-grant buffering bounded.
    buffered_tabs: Option<Vec<TabInfo>>,
    buffered_panes: Option<PaneManifest>,
    buffered_cwds: Vec<(u32, PathBuf)>,
    /// PaneManifest whose tab positions weren't all mapped yet (a PaneUpdate
    /// raced ahead of its TabUpdate); replayed after the next TabUpdate
    deferred_panes: Option<PaneManifest>,
    /// Whether to detect git repos for tab naming (default: true)
    git_detection: bool,
    /// Format string for pane count suffix, e.g. " ({pane_count})"
    pane_count_format: Option<String>,
    /// Stable tab_id → last base name we computed (without decorations)
    base_names: HashMap<usize, String>,
    /// tab_id → last rendered name sent to zellij (skip redundant renames)
    rendered_names: HashMap<usize, String>,
    /// Current non-plugin pane count per tab
    tab_pane_counts: HashMap<usize, usize>,
    /// Mapping: tab_index → stable tab_id
    index_to_id: HashMap<usize, usize>,
    /// Tabs whose floating-pane layer is currently visible (is_focused on a
    /// PaneInfo is per-layer, so the visible layer decides who speaks)
    floating_visible: HashSet<usize>,
    /// Mapping: pane_id → tab_id (to resolve CwdChanged events)
    pane_to_tab: HashMap<u32, usize>,
    /// Git roots discovered so far (cache to avoid re-running git)
    git_roots: HashSet<String>,
    /// Paths known to NOT be in a git repo. Never invalidated: a `git init` in
    /// an already-visited path goes unnoticed until the session restarts.
    not_git: HashSet<String>,
    /// Queried cwd → its git root, for cwds (symlinks, case differences) whose
    /// git root is not one of their string ancestors. Never invalidated, like
    /// not_git.
    root_aliases: HashMap<String, String>,
    /// Cwds whose git toplevel has actually been resolved by a query. Only then
    /// is the `find_git_root` ancestor walk trustworthy: an unqueried cwd could
    /// sit in a *deeper* repo nested under a known ancestor root, and matching
    /// the ancestor would name it wrongly (and permanently). Never invalidated,
    /// like not_git.
    resolved_cwds: HashSet<String>,
    /// CwdChanged events for pane_ids not yet in pane_to_tab — latest event per
    /// pane, kept in arrival order so replay stays deterministic
    pending_cwds: Vec<(u32, PathBuf)>,
    /// Last known CWD per tab_id
    tab_cwds: HashMap<usize, PathBuf>,
    /// CWD with a git rev-parse in flight → tabs awaiting the result
    pending_git: HashMap<String, Vec<usize>>,
    /// Tab ID of the currently active tab
    active_tab_id: Option<usize>,

    /// Per-tab prefix set via pipe API (e.g. "🤖 ")
    tab_prefixes: HashMap<usize, String>,
    /// Per-tab suffix set via pipe API (e.g. " [building]")
    tab_suffixes: HashMap<usize, String>,

    /// $HOME path for ~ substitution
    home_dir: String,
}

#[cfg(target_arch = "wasm32")]
register_plugin!(State);

// ─── Adapter: the only place plugin behaviour touches the zellij host ───────
// wasm-only: the host functions are extern symbols that don't exist on native,
// so the linker itself guarantees the core below stays free of host calls.

#[cfg(target_arch = "wasm32")]
impl ZellijPlugin for State {
    fn load(&mut self, config: BTreeMap<String, String>) {
        let home_dir = std::env::var("HOME").unwrap_or_default();
        let effects = self.init(config, home_dir);
        self.drive(effects);
    }

    fn update(&mut self, event: Event) -> bool {
        let effects = self.handle(event);
        self.drive(effects);
        false
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        let effects = self.handle_pipe(pipe_message);
        self.drive(effects);
        false
    }

    fn render(&mut self, _rows: usize, _cols: usize) {}
}

#[cfg(target_arch = "wasm32")]
impl State {
    /// Execute effects against the host. Discovery-query answers are fed back
    /// into the core as synthetic CwdChanged events; the loop is bounded
    /// because responses never emit further queries.
    fn drive(&mut self, mut effects: Vec<Effect>) {
        while !effects.is_empty() {
            let mut reinjected = Vec::new();
            for effect in effects {
                match effect {
                    Effect::RequestPermissions(permissions) => request_permission(&permissions),
                    Effect::Subscribe(events) => subscribe(&events),
                    Effect::RenameTab { tab_id, name } => rename_tab_with_id(tab_id as u64, &name),
                    Effect::RunGit { cwd, context } => run_command_with_env_variables_and_cwd(
                        &["git", "rev-parse", "--show-toplevel"],
                        BTreeMap::new(),
                        cwd,
                        context,
                    ),
                    Effect::QueryPaneCwd { pane_id } => {
                        // failed query: stay silent, the first CwdChanged takes over
                        if let Ok(cwd) = get_pane_cwd(PaneId::Terminal(pane_id)) {
                            reinjected.extend(self.handle(Event::CwdChanged(
                                PaneId::Terminal(pane_id),
                                cwd,
                                Default::default(),
                            )));
                        }
                    }
                    Effect::UnblockCliPipe(pipe_id) => unblock_cli_pipe_input(&pipe_id),
                }
            }
            effects = reinjected;
        }
    }
}

/// The real entrypoint comes from register_plugin! on wasm; this keeps
/// native builds (cargo test / check) linkable.
#[cfg(not(target_arch = "wasm32"))]
fn main() {}

// ─── Core: pure state machine — events in, effects out ──────────────────────

impl State {
    fn init(&mut self, config: BTreeMap<String, String>, home_dir: String) -> Vec<Effect> {
        self.git_detection = config
            .get("git_detection")
            .map(|v| v != "false")
            .unwrap_or(true);
        self.pane_count_format = config.get("pane_count").cloned();
        self.home_dir = home_dir;

        let mut permissions = vec![
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
        ];
        let mut events = vec![
            EventType::TabUpdate,
            EventType::PaneUpdate,
            EventType::CwdChanged,
            EventType::PermissionRequestResult,
        ];
        if self.git_detection {
            permissions.push(PermissionType::RunCommands);
            events.push(EventType::RunCommandResult);
        }
        self.effects.push(Effect::RequestPermissions(permissions));
        self.effects.push(Effect::Subscribe(events));
        std::mem::take(&mut self.effects)
    }

    fn handle(&mut self, event: Event) -> Vec<Effect> {
        if !self.got_permissions {
            match event {
                Event::PermissionRequestResult(PermissionStatus::Granted) => {
                    self.got_permissions = true;
                    if let Some(tabs) = self.buffered_tabs.take() {
                        self.on_tab_update(tabs);
                    }
                    // route buffered cwds through pending_cwds: on_pane_update
                    // resolves them before deciding which tabs still need a
                    // discovery query (pending is empty pre-grant)
                    self.pending_cwds
                        .extend(std::mem::take(&mut self.buffered_cwds));
                    if let Some(manifest) = self.buffered_panes.take() {
                        self.on_pane_update(manifest);
                    }
                }
                Event::TabUpdate(tabs) => self.buffered_tabs = Some(tabs),
                Event::PaneUpdate(manifest) => self.buffered_panes = Some(manifest),
                Event::CwdChanged(PaneId::Terminal(terminal_id), cwd, _clients) => {
                    upsert_cwd(&mut self.buffered_cwds, terminal_id, cwd);
                }
                _ => {}
            }
        } else {
            self.handle_event(event);
        }
        std::mem::take(&mut self.effects)
    }

    fn handle_pipe(&mut self, pipe_message: PipeMessage) -> Vec<Effect> {
        if let PipeSource::Cli(pipe_id) = &pipe_message.source {
            self.effects.push(Effect::UnblockCliPipe(pipe_id.clone()));
        }

        let args = &pipe_message.args;
        match pipe_message.name.as_str() {
            "set_prefix" => {
                let value = args.get("value").cloned().unwrap_or_default();
                self.set_decoration(args, Decoration::Prefix, value);
            }
            "set_suffix" => {
                let value = args.get("value").cloned().unwrap_or_default();
                self.set_decoration(args, Decoration::Suffix, value);
            }
            "clear_prefix" => self.set_decoration(args, Decoration::Prefix, String::new()),
            "clear_suffix" => self.set_decoration(args, Decoration::Suffix, String::new()),
            "clear_all" => {
                if let Some(tab_id) = self.resolve_tab_id(args) {
                    let had_prefix = self.tab_prefixes.remove(&tab_id).is_some();
                    let had_suffix = self.tab_suffixes.remove(&tab_id).is_some();
                    if had_prefix || had_suffix {
                        self.refresh_tab_name(tab_id);
                    }
                }
            }
            _ => {}
        }
        std::mem::take(&mut self.effects)
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::TabUpdate(tabs) => self.on_tab_update(tabs),
            Event::PaneUpdate(manifest) => self.on_pane_update(manifest),
            Event::CwdChanged(pane_id, cwd, _clients) => self.on_cwd_changed(pane_id, cwd),
            Event::RunCommandResult(exit_code, stdout, _stderr, context) => {
                self.on_run_command_result(exit_code, stdout, context);
            }
            _ => {}
        }
    }

    fn on_tab_update(&mut self, tabs: Vec<TabInfo>) {
        let alive_ids: HashSet<usize> = tabs.iter().map(|t| t.tab_id).collect();

        for tab in &tabs {
            if tab.active {
                self.active_tab_id = Some(tab.tab_id);
            }
        }

        // GC stale entries for deleted tabs
        self.base_names.retain(|id, _| alive_ids.contains(id));
        self.rendered_names.retain(|id, _| alive_ids.contains(id));
        self.tab_cwds.retain(|id, _| alive_ids.contains(id));
        self.tab_pane_counts.retain(|id, _| alive_ids.contains(id));
        self.tab_prefixes.retain(|id, _| alive_ids.contains(id));
        self.tab_suffixes.retain(|id, _| alive_ids.contains(id));
        for waiters in self.pending_git.values_mut() {
            waiters.retain(|id| alive_ids.contains(id));
        }
        self.pending_git.retain(|_, waiters| !waiters.is_empty());

        self.index_to_id.clear();
        self.floating_visible.clear();
        for tab in &tabs {
            self.index_to_id.insert(tab.position, tab.tab_id);
            if tab.are_floating_panes_visible {
                self.floating_visible.insert(tab.tab_id);
            }
        }

        // A PaneUpdate that raced ahead of this TabUpdate can now be resolved
        if let Some(manifest) = self.deferred_panes.take() {
            self.on_pane_update(manifest);
        }
    }

    fn on_pane_update(&mut self, manifest: PaneManifest) {
        // a newer manifest supersedes any deferred one
        self.deferred_panes = None;
        let old_pane_to_tab = std::mem::take(&mut self.pane_to_tab);
        let mut tabs_with_new_panes: Vec<(usize, usize)> = Vec::new();
        let mut new_counts: HashMap<usize, usize> = HashMap::new();
        let mut has_unmapped_tab = false;

        for (tab_index, panes) in &manifest.panes {
            let Some(&tab_id) = self.index_to_id.get(tab_index) else {
                has_unmapped_tab = true;
                continue;
            };
            let mut count = 0usize;
            let mut has_new_pane = false;
            for pane in panes {
                if !pane.is_plugin {
                    count += 1;
                    // new to this tab: brand new, or moved from another tab
                    if old_pane_to_tab.get(&pane.id) != Some(&tab_id) {
                        has_new_pane = true;
                    }
                    self.pane_to_tab.insert(pane.id, tab_id);
                }
            }
            new_counts.insert(tab_id, count);
            if has_new_pane {
                tabs_with_new_panes.push((tab_id, *tab_index));
            }
        }

        // Detect tabs whose pane count changed and re-apply their name
        let old_counts = std::mem::replace(&mut self.tab_pane_counts, new_counts);
        if self.pane_count_format.is_some() {
            let changed: Vec<usize> = self
                .tab_pane_counts
                .iter()
                .filter(|(id, count)| old_counts.get(id).copied().unwrap_or(0) != **count)
                .map(|(&id, _)| id)
                .collect();
            for tab_id in changed {
                self.refresh_tab_name(tab_id);
            }
        }

        // Replay buffered CwdChanged events (on_cwd_changed re-buffers if still
        // unresolved), then drop the ones whose pane no longer exists
        let pending = std::mem::take(&mut self.pending_cwds);
        for (terminal_id, cwd) in pending {
            self.on_cwd_changed(PaneId::Terminal(terminal_id), cwd);
        }
        let manifest_pane_ids: HashSet<u32> = manifest
            .panes
            .values()
            .flatten()
            .filter(|p| !p.is_plugin)
            .map(|p| p.id)
            .collect();
        self.pending_cwds
            .retain(|(id, _)| manifest_pane_ids.contains(id));

        // Discovery query (ADR-0002): name tabs that still have no base name
        // from the actual cwd of the focused pane in their visible layer
        for (tab_id, tab_index) in tabs_with_new_panes {
            if self.base_names.contains_key(&tab_id) || self.tab_cwds.contains_key(&tab_id) {
                continue;
            }
            let Some(panes) = manifest.panes.get(&tab_index) else {
                continue;
            };
            if let Some(pane_id) = self.choose_discovery_pane(tab_id, panes) {
                self.effects.push(Effect::QueryPaneCwd { pane_id });
            }
        }

        // PaneUpdate raced ahead of TabUpdate: keep the manifest for replay
        // once the next TabUpdate maps the missing tab positions
        if has_unmapped_tab {
            self.deferred_panes = Some(manifest);
        }
    }

    /// Pick the pane whose cwd names the tab: the focused pane of the visible
    /// layer (is_focused is per-layer), then any candidate of the visible
    /// layer, then any focused pane, then any candidate — what the user sees
    /// beats a focus hidden in the other layer.
    fn choose_discovery_pane(&self, tab_id: usize, panes: &[PaneInfo]) -> Option<u32> {
        let want_floating = self.floating_visible.contains(&tab_id);
        let candidates = || panes.iter().filter(|p| !p.is_plugin && !p.is_suppressed);
        candidates()
            .find(|p| p.is_focused && p.is_floating == want_floating)
            .or_else(|| candidates().find(|p| p.is_floating == want_floating))
            .or_else(|| candidates().find(|p| p.is_focused))
            .or_else(|| candidates().next())
            .map(|p| p.id)
    }

    fn on_cwd_changed(&mut self, pane_id: PaneId, cwd: PathBuf) {
        let PaneId::Terminal(terminal_id) = pane_id else {
            return;
        };

        let Some(&tab_id) = self.pane_to_tab.get(&terminal_id) else {
            upsert_cwd(&mut self.pending_cwds, terminal_id, cwd);
            return;
        };

        // an empty cwd must leave no trace: recording it in tab_cwds would
        // wrongly convince the discovery that the tab is already resolved
        let cwd_str = cwd.to_string_lossy().to_string();
        if cwd_str.is_empty() {
            return;
        }

        self.tab_cwds.insert(tab_id, cwd.clone());

        if !self.git_detection {
            let name = self.derive_name(&cwd_str);
            self.apply_name(tab_id, name);
            return;
        }

        if let Some(waiters) = self.pending_git.get_mut(&cwd_str) {
            if !waiters.contains(&tab_id) {
                waiters.push(tab_id);
            }
            return;
        }

        if self.apply_cached_name(tab_id, &cwd_str) {
            return;
        }

        self.pending_git.insert(cwd_str.clone(), vec![tab_id]);
        let context = BTreeMap::from([(CTX_CWD.to_string(), cwd_str)]);
        self.effects.push(Effect::RunGit { cwd, context });
    }

    /// Walk path ancestors and return the git root's basename if one is known.
    fn find_git_root(&self, cwd: &str) -> Option<String> {
        if let Some(git_root) = self.root_aliases.get(cwd) {
            return Some(basename(git_root));
        }
        Path::new(cwd)
            .ancestors()
            .filter_map(Path::to_str)
            .find(|path| self.git_roots.contains(*path))
            .map(basename)
    }

    /// Name the tab from the git caches; false if its cwd isn't resolved yet.
    fn apply_cached_name(&mut self, tab_id: usize, cwd: &str) -> bool {
        let name = if self.not_git.contains(cwd) {
            self.derive_name(cwd)
        } else if self.can_trust_git_root(cwd) {
            match self.find_git_root(cwd) {
                Some(repo_name) => repo_name,
                None => return false,
            }
        } else {
            return false;
        };
        self.apply_name(tab_id, name);
        true
    }

    /// Whether `find_git_root`'s ancestor walk can be trusted for this cwd — i.e.
    /// it won't miss a nested repo. True only for an exact hit (the cwd is itself
    /// a known root or alias) or a cwd git has already resolved: for anything
    /// else a `git rev-parse` must run, in case the cwd's real toplevel is a repo
    /// nested below a known ancestor root.
    fn can_trust_git_root(&self, cwd: &str) -> bool {
        self.git_roots.contains(cwd)
            || self.root_aliases.contains_key(cwd)
            || self.resolved_cwds.contains(cwd)
    }

    /// Total entries across the never-invalidated git caches (for the cap).
    fn git_cache_len(&self) -> usize {
        self.git_roots.len()
            + self.not_git.len()
            + self.root_aliases.len()
            + self.resolved_cwds.len()
    }

    fn on_run_command_result(
        &mut self,
        exit_code: Option<i32>,
        stdout: Vec<u8>,
        context: BTreeMap<String, String>,
    ) {
        let Some(cwd) = context.get(CTX_CWD) else {
            return;
        };
        let waiters = self.pending_git.remove(cwd).unwrap_or_default();

        // Bound the never-invalidated git caches (event-driven, no timer). Done
        // before inserting this result so the fresh entry always survives.
        if self.git_cache_len() >= MAX_GIT_CACHE_ENTRIES {
            self.git_roots.clear();
            self.not_git.clear();
            self.root_aliases.clear();
            self.resolved_cwds.clear();
        }

        let git_root = (exit_code == Some(0))
            .then(|| String::from_utf8_lossy(&stdout).trim().to_string())
            .filter(|s| !s.is_empty());

        match git_root {
            Some(git_root) => {
                let is_ancestor = Path::new(cwd)
                    .ancestors()
                    .filter_map(Path::to_str)
                    .any(|p| p == git_root);
                if !is_ancestor {
                    self.root_aliases.insert(cwd.clone(), git_root.clone());
                }
                self.git_roots.insert(git_root);
                // The ancestor walk is now trustworthy for this exact cwd: its
                // true (possibly deeper) toplevel is the nearest cached root.
                self.resolved_cwds.insert(cwd.clone());
            }
            None => {
                self.not_git.insert(cwd.clone());
            }
        }

        // Re-resolve each waiter from its *current* cwd: a tab that moved on since
        // this query was launched keeps its newer name (its own query handles it)
        for tab_id in waiters {
            let Some(tab_cwd) = self.tab_cwds.get(&tab_id) else {
                continue;
            };
            let cwd_str = tab_cwd.to_string_lossy().to_string();
            self.apply_cached_name(tab_id, &cwd_str);
        }
    }

    fn apply_name(&mut self, tab_id: usize, base_name: String) {
        self.base_names.insert(tab_id, base_name);
        self.refresh_tab_name(tab_id);
    }

    /// Compose prefix + base name + suffix + pane count and emit a rename,
    /// unless that exact name is already displayed.
    fn refresh_tab_name(&mut self, tab_id: usize) {
        let Some(base_name) = self.base_names.get(&tab_id) else {
            return;
        };
        let rendered_name = self.compose_rendered_name(tab_id, base_name);
        if self.rendered_names.get(&tab_id) == Some(&rendered_name) {
            return;
        }
        self.effects.push(Effect::RenameTab {
            tab_id,
            name: rendered_name.clone(),
        });
        self.rendered_names.insert(tab_id, rendered_name);
    }

    fn compose_rendered_name(&self, tab_id: usize, base_name: &str) -> String {
        let prefix = self
            .tab_prefixes
            .get(&tab_id)
            .map(|s| s.as_str())
            .unwrap_or("");
        let suffix = self
            .tab_suffixes
            .get(&tab_id)
            .map(|s| s.as_str())
            .unwrap_or("");
        let pane_count = self.tab_pane_counts.get(&tab_id).copied().unwrap_or(1);
        let pane_count_suffix = match &self.pane_count_format {
            Some(fmt) if pane_count > 1 => fmt.replace("{pane_count}", &pane_count.to_string()),
            _ => String::new(),
        };
        format!("{prefix}{base_name}{suffix}{pane_count_suffix}")
    }

    fn resolve_tab_id(&self, args: &BTreeMap<String, String>) -> Option<usize> {
        match args.get("tab_id").map(|s| s.as_str()) {
            Some("active") | None => self.active_tab_id,
            Some(id_str) => id_str.parse::<usize>().ok(),
        }
    }

    /// Set or clear (empty value) a tab's prefix/suffix, then re-render its name.
    fn set_decoration(
        &mut self,
        args: &BTreeMap<String, String>,
        decoration: Decoration,
        value: String,
    ) {
        let Some(tab_id) = self.resolve_tab_id(args) else {
            return;
        };
        let map = match decoration {
            Decoration::Prefix => &mut self.tab_prefixes,
            Decoration::Suffix => &mut self.tab_suffixes,
        };
        let current = map.get(&tab_id).map(|s| s.as_str()).unwrap_or("");
        if value == current {
            return;
        }
        if value.is_empty() {
            map.remove(&tab_id);
        } else {
            map.insert(tab_id, value);
        }
        self.refresh_tab_name(tab_id);
    }

    fn derive_name(&self, cwd: &str) -> String {
        if !self.home_dir.is_empty() && cwd == self.home_dir {
            return "~".to_string();
        }
        basename(cwd)
    }
}

fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("/")
        .to_string()
}

/// Keep only the latest cwd per pane, in arrival order of the latest events,
/// so replay is deterministic and faithful to the live last-write-wins flow.
fn upsert_cwd(list: &mut Vec<(u32, PathBuf)>, terminal_id: u32, cwd: PathBuf) {
    list.retain(|(id, _)| *id != terminal_id);
    list.push((terminal_id, cwd));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(id: usize, position: usize, active: bool) -> TabInfo {
        TabInfo {
            tab_id: id,
            position,
            active,
            ..Default::default()
        }
    }

    fn manifest(entries: &[(usize, &[u32])]) -> PaneManifest {
        let panes = entries
            .iter()
            .map(|(position, pane_ids)| {
                let panes = pane_ids
                    .iter()
                    .map(|&id| PaneInfo {
                        id,
                        ..Default::default()
                    })
                    .collect();
                (*position, panes)
            })
            .collect();
        PaneManifest { panes }
    }

    fn manifest_with(entries: Vec<(usize, Vec<PaneInfo>)>) -> PaneManifest {
        PaneManifest {
            panes: entries.into_iter().collect(),
        }
    }

    fn cwd_event(pane_id: u32, path: &str) -> Event {
        Event::CwdChanged(
            PaneId::Terminal(pane_id),
            PathBuf::from(path),
            Default::default(),
        )
    }

    fn ctx(cwd: &str) -> BTreeMap<String, String> {
        BTreeMap::from([(CTX_CWD.to_string(), cwd.to_string())])
    }

    fn git_result(exit_code: i32, stdout: &str, cwd: &str) -> Event {
        Event::RunCommandResult(
            Some(exit_code),
            stdout.as_bytes().to_vec(),
            Vec::new(),
            ctx(cwd),
        )
    }

    fn pipe_msg(name: &str, args: &[(&str, &str)]) -> PipeMessage {
        PipeMessage {
            source: PipeSource::Plugin(0),
            name: name.to_string(),
            payload: None,
            args: args
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            is_private: false,
        }
    }

    /// Permissions granted, one active tab (id 1, position 0) holding pane 10.
    fn ready_state(config: &[(&str, &str)]) -> State {
        let mut state = State::default();
        let config = config
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        state.init(config, "/home/u".to_string());
        state.handle(Event::PermissionRequestResult(PermissionStatus::Granted));
        state.handle(Event::TabUpdate(vec![tab(1, 0, true)]));
        state.handle(Event::PaneUpdate(manifest(&[(0, &[10])])));
        state
    }

    #[test]
    fn init_gates_run_commands_on_git_detection() {
        let mut state = State::default();
        let effects = state.init(BTreeMap::new(), String::new());
        assert_eq!(
            effects,
            vec![
                Effect::RequestPermissions(vec![
                    PermissionType::ReadApplicationState,
                    PermissionType::ChangeApplicationState,
                    PermissionType::RunCommands,
                ]),
                Effect::Subscribe(vec![
                    EventType::TabUpdate,
                    EventType::PaneUpdate,
                    EventType::CwdChanged,
                    EventType::PermissionRequestResult,
                    EventType::RunCommandResult,
                ]),
            ]
        );

        let mut state = State::default();
        let config = BTreeMap::from([("git_detection".to_string(), "false".to_string())]);
        let effects = state.init(config, String::new());
        assert_eq!(
            effects[0],
            Effect::RequestPermissions(vec![
                PermissionType::ReadApplicationState,
                PermissionType::ChangeApplicationState,
            ])
        );
    }

    #[test]
    fn buffers_events_until_permission_granted() {
        let mut state = State::default();
        state.init(
            BTreeMap::from([("git_detection".to_string(), "false".to_string())]),
            String::new(),
        );
        assert_eq!(
            state.handle(Event::TabUpdate(vec![tab(1, 0, true)])),
            vec![]
        );
        assert_eq!(
            state.handle(Event::PaneUpdate(manifest(&[(0, &[10])]))),
            vec![]
        );
        assert_eq!(state.handle(cwd_event(10, "/proj/api")), vec![]);

        let effects = state.handle(Event::PermissionRequestResult(PermissionStatus::Granted));
        // the buffered cwd resolves the tab before discovery: no redundant query
        assert_eq!(
            effects,
            vec![Effect::RenameTab {
                tab_id: 1,
                name: "api".to_string(),
            }]
        );
    }

    #[test]
    fn pane_update_before_tab_update_defers_discovery() {
        let mut state = State::default();
        state.init(BTreeMap::new(), "/home/u".to_string());
        state.handle(Event::PermissionRequestResult(PermissionStatus::Granted));

        // PaneUpdate races ahead of the TabUpdate that maps position 0
        assert_eq!(
            state.handle(Event::PaneUpdate(manifest(&[(0, &[10])]))),
            vec![]
        );
        let effects = state.handle(Event::TabUpdate(vec![tab(1, 0, true)]));
        assert_eq!(effects, vec![Effect::QueryPaneCwd { pane_id: 10 }]);
    }

    #[test]
    fn moved_pane_triggers_discovery_for_its_new_tab() {
        let mut state = ready_state(&[("git_detection", "false")]);
        state.handle(Event::PaneUpdate(manifest(&[(0, &[10, 11])])));
        state.handle(cwd_event(10, "/proj"));

        // pane 11 breaks out into a brand-new tab
        state.handle(Event::TabUpdate(vec![tab(1, 0, true), tab(2, 1, false)]));
        let effects = state.handle(Event::PaneUpdate(manifest(&[(0, &[10]), (1, &[11])])));
        assert_eq!(effects, vec![Effect::QueryPaneCwd { pane_id: 11 }]);
    }

    #[test]
    fn empty_cwd_does_not_block_discovery() {
        let mut state = ready_state(&[("git_detection", "false")]);
        // an empty cwd must leave no trace
        assert_eq!(state.handle(cwd_event(10, "")), vec![]);

        // a later split finds the tab still unresolved: discovery fires
        let effects = state.handle(Event::PaneUpdate(manifest(&[(0, &[10, 11])])));
        assert_eq!(effects, vec![Effect::QueryPaneCwd { pane_id: 10 }]);
    }

    #[test]
    fn buffered_cwds_replay_in_arrival_order() {
        let mut state = State::default();
        state.init(
            BTreeMap::from([("git_detection".to_string(), "false".to_string())]),
            String::new(),
        );
        state.handle(Event::TabUpdate(vec![tab(1, 0, true)]));
        state.handle(Event::PaneUpdate(manifest(&[(0, &[10, 11])])));
        // two panes of the same tab report different cwds before the grant
        state.handle(cwd_event(10, "/proj/api"));
        state.handle(cwd_event(11, "/proj/web"));

        let effects = state.handle(Event::PermissionRequestResult(PermissionStatus::Granted));
        // deterministic: the last arrival names the tab last
        assert_eq!(
            effects,
            vec![
                Effect::RenameTab {
                    tab_id: 1,
                    name: "api".to_string(),
                },
                Effect::RenameTab {
                    tab_id: 1,
                    name: "web".to_string(),
                },
            ]
        );
    }

    #[test]
    fn discovery_ignores_focused_pane_of_hidden_floating_layer() {
        fn layered_panes() -> Vec<PaneInfo> {
            vec![
                PaneInfo {
                    id: 10,
                    is_focused: true,
                    ..Default::default()
                },
                PaneInfo {
                    id: 11,
                    is_focused: true,
                    is_floating: true,
                    ..Default::default()
                },
            ]
        }

        // floating layer hidden: the focused tiled pane speaks for the tab
        let mut state = State::default();
        state.init(BTreeMap::new(), "/home/u".to_string());
        state.handle(Event::PermissionRequestResult(PermissionStatus::Granted));
        state.handle(Event::TabUpdate(vec![tab(1, 0, true)]));
        let effects = state.handle(Event::PaneUpdate(manifest_with(vec![(0, layered_panes())])));
        assert_eq!(effects, vec![Effect::QueryPaneCwd { pane_id: 10 }]);

        // floating layer visible: the focused floating pane wins
        let mut state = State::default();
        state.init(BTreeMap::new(), "/home/u".to_string());
        state.handle(Event::PermissionRequestResult(PermissionStatus::Granted));
        let mut floating_tab = tab(1, 0, true);
        floating_tab.are_floating_panes_visible = true;
        state.handle(Event::TabUpdate(vec![floating_tab]));
        let effects = state.handle(Event::PaneUpdate(manifest_with(vec![(0, layered_panes())])));
        assert_eq!(effects, vec![Effect::QueryPaneCwd { pane_id: 11 }]);
    }

    #[test]
    fn git_result_renames_every_tab_waiting_on_the_same_cwd() {
        let mut state = ready_state(&[]);
        state.handle(Event::TabUpdate(vec![tab(1, 0, true), tab(2, 1, false)]));
        state.handle(Event::PaneUpdate(manifest(&[(0, &[10]), (1, &[20])])));

        let effects = state.handle(cwd_event(10, "/repo"));
        assert_eq!(
            effects,
            vec![Effect::RunGit {
                cwd: PathBuf::from("/repo"),
                context: ctx("/repo"),
            }]
        );

        // second tab, same cwd: joins the waiters, no duplicate query
        assert_eq!(state.handle(cwd_event(20, "/repo")), vec![]);

        let effects = state.handle(git_result(0, "/repo\n", "/repo"));
        assert_eq!(
            effects,
            vec![
                Effect::RenameTab {
                    tab_id: 1,
                    name: "repo".to_string(),
                },
                Effect::RenameTab {
                    tab_id: 2,
                    name: "repo".to_string(),
                },
            ]
        );
    }

    #[test]
    fn stale_git_result_does_not_overwrite_newer_name() {
        let mut state = ready_state(&[]);

        assert_eq!(
            state.handle(cwd_event(10, "/repo-a")),
            vec![Effect::RunGit {
                cwd: PathBuf::from("/repo-a"),
                context: ctx("/repo-a"),
            }]
        );
        assert_eq!(
            state.handle(cwd_event(10, "/elsewhere")),
            vec![Effect::RunGit {
                cwd: PathBuf::from("/elsewhere"),
                context: ctx("/elsewhere"),
            }]
        );

        // /elsewhere resolves first: not a git repo, named by basename
        let effects = state.handle(git_result(128, "", "/elsewhere"));
        assert_eq!(
            effects,
            vec![Effect::RenameTab {
                tab_id: 1,
                name: "elsewhere".to_string(),
            }]
        );

        // the stale result for /repo-a lands afterwards: the tab moved on, no rename
        assert_eq!(state.handle(git_result(0, "/repo-a\n", "/repo-a")), vec![]);
    }

    #[test]
    fn home_directory_renders_as_tilde() {
        let mut state = ready_state(&[("git_detection", "false")]);
        let effects = state.handle(cwd_event(10, "/home/u"));
        assert_eq!(
            effects,
            vec![Effect::RenameTab {
                tab_id: 1,
                name: "~".to_string(),
            }]
        );
    }

    #[test]
    fn pane_count_suffix_follows_pane_count_changes() {
        let mut state = ready_state(&[
            ("git_detection", "false"),
            ("pane_count", " ({pane_count})"),
        ]);
        state.handle(cwd_event(10, "/proj"));

        let effects = state.handle(Event::PaneUpdate(manifest(&[(0, &[10, 11])])));
        assert_eq!(
            effects,
            vec![Effect::RenameTab {
                tab_id: 1,
                name: "proj (2)".to_string(),
            }]
        );

        let effects = state.handle(Event::PaneUpdate(manifest(&[(0, &[10])])));
        assert_eq!(
            effects,
            vec![Effect::RenameTab {
                tab_id: 1,
                name: "proj".to_string(),
            }]
        );
    }

    #[test]
    fn pipe_decorations_wrap_the_base_name() {
        let mut state = ready_state(&[("git_detection", "false")]);
        state.handle(cwd_event(10, "/proj"));

        let effects = state.handle_pipe(pipe_msg("set_prefix", &[("value", "* ")]));
        assert_eq!(
            effects,
            vec![Effect::RenameTab {
                tab_id: 1,
                name: "* proj".to_string(),
            }]
        );

        // setting the same prefix again is a no-op
        assert_eq!(
            state.handle_pipe(pipe_msg("set_prefix", &[("value", "* ")])),
            vec![]
        );

        let effects = state.handle_pipe(pipe_msg("clear_prefix", &[]));
        assert_eq!(
            effects,
            vec![Effect::RenameTab {
                tab_id: 1,
                name: "proj".to_string(),
            }]
        );

        // clearing when nothing is set stays silent
        assert_eq!(state.handle_pipe(pipe_msg("clear_prefix", &[])), vec![]);
    }

    #[test]
    fn cli_pipe_source_is_unblocked() {
        let mut state = ready_state(&[]);
        let message = PipeMessage {
            source: PipeSource::Cli("pipe-1".to_string()),
            name: "unknown".to_string(),
            payload: None,
            args: BTreeMap::new(),
            is_private: false,
        };
        assert_eq!(
            state.handle_pipe(message),
            vec![Effect::UnblockCliPipe("pipe-1".to_string())]
        );
    }

    #[test]
    fn buffered_cwd_for_a_closed_pane_is_dropped() {
        let mut state = ready_state(&[("git_detection", "false")]);
        // cwd for a pane zellij hasn't mapped yet: buffered
        assert_eq!(state.handle(cwd_event(99, "/ghost")), vec![]);
        // next manifest doesn't contain pane 99: the buffered cwd is dropped
        state.handle(Event::PaneUpdate(manifest(&[(0, &[10])])));
        // when pane 99 later appears in a new tab, the ghost cwd must not name
        // it — only a fresh discovery query is emitted
        state.handle(Event::TabUpdate(vec![tab(1, 0, true), tab(2, 1, false)]));
        let effects = state.handle(Event::PaneUpdate(manifest(&[(0, &[10]), (1, &[99])])));
        assert_eq!(effects, vec![Effect::QueryPaneCwd { pane_id: 99 }]);
    }

    #[test]
    fn discovery_query_prefers_the_focused_pane() {
        let mut state = State::default();
        state.init(
            BTreeMap::from([("git_detection".to_string(), "false".to_string())]),
            "/home/u".to_string(),
        );
        state.handle(Event::PermissionRequestResult(PermissionStatus::Granted));
        state.handle(Event::TabUpdate(vec![tab(1, 0, true)]));

        let panes = vec![
            PaneInfo {
                id: 10,
                ..Default::default()
            },
            PaneInfo {
                id: 11,
                is_focused: true,
                ..Default::default()
            },
        ];
        let effects = state.handle(Event::PaneUpdate(manifest_with(vec![(0, panes)])));
        assert_eq!(effects, vec![Effect::QueryPaneCwd { pane_id: 11 }]);

        // the adapter reinjects the answer as a synthetic CwdChanged
        let effects = state.handle(cwd_event(11, "/proj"));
        assert_eq!(
            effects,
            vec![Effect::RenameTab {
                tab_id: 1,
                name: "proj".to_string(),
            }]
        );
    }

    #[test]
    fn restore_names_every_tab_from_its_panes() {
        let mut state = State::default();
        state.init(
            BTreeMap::from([("git_detection".to_string(), "false".to_string())]),
            "/home/u".to_string(),
        );
        state.handle(Event::PermissionRequestResult(PermissionStatus::Granted));
        state.handle(Event::TabUpdate(vec![tab(1, 0, true), tab(2, 1, false)]));

        // a restored session arrives as one manifest with every pane at once
        let effects = state.handle(Event::PaneUpdate(manifest(&[(0, &[10]), (1, &[20])])));
        assert_eq!(effects.len(), 2);
        assert!(effects.contains(&Effect::QueryPaneCwd { pane_id: 10 }));
        assert!(effects.contains(&Effect::QueryPaneCwd { pane_id: 20 }));

        assert_eq!(
            state.handle(cwd_event(10, "/proj/api")),
            vec![Effect::RenameTab {
                tab_id: 1,
                name: "api".to_string(),
            }]
        );
        assert_eq!(
            state.handle(cwd_event(20, "/proj/web")),
            vec![Effect::RenameTab {
                tab_id: 2,
                name: "web".to_string(),
            }]
        );
    }

    #[test]
    fn discovery_response_flows_through_git_resolution() {
        let mut state = State::default();
        state.init(BTreeMap::new(), "/home/u".to_string());
        state.handle(Event::PermissionRequestResult(PermissionStatus::Granted));
        state.handle(Event::TabUpdate(vec![tab(1, 0, true)]));

        let effects = state.handle(Event::PaneUpdate(manifest(&[(0, &[10])])));
        assert_eq!(effects, vec![Effect::QueryPaneCwd { pane_id: 10 }]);

        let effects = state.handle(cwd_event(10, "/repo/sub"));
        assert_eq!(
            effects,
            vec![Effect::RunGit {
                cwd: PathBuf::from("/repo/sub"),
                context: ctx("/repo/sub"),
            }]
        );

        let effects = state.handle(git_result(0, "/repo\n", "/repo/sub"));
        assert_eq!(
            effects,
            vec![Effect::RenameTab {
                tab_id: 1,
                name: "repo".to_string(),
            }]
        );
    }

    #[test]
    fn no_second_discovery_once_the_tab_is_named() {
        let mut state = ready_state(&[("git_detection", "false")]);
        state.handle(cwd_event(10, "/proj"));

        // a split opens a new pane in the already-named tab: no query
        let effects = state.handle(Event::PaneUpdate(manifest(&[(0, &[10, 11])])));
        assert_eq!(effects, vec![]);
    }

    #[test]
    fn visible_layer_beats_focus_of_hidden_layer() {
        let mut state = State::default();
        state.init(BTreeMap::new(), "/home/u".to_string());
        state.handle(Event::PermissionRequestResult(PermissionStatus::Granted));
        let mut floating_tab = tab(1, 0, true);
        floating_tab.are_floating_panes_visible = true;
        state.handle(Event::TabUpdate(vec![floating_tab]));

        // the tiled pane is focused in its (hidden) layer; the visible
        // floating pane is not focused — what the user sees must win
        let panes = vec![
            PaneInfo {
                id: 10,
                is_focused: true,
                ..Default::default()
            },
            PaneInfo {
                id: 11,
                is_floating: true,
                ..Default::default()
            },
        ];
        let effects = state.handle(Event::PaneUpdate(manifest_with(vec![(0, panes)])));
        assert_eq!(effects, vec![Effect::QueryPaneCwd { pane_id: 11 }]);
    }

    #[test]
    fn dead_tab_waiters_are_purged() {
        let mut state = ready_state(&[]);
        state.handle(cwd_event(10, "/repo"));
        assert!(!state.pending_git.is_empty());

        // the waiting tab closes before the git result arrives
        state.handle(Event::TabUpdate(vec![tab(2, 0, true)]));
        assert!(state.pending_git.is_empty());

        // the late result still feeds the caches, without effects
        assert_eq!(state.handle(git_result(0, "/repo\n", "/repo")), vec![]);
        assert!(state.git_roots.contains("/repo"));
    }

    #[test]
    fn symlinked_cwd_resolves_via_root_alias() {
        let mut state = ready_state(&[]);
        state.handle(Event::TabUpdate(vec![tab(1, 0, true), tab(2, 1, false)]));
        state.handle(Event::PaneUpdate(manifest(&[(0, &[10]), (1, &[20])])));

        // the cwd goes through a symlink: git resolves it to another path
        assert_eq!(
            state.handle(cwd_event(10, "/links/repo/sub")),
            vec![Effect::RunGit {
                cwd: PathBuf::from("/links/repo/sub"),
                context: ctx("/links/repo/sub"),
            }]
        );
        let effects = state.handle(git_result(0, "/real/repo\n", "/links/repo/sub"));
        assert_eq!(
            effects,
            vec![Effect::RenameTab {
                tab_id: 1,
                name: "repo".to_string(),
            }]
        );

        // a second tab on the same symlinked cwd resolves from the cache,
        // without launching another git query
        let effects = state.handle(cwd_event(20, "/links/repo/sub"));
        assert_eq!(
            effects,
            vec![Effect::RenameTab {
                tab_id: 2,
                name: "repo".to_string(),
            }]
        );
    }

    #[test]
    fn nested_repo_is_not_named_after_an_ancestor_root() {
        let mut state = State::default();
        state.init(BTreeMap::new(), "/home/u".to_string());
        state.handle(Event::PermissionRequestResult(PermissionStatus::Granted));
        state.handle(Event::TabUpdate(vec![tab(1, 0, true), tab(2, 1, false)]));
        state.handle(Event::PaneUpdate(manifest(&[(0, &[10]), (1, &[20])])));

        // Tab 1 makes the git-tracked home a known root.
        state.handle(cwd_event(10, "/home/u"));
        state.handle(git_result(0, "/home/u\n", "/home/u"));

        // Tab 2 enters a repo nested UNDER that root. It must query git for its
        // own toplevel, not silently inherit the ancestor's name ("u").
        assert_eq!(
            state.handle(cwd_event(20, "/home/u/dev/realproject")),
            vec![Effect::RunGit {
                cwd: PathBuf::from("/home/u/dev/realproject"),
                context: ctx("/home/u/dev/realproject"),
            }],
            "a nested cwd must be queried, not named after an ancestor root"
        );

        // git reports the deeper toplevel → the tab is named after it.
        let effects = state.handle(git_result(
            0,
            "/home/u/dev/realproject\n",
            "/home/u/dev/realproject",
        ));
        assert_eq!(
            effects,
            vec![Effect::RenameTab {
                tab_id: 2,
                name: "realproject".to_string(),
            }]
        );
    }

    #[test]
    fn git_caches_stay_bounded() {
        let mut state = ready_state(&[]);
        // Visit far more distinct cwds than the cap; the caches must not grow
        // without bound (they clear and rebuild, event-driven, no timer).
        for i in 0..(MAX_GIT_CACHE_ENTRIES + 50) {
            let cwd = format!("/p/dir{i}");
            state.handle(cwd_event(10, &cwd));
            state.handle(git_result(0, &format!("{cwd}\n"), &cwd));
        }
        assert!(
            state.git_cache_len() <= MAX_GIT_CACHE_ENTRIES,
            "git caches must stay bounded, got {}",
            state.git_cache_len()
        );
    }
}
