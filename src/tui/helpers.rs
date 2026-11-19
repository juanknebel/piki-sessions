use std::io;
use std::path::PathBuf;

use crossterm::ExecutableCommand;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};
use ratatui::style::{Color, Style};
use ratatui::text::Span;

pub fn key_to_bytes(key: KeyEvent) -> Option<Vec<u8>> {
    let mods = key.modifiers;
    match key.code {
        KeyCode::Char(c) => {
            if mods.contains(KeyModifiers::CONTROL) {
                if c.is_ascii_lowercase() {
                    Some(vec![(c as u8) - b'a' + 1])
                } else if c.is_ascii_uppercase() {
                    Some(vec![(c as u8) - b'A' + 1])
                } else {
                    None
                }
            } else if mods.contains(KeyModifiers::ALT) {
                let mut v = vec![0x1b];
                let mut buf = [0u8; 4];
                v.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                Some(v)
            } else {
                let mut buf = [0u8; 4];
                Some(c.encode_utf8(&mut buf).as_bytes().to_vec())
            }
        }
        KeyCode::Enter => Some(vec![b'\r']),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Tab => Some(vec![b'\t']),
        KeyCode::Esc => Some(vec![0x1b]),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        KeyCode::Home => Some(b"\x1b[H".to_vec()),
        KeyCode::End => Some(b"\x1b[F".to_vec()),
        KeyCode::PageUp => Some(b"\x1b[5~".to_vec()),
        KeyCode::PageDown => Some(b"\x1b[6~".to_vec()),
        KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
        KeyCode::Insert => Some(b"\x1b[2~".to_vec()),
        _ => None,
    }
}

pub fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("PIKI_SESSIONS_DATA_DIR") {
        return PathBuf::from(d);
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("piki-sessions")
}

pub fn launch_daemon() -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    std::process::Command::new(exe).arg("serve").spawn()?;
    Ok(())
}

/// Marker for a selectable overlay row: accent `>` when selected, green
/// `*` for the current item, blank otherwise.
pub fn marker_span(selected: bool, current: bool, accent: Color) -> Span<'static> {
    if selected {
        Span::styled(">", Style::default().fg(accent))
    } else if current {
        Span::styled("*", Style::default().fg(Color::Green))
    } else {
        Span::raw(" ")
    }
}

/// Restores the terminal when dropped, including on panic.
/// Create after entering raw mode / alternate screen.
pub struct TerminalGuard {
    mouse: bool,
}

impl TerminalGuard {
    pub fn new(mouse: bool) -> Self {
        Self { mouse }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        if self.mouse {
            let _ = crossterm::execute!(io::stdout(), crossterm::event::DisableMouseCapture);
        }
        let _ = io::stdout().execute(LeaveAlternateScreen);
    }
}

#[cfg(test)]
mod tests {
    use super::{Color, marker_span};

    #[test]
    fn marker_uses_accent_when_selected() {
        let s = marker_span(true, false, Color::Cyan);
        assert_eq!(s.content.as_ref(), ">");
        assert_eq!(s.style.fg, Some(Color::Cyan));
    }

    #[test]
    fn marker_uses_green_star_for_current() {
        let s = marker_span(false, true, Color::Cyan);
        assert_eq!(s.content.as_ref(), "*");
        assert_eq!(s.style.fg, Some(Color::Green));
    }

    #[test]
    fn marker_is_blank_otherwise() {
        let s = marker_span(false, false, Color::Cyan);
        assert_eq!(s.content.as_ref(), " ");
        assert_eq!(s.style.fg, None);
    }

    #[test]
    fn terminal_guard_drop_is_safe_outside_raw_mode() {
        // Must not panic when the terminal was never put in raw mode.
        drop(super::TerminalGuard::new(true));
        drop(super::TerminalGuard::new(false));
    }
}
