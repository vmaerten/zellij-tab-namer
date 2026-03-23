use std::collections::{BTreeMap, HashMap, HashSet};
use zellij_tile::prelude::*;

#[derive(Default)]
struct State {
    got_permissions: bool,
    buffered_events: Vec<Event>,
    /// Stable tab_id → last name we applied (anti-flicker cache)
    applied_names: HashMap<usize, String>,
    /// Stable tab_ids the user renamed manually — we never touch these
    manually_renamed: HashSet<usize>,
    /// Stable tab_id → last known tab name (to detect manual renames)
    last_tab_names: HashMap<usize, String>,
    /// Mapping: tab_index → stable tab_id
    index_to_id: HashMap<usize, usize>,
    /// Mapping: stable tab_id → tab position (for rename_tab API)
    id_to_position: HashMap<usize, usize>,
    /// Mapping: pane_id → tab_id (to resolve CwdChanged events)
    pane_to_tab: HashMap<u32, usize>,
    /// $HOME path for ~ substitution
    home_dir: String,
}

register_plugin!(State);

impl ZellijPlugin for State {
    fn load(&mut self, _config: BTreeMap<String, String>) {
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ChangeApplicationState,
        ]);
        subscribe(&[
            EventType::TabUpdate,
            EventType::PaneUpdate,
            EventType::CwdChanged,
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

    fn render(&mut self, _rows: usize, _cols: usize) {}
}

impl State {
    fn handle_event(&mut self, event: Event) {
        match event {
            Event::TabUpdate(tabs) => self.on_tab_update(tabs),
            Event::PaneUpdate(manifest) => self.on_pane_update(manifest),
            Event::CwdChanged(pane_id, cwd, _clients) => self.on_cwd_changed(pane_id, cwd),
            _ => {}
        }
    }

    fn on_tab_update(&mut self, tabs: Vec<TabInfo>) {
        let alive_ids: HashSet<usize> = tabs.iter().map(|t| t.tab_id).collect();

        // GC stale entries for deleted tabs
        self.applied_names.retain(|id, _| alive_ids.contains(id));
        self.manually_renamed.retain(|id| alive_ids.contains(id));
        self.last_tab_names.retain(|id, _| alive_ids.contains(id));

        // Rebuild index/position mappings
        self.index_to_id.clear();
        self.id_to_position.clear();
        for tab in &tabs {
            self.index_to_id.insert(tab.position, tab.tab_id);
            self.id_to_position.insert(tab.tab_id, tab.position);
        }

        // Detect manual renames
        for tab in &tabs {
            let id = tab.tab_id;
            if let Some(prev_name) = self.last_tab_names.get(&id) {
                if *prev_name != tab.name {
                    let we_set_it = self
                        .applied_names
                        .get(&id)
                        .is_some_and(|n| *n == tab.name);
                    if !we_set_it {
                        self.manually_renamed.insert(id);
                    }
                }
            }
            self.last_tab_names.insert(id, tab.name.clone());
        }
    }

    fn on_pane_update(&mut self, manifest: PaneManifest) {
        // Rebuild pane → tab mapping
        self.pane_to_tab.clear();
        for (tab_index, panes) in &manifest.panes {
            let Some(&tab_id) = self.index_to_id.get(tab_index) else {
                continue;
            };
            for pane in panes {
                if !pane.is_plugin {
                    self.pane_to_tab.insert(pane.id, tab_id);
                }
            }
        }
    }

    fn on_cwd_changed(&mut self, pane_id: PaneId, cwd: std::path::PathBuf) {
        let PaneId::Terminal(terminal_id) = pane_id else {
            return;
        };

        let Some(&tab_id) = self.pane_to_tab.get(&terminal_id) else {
            return;
        };

        if self.manually_renamed.contains(&tab_id) {
            return;
        }

        let Some(&tab_pos) = self.id_to_position.get(&tab_id) else {
            return;
        };

        let cwd_str = cwd.to_string_lossy();
        if cwd_str.is_empty() {
            return;
        }

        let new_name = self.derive_name(&cwd_str);

        let already_set = self
            .applied_names
            .get(&tab_id)
            .is_some_and(|n| *n == new_name);

        if !already_set {
            rename_tab(tab_pos as u32, &new_name);
            self.applied_names.insert(tab_id, new_name.clone());
            self.last_tab_names.insert(tab_id, new_name);
        }
    }

    fn derive_name(&self, cwd: &str) -> String {
        if !self.home_dir.is_empty() && cwd == self.home_dir {
            return "~".to_string();
        }

        match cwd.rsplit('/').next() {
            Some(basename) if !basename.is_empty() => basename.to_string(),
            _ => "/".to_string(),
        }
    }
}
