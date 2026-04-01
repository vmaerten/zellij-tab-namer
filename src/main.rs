use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use zellij_tile::prelude::*;

const CTX_TAB_ID: &str = "tab_id";
const CTX_CWD: &str = "cwd";

enum Decoration {
    Prefix,
    Suffix,
}

#[derive(Default)]
struct State {
    got_permissions: bool,
    buffered_events: Vec<Event>,
    /// Whether to detect git repos for tab naming (default: true)
    git_detection: bool,
    /// Format template for tab name, e.g. "{process}: {name}"
    format: String,
    /// Fallback format when no process is detected, e.g. "{name}"
    format_no_proc: String,
    /// Shell names to exclude from process detection
    shell_names: HashSet<String>,
    /// Stable tab_id → last base name we computed (without pane count)
    applied_names: HashMap<usize, String>,
    /// Current non-plugin pane count per tab
    tab_pane_counts: HashMap<usize, usize>,
    /// Mapping: tab_index → stable tab_id
    index_to_id: HashMap<usize, usize>,
    /// Mapping: pane_id → tab_id (to resolve CwdChanged events)
    pane_to_tab: HashMap<u32, usize>,
    /// Git toplevel path → repo basename (cache to avoid re-running git)
    git_roots: HashMap<String, String>,
    /// Paths known to NOT be in a git repo
    not_git: HashSet<String>,
    /// CwdChanged events for pane_ids not yet in pane_to_tab
    pending_cwds: HashMap<u32, PathBuf>,
    /// Last known CWD per tab_id
    tab_cwds: HashMap<usize, PathBuf>,
    /// Tab ID of the previously active tab (before the current one)
    prev_active_tab_id: Option<usize>,
    /// Tab ID of the currently active tab
    active_tab_id: Option<usize>,

    /// Per-tab detected process name (from focused pane title)
    tab_processes: HashMap<usize, String>,
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
        self.format = config
            .get("format")
            .cloned()
            .unwrap_or_else(|| "{name}".to_string());
        self.format_no_proc = config
            .get("format_no_proc")
            .cloned()
            .unwrap_or_else(|| self.format.clone());
        self.shell_names = config
            .get("shell_names")
            .map(|v| v.split(',').map(|s| s.trim().to_lowercase()).collect())
            .unwrap_or_else(default_shell_names);
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
            PermissionType::RunCommands,
        ]);
        subscribe(&[
            EventType::TabUpdate,
            EventType::PaneUpdate,
            EventType::CwdChanged,
            EventType::RunCommandResult,
            EventType::PermissionRequestResult,
        ]);
        self.home_dir = std::env::var("HOME").unwrap_or_default();
    }

    fn update(&mut self, event: Event) -> bool {
        if !self.got_permissions {
            if let Event::PermissionRequestResult(PermissionStatus::Granted) = &event {
                self.got_permissions = true;
                let buffered = std::mem::take(&mut self.buffered_events);
                for ev in buffered {
                    self.handle_event(ev);
                }
            } else {
                self.buffered_events.push(event);
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

        match pipe_message.name.as_str() {
            "set_prefix" => self.handle_set_decoration(&pipe_message.args, Decoration::Prefix),
            "set_suffix" => self.handle_set_decoration(&pipe_message.args, Decoration::Suffix),
            "clear_prefix" => self.handle_clear_decoration(&pipe_message.args, Decoration::Prefix),
            "clear_suffix" => self.handle_clear_decoration(&pipe_message.args, Decoration::Suffix),
            "clear_all" => {
                if let Some(tab_id) = self.resolve_tab_id(&pipe_message.args) {
                    let had_prefix = self.tab_prefixes.remove(&tab_id).is_some();
                    let had_suffix = self.tab_suffixes.remove(&tab_id).is_some();
                    if had_prefix || had_suffix {
                        self.reapply_name(tab_id);
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
        self.tab_cwds.retain(|id, _| alive_ids.contains(id));
        self.tab_pane_counts.retain(|id, _| alive_ids.contains(id));
        self.tab_processes.retain(|id, _| alive_ids.contains(id));
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

        let uses_process =
            self.format.contains("{process}") || self.format_no_proc.contains("{process}");
        let uses_pane_count =
            self.format.contains("{pane_count}") || self.format_no_proc.contains("{pane_count}");

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

            // Detect process from focused pane title
            if uses_process {
                let process = panes
                    .iter()
                    .find(|p| !p.is_plugin && p.is_focused)
                    .map(|p| p.title.as_str())
                    .and_then(|t| extract_process_name(t, &self.shell_names));

                let old = self.tab_processes.get(&tab_id).map(|s| s.as_str());
                if old != process.as_deref() {
                    match process {
                        Some(p) => self.tab_processes.insert(tab_id, p),
                        None => self.tab_processes.remove(&tab_id),
                    };
                    self.reapply_name(tab_id);
                }
            }
        }

        // Detect tabs whose pane count changed and re-apply their name
        let old_counts = std::mem::replace(&mut self.tab_pane_counts, new_counts);
        if uses_pane_count {
            for (&tab_id, &new_count) in &self.tab_pane_counts {
                let old_count = old_counts.get(&tab_id).copied().unwrap_or(0);
                if old_count != new_count {
                    self.reapply_name(tab_id);
                }
            }
        }

        // Replay buffered CwdChanged events (on_cwd_changed re-buffers if still unresolved)
        let pending = std::mem::take(&mut self.pending_cwds);
        for (terminal_id, cwd) in pending {
            self.on_cwd_changed(PaneId::Terminal(terminal_id), cwd);
        }

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

        if let Some(repo_name) = self.find_git_root(&cwd_str) {
            self.apply_name(tab_id, repo_name);
            return;
        }

        if self.not_git.contains(&cwd_str) {
            let name = self.derive_name(&cwd_str);
            self.apply_name(tab_id, name);
            return;
        }

        let mut context = BTreeMap::new();
        context.insert(CTX_TAB_ID.to_string(), tab_id.to_string());
        context.insert(CTX_CWD.to_string(), cwd_str);
        run_command_with_env_variables_and_cwd(
            &["git", "rev-parse", "--show-toplevel"],
            BTreeMap::new(),
            cwd,
            context,
        );
    }

    /// Walk path ancestors and check if any is a known git root. O(depth) hash lookups.
    fn find_git_root(&self, cwd: &str) -> Option<String> {
        let mut path = Path::new(cwd);
        loop {
            if let Some(repo_name) = self.git_roots.get(path.to_str()?) {
                return Some(repo_name.clone());
            }
            path = path.parent()?;
        }
    }

    fn on_run_command_result(
        &mut self,
        exit_code: Option<i32>,
        stdout: Vec<u8>,
        context: BTreeMap<String, String>,
    ) {
        if !self.git_detection {
            return;
        }
        let Some(tab_id_str) = context.get(CTX_TAB_ID) else {
            return;
        };
        let Ok(tab_id) = tab_id_str.parse::<usize>() else {
            return;
        };
        let Some(cwd) = context.get(CTX_CWD) else {
            return;
        };

        let toplevel = (exit_code == Some(0))
            .then(|| String::from_utf8_lossy(&stdout).trim().to_string())
            .filter(|s| !s.is_empty());

        let new_name = match toplevel {
            Some(tl) => {
                let repo_name = basename(&tl);
                self.git_roots.insert(tl, repo_name.clone());
                repo_name
            }
            None => {
                self.not_git.insert(cwd.clone());
                self.derive_name(cwd)
            }
        };

        self.apply_name(tab_id, new_name);
    }

    fn apply_name(&mut self, tab_id: usize, new_name: String) {
        let already_set = self
            .applied_names
            .get(&tab_id)
            .is_some_and(|n| *n == new_name);

        if !already_set {
            let full_name = self.compose_full_name(tab_id, &new_name);
            rename_tab_with_id(tab_id as u64, &full_name);
            self.applied_names.insert(tab_id, new_name);
        }
    }

    fn reapply_name(&self, tab_id: usize) {
        if let Some(base_name) = self.applied_names.get(&tab_id) {
            let full_name = self.compose_full_name(tab_id, base_name);
            rename_tab_with_id(tab_id as u64, &full_name);
        }
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

        let process = self.tab_processes.get(&tab_id);
        let template = match process {
            Some(_) => &self.format,
            None => &self.format_no_proc,
        };

        let mut result = template.replace("{name}", base_name);
        if let Some(proc) = process {
            result = result.replace("{process}", proc);
        }
        let pane_count = self.tab_pane_counts.get(&tab_id).copied().unwrap_or(1);
        result = result.replace("{pane_count}", &pane_count.to_string());

        format!("{prefix}{result}{suffix}")
    }

    fn resolve_tab_id(&self, args: &BTreeMap<String, String>) -> Option<usize> {
        match args.get("tab_id").map(|s| s.as_str()) {
            Some("active") | None => self.active_tab_id,
            Some(id_str) => id_str.parse::<usize>().ok(),
        }
    }

    fn handle_set_decoration(&mut self, args: &BTreeMap<String, String>, decoration: Decoration) {
        let Some(tab_id) = self.resolve_tab_id(args) else {
            return;
        };
        let value = args.get("value").cloned().unwrap_or_default();
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
        self.reapply_name(tab_id);
    }

    fn handle_clear_decoration(&mut self, args: &BTreeMap<String, String>, decoration: Decoration) {
        let Some(tab_id) = self.resolve_tab_id(args) else {
            return;
        };
        let removed = match decoration {
            Decoration::Prefix => self.tab_prefixes.remove(&tab_id),
            Decoration::Suffix => self.tab_suffixes.remove(&tab_id),
        };
        if removed.is_some() {
            self.reapply_name(tab_id);
        }
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

fn extract_process_name(title: &str, shell_names: &HashSet<String>) -> Option<String> {
    let command = title.split_whitespace().next()?;
    let base = command.rsplit('/').next().unwrap_or(command);
    if base.is_empty() || shell_names.contains(&base.to_lowercase()) {
        return None;
    }
    Some(base.to_string())
}

fn default_shell_names() -> HashSet<String> {
    [
        "bash", "zsh", "fish", "nu", "nushell", "sh", "dash", "ksh", "tcsh", "csh", "elvish",
        "pwsh",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}
