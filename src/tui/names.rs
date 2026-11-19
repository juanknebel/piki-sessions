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
}

/// Default tab names (`tab-0`, `tab-1`, …) plus persisted renames, scoped
/// to the current daemon generation. Local (daemon-less) sessions use an
/// in-memory counter instead, since they die with the TUI anyway.
pub struct TabNames {
    next: u64,
    names: HashMap<String, String>,
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
    pub fn prune(&mut self, live: &HashSet<String>) {
        self.names.retain(|id, _| live.contains(id));
        self.save();
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
}
