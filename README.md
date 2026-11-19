# piki-sessions

tmux-style terminal multiplexer built on `piki-multiplex` (PTY + persistent
daemon + restore + shell integration). Binary: `pks`.

## Build

```bash
cargo build
cargo build --release
```

## Usage

```bash
# Run the TUI (default)
pks

# Daemon
pks serve               # run in foreground
pks serve --foreground  # (same)
pks list                # list sessions in the daemon
pks attach <id>         # attach to an existing session
pks shutdown            # stop the daemon (refused if live sessions)
pks shutdown --force    # force stop
```

## Keybindings (TUI)

- `Ctrl+G` prefix, then:
  - `c` new tab
  - `n` / `p` next / previous tab
  - `x` close pane or tab (quits if last)
  - `v` / `h` split pane vertically / horizontally
  - `o` next pane
  - `s` switch tab (list)
  - `w` list all panes as tab.pane + names (Enter jumps)
  - `[` scrollback mode (`/` search, `y` copy, `q` exit)
  - `k` command palette
  - `,` / `R` rename tab / rename pane
  - `1`-`9` select tab
- `Ctrl+Q` quit (detach from remote sessions)

## Tab names

New tabs are named `tab-0`, `tab-1`, … from a counter that lives as long as
the daemon (it restarts when the daemon dies). Names — including your
renames — persist across TUI restarts. Pane names default to the shell and
are not persisted.

## Status

See `PLAN.md` for roadmap and progress.
