use std::io;
use std::path::PathBuf;

use crossterm::ExecutableCommand;
use crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::terminal::{LeaveAlternateScreen, disable_raw_mode};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

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

/// True when `(col, row)` falls inside `area`.
pub fn hit(area: &Rect, col: u16, row: u16) -> bool {
    col >= area.x && col < area.x + area.width && row >= area.y && row < area.y + area.height
}

/// Keep a tab's per-pane sizes valid for `n` panes sharing `total` cells
/// along the split direction: reset to an even split when the pane count
/// changed underneath it (a split or close that didn't update `sizes`
/// itself), otherwise rescale proportionally so they still sum to `total`
/// (e.g. after a terminal resize) while keeping the user's drag-resized
/// ratios. A no-op if `sizes` already sums to `total` at the right length.
pub fn normalize_sizes(sizes: &mut Vec<u16>, n: usize, total: u16) {
    if n == 0 || total == 0 {
        sizes.clear();
        return;
    }
    let even_split = |total: u16| -> Vec<u16> {
        let base = total / n as u16;
        let mut v = vec![base; n];
        if let Some(last) = v.last_mut() {
            *last += total - base * n as u16;
        }
        v
    };
    if sizes.len() != n {
        *sizes = even_split(total);
        return;
    }
    let sum: u32 = sizes.iter().map(|&s| s as u32).sum();
    if sum == 0 {
        *sizes = even_split(total);
        return;
    }
    if sum == total as u32 {
        return;
    }
    let mut rescaled: Vec<u16> = sizes
        .iter()
        .map(|&s| (((s as u32) * (total as u32)) / sum).max(1) as u16)
        .collect();
    let rescaled_sum: u16 = rescaled.iter().sum();
    if let Some(last) = rescaled.last_mut() {
        *last = last.saturating_add(total.saturating_sub(rescaled_sum));
    }
    *sizes = rescaled;
}

/// Encode a mouse event as the xterm byte sequence the focused pane's app
/// asked for (`None` when it hasn't enabled mouse reporting, or the
/// negotiated mode doesn't cover this kind of event). `col`/`row` are
/// 1-based and already relative to the pane's own content area.
pub fn mouse_to_bytes(
    me: MouseEvent,
    mode: MouseProtocolMode,
    encoding: MouseProtocolEncoding,
    col: u16,
    row: u16,
) -> Option<Vec<u8>> {
    if mode == MouseProtocolMode::None {
        return None;
    }

    let (base, is_release, is_motion) = match me.kind {
        MouseEventKind::Down(b) => (button_number(b), false, false),
        MouseEventKind::Up(b) => (button_number(b), true, false),
        MouseEventKind::Drag(b) => (button_number(b), false, true),
        MouseEventKind::Moved => (3, false, true),
        MouseEventKind::ScrollUp => (64, false, false),
        MouseEventKind::ScrollDown => (65, false, false),
        MouseEventKind::ScrollLeft => (66, false, false),
        MouseEventKind::ScrollRight => (67, false, false),
    };

    // Only report what the negotiated mode actually covers: Press wants
    // button-down only, PressRelease adds release, ButtonMotion adds drag
    // (but not bare moves with no button held), AnyMotion wants everything.
    let covered = match mode {
        MouseProtocolMode::None => false,
        MouseProtocolMode::Press => !is_release && !is_motion,
        MouseProtocolMode::PressRelease => !is_motion,
        MouseProtocolMode::ButtonMotion => !matches!(me.kind, MouseEventKind::Moved),
        MouseProtocolMode::AnyMotion => true,
    };
    if !covered {
        return None;
    }

    let mut code = base;
    if is_motion {
        code |= 0x20;
    }
    if me.modifiers.contains(KeyModifiers::SHIFT) {
        code |= 0x04;
    }
    if me.modifiers.contains(KeyModifiers::ALT) {
        code |= 0x08;
    }
    if me.modifiers.contains(KeyModifiers::CONTROL) {
        code |= 0x10;
    }

    match encoding {
        MouseProtocolEncoding::Sgr => {
            let tail = if is_release { 'm' } else { 'M' };
            Some(format!("\x1b[<{code};{col};{row}{tail}").into_bytes())
        }
        // Default/UTF-8 (X10/VT200) encoding can't report which button was
        // released; xterm always sends code 3 for a release here. Legacy
        // byte-offset encoding also caps coordinates at 223.
        MouseProtocolEncoding::Default | MouseProtocolEncoding::Utf8 => {
            let cb = if is_release { 3 } else { code };
            let coord = |v: u16| -> u8 { v.min(223) as u8 + 32 };
            Some(vec![0x1b, b'[', b'M', cb + 32, coord(col), coord(row)])
        }
    }
}

fn button_number(b: MouseButton) -> u8 {
    match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
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
    use super::{
        Color, MouseButton, MouseEvent, MouseEventKind, MouseProtocolEncoding, MouseProtocolMode,
        hit, marker_span, mouse_to_bytes, normalize_sizes,
    };
    use crossterm::event::KeyModifiers;
    use ratatui::layout::Rect;

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

    #[test]
    fn hit_tests_rect_bounds() {
        let r = Rect::new(2, 3, 4, 5); // x:2..6, y:3..8
        assert!(hit(&r, 2, 3));
        assert!(hit(&r, 5, 7));
        assert!(!hit(&r, 6, 3));
        assert!(!hit(&r, 2, 8));
        assert!(!hit(&r, 1, 3));
    }

    #[test]
    fn normalize_resets_evenly_on_count_mismatch() {
        let mut sizes = vec![10, 10];
        normalize_sizes(&mut sizes, 3, 10);
        assert_eq!(sizes, vec![3, 3, 4]);
        assert_eq!(sizes.iter().sum::<u16>(), 10);
    }

    #[test]
    fn normalize_keeps_matching_sizes_untouched() {
        let mut sizes = vec![7, 13];
        normalize_sizes(&mut sizes, 2, 20);
        assert_eq!(sizes, vec![7, 13]);
    }

    #[test]
    fn normalize_rescales_proportionally_on_resize() {
        let mut sizes = vec![10, 30]; // 1:3 ratio over 40
        normalize_sizes(&mut sizes, 2, 80); // same ratio over 80
        assert_eq!(sizes, vec![20, 60]);
        assert_eq!(sizes.iter().sum::<u16>(), 80);
    }

    #[test]
    fn normalize_handles_empty_input() {
        let mut sizes = Vec::new();
        normalize_sizes(&mut sizes, 0, 0);
        assert!(sizes.is_empty());
    }

    fn click(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn mouse_bytes_none_when_app_has_no_mouse_mode() {
        assert!(
            mouse_to_bytes(
                click(1, 1),
                MouseProtocolMode::None,
                MouseProtocolEncoding::Sgr,
                5,
                6
            )
            .is_none()
        );
    }

    #[test]
    fn mouse_bytes_sgr_press_and_release() {
        let down = mouse_to_bytes(
            click(0, 0),
            MouseProtocolMode::PressRelease,
            MouseProtocolEncoding::Sgr,
            5,
            6,
        )
        .unwrap();
        assert_eq!(down, b"\x1b[<0;5;6M");

        let up = mouse_to_bytes(
            MouseEvent {
                kind: MouseEventKind::Up(MouseButton::Left),
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
            MouseProtocolMode::PressRelease,
            MouseProtocolEncoding::Sgr,
            5,
            6,
        )
        .unwrap();
        assert_eq!(up, b"\x1b[<0;5;6m");
    }

    #[test]
    fn mouse_bytes_press_only_mode_drops_release() {
        let up = MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        assert!(
            mouse_to_bytes(up, MouseProtocolMode::Press, MouseProtocolEncoding::Sgr, 1, 1)
                .is_none()
        );
    }

    #[test]
    fn mouse_bytes_button_motion_drops_bare_move() {
        let moved = MouseEvent {
            kind: MouseEventKind::Moved,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        assert!(
            mouse_to_bytes(
                moved,
                MouseProtocolMode::ButtonMotion,
                MouseProtocolEncoding::Sgr,
                1,
                1
            )
            .is_none()
        );
        assert!(
            mouse_to_bytes(
                moved,
                MouseProtocolMode::AnyMotion,
                MouseProtocolEncoding::Sgr,
                1,
                1
            )
            .is_some()
        );
    }

    #[test]
    fn mouse_bytes_default_encoding_offsets_by_32() {
        let bytes = mouse_to_bytes(
            click(0, 0),
            MouseProtocolMode::PressRelease,
            MouseProtocolEncoding::Default,
            1,
            1,
        )
        .unwrap();
        assert_eq!(bytes, vec![0x1b, b'[', b'M', 32, 33, 33]);
    }
}
