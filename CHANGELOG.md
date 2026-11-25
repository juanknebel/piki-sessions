# Changelog

All notable changes to this project are documented here.
Format loosely follows Keep a Changelog.

## [Unreleased]
### Added
- Pane zoom (`Ctrl+G f`): fills the tab with the focused pane, leaving the
  rest of the layout untouched underneath so un-zooming restores it exactly.

## [0.3.0] - 2026-11-24
### Added
- Mouse support in the TUI: click a pane to focus it, click a tab label to
  switch tabs (double-click to rename it), drag the divider between panes
  to resize them, and forward clicks/drags/scroll to the focused pane's own
  app (vim, htop, tmux, ...) when it has negotiated xterm mouse reporting.
### Fixed
- Startup no longer reattaches to exited daemon sessions (which died
  instantly and could leave `pks` exiting immediately with nothing shown),
  and multi-pane tabs are now regrouped correctly across restarts instead
  of scattering into one tab per pane.
- Closing the last pane/tab (Ctrl+G x) now actually kills its session
  instead of leaving it "live" in the daemon after `pks` exits.

## [0.2.0] - 2026-11-20
### Added
- tmux-style TUI (`pks`): tabs, split panes, command palette, scrollback
  with search, system-metrics status bar.
- Persistent daemon sessions with reattach, restore, and shell integration
  (via `piki-multiplex`).
- Tab names (`tab-0`, `tab-1`, …) persisted per daemon generation, plus
  tab/pane rename prompts.
- Active-pane borders, per-overlay accent colors, tabs/panes list popups.
- `scripts/install.sh` release installer and `scripts/release.sh`.
