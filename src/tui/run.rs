use std::io;
use std::time::Instant;

use crossterm::ExecutableCommand;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use piki_multiplex::pty::{PtyOutputSignal, PtySession};
use piki_multiplex::session::client::ensure_daemon;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use tui_term::widget::PseudoTerminal;

use super::actions::{
    Command, ExecCtx, RenameState, SpawnCtx, cmd_select_tab, default_name, filtered_commands,
    flat_panes, new_tab, pane_counts,
};
use super::helpers::{TerminalGuard, data_dir, key_to_bytes, launch_daemon, marker_span};
use super::metrics::{read_battery, read_cpu_sample, read_mem_info};
use super::names::TabNames;
use super::{Pane, SplitDir, Tab};

/// Border for a pane: only the focused pane of a multi-pane tab gets one.
/// Single-pane tabs render borderless at full area.
pub fn pane_border(
    pane_count: usize,
    focus: usize,
    index: usize,
    title: Option<&str>,
) -> Option<Block<'static>> {
    if pane_count > 1 && index == focus {
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Green));
        if let Some(t) = title {
            block = block.title_top(t.to_string());
        }
        Some(block)
    } else {
        None
    }
}

pub fn run() -> anyhow::Result<()> {
    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let mut cols = if cols < 4 { 80 } else { cols };
    let rows = if rows < 4 { 24 } else { rows };

    let cmd = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
    let dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));

    let rt = tokio::runtime::Runtime::new()?;
    let signal = PtyOutputSignal::new();

    let mut pty_rows = rows.saturating_sub(1).max(1);

    // Try to use the persistent session daemon.
    let socket = data_dir().join("sessions").join("daemon.sock");
    let daemon = ensure_daemon(&socket, launch_daemon).ok();

    // Tab names live as long as the daemon: persisted per session id,
    // with a counter that resets when the daemon generation changes.
    let mut names = match &daemon {
        Some(_) => TabNames::load(data_dir().join("sessions").join("tab-names.json"), &socket),
        None => TabNames::memory(),
    };

    let ctx = |rows: u16, cols: u16| SpawnCtx {
        daemon: daemon.as_ref(),
        rt: &rt,
        signal: &signal,
        cmd: cmd.as_str(),
        dir: dir.as_path(),
        rows,
        cols,
    };

    // Initial tabs: reattach daemon sessions, grouped back into their
    // original multi-pane tabs where we persisted that layout, or create one.
    let mut tabs: Vec<Tab> = if let Some(d) = &daemon {
        let mut ts = Vec::new();
        if let Ok(sessions) = d.list() {
            names.prune(&sessions.iter().map(|s| s.id.clone()).collect());

            // Exited sessions have nothing live to reattach to; only live
            // ones get regrouped into tabs below.
            let live_ids: std::collections::HashSet<String> = sessions
                .iter()
                .filter(|s| s.state.is_live())
                .map(|s| s.id.clone())
                .collect();

            // Reverse index: a pane's session id -> the tab id (its tab's
            // first pane) it was persisted under, so panes attach in the
            // order they're found instead of one tab per session.
            let mut member_of: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            for (tab_id, pane_ids) in names.all_tabs() {
                for pid in pane_ids {
                    member_of.insert(pid.clone(), tab_id.clone());
                }
            }

            let mut seen_tabs = std::collections::HashSet::new();
            for info in &sessions {
                if !live_ids.contains(&info.id) {
                    continue;
                }
                let tab_id = member_of
                    .get(&info.id)
                    .cloned()
                    .unwrap_or_else(|| info.id.clone());
                if !seen_tabs.insert(tab_id.clone()) {
                    continue;
                }
                let pane_ids: Vec<String> = names
                    .panes_for(&tab_id)
                    .map(|ids| {
                        ids.iter()
                            .filter(|id| live_ids.contains(*id))
                            .cloned()
                            .collect()
                    })
                    .filter(|ids: &Vec<String>| !ids.is_empty())
                    .unwrap_or_else(|| vec![info.id.clone()]);

                let mut panes = Vec::new();
                for pid in &pane_ids {
                    let Some(pinfo) = sessions.iter().find(|s| &s.id == pid) else {
                        continue;
                    };
                    if let Ok(att) = d.attach(pid, pty_rows, cols) {
                        let session =
                            PtySession::from_attachment(att, true, None, Some(signal.clone()));
                        panes.push(Pane {
                            session,
                            name: default_name(&pinfo.command),
                            id: Some(pid.clone()),
                        });
                    }
                }
                if panes.is_empty() {
                    continue;
                }
                let horizontal = names.horizontal_for(&tab_id);
                // Re-register the group (covers legacy/ungrouped sessions
                // from before this persisted, so future restarts regroup).
                names.set_panes(&tab_id, &pane_ids, horizontal);
                ts.push(Tab {
                    panes,
                    focus: 0,
                    split: if horizontal {
                        SplitDir::Horizontal
                    } else {
                        SplitDir::Vertical
                    },
                    name: names.assign(Some(&tab_id)),
                    id: Some(tab_id),
                });
            }
        }
        if ts.is_empty() {
            ts.push(new_tab(&ctx(pty_rows, cols), &mut names)?);
        }
        ts
    } else {
        vec![new_tab(&ctx(pty_rows, cols), &mut names)?]
    };
    let mut active: usize = 0;
    let mut prefix_pending = false;
    let mut needs_draw = true;
    let mut last_metric = Instant::now();
    let mut cpu_usage: f64 = 0.0;
    let mut mem_used: f64 = 0.0;
    let mut mem_total: f64 = 0.0;
    let mut bat_cap: Option<u8> = None;
    let mut bat_status: String = String::new();
    let mut prev_idle: u64 = 0;
    let mut prev_total: u64 = 0;
    let mut palette_open = false;
    let mut palette_query = String::new();
    let mut palette_sel: usize = 0;
    let mut tabs_open = false;
    let mut tabs_sel: usize = 0;
    let mut panes_open = false;
    let mut panes_sel: usize = 0;
    let mut scroll_mode = false;
    let mut rename: Option<RenameState> = None;
    let mut search_mode = false;
    let mut search_query = String::new();
    let mut quit = false;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    stdout.execute(crossterm::event::EnableMouseCapture)?;
    let _term_guard = TerminalGuard::new(true);
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    while !quit {
        // Drop dead tabs (auto-close). Keep at least the intent to exit if none.
        if tabs.is_empty() {
            break;
        }
        if active >= tabs.len() {
            active = tabs.len() - 1;
        }
        // Auto-close exited tabs when more than one exists.
        if tabs.len() > 1 && !tabs[active].session().peek_alive() {
            let _ = tabs[active].session_mut().kill();
            tabs.remove(active);
            if active >= tabs.len() {
                active = tabs.len().saturating_sub(1);
            }
            needs_draw = true;
            continue;
        }

        // Sample system metrics ~1/s.
        if last_metric.elapsed() >= std::time::Duration::from_secs(1) {
            let (idle, total) = read_cpu_sample();
            if prev_total > 0 {
                let idle_d = idle.saturating_sub(prev_idle);
                let total_d = total.saturating_sub(prev_total);
                if total_d > 0 {
                    cpu_usage = (1.0 - idle_d as f64 / total_d as f64) * 100.0;
                }
            }
            prev_idle = idle;
            prev_total = total;
            let (mem_used_gb, mem_total_gb, _) = read_mem_info();
            mem_used = mem_used_gb;
            mem_total = mem_total_gb;
            if let Some((c, st)) = read_battery() {
                bat_cap = Some(c);
                bat_status = st;
            } else {
                bat_cap = None;
            }
            last_metric = Instant::now();
            needs_draw = true;
        }

        let had_output = signal.take();
        if had_output {
            needs_draw = true;
        }
        let mem_used_r = &mem_used;
        let mem_total_r = &mem_total;
        let cpu_usage_r = &cpu_usage;
        let bat_cap_r = &bat_cap;
        let bat_status_r = &bat_status;
        if needs_draw {
            terminal.draw(|f| {
                let area = f.area();
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(1), Constraint::Length(1)])
                    .split(area);

                let tab = &tabs[active];
                let n = tab.panes.len().max(1);
                let pane_areas = if n == 1 {
                    vec![chunks[0]]
                } else {
                    let dir = match tab.split {
                        SplitDir::Horizontal => Direction::Horizontal,
                        SplitDir::Vertical => Direction::Vertical,
                    };
                    Layout::default()
                        .direction(dir)
                        .constraints(vec![Constraint::Ratio(1, n as u32); n])
                        .split(chunks[0])
                        .to_vec()
                };
                for (i, pane) in tab.panes.iter().enumerate() {
                    let area = pane_areas[i];
                    let title = (tab.panes.len() > 1).then_some(pane.name.as_str());
                    if let Some(block) = pane_border(tab.panes.len(), tab.focus, i, title) {
                        let inner = block.inner(area);
                        if inner.width >= 4 && inner.height >= 4 {
                            let _ = pane.session.resize(inner.height, inner.width);
                        }
                        let parser = pane.session.parser().lock();
                        let pt = PseudoTerminal::new(parser.screen());
                        f.render_widget(block, area);
                        f.render_widget(pt, inner);
                    } else {
                        if area.width >= 4 && area.height >= 4 {
                            let _ = pane.session.resize(area.height, area.width);
                        }
                        let parser = pane.session.parser().lock();
                        let pt = PseudoTerminal::new(parser.screen());
                        f.render_widget(pt, area);
                    }
                }

                // Clean layout: terminal + status bar
                let agent_status = tabs[active]
                    .session()
                    .shell()
                    .map(|sh| {
                        let s = sh.lock();
                        if s.state.sidecar.is_some() {
                            "active"
                        } else {
                            "idle"
                        }
                    })
                    .unwrap_or("idle")
                    .to_string();
                let agent_span = Span::styled(
                    format!(" 🤖 agent: {} ", agent_status),
                    Style::default().fg(Color::Green),
                );
                let tab_span = Span::styled(
                    format!(
                        " │ tab {}/{}: {} ",
                        active + 1,
                        tabs.len(),
                        tabs[active].name
                    ),
                    Style::default().fg(Color::Cyan),
                );
                let pane_span = Span::styled(
                    format!(
                        " │ pane {}/{}: {} ",
                        tabs[active].focus + 1,
                        tabs[active].panes.len(),
                        tabs[active].panes[tabs[active].focus].name
                    ),
                    Style::default().fg(Color::Magenta),
                );
                let prefix_span = if prefix_pending {
                    Span::styled(
                        " │ prefix: ON ",
                        Style::default().fg(Color::Black).bg(Color::Yellow),
                    )
                } else {
                    Span::styled(" │ prefix: off ", Style::default().fg(Color::DarkGray))
                };
                let scroll_span = if scroll_mode {
                    Span::styled(
                        " │ SCROLL ",
                        Style::default().fg(Color::Black).bg(Color::Cyan),
                    )
                } else {
                    Span::styled("", Style::default())
                };
                let metrics = if let Some(c) = *bat_cap_r {
                    format!(
                        "🔋 {}% {} │ CPU {:.0}% │ MEM {:.1}/{:.1}GB",
                        c, bat_status_r, *cpu_usage_r, *mem_used_r, *mem_total_r
                    )
                } else {
                    format!(
                        "CPU {:.0}% │ MEM {:.1}/{:.1}GB",
                        *cpu_usage_r, *mem_used_r, *mem_total_r
                    )
                };
                let mut spans = vec![agent_span, tab_span];
                if tabs[active].panes.len() > 1 {
                    spans.push(pane_span);
                }
                spans.push(prefix_span);
                spans.push(scroll_span);
                let left_line = Line::from(spans);
                let left_para = Paragraph::new(left_line);
                let right_para = Paragraph::new(metrics)
                    .alignment(Alignment::Right)
                    .style(Style::default().fg(Color::Yellow));
                let status_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Min(0), Constraint::Length(28)])
                    .split(chunks[1]);
                f.render_widget(left_para, status_chunks[0]);
                f.render_widget(right_para, status_chunks[1]);
                if tabs_open {
                    let area = f.area();
                    let w = 40.min(area.width);
                    let h = (tabs.len() as u16 + 3).min(area.height).max(4);
                    let x = area.x + (area.width.saturating_sub(w)) / 2;
                    let y = area.y + (area.height.saturating_sub(h)) / 2;
                    let popup = Rect::new(x, y, w, h);
                    let mut lines = vec![Line::styled(
                        format!("Tabs ({}):", tabs.len()),
                        Style::default().fg(Color::DarkGray),
                    )];
                    for (i, tab) in tabs.iter().enumerate() {
                        lines.push(Line::from(vec![
                            marker_span(i == tabs_sel, i == active, Color::Cyan),
                            Span::raw(format!(" {}  {}", i + 1, tab.name)),
                        ]));
                    }
                    let para = Paragraph::new(lines).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .border_style(Style::default().fg(Color::Cyan))
                            .title(Span::styled("Tabs", Style::default().fg(Color::Cyan))),
                    );
                    f.render_widget(Clear, popup);
                    f.render_widget(para, popup);
                }
                if panes_open {
                    let area = f.area();
                    let flat = flat_panes(pane_counts(&tabs));
                    let focus = tabs[active].focus;
                    let w = 40.min(area.width);
                    let h = (flat.len() as u16 + 3).min(area.height).max(4);
                    let x = area.x + (area.width.saturating_sub(w)) / 2;
                    let y = area.y + (area.height.saturating_sub(h)) / 2;
                    let popup = Rect::new(x, y, w, h);
                    let mut lines = vec![Line::styled(
                        format!("Panes ({}):", flat.len()),
                        Style::default().fg(Color::DarkGray),
                    )];
                    for (i, &(ti, pi)) in flat.iter().enumerate() {
                        lines.push(Line::from(vec![
                            marker_span(
                                i == panes_sel,
                                ti == active && pi == focus,
                                Color::Magenta,
                            ),
                            Span::raw(format!(
                                " {}.{}  {} ({})",
                                ti + 1,
                                pi + 1,
                                tabs[ti].panes[pi].name,
                                tabs[ti].name
                            )),
                        ]));
                    }
                    let para = Paragraph::new(lines).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .border_style(Style::default().fg(Color::Magenta))
                            .title(Span::styled("Panes", Style::default().fg(Color::Magenta))),
                    );
                    f.render_widget(Clear, popup);
                    f.render_widget(para, popup);
                }

                if palette_open {
                    let area = f.area();
                    let filtered = filtered_commands(&palette_query);
                    let w = 60.min(area.width);
                    let h = (filtered.len() as u16 + 3).min(area.height).max(4);
                    if h > 0 {
                        let x = area.x + (area.width.saturating_sub(w)) / 2;
                        let y = area.y + (area.height.saturating_sub(h)) / 2;
                        let popup = Rect::new(x, y, w, h);
                        let mut lines = vec![Line::from(vec![
                            marker_span(true, false, Color::Yellow),
                            Span::raw(format!(" {}", palette_query)),
                        ])];
                        for (i, cmd) in filtered.iter().enumerate() {
                            lines.push(Line::from(vec![
                                marker_span(i == palette_sel, false, Color::Yellow),
                                Span::raw(format!(" {}  -  {}  (", cmd.id(), cmd.description())),
                                Span::styled(cmd.shortcut(), Style::default().fg(Color::DarkGray)),
                                Span::styled(")", Style::default().fg(Color::DarkGray)),
                            ]));
                        }
                        if filtered.is_empty() {
                            lines.push(Line::styled(
                                "(no matches)",
                                Style::default().fg(Color::DarkGray),
                            ));
                        }
                        let para = Paragraph::new(lines).block(
                            Block::default()
                                .borders(Borders::ALL)
                                .border_type(BorderType::Rounded)
                                .border_style(Style::default().fg(Color::Yellow))
                                .title(Span::styled(
                                    "Command Palette",
                                    Style::default().fg(Color::Yellow),
                                )),
                        );
                        f.render_widget(Clear, popup);
                        f.render_widget(para, popup);
                    }
                }

                if let Some(r) = &rename {
                    let area = f.area();
                    let w = 50.min(area.width);
                    let h = 3;
                    let x = area.x + (area.width.saturating_sub(w)) / 2;
                    let y = area.y + (area.height.saturating_sub(h)) / 2;
                    let popup = Rect::new(x, y, w, h);
                    let title = if r.pane { "Rename pane" } else { "Rename tab" };
                    let accent = if r.pane { Color::Magenta } else { Color::Cyan };
                    let line = Line::from(vec![
                        Span::styled(">", Style::default().fg(accent)),
                        Span::raw(format!(" {}", r.buffer)),
                        Span::styled("█", Style::default().fg(accent)),
                    ]);
                    let para = Paragraph::new(line).block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .border_style(Style::default().fg(accent))
                            .title(Span::styled(title, Style::default().fg(accent))),
                    );
                    f.render_widget(Clear, popup);
                    f.render_widget(para, popup);
                }
            })?;
            needs_draw = false;
        }

        // Wait for output or a timeout.
        rt.block_on(async {
            tokio::select! {
                _ = signal.notified() => {}
                _ = tokio::time::sleep(std::time::Duration::from_millis(8)) => {}
            }
        });

        // Drain available input events.
        while event::poll(std::time::Duration::from_millis(0))? {
            match event::read()? {
                Event::Mouse(me) => {
                    needs_draw = true;
                    if tabs.is_empty() {
                        continue;
                    }
                    let session = tabs[active].session();
                    let is_alt = session.parser().lock().screen().alternate_screen();
                    match me.kind {
                        crossterm::event::MouseEventKind::ScrollUp => {
                            if is_alt {
                                let _ = session.write(b"\x1b[A");
                            } else {
                                let cur = session.parser().lock().screen().scrollback();
                                session
                                    .parser()
                                    .lock()
                                    .screen_mut()
                                    .set_scrollback(cur.saturating_add(3));
                            }
                        }
                        crossterm::event::MouseEventKind::ScrollDown => {
                            if is_alt {
                                let _ = session.write(b"\x1b[B");
                            } else {
                                let cur = session.parser().lock().screen().scrollback();
                                session
                                    .parser()
                                    .lock()
                                    .screen_mut()
                                    .set_scrollback(cur.saturating_sub(3));
                            }
                        }
                        _ => {}
                    }
                }
                Event::Key(key) => {
                    needs_draw = true;
                    if rename.is_some() {
                        // C-q still quits from the prompt.
                        if key.code == KeyCode::Char('q')
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            quit = true;
                            continue;
                        }
                        match key.code {
                            KeyCode::Esc => rename = None,
                            KeyCode::Enter => {
                                if let Some(r) = rename.take() {
                                    let name = r.buffer.trim().to_string();
                                    if !name.is_empty() && active < tabs.len() {
                                        if r.pane {
                                            let focus = tabs[active].focus;
                                            if let Some(p) = tabs[active].panes.get_mut(focus) {
                                                p.name = name;
                                            }
                                        } else if let Some(t) = tabs.get_mut(active) {
                                            t.name = name.clone();
                                            let id = t.id.clone();
                                            names.set(id.as_deref(), name);
                                        }
                                    }
                                }
                            }
                            KeyCode::Backspace => {
                                if let Some(r) = rename.as_mut() {
                                    r.backspace();
                                }
                            }
                            KeyCode::Char(c)
                                if !key.modifiers.contains(KeyModifiers::CONTROL)
                                    && !key.modifiers.contains(KeyModifiers::ALT) =>
                            {
                                if let Some(r) = rename.as_mut() {
                                    r.push(c);
                                }
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if search_mode {
                        match key.code {
                            KeyCode::Esc => {
                                search_mode = false;
                                search_query.clear();
                            }
                            KeyCode::Enter => {
                                // search scrollback
                                let q = search_query.clone();
                                if !q.is_empty() {
                                    let session = tabs[active].session();
                                    let mut parser = session.parser().lock();
                                    let screen = parser.screen_mut();
                                    let mut found = None;
                                    let mut offset = 0;
                                    loop {
                                        screen.set_scrollback(offset);
                                        let contents = screen.contents();
                                        if contents.trim().is_empty() && offset > 0 {
                                            break;
                                        }
                                        if contents.to_lowercase().contains(&q.to_lowercase()) {
                                            found = Some(offset);
                                            break;
                                        }
                                        offset += 1;
                                        if offset > 10000 {
                                            break;
                                        }
                                    }
                                    if let Some(o) = found {
                                        screen.set_scrollback(o);
                                    }
                                }
                                search_mode = false;
                            }
                            KeyCode::Backspace => {
                                search_query.pop();
                            }
                            KeyCode::Char(c) => {
                                search_query.push(c);
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if scroll_mode {
                        let session = tabs[active].session();
                        let mut parser = session.parser().lock();
                        let screen = parser.screen_mut();
                        match key.code {
                            KeyCode::Esc | KeyCode::Char('q') => {
                                screen.set_scrollback(0);
                                scroll_mode = false;
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                screen.set_scrollback(screen.scrollback().saturating_add(1));
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                screen.set_scrollback(screen.scrollback().saturating_sub(1));
                            }
                            KeyCode::PageUp => {
                                screen.set_scrollback(screen.scrollback().saturating_add(10));
                            }
                            KeyCode::PageDown => {
                                screen.set_scrollback(screen.scrollback().saturating_sub(10));
                            }
                            KeyCode::Char('g') => {
                                screen.set_scrollback(usize::MAX);
                            }
                            KeyCode::Char('G') => {
                                screen.set_scrollback(0);
                            }
                            KeyCode::Char('/') => {
                                search_mode = true;
                                search_query.clear();
                            }
                            KeyCode::Char('y') => {
                                let contents = screen.contents();
                                screen.set_scrollback(0);
                                scroll_mode = false;
                                drop(parser);
                                copy_to_clipboard(&contents);
                                continue;
                            }
                            _ => {}
                        }
                        drop(parser);
                        continue;
                    }
                    if tabs_open {
                        match key.code {
                            KeyCode::Esc => tabs_open = false,
                            KeyCode::Up => {
                                if !tabs.is_empty() {
                                    tabs_sel = if tabs_sel == 0 {
                                        tabs.len() - 1
                                    } else {
                                        tabs_sel - 1
                                    };
                                }
                            }
                            KeyCode::Down => {
                                if !tabs.is_empty() {
                                    tabs_sel = (tabs_sel + 1) % tabs.len();
                                }
                            }
                            KeyCode::Enter => {
                                active = tabs_sel;
                                tabs_open = false;
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if panes_open {
                        match key.code {
                            KeyCode::Esc => panes_open = false,
                            KeyCode::Up => {
                                let n = flat_panes(pane_counts(&tabs)).len();
                                if n > 0 {
                                    panes_sel = if panes_sel == 0 { n - 1 } else { panes_sel - 1 };
                                }
                            }
                            KeyCode::Down => {
                                let n = flat_panes(pane_counts(&tabs)).len();
                                if n > 0 {
                                    panes_sel = (panes_sel + 1) % n;
                                }
                            }
                            KeyCode::Enter => {
                                if let Some(&(ti, pi)) =
                                    flat_panes(pane_counts(&tabs)).get(panes_sel)
                                {
                                    active = ti;
                                    tabs[ti].focus = pi;
                                }
                                panes_open = false;
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if palette_open {
                        match key.code {
                            KeyCode::Esc => {
                                palette_open = false;
                                palette_query.clear();
                            }
                            KeyCode::Enter => {
                                let filtered = filtered_commands(&palette_query);
                                if let Some(cmd) = filtered.get(palette_sel) {
                                    exec_cmd(
                                        *cmd,
                                        &mut tabs,
                                        &mut active,
                                        &ctx(pty_rows, cols),
                                        &mut tabs_open,
                                        &mut tabs_sel,
                                        &mut panes_open,
                                        &mut panes_sel,
                                        &mut scroll_mode,
                                        &mut rename,
                                        &mut names,
                                        &mut quit,
                                    );
                                }
                                palette_open = false;
                                palette_query.clear();
                            }
                            KeyCode::Up => {
                                let n = filtered_commands(&palette_query).len();
                                if n > 0 {
                                    palette_sel = if palette_sel == 0 {
                                        n - 1
                                    } else {
                                        palette_sel - 1
                                    };
                                }
                            }
                            KeyCode::Down => {
                                let n = filtered_commands(&palette_query).len();
                                if n > 0 {
                                    palette_sel = (palette_sel + 1) % n;
                                }
                            }
                            KeyCode::Backspace => {
                                palette_query.pop();
                                palette_sel = 0;
                            }
                            KeyCode::Char(c) => {
                                palette_query.push(c);
                                palette_sel = 0;
                            }
                            _ => {}
                        }
                        continue;
                    }
                    if prefix_pending {
                        prefix_pending = false;
                        match key.code {
                            KeyCode::Char('k') => {
                                palette_open = true;
                                palette_query.clear();
                                palette_sel = 0;
                            }
                            KeyCode::Char(c) if c.is_ascii_digit() => {
                                // Digit shortcuts bypass Command: closed family.
                                if ('1'..='9').contains(&c) {
                                    cmd_select_tab(&tabs, &mut active, (c as u8 - b'1') as usize);
                                } else {
                                    forward_to_shell(&tabs, active, key);
                                }
                            }
                            code => {
                                let cmd = match code {
                                    KeyCode::Char('c')
                                        if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                                    {
                                        Some(Command::NewTab)
                                    }
                                    KeyCode::Char('v') => Some(Command::SplitVertical),
                                    KeyCode::Char('h') => Some(Command::SplitHorizontal),
                                    KeyCode::Char('o') => Some(Command::NextPane),
                                    KeyCode::Char('x') => Some(Command::Close),
                                    KeyCode::Char('p') => Some(Command::PrevTab),
                                    KeyCode::Char('n') => Some(Command::NextTab),
                                    KeyCode::Char('s') => Some(Command::SwitchTab),
                                    KeyCode::Char('[') => Some(Command::ScrollMode),
                                    KeyCode::Char(',') => Some(Command::RenameTab),
                                    KeyCode::Char('R') => Some(Command::RenamePane),
                                    KeyCode::Char('w') => Some(Command::ListPanes),
                                    _ => None,
                                };
                                if let Some(cmd) = cmd {
                                    exec_cmd(
                                        cmd,
                                        &mut tabs,
                                        &mut active,
                                        &ctx(pty_rows, cols),
                                        &mut tabs_open,
                                        &mut tabs_sel,
                                        &mut panes_open,
                                        &mut panes_sel,
                                        &mut scroll_mode,
                                        &mut rename,
                                        &mut names,
                                        &mut quit,
                                    );
                                } else {
                                    forward_to_shell(&tabs, active, key);
                                }
                            }
                        }
                    } else if key.code == KeyCode::Char('g')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        prefix_pending = true;
                    } else if key.code == KeyCode::Char('q')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        quit = true;
                    } else if let Some(bytes) = key_to_bytes(key) {
                        let _ = tabs[active].session().write(&bytes);
                    }
                }
                Event::Resize(c, r) => {
                    needs_draw = true;
                    if c < 4 || r < 4 {
                        continue;
                    }
                    cols = c;
                    pty_rows = r.saturating_sub(1).max(1);
                }
                _ => {}
            }
        }

        // If active session died and it's the last one, exit.
        if !tabs.is_empty() && !tabs[active].session().peek_alive() && tabs.len() == 1 {
            quit = true;
        }
    }

    disable_raw_mode()?;
    let _ = crossterm::execute!(
        terminal.backend_mut(),
        crossterm::event::DisableMouseCapture
    );
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    for mut t in tabs {
        for p in &mut t.panes {
            if !p.session.is_remote() {
                let _ = p.session.kill();
            }
        }
    }
    // remote sessions detach on drop (persist)
    Ok(())
}

/// Dispatch a [`Command`] with single-use borrows, so prefix and palette
/// handlers never hold TUI state across arms.
#[allow(clippy::too_many_arguments)]
fn exec_cmd(
    cmd: Command,
    tabs: &mut Vec<Tab>,
    active: &mut usize,
    spawn: &SpawnCtx,
    tabs_open: &mut bool,
    tabs_sel: &mut usize,
    panes_open: &mut bool,
    panes_sel: &mut usize,
    scroll_mode: &mut bool,
    rename: &mut Option<RenameState>,
    names: &mut TabNames,
    quit: &mut bool,
) {
    cmd.execute(&mut ExecCtx {
        tabs,
        active,
        spawn,
        tabs_open,
        tabs_sel,
        panes_open,
        panes_sel,
        scroll_mode,
        rename,
        names,
        quit,
    });
}

/// Forward Ctrl+G + key to the focused shell (unknown prefix command).
fn forward_to_shell(tabs: &[Tab], active: usize, key: crossterm::event::KeyEvent) {
    if let Some(bytes) = key_to_bytes(key) {
        let mut v = vec![0x07u8];
        v.extend_from_slice(&bytes);
        if let Some(tab) = tabs.get(active) {
            let _ = tab.session().write(&v);
        }
    }
}

fn copy_to_clipboard(text: &str) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let candidates = [
        ("wl-copy", Vec::<&str>::new()),
        ("xclip", vec!["-selection", "clipboard"]),
        ("xsel", vec!["--clipboard", "--input"]),
    ];
    for (cmd, args) in candidates {
        let mut child = match Command::new(cmd).args(&args).stdin(Stdio::piped()).spawn() {
            Ok(c) => c,
            Err(_) => continue,
        };
        if let Some(stdin) = child.stdin.as_mut() {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::pane_border;

    #[test]
    fn solo_pane_has_no_border() {
        assert!(pane_border(1, 0, 0, None).is_none());
    }

    #[test]
    fn only_focused_pane_has_border() {
        assert!(pane_border(3, 1, 1, Some("ed")).is_some());
        assert!(pane_border(3, 1, 0, Some("sh")).is_none());
        assert!(pane_border(3, 1, 2, None).is_none());
    }
}
