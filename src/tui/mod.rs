mod actions;
mod attach;
mod helpers;
mod metrics;
mod names;
mod run;

pub use attach::attach;
pub use helpers::{data_dir, launch_daemon};
pub use run::run;

#[derive(Copy, Clone)]
pub enum SplitDir {
    Horizontal,
    Vertical,
}

pub struct Pane {
    pub session: piki_multiplex::pty::PtySession,
    pub name: String,
    /// Daemon session id backing this pane (`None` when local). Lets a tab
    /// persist which sessions are its panes, so a restart can regroup them.
    pub id: Option<String>,
}

pub struct Tab {
    pub panes: Vec<Pane>,
    pub focus: usize,
    pub split: SplitDir,
    pub name: String,
    /// Daemon session id for the tab's first session (`None` when local).
    /// Used to persist the tab name across TUI restarts.
    pub id: Option<String>,
    /// Each pane's size along the split direction, in terminal cells.
    /// Rebuilt (evenly, or rescaled) by `normalize_sizes` whenever its
    /// length doesn't match `panes.len()` or its sum doesn't match the
    /// available space — so an empty vec here is always a valid start.
    pub sizes: Vec<u16>,
}

impl Tab {
    pub fn session(&self) -> &piki_multiplex::pty::PtySession {
        &self.panes[self.focus].session
    }
    pub fn session_mut(&mut self) -> &mut piki_multiplex::pty::PtySession {
        &mut self.panes[self.focus].session
    }
}
