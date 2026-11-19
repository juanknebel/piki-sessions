use std::io;

use crossterm::ExecutableCommand;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use piki_multiplex::pty::{PtyOutputSignal, PtySession};
use piki_multiplex::session::client::Attachment;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tui_term::widget::PseudoTerminal;

use super::helpers::{TerminalGuard, key_to_bytes};

pub fn attach(att: Attachment) -> anyhow::Result<()> {
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let cols = if cols < 4 { 80 } else { cols };
    let rows = if rows < 4 { 24 } else { rows };

    let signal = PtyOutputSignal::new();
    let session = PtySession::from_attachment(att, true, None, Some(signal.clone()));

    // resize to current terminal
    let _ = session.resize(rows, cols);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    let _term_guard = TerminalGuard::new(false);
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let rt = tokio::runtime::Runtime::new()?;

    let mut quit = false;
    while !quit {
        terminal.draw(|f| {
            let area = f.area();
            let parser = session.parser().lock();
            let pt = PseudoTerminal::new(parser.screen());
            f.render_widget(pt, area);
        })?;

        if !session.peek_alive() {
            // session exited; wait for user to quit
        }

        rt.block_on(async {
            tokio::select! {
                _ = signal.notified() => {}
                _ = tokio::time::sleep(std::time::Duration::from_millis(8)) => {}
            }
        });

        while event::poll(std::time::Duration::from_millis(0))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.code == KeyCode::Char('q')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        quit = true;
                    } else if let Some(bytes) = key_to_bytes(key) {
                        let _ = session.write(&bytes);
                    }
                }
                Event::Resize(c, r) if c >= 4 && r >= 4 => {
                    let _ = session.resize(r, c);
                }
                _ => {}
            }
        }
    }

    disable_raw_mode()?;
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    // Detach (remote) — do not kill.
    let _ = session;
    Ok(())
}
