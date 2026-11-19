# piki-sessions — UI Style Guide

## Layout (not customizable)
- Terminal area occupies all rows except the last one.
- Status bar is exactly one row at the bottom.
- Overlays (command palette, panes list) are centered on screen.
- No tab bar widget; tabs are reflected in the status bar count and overlays.

## Color palette (customizable via theme)
| Element            | Default color     |
|--------------------|-------------------|
| App / tabs count   | Cyan              |
| Agent status       | Green             |
| Pane / shell       | Magenta           |
| Active pane border | Green             |
| Prefix ON          | Black on Yellow    |
| Prefix off         | DarkGray          |
| Hints              | DarkGray          |
| Metrics            | Yellow            |
| Tabs popup accent  | Cyan              |
| Panes popup accent | Magenta           |
| Palette accent     | Yellow            |
| Rename tab accent  | Cyan              |
| Rename pane accent | Magenta           |
| Current marker (*) | Green             |
| Popup headers      | DarkGray          |

## Customizable
- Theme colors (foreground/background per element).
- Keybindings (prefix key, app chords).
- Status bar segments (which info to show).
- Shell, scrollback size, idle timeout (config.toml, Phase 5).

## Not customizable
- Overall layout structure (terminal + 1-row status bar).
- Overlay positioning (centered).
- Prefix-based interaction model.
- Minimum terminal size guards (>= 4 cols/rows).
