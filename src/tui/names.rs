use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// Persisted tab-name state. The counter and name map belong to one daemon
/// generation, identified by the daemon socket's (inode, ctime): when the
/// daemon dies its sessions die with it, the next daemon recreates the
/// socket, and naming restarts from `tab-0`.
#[derive(serde::Serialize, serde::Deserialize)]
struct NamesFile {
    sock_ino: u64,
    sock_ctime: i64,
    next: u64,
    names: HashMap<String, String>,
    #[serde(default)]
    tabs: HashMap<String, TabRecord>,
}

/// A tab's pane layout, keyed by its first pane's session id (same key as
/// `NamesFile::names`). `panes` lists every pane's session id, in order.
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
struct TabRecord {
    panes: Vec<String>,
    #[serde(default)]
    horizontal: bool,
}

/// Default tab names (`tab-0`, `tab-1`, …) plus persisted renames, scoped
/// to the current daemon generation. Local (daemon-less) sessions use an
/// in-memory counter instead, since they die with the TUI anyway.
pub struct TabNames {
    next: u64,
    names: HashMap<String, String>,
    tabs: HashMap<String, TabRecord>,
    path: Option<PathBuf>,
    generation: Option<(u64, i64)>,
}

impl TabNames {
    /// Load state for the daemon behind `socket`, resetting when the
    /// socket belongs to a different daemon generation (or is unreadable).
    pub fn load(path: PathBuf, socket: &Path) -> Self {
        let generation = std::fs::metadata(socket).ok().map(|m| (m.ino(), m.ctime()));
        let mut state = Self {
            next: 0,
            names: HashMap::new(),
            tabs: HashMap::new(),
            path: Some(path),
            generation,
        };
        if let Some(file) = state
            .path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<NamesFile>(&s).ok())
            && Some((file.sock_ino, file.sock_ctime)) == generation
        {
            state.next = file.next;
            state.names = file.names;
            state.tabs = file.tabs;
        } else {
            state.save();
        }
        state
    }

    /// In-memory counter for local (daemon-less) sessions.
    pub fn memory() -> Self {
        Self {
            next: 0,
            names: HashMap::new(),
            tabs: HashMap::new(),
            path: None,
            generation: None,
        }
    }

    /// Name for a tab: the persisted one when the session id is known,
    /// else a fresh `tab-N` (skipping names already taken).
    pub fn assign(&mut self, id: Option<&str>) -> String {
        if let Some(id) = id
            && let Some(name) = self.names.get(id)
        {
            return name.clone();
        }
        let name = loop {
            let candidate = format!("tab-{}", self.next);
            self.next += 1;
            if !self.names.values().any(|n| n == &candidate) {
                break candidate;
            }
        };
        if let Some(id) = id {
            self.names.insert(id.to_string(), name.clone());
            self.save();
        }
        name
    }

    /// Persist a rename. `None` (local sessions) is a no-op: the live
    /// `Tab` already holds the name and dies with the TUI anyway.
    pub fn set(&mut self, id: Option<&str>, name: String) {
        if let Some(id) = id {
            self.names.insert(id.to_string(), name);
            self.save();
        }
    }

    /// Drop names of sessions the daemon no longer lists.
    ///
    /// A tab record is keyed by its first pane's session id (same as the
    /// name map): if that pane is gone, the whole record — and with it the
    /// tab's grouping of any surviving panes — is dropped too. Narrower than
    /// ideal, but matches `Tab::id`'s own lifetime today.
    pub fn prune(&mut self, live: &HashSet<String>) {
        self.names.retain(|id, _| live.contains(id));
        self.tabs.retain(|id, rec| {
            rec.panes.retain(|p| live.contains(p));
            live.contains(id) && !rec.panes.is_empty()
        });
        self.save();
    }

    /// Persist (or replace) a tab's pane layout: `tab_id` is its first
    /// pane's session id, `panes` every pane's session id in order.
    pub fn set_panes(&mut self, tab_id: &str, panes: &[String], horizontal: bool) {
        self.tabs.insert(
            tab_id.to_string(),
            TabRecord {
                panes: panes.to_vec(),
                horizontal,
            },
        );
        self.save();
    }

    /// The persisted pane order for a tab, if any was recorded.
    pub fn panes_for(&self, tab_id: &str) -> Option<&[String]> {
        self.tabs.get(tab_id).map(|r| r.panes.as_slice())
    }

    /// The persisted split direction for a tab (vertical by default).
    pub fn horizontal_for(&self, tab_id: &str) -> bool {
        self.tabs.get(tab_id).map(|r| r.horizontal).unwrap_or(false)
    }

    /// Every persisted tab id with its ordered pane ids, for reverse lookup
    /// (which tab does a given pane id belong to) during reattach.
    pub fn all_tabs(&self) -> impl Iterator<Item = (&String, &Vec<String>)> {
        self.tabs.iter().map(|(id, r)| (id, &r.panes))
    }

    fn save(&self) {
        let (Some(path), Some((ino, ctime))) = (&self.path, self.generation) else {
            return;
        };
        if path
            .parent()
            .map(std::fs::create_dir_all)
            .transpose()
            .is_err()
        {
            return;
        }
        let file = NamesFile {
            sock_ino: ino,
            sock_ctime: ctime,
            next: self.next,
            names: self.names.clone(),
            tabs: self.tabs.clone(),
        };
        let Ok(text) = serde_json::to_string(&file) else {
            return;
        };
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("pks-names-test-{}-{}", std::process::id(), tag))
    }

    fn test_socket(tag: &str) -> PathBuf {
        let p = test_path(&format!("sock-{tag}"));
        std::fs::write(&p, b"").unwrap();
        p
    }

    fn cleanup(paths: &[PathBuf]) {
        for p in paths {
            let _ = std::fs::remove_file(p);
            let _ = std::fs::remove_file(p.with_extension("tmp"));
        }
    }

    #[test]
    fn counter_starts_at_zero_and_increments() {
        let sock = test_socket("counter");
        let path = test_path("counter.json");
        let mut names = TabNames::load(path.clone(), &sock);
        assert_eq!(names.assign(None), "tab-0");
        assert_eq!(names.assign(None), "tab-1");
        cleanup(&[sock, path]);
    }

    #[test]
    fn known_session_keeps_its_name() {
        let sock = test_socket("stable");
        let path = test_path("stable.json");
        let mut names = TabNames::load(path.clone(), &sock);
        assert_eq!(names.assign(Some("aaa")), "tab-0");
        assert_eq!(names.assign(Some("bbb")), "tab-1");
        assert_eq!(names.assign(Some("aaa")), "tab-0");
        cleanup(&[sock, path]);
    }

    #[test]
    fn rename_persists_across_reload() {
        let sock = test_socket("persist");
        let path = test_path("persist.json");
        let mut names = TabNames::load(path.clone(), &sock);
        assert_eq!(names.assign(Some("aaa")), "tab-0");
        names.set(Some("aaa"), "work".to_string());
        drop(names);
        let mut names = TabNames::load(path.clone(), &sock);
        assert_eq!(names.assign(Some("aaa")), "work");
        assert_eq!(names.assign(Some("bbb")), "tab-1");
        cleanup(&[sock, path]);
    }

    #[test]
    fn fresh_counter_skips_taken_names() {
        let sock = test_socket("skip");
        let path = test_path("skip.json");
        let mut names = TabNames::load(path.clone(), &sock);
        names.set(Some("aaa"), "tab-0".to_string());
        assert_eq!(names.assign(Some("bbb")), "tab-1");
        cleanup(&[sock, path]);
    }

    #[test]
    fn new_daemon_generation_resets() {
        let sock1 = test_socket("gen1");
        let sock2 = test_socket("gen2");
        let path = test_path("gen.json");
        let mut names = TabNames::load(path.clone(), &sock1);
        assert_eq!(names.assign(Some("aaa")), "tab-0");
        assert_eq!(names.assign(Some("bbb")), "tab-1");
        drop(names);
        let mut names = TabNames::load(path.clone(), &sock2);
        assert_eq!(names.assign(Some("ccc")), "tab-0");
        cleanup(&[sock1, sock2, path]);
    }

    #[test]
    fn corrupt_file_resets_without_panic() {
        let sock = test_socket("corrupt");
        let path = test_path("corrupt.json");
        std::fs::write(&path, b"not json{{{").unwrap();
        let mut names = TabNames::load(path.clone(), &sock);
        assert_eq!(names.assign(Some("aaa")), "tab-0");
        cleanup(&[sock, path]);
    }

    #[test]
    fn prune_drops_dead_sessions() {
        let sock = test_socket("prune");
        let path = test_path("prune.json");
        let mut names = TabNames::load(path.clone(), &sock);
        assert_eq!(names.assign(Some("aaa")), "tab-0");
        assert_eq!(names.assign(Some("bbb")), "tab-1");
        names.prune(&HashSet::from(["bbb".to_string()]));
        drop(names);
        let mut names = TabNames::load(path.clone(), &sock);
        assert_eq!(names.assign(Some("aaa")), "tab-2");
        assert_eq!(names.assign(Some("bbb")), "tab-1");
        cleanup(&[sock, path]);
    }

    #[test]
    fn pane_group_persists_across_reload() {
        let sock = test_socket("panes");
        let path = test_path("panes.json");
        let mut names = TabNames::load(path.clone(), &sock);
        names.set_panes("aaa", &["aaa".to_string(), "bbb".to_string()], true);
        drop(names);
        let names = TabNames::load(path.clone(), &sock);
        assert_eq!(
            names.panes_for("aaa"),
            Some(["aaa".to_string(), "bbb".to_string()].as_slice())
        );
        assert!(names.horizontal_for("aaa"));
        cleanup(&[sock, path]);
    }

    #[test]
    fn prune_drops_dead_panes_but_keeps_surviving_group() {
        let sock = test_socket("prune-panes");
        let path = test_path("prune-panes.json");
        let mut names = TabNames::load(path.clone(), &sock);
        names.set_panes(
            "aaa",
            &["aaa".to_string(), "bbb".to_string(), "ccc".to_string()],
            false,
        );
        // "bbb" exited; "aaa" (the tab id) and "ccc" are still live.
        names.prune(&HashSet::from(["aaa".to_string(), "ccc".to_string()]));
        assert_eq!(
            names.panes_for("aaa"),
            Some(["aaa".to_string(), "ccc".to_string()].as_slice())
        );
        cleanup(&[sock, path]);
    }

    #[test]
    fn prune_drops_group_when_tab_id_dies() {
        let sock = test_socket("prune-group");
        let path = test_path("prune-group.json");
        let mut names = TabNames::load(path.clone(), &sock);
        names.set_panes("aaa", &["aaa".to_string(), "bbb".to_string()], false);
        // The tab's first pane ("aaa") exited, even though "bbb" survives.
        names.prune(&HashSet::from(["bbb".to_string()]));
        assert_eq!(names.panes_for("aaa"), None);
        cleanup(&[sock, path]);
    }
}
