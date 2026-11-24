use std::path::Path;

use piki_multiplex::pty::{PtyOutputSignal, PtySession, SpawnOptions};
use piki_multiplex::session::client::Daemon;
use piki_multiplex::session::protocol::SpawnRequest;
use strum::IntoEnumIterator;

use super::names::TabNames;
use super::{Pane, SplitDir, Tab};

/// TERM advertised inside sessions. Like tmux (screen/tmux-256color), we
/// advertise what we emulate (xterm 256-color) instead of propagating the
/// outer $TERM, whose extra capabilities our vt100 screen may not support.
pub const SESSION_TERM: &str = "xterm-256color";

/// Set (or override) TERM in an environment list, without duplicates.
pub fn with_session_term(mut env: Vec<(String, String)>) -> Vec<(String, String)> {
    match env.iter_mut().find(|(k, _)| k == "TERM") {
        Some((_, v)) => *v = SESSION_TERM.to_string(),
        None => env.push(("TERM".to_string(), SESSION_TERM.to_string())),
    }
    env
}

/// Complete environment for daemon spawns: the daemon `env_clear()`s before
/// applying `SpawnRequest.env`, so an empty list leaves shells without TERM
/// (ncurses then fails with `cannot initialize terminal type`). Inherit ours
/// and force a TERM we emulate.
pub fn session_env() -> Vec<(String, String)> {
    with_session_term(std::env::vars().collect())
}

/// Default tab/pane name: basename of the shell command.
pub fn default_name(cmd: &str) -> String {
    let base = cmd.rsplit('/').next().unwrap_or(cmd);
    let base = base.split_whitespace().next().unwrap_or(base);
    if base.is_empty() {
        "shell".to_string()
    } else {
        base.to_string()
    }
}

/// Rename prompt state: `pane` selects the target, `buffer` the new name.
/// The buffer starts prefilled with the current name and selected: the
/// first keystroke replaces it, like select-all in a text input.
pub struct RenameState {
    pub pane: bool,
    pub buffer: String,
    pub fresh: bool,
}

impl RenameState {
    pub fn push(&mut self, c: char) {
        if self.fresh {
            self.buffer.clear();
            self.fresh = false;
        }
        if self.buffer.len() < 32 {
            self.buffer.push(c);
        }
    }

    pub fn backspace(&mut self) {
        if self.fresh {
            self.buffer.clear();
            self.fresh = false;
        } else {
            self.buffer.pop();
        }
    }
}

/// Every user-facing action. This enum is the single source of truth:
/// palette rows are generated from [`Command::iter`], and dispatch goes
/// through the exhaustive [`Command::execute`]. Adding a variant without
/// palette metadata (`id`/`description`/`shortcut`) or without an
/// implementation fails to compile, and the `cmd_*` helpers are
/// module-private so `run.rs` cannot bypass the palette.
#[derive(strum::EnumIter, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    NewTab,
    NextTab,
    PrevTab,
    Close,
    SwitchTab,
    ScrollMode,
    SplitVertical,
    SplitHorizontal,
    NextPane,
    Quit,
    RenameTab,
    RenamePane,
    ListPanes,
}

impl Command {
    pub fn id(self) -> &'static str {
        match self {
            Command::NewTab => "new-tab",
            Command::NextTab => "next-tab",
            Command::PrevTab => "prev-tab",
            Command::Close => "close",
            Command::SwitchTab => "switch-tab",
            Command::ScrollMode => "scroll-mode",
            Command::SplitVertical => "split-vertical",
            Command::SplitHorizontal => "split-horizontal",
            Command::NextPane => "next-pane",
            Command::Quit => "quit",
            Command::RenameTab => "rename-tab",
            Command::RenamePane => "rename-pane",
            Command::ListPanes => "list-panes",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Command::NewTab => "Create new tab",
            Command::NextTab => "Next tab",
            Command::PrevTab => "Previous tab",
            Command::Close => "Close pane or tab",
            Command::SwitchTab => "Switch tab",
            Command::ScrollMode => "Enter scrollback mode",
            Command::SplitVertical => "Split pane vertically",
            Command::SplitHorizontal => "Split pane horizontally",
            Command::NextPane => "Next pane",
            Command::Quit => "Quit",
            Command::RenameTab => "Rename tab",
            Command::RenamePane => "Rename pane",
            Command::ListPanes => "List panes",
        }
    }

    pub fn shortcut(self) -> &'static str {
        match self {
            Command::NewTab => "Ctrl+G c",
            Command::NextTab => "Ctrl+G n",
            Command::PrevTab => "Ctrl+G p",
            Command::Close => "Ctrl+G x",
            Command::SwitchTab => "Ctrl+G s",
            Command::ScrollMode => "Ctrl+G [",
            Command::SplitVertical => "Ctrl+G v",
            Command::SplitHorizontal => "Ctrl+G h",
            Command::NextPane => "Ctrl+G o",
            Command::Quit => "Ctrl+Q",
            Command::RenameTab => "Ctrl+G ,",
            Command::RenamePane => "Ctrl+G R",
            Command::ListPanes => "Ctrl+G w",
        }
    }

    pub fn execute(self, ctx: &mut ExecCtx) {
        match self {
            Command::NewTab => cmd_new_tab(ctx.tabs, ctx.active, ctx.spawn, ctx.names),
            Command::NextTab => cmd_next_tab(ctx.tabs, ctx.active),
            Command::PrevTab => cmd_prev_tab(ctx.tabs, ctx.active),
            Command::Close => {
                if cmd_close(ctx.tabs, ctx.active, ctx.names) {
                    *ctx.quit = true;
                }
            }
            Command::SwitchTab => {
                *ctx.tabs_open = true;
                *ctx.tabs_sel = *ctx.active;
            }
            Command::ScrollMode => *ctx.scroll_mode = true,
            Command::SplitVertical => {
                cmd_split(ctx.tabs, *ctx.active, SplitDir::Vertical, ctx.spawn, ctx.names)
            }
            Command::SplitHorizontal => {
                cmd_split(ctx.tabs, *ctx.active, SplitDir::Horizontal, ctx.spawn, ctx.names)
            }
            Command::NextPane => cmd_next_pane(ctx.tabs, *ctx.active),
            Command::Quit => *ctx.quit = true,
            Command::RenameTab => {
                let cur = ctx
                    .tabs
                    .get(*ctx.active)
                    .map(|t| t.name.clone())
                    .unwrap_or_default();
                *ctx.rename = Some(RenameState {
                    pane: false,
                    buffer: cur,
                    fresh: true,
                });
            }
            Command::RenamePane => {
                let cur = ctx
                    .tabs
                    .get(*ctx.active)
                    .and_then(|t| t.panes.get(t.focus))
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                *ctx.rename = Some(RenameState {
                    pane: true,
                    buffer: cur,
                    fresh: true,
                });
            }
            Command::ListPanes => {
                let focus = ctx.tabs.get(*ctx.active).map(|t| t.focus).unwrap_or(0);
                *ctx.panes_open = true;
                *ctx.panes_sel = flat_panes(pane_counts(ctx.tabs))
                    .iter()
                    .position(|&(ti, pi)| ti == *ctx.active && pi == focus)
                    .unwrap_or(0);
            }
        }
    }
}

/// Mutable TUI state a [`Command`] may touch.
pub struct ExecCtx<'a> {
    pub tabs: &'a mut Vec<Tab>,
    pub active: &'a mut usize,
    pub spawn: &'a SpawnCtx<'a>,
    pub tabs_open: &'a mut bool,
    pub tabs_sel: &'a mut usize,
    pub panes_open: &'a mut bool,
    pub panes_sel: &'a mut usize,
    pub scroll_mode: &'a mut bool,
    pub rename: &'a mut Option<RenameState>,
    pub names: &'a mut TabNames,
    pub quit: &'a mut bool,
}

/// Palette rows: every [`Command`], filtered by query. Quit always sorts
/// last so a destructive action never sits under the cursor by default.
pub fn filtered_commands(query: &str) -> Vec<Command> {
    let q = query.to_lowercase();
    let mut rows: Vec<Command> = Command::iter()
        .filter(|c| {
            q.is_empty()
                || c.id().to_lowercase().contains(q.as_str())
                || c.description().to_lowercase().contains(q.as_str())
        })
        .collect();
    if let Some(pos) = rows.iter().position(|c| *c == Command::Quit) {
        let quit = rows.remove(pos);
        rows.push(quit);
    }
    rows
}

/// Pane count per tab, for the global panes popup.
pub(crate) fn pane_counts(tabs: &[Tab]) -> impl Iterator<Item = usize> + '_ {
    tabs.iter().map(|t| t.panes.len())
}

/// Flat (tab, pane) index over per-tab pane counts, tab-major.
pub(crate) fn flat_panes(counts: impl Iterator<Item = usize>) -> Vec<(usize, usize)> {
    let mut flat = Vec::new();
    for (ti, n) in counts.enumerate() {
        for pi in 0..n {
            flat.push((ti, pi));
        }
    }
    flat
}

/// Everything needed to spawn a new session, daemon-backed or local.
pub struct SpawnCtx<'a> {
    pub daemon: Option<&'a Daemon>,
    pub rt: &'a tokio::runtime::Runtime,
    pub signal: &'a PtyOutputSignal,
    pub cmd: &'a str,
    pub dir: &'a Path,
    pub rows: u16,
    pub cols: u16,
}

pub fn spawn_session(ctx: &SpawnCtx, id: Option<String>) -> anyhow::Result<PtySession> {
    if let Some(d) = ctx.daemon {
        let req = SpawnRequest {
            id: id.unwrap_or_else(piki_multiplex::session::new_session_id),
            command: ctx.cmd.to_string(),
            cwd: ctx.dir.to_path_buf(),
            env: session_env(),
            rows: ctx.rows,
            cols: ctx.cols,
            integration_on: true,
            attach: true,
            ..Default::default()
        };
        let att = d.spawn_attach(req)?;
        Ok(PtySession::from_attachment(
            att,
            true,
            None,
            Some(ctx.signal.clone()),
        ))
    } else {
        // Local spawns inherit our environment; only TERM needs overriding.
        let term_override = [(String::from("TERM"), String::from(SESSION_TERM))];
        Ok(ctx.rt.block_on(async {
            PtySession::spawn_with(
                SpawnOptions::new(ctx.dir, ctx.cmd)
                    .with_size(ctx.rows, ctx.cols)
                    .with_shell_integration(true)
                    .with_extra_env(&term_override)
                    .with_output_signal(ctx.signal.clone()),
            )
            .await
        })?)
    }
}

pub fn new_tab(ctx: &SpawnCtx, names: &mut TabNames) -> anyhow::Result<Tab> {
    let id = ctx
        .daemon
        .map(|_| piki_multiplex::session::new_session_id());
    let session = spawn_session(ctx, id.clone())?;
    let name = names.assign(id.as_deref());
    if let Some(id) = &id {
        names.set_panes(id, std::slice::from_ref(id), false);
    }
    Ok(Tab {
        panes: vec![Pane {
            session,
            name: default_name(ctx.cmd),
            id: id.clone(),
        }],
        focus: 0,
        split: SplitDir::Vertical,
        name,
        id,
        sizes: Vec::new(),
    })
}

fn cmd_new_tab(tabs: &mut Vec<Tab>, active: &mut usize, ctx: &SpawnCtx, names: &mut TabNames) {
    if let Ok(t) = new_tab(ctx, names) {
        tabs.push(t);
        *active = tabs.len() - 1;
    }
}

fn cmd_split(tabs: &mut [Tab], active: usize, dir: SplitDir, ctx: &SpawnCtx, names: &mut TabNames) {
    let Some(tab) = tabs.get_mut(active) else {
        return;
    };
    let new_id = ctx.daemon.map(|_| piki_multiplex::session::new_session_id());
    let session = match spawn_session(ctx, new_id.clone()) {
        Ok(s) => s,
        Err(_) => return,
    };
    let old_focus = tab.focus;
    tab.split = dir;
    tab.panes.push(Pane {
        session,
        name: default_name(ctx.cmd),
        id: new_id,
    });
    tab.focus = tab.panes.len() - 1;
    // Give the new pane half the old focused pane's space instead of
    // rebalancing every pane; `run()` resets to an even split anyway if
    // this ever falls out of sync with the pane count.
    if tab.sizes.len() == tab.panes.len() - 1
        && let Some(old) = tab.sizes.get_mut(old_focus)
    {
        let half = *old / 2;
        *old -= half;
        tab.sizes.push(half.max(1));
    }
    // Keep the persisted pane group in sync so a restart can regroup these
    // panes under one tab instead of scattering them.
    if let Some(tab_id) = &tab.id {
        let pane_ids: Vec<String> = tab.panes.iter().filter_map(|p| p.id.clone()).collect();
        names.set_panes(tab_id, &pane_ids, matches!(tab.split, SplitDir::Horizontal));
    }
}

/// Close the focused pane, else the current tab.
/// Returns true when the last tab was closed (caller should quit).
fn cmd_close(tabs: &mut Vec<Tab>, active: &mut usize, names: &mut TabNames) -> bool {
    if tabs.is_empty() {
        return true;
    }
    if *active >= tabs.len() {
        *active = tabs.len() - 1;
    }
    if tabs[*active].panes.len() > 1 {
        let idx = tabs[*active].focus;
        let _ = tabs[*active].panes[idx].session.kill();
        tabs[*active].panes.remove(idx);
        if tabs[*active].focus >= tabs[*active].panes.len() {
            tabs[*active].focus = tabs[*active].panes.len().saturating_sub(1);
        }
        // Give the closed pane's space to its neighbor instead of
        // rebalancing everyone; falls back to an even split (via `run()`)
        // if this was already out of sync with the pane count.
        if tabs[*active].sizes.len() == tabs[*active].panes.len() + 1 {
            let freed = tabs[*active].sizes.remove(idx);
            let neighbor = idx.min(tabs[*active].sizes.len().saturating_sub(1));
            if let Some(s) = tabs[*active].sizes.get_mut(neighbor) {
                *s += freed;
            }
        }
        // Trim the persisted group now rather than waiting for the next
        // restart's prune to notice the killed pane is gone.
        if let Some(tab_id) = &tabs[*active].id {
            let pane_ids: Vec<String> = tabs[*active]
                .panes
                .iter()
                .filter_map(|p| p.id.clone())
                .collect();
            let horizontal = matches!(tabs[*active].split, SplitDir::Horizontal);
            names.set_panes(tab_id, &pane_ids, horizontal);
        }
        false
    } else if tabs.len() > 1 {
        let _ = tabs[*active].session_mut().kill();
        tabs.remove(*active);
        if *active >= tabs.len() {
            *active = tabs.len() - 1;
        }
        false
    } else {
        // Last tab, last pane: still an explicit close, so kill it too —
        // only the "quit after this" signal is special here, not whether
        // the session should die.
        let _ = tabs[*active].session_mut().kill();
        true
    }
}

fn cmd_next_tab(tabs: &[Tab], active: &mut usize) {
    if !tabs.is_empty() {
        *active = (*active + 1) % tabs.len();
    }
}

fn cmd_prev_tab(tabs: &[Tab], active: &mut usize) {
    if !tabs.is_empty() {
        *active = if *active == 0 {
            tabs.len() - 1
        } else {
            *active - 1
        };
    }
}

fn cmd_next_pane(tabs: &mut [Tab], active: usize) {
    if let Some(tab) = tabs.get_mut(active)
        && tab.panes.len() > 1
    {
        tab.focus = (tab.focus + 1) % tab.panes.len();
    }
}

/// Select tab by index. `pub(crate)` for the digit shortcuts only: digits
/// are a closed family (1-9 can never grow), not palette commands, so they
/// dispatch here directly instead of through [`Command`].
pub(crate) fn cmd_select_tab(tabs: &[Tab], active: &mut usize, idx: usize) {
    if idx < tabs.len() {
        *active = idx;
    }
}

#[cfg(test)]
mod tests {
    use strum::IntoEnumIterator;

    use super::{Command, SESSION_TERM, default_name, filtered_commands, with_session_term};

    fn term_of(env: &[(String, String)]) -> Vec<&str> {
        env.iter()
            .filter(|(k, _)| k == "TERM")
            .map(|(_, v)| v.as_str())
            .collect()
    }

    #[test]
    fn overrides_outer_term() {
        let env = vec![
            ("TERM".to_string(), "xterm-ghostty".to_string()),
            ("HOME".to_string(), "/home/u".to_string()),
        ];
        let out = with_session_term(env);
        assert_eq!(term_of(&out), vec![SESSION_TERM]);
        assert!(out.contains(&("HOME".to_string(), "/home/u".to_string())));
    }

    #[test]
    fn adds_term_when_missing() {
        let env = vec![("PATH".to_string(), "/bin".to_string())];
        let out = with_session_term(env);
        assert_eq!(term_of(&out), vec![SESSION_TERM]);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn empty_env_gets_term() {
        let out = with_session_term(vec![]);
        assert_eq!(out, vec![("TERM".to_string(), SESSION_TERM.to_string())]);
    }

    fn ids() -> Vec<&'static str> {
        filtered_commands("").iter().map(|c| c.id()).collect()
    }

    #[test]
    fn palette_covers_all_prefix_commands() {
        for expected in [
            "new-tab",
            "next-tab",
            "prev-tab",
            "close",
            "switch-tab",
            "scroll-mode",
            "split-vertical",
            "split-horizontal",
            "next-pane",
            "quit",
            "rename-tab",
            "rename-pane",
            "list-panes",
        ] {
            assert!(
                ids().contains(&expected),
                "missing palette entry: {expected}"
            );
        }
    }

    #[test]
    fn palette_holds_only_commands() {
        assert_eq!(filtered_commands("").len(), Command::iter().len());
    }

    #[test]
    fn palette_has_single_close_entry() {
        let n = ids().iter().filter(|id| id.contains("close")).count();
        assert_eq!(n, 1, "expected a single close entry, found {n}");
    }

    #[test]
    fn palette_filter_matches_case_insensitively() {
        assert_eq!(filtered_commands("SCROLL"), filtered_commands("scroll"));
        assert_eq!(filtered_commands("TAB"), filtered_commands("tab"));
    }

    #[test]
    fn palette_entries_all_have_shortcuts() {
        for cmd in Command::iter() {
            assert!(!cmd.shortcut().is_empty(), "missing shortcut: {cmd:?}");
        }
    }

    #[test]
    fn default_name_uses_shell_basename() {
        assert_eq!(default_name("/bin/fish"), "fish");
        assert_eq!(default_name("zsh"), "zsh");
        assert_eq!(default_name("/bin/sh -l"), "sh");
        assert_eq!(default_name(""), "shell");
    }

    #[test]
    fn rename_first_keystroke_replaces_prefill() {
        let mut r = super::RenameState {
            pane: false,
            buffer: "fish".to_string(),
            fresh: true,
        };
        r.push('w');
        assert_eq!(r.buffer, "w");
        r.push('o');
        assert_eq!(r.buffer, "wo");
        r.backspace();
        assert_eq!(r.buffer, "w");
    }

    #[test]
    fn rename_backspace_clears_fresh_prefill() {
        let mut r = super::RenameState {
            pane: true,
            buffer: "fish".to_string(),
            fresh: true,
        };
        r.backspace();
        assert_eq!(r.buffer, "");
    }

    #[test]
    fn rename_buffer_capped_at_32_chars() {
        let mut r = super::RenameState {
            pane: false,
            buffer: String::new(),
            fresh: false,
        };
        for _ in 0..40 {
            r.push('x');
        }
        assert_eq!(r.buffer.len(), 32);
    }

    #[test]
    fn flat_panes_lists_every_pane_tab_major() {
        assert_eq!(
            super::flat_panes([2usize, 1, 3].into_iter()),
            vec![(0, 0), (0, 1), (1, 0), (2, 0), (2, 1), (2, 2),]
        );
        assert!(super::flat_panes([0usize; 0].into_iter()).is_empty());
        assert_eq!(
            super::flat_panes([0usize, 2].into_iter()),
            vec![(1, 0), (1, 1)]
        );
    }

    #[test]
    fn palette_quit_is_always_last() {
        for q in ["", "i", "t", "quit", "RENAME"] {
            let rows = filtered_commands(q);
            if rows.contains(&Command::Quit) {
                assert_eq!(
                    rows.last(),
                    Some(&Command::Quit),
                    "quit not last for query {q:?}"
                );
            }
        }
        assert_eq!(filtered_commands("quit"), vec![Command::Quit]);
    }

    #[test]
    fn palette_shortcuts_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for cmd in Command::iter() {
            assert!(
                seen.insert(cmd.shortcut()),
                "duplicate shortcut: {}",
                cmd.shortcut()
            );
        }
    }
}
