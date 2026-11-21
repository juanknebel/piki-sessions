# Changelog

All notable changes to this project are documented here.
Format loosely follows Keep a Changelog.

## [Unreleased]

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
