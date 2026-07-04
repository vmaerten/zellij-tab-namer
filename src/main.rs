use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use zellij_tile::prelude::*;

const CTX_CWD: &str = "cwd";

enum Decoration {
    Prefix,
    Suffix,
}

#[derive(Default)]
struct State {
    got_permissions: bool,
    /// Latest state snapshots seen before permissions were granted, replayed on
    /// grant. TabUpdate/PaneUpdate are full snapshots, so only the last of each
    /// kind matters — this keeps pre-grant buffering bounded.
    buffered_tabs: Option<Vec<TabInfo>>,
    buffered_panes: Option<PaneManifest>,
    buffered_cwds: HashMap<u32, PathBuf>,
    /// Whether to detect git repos for tab naming (default: true)
    git_detection: bool,
    /// Format string for pane count suffix, e.g. " ({pane_count})"
    pane_count_format: Option<String>,
    /// Stable tab_id → last base name we computed (without decorations)
    applied_names: HashMap<usize, String>,
    /// tab_id → last full name sent to zellij (skip redundant renames)
    rendered_names: HashMap<usize, String>,
    /// Current non-plugin pane count per tab
    tab_pane_counts: HashMap<usize, usize>,
    /// Mapping: tab_index → stable tab_id
    index_to_id: HashMap<usize, usize>,
    /// Mapping: pane_id → tab_id (to resolve CwdChanged events)
    pane_to_tab: HashMap<u32, usize>,
    /// Git toplevel paths discovered so far (cache to avoid re-running git)
    git_roots: HashSet<String>,
    /// Paths known to NOT be in a git repo. Never invalidated: a `git init` in
    /// an already-visited path goes unnoticed until the session restarts.
    not_git: HashSet<String>,
    /// CwdChanged events for pane_ids not yet in pane_to_tab
    pending_cwds: HashMap<u32, PathBuf>,
    /// Last known CWD per tab_id
    tab_cwds: HashMap<usize, PathBuf>,
    /// CWD with a git rev-parse in flight → tabs awaiting the result
    pending_git: HashMap<String, Vec<usize>>,
    /// Tab ID of the previously active tab (before the current one)
    prev_active_tab_id: Option<usize>,
    /// Tab ID of the currently active tab
    active_tab_id: Option<usize>,

    /// Per-tab prefix set via pipe API (e.g. "🤖 ")
    tab_prefixes: HashMap<usize, String>,
    /// Per-tab suffix set via pipe API (e.g. " [building]")
    tab_suffixes: HashMap<usize, String>,

    /// $HOME path for ~ substitution
    home_dir: String,
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, config: BTreeMap<String, String>) {
        self.git_detection = config
            .get("git_detection")
            .map(|v| v != "false")
            .unwrap_or(true);
        self.pane_count_format = config.get("pane_count").cloned();
        self.home_dir = std::env::var("HOME").unwrap_or_default();

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
        request_permission(&permissions);
        subscribe(&events);
    }

    fn update(&mut self, event: Event) -> bool {
        if !self.got_permissions {
            match event {
                Event::PermissionRequestResult(PermissionStatus::Granted) => {
                    self.got_permissions = true;
                    if let Some(tabs) = self.buffered_tabs.take() {
                        self.on_tab_update(tabs);
                    }
                    if let Some(manifest) = self.buffered_panes.take() {
                        self.on_pane_update(manifest);
                    }
                    for (terminal_id, cwd) in std::mem::take(&mut self.buffered_cwds) {
                        self.on_cwd_changed(PaneId::Terminal(terminal_id), cwd);
                    }
                }
                Event::TabUpdate(tabs) => self.buffered_tabs = Some(tabs),
                Event::PaneUpdate(manifest) => self.buffered_panes = Some(manifest),
                Event::CwdChanged(PaneId::Terminal(terminal_id), cwd, _clients) => {
                    self.buffered_cwds.insert(terminal_id, cwd);
                }
                _ => {}
            }
            return false;
        }

        self.handle_event(event);
        false
    }

    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        if let PipeSource::Cli(pipe_id) = &pipe_message.source {
            unblock_cli_pipe_input(pipe_id);
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
        false
    }

    fn render(&mut self, _rows: usize, _cols: usize) {}
}

impl State {
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
            if tab.active && self.active_tab_id != Some(tab.tab_id) {
                self.prev_active_tab_id = self.active_tab_id;
                self.active_tab_id = Some(tab.tab_id);
            }
        }

        // GC stale entries for deleted tabs
        self.applied_names.retain(|id, _| alive_ids.contains(id));
        self.rendered_names.retain(|id, _| alive_ids.contains(id));
        self.tab_cwds.retain(|id, _| alive_ids.contains(id));
        self.tab_pane_counts.retain(|id, _| alive_ids.contains(id));
        self.tab_prefixes.retain(|id, _| alive_ids.contains(id));
        self.tab_suffixes.retain(|id, _| alive_ids.contains(id));
        if let Some(id) = self.prev_active_tab_id {
            if !alive_ids.contains(&id) {
                self.prev_active_tab_id = None;
            }
        }

        self.index_to_id.clear();
        for tab in &tabs {
            self.index_to_id.insert(tab.position, tab.tab_id);
        }
    }

    fn on_pane_update(&mut self, manifest: PaneManifest) {
        let old_pane_to_tab = std::mem::take(&mut self.pane_to_tab);
        let mut new_panes: Vec<(u32, usize)> = Vec::new();
        let mut new_counts: HashMap<usize, usize> = HashMap::new();

        for (tab_index, panes) in &manifest.panes {
            let Some(&tab_id) = self.index_to_id.get(tab_index) else {
                continue;
            };
            let mut count = 0usize;
            for pane in panes {
                if !pane.is_plugin {
                    count += 1;
                    if !old_pane_to_tab.contains_key(&pane.id) {
                        new_panes.push((pane.id, tab_id));
                    }
                    self.pane_to_tab.insert(pane.id, tab_id);
                }
            }
            new_counts.insert(tab_id, count);
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
            .retain(|id, _| manifest_pane_ids.contains(id));

        // For new panes in tabs without an applied name, use the previous tab's CWD
        for (pane_id, tab_id) in new_panes {
            if self.applied_names.contains_key(&tab_id) || self.tab_cwds.contains_key(&tab_id) {
                continue;
            }
            let cwd = self
                .prev_active_tab_id
                .and_then(|id| self.tab_cwds.get(&id))
                .cloned();
            if let Some(cwd) = cwd {
                self.on_cwd_changed(PaneId::Terminal(pane_id), cwd);
            }
        }
    }

    fn on_cwd_changed(&mut self, pane_id: PaneId, cwd: PathBuf) {
        let PaneId::Terminal(terminal_id) = pane_id else {
            return;
        };

        let Some(&tab_id) = self.pane_to_tab.get(&terminal_id) else {
            self.pending_cwds.insert(terminal_id, cwd);
            return;
        };

        self.tab_cwds.insert(tab_id, cwd.clone());

        let cwd_str = cwd.to_string_lossy().to_string();
        if cwd_str.is_empty() {
            return;
        }

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
        run_command_with_env_variables_and_cwd(
            &["git", "rev-parse", "--show-toplevel"],
            BTreeMap::new(),
            cwd,
            context,
        );
    }

    /// Walk path ancestors and return the repo basename if any is a known git root.
    fn find_git_root(&self, cwd: &str) -> Option<String> {
        Path::new(cwd)
            .ancestors()
            .filter_map(Path::to_str)
            .find(|path| self.git_roots.contains(*path))
            .map(basename)
    }

    /// Name the tab from the git caches; false if its cwd is in neither cache yet.
    fn apply_cached_name(&mut self, tab_id: usize, cwd: &str) -> bool {
        let name = if let Some(repo_name) = self.find_git_root(cwd) {
            repo_name
        } else if self.not_git.contains(cwd) {
            self.derive_name(cwd)
        } else {
            return false;
        };
        self.apply_name(tab_id, name);
        true
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

        let toplevel = (exit_code == Some(0))
            .then(|| String::from_utf8_lossy(&stdout).trim().to_string())
            .filter(|s| !s.is_empty());

        match toplevel {
            Some(toplevel) => {
                self.git_roots.insert(toplevel);
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
        self.applied_names.insert(tab_id, base_name);
        self.refresh_tab_name(tab_id);
    }

    /// Compose prefix + base name + suffix + pane count and push it to zellij,
    /// unless that exact name is already displayed.
    fn refresh_tab_name(&mut self, tab_id: usize) {
        let Some(base_name) = self.applied_names.get(&tab_id) else {
            return;
        };
        let full_name = self.compose_full_name(tab_id, base_name);
        if self.rendered_names.get(&tab_id) == Some(&full_name) {
            return;
        }
        rename_tab_with_id(tab_id as u64, &full_name);
        self.rendered_names.insert(tab_id, full_name);
    }

    fn compose_full_name(&self, tab_id: usize, base_name: &str) -> String {
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
