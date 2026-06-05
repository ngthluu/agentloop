use crate::events::{Command, Event};
use crossterm::event::{KeyCode, KeyEvent};

#[derive(Clone)]
pub struct Job {
    pub id: String,
    pub label: String,
    pub tool: String,
    pub model: String,
    pub status: String,
    pub log_path: Option<std::path::PathBuf>,
    pub started: Option<std::time::Instant>,
    pub frozen: Option<std::time::Duration>,
}

impl Job {
    /// Frozen duration if finished, else live elapsed since dispatch, else None.
    pub fn elapsed(&self) -> Option<std::time::Duration> {
        self.frozen.or_else(|| self.started.map(|s| s.elapsed()))
    }
}

/// One role's routing as shown/edited in the ctrl-o model-config panel.
/// Empty model/effort = unset: the tool's own default applies.
#[derive(Clone, Debug, PartialEq)]
pub struct RoleEntry {
    pub role: String,
    pub tool: String,
    pub model: String,
    pub effort: String,
}

/// The full row snapshot as a SetRole command (empty fields = tool default).
fn set_role_cmd(row: &RoleEntry) -> Command {
    Command::SetRole {
        role: row.role.clone(),
        tool: row.tool.clone(),
        model: row.model.clone(),
        effort: row.effort.clone(),
    }
}

#[derive(PartialEq, Clone, Copy)]
enum View {
    GoalEntry,
    List,
    JobDetail,
    ModelConfig,
}

pub struct AppState {
    pub goal: String,
    pub jobs: Vec<Job>,
    pub iter: u32,
    pub gate: String,
    pub open: i64,
    pub standby: bool,
    pub standby_reason: String,
    input: String,
    view: View,
    goal_focus_continue: bool,
    selected_job: usize,
    log_scroll: u16,
    started: std::time::Instant,
    // ctrl-o model-config panel state.
    roles: Vec<RoleEntry>,
    prev_view: View,
    cfg_row: usize,
    cfg_col: usize, // 0 = tool, 1 = model, 2 = effort
    cfg_edit: Option<String>,
}

impl AppState {
    pub fn new(goal: String) -> Self {
        Self {
            goal: goal.clone(),
            jobs: vec![],
            iter: 0,
            gate: "init".into(),
            open: 0,
            standby: false,
            standby_reason: String::new(),
            input: goal,
            view: View::GoalEntry,
            goal_focus_continue: false,
            selected_job: 0,
            log_scroll: 0,
            started: std::time::Instant::now(),
            roles: vec![],
            prev_view: View::GoalEntry,
            cfg_row: 0,
            cfg_col: 0,
            cfg_edit: None,
        }
    }

    pub fn apply(&mut self, ev: Event) {
        match ev {
            Event::JobDispatched {
                id,
                label,
                tool,
                model,
                log_path,
            } => {
                self.standby = false;
                let now = std::time::Instant::now();
                if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
                    j.label = label;
                    j.tool = tool;
                    j.model = model;
                    j.status = "running".into();
                    j.log_path = log_path;
                    j.started = Some(now);
                    j.frozen = None;
                } else {
                    self.jobs.push(Job {
                        id,
                        label,
                        tool,
                        model,
                        status: "running".into(),
                        log_path,
                        started: Some(now),
                        frozen: None,
                    });
                }
            }
            Event::JobStatus { id, status } => {
                if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
                    if is_terminal_status(&status) && j.frozen.is_none() {
                        j.frozen = j.started.map(|s| s.elapsed());
                    }
                    j.status = status;
                }
            }
            Event::Iteration {
                n,
                merged: _,
                gate,
                open,
            } => {
                self.iter = n;
                self.gate = gate;
                self.open = open;
            }
            Event::EnteredStandby { reason } => {
                self.standby = true;
                self.standby_reason = reason;
            }
            Event::Shutdown => {}
        }
    }

    /// Map a key to an optional Command. Returns None when the key only changes UI state.
    pub fn on_key(&mut self, k: KeyEvent) -> Option<Command> {
        // ctrl-o toggles the model-routing panel from any view.
        if k.code == KeyCode::Char('o')
            && k.modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL)
        {
            if self.view == View::ModelConfig {
                self.cfg_edit = None;
                self.view = self.prev_view;
            } else {
                self.prev_view = self.view;
                self.view = View::ModelConfig;
            }
            return None;
        }
        match self.view {
            View::GoalEntry => self.on_key_goal_entry(k),
            View::JobDetail => self.on_key_job_detail(k),
            View::List => self.on_key_list(k),
            View::ModelConfig => self.on_key_model_config(k),
        }
    }

    fn is_newline(k: &KeyEvent) -> bool {
        k.code == KeyCode::Enter
            && k.modifiers.intersects(
                crossterm::event::KeyModifiers::SHIFT | crossterm::event::KeyModifiers::ALT,
            )
    }

    fn on_key_goal_entry(&mut self, k: KeyEvent) -> Option<Command> {
        if Self::is_newline(&k) {
            self.input.push('\n');
            return None;
        }
        match k.code {
            KeyCode::Enter => {
                let goal = self.input.trim().to_string();
                if goal.is_empty() {
                    return None; // nothing to start yet; stay on the entry screen
                }
                self.goal = goal.clone();
                self.input.clear();
                self.view = View::List;
                Some(Command::StartRun { goal })
            }
            KeyCode::Tab => {
                self.goal_focus_continue = !self.goal_focus_continue;
                None
            }
            KeyCode::Backspace => {
                self.input.pop();
                None
            }
            KeyCode::Esc => {
                self.input.clear();
                None
            }
            KeyCode::Char(c) => {
                self.input.push(c);
                None
            }
            _ => None,
        }
    }

    fn on_key_job_detail(&mut self, k: KeyEvent) -> Option<Command> {
        if Self::is_newline(&k) {
            self.input.push('\n');
            return None;
        }
        match k.code {
            KeyCode::Esc => {
                self.view = View::List;
                self.log_scroll = 0;
                None
            }
            KeyCode::Up => {
                self.log_scroll = self.log_scroll.saturating_add(1);
                None
            }
            KeyCode::Down => {
                self.log_scroll = self.log_scroll.saturating_sub(1);
                None
            }
            KeyCode::Enter => self.submit(),
            KeyCode::Backspace => {
                self.input.pop();
                None
            }
            KeyCode::Char('q') if self.input.trim().is_empty() => Some(Command::Quit),
            KeyCode::Char(c) => {
                self.input.push(c);
                None
            }
            _ => None,
        }
    }

    fn on_key_list(&mut self, k: KeyEvent) -> Option<Command> {
        if Self::is_newline(&k) {
            self.input.push('\n');
            return None;
        }
        match k.code {
            KeyCode::Up => {
                if self.selected_job > 0 {
                    self.selected_job -= 1;
                }
                None
            }
            KeyCode::Down => {
                if self.selected_job + 1 < self.jobs.len() {
                    self.selected_job += 1;
                }
                None
            }
            KeyCode::Enter => {
                // Non-empty input submits a task; empty input opens the selected job.
                if self.input.trim().is_empty() {
                    if self.selected_job < self.jobs.len() {
                        self.view = View::JobDetail;
                        self.log_scroll = 0;
                    }
                    None
                } else {
                    self.submit()
                }
            }
            KeyCode::Backspace => {
                self.input.pop();
                None
            }
            KeyCode::Esc => {
                self.input.clear();
                None
            }
            KeyCode::Char('q') if self.input.trim().is_empty() => Some(Command::Quit),
            KeyCode::Char(c) => {
                self.input.push(c);
                None
            }
            _ => None,
        }
    }

    fn on_key_model_config(&mut self, k: KeyEvent) -> Option<Command> {
        // Editing a cell: keys go to the edit buffer until Enter commits / Esc cancels.
        if self.cfg_edit.is_some() {
            match k.code {
                KeyCode::Enter => {
                    let col = self.cfg_col;
                    let row = self.roles.get_mut(self.cfg_row)?;
                    let value = self.cfg_edit.take().unwrap_or_default().trim().to_string();
                    if col == 1 {
                        row.model = value;
                    } else {
                        row.effort = value;
                    }
                    return Some(set_role_cmd(row));
                }
                KeyCode::Esc => self.cfg_edit = None,
                KeyCode::Backspace => {
                    if let Some(buf) = self.cfg_edit.as_mut() {
                        buf.pop();
                    }
                }
                KeyCode::Char(c) => {
                    if let Some(buf) = self.cfg_edit.as_mut() {
                        buf.push(c);
                    }
                }
                _ => {}
            }
            return None;
        }
        match k.code {
            KeyCode::Esc => {
                self.view = self.prev_view;
                None
            }
            KeyCode::Up => {
                self.cfg_row = self.cfg_row.saturating_sub(1);
                None
            }
            KeyCode::Down => {
                if self.cfg_row + 1 < self.roles.len() {
                    self.cfg_row += 1;
                }
                None
            }
            KeyCode::Left => {
                self.cfg_col = self.cfg_col.saturating_sub(1);
                None
            }
            KeyCode::Right => {
                if self.cfg_col < 2 {
                    self.cfg_col += 1;
                }
                None
            }
            KeyCode::Enter => {
                let col = self.cfg_col;
                let row = self.roles.get_mut(self.cfg_row)?;
                if col == 0 {
                    // Only two known tools; Enter cycles instead of free text.
                    row.tool = if row.tool == "claude" {
                        "codex".into()
                    } else {
                        "claude".into()
                    };
                    Some(set_role_cmd(row))
                } else {
                    self.cfg_edit = Some(if col == 1 {
                        row.model.clone()
                    } else {
                        row.effort.clone()
                    });
                    None
                }
            }
            _ => None,
        }
    }

    /// Submit the current input as a new task for the manager. Clears the input.
    fn submit(&mut self) -> Option<Command> {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return None;
        }
        self.input.clear();
        Some(Command::AddTask { request: text })
    }

    pub fn input_buffer(&self) -> &str {
        &self.input
    }

    pub fn in_job_detail(&self) -> bool {
        self.view == View::JobDetail
    }

    pub fn in_goal_entry(&self) -> bool {
        self.view == View::GoalEntry
    }

    /// Seed the ctrl-o panel rows (sorted by role; from the loaded config).
    pub fn set_routing(&mut self, roles: Vec<RoleEntry>) {
        self.roles = roles;
        self.cfg_row = 0;
        self.cfg_col = 0;
        self.cfg_edit = None;
    }

    pub fn in_model_config(&self) -> bool {
        self.view == View::ModelConfig
    }

    pub fn model_rows(&self) -> &[RoleEntry] {
        &self.roles
    }

    /// (row, col) of the panel's cell cursor; col 0 = tool, 1 = model, 2 = effort.
    pub fn model_selection(&self) -> (usize, usize) {
        (self.cfg_row, self.cfg_col)
    }

    /// The in-progress cell edit, if any.
    pub fn model_edit_buffer(&self) -> Option<&str> {
        self.cfg_edit.as_deref()
    }

    pub fn goal_continue_focused(&self) -> bool {
        self.goal_focus_continue
    }

    /// Wall-clock time since the session (TUI) started.
    pub fn total_elapsed(&self) -> std::time::Duration {
        self.started.elapsed()
    }
}

/// Truncate `s` to at most `max` chars, ending in `…` when cut.
pub fn ellipsize(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// Human working-time: "{s}s" under a minute, "{m}m{s:02}s" under an hour,
/// else "{h}h{m:02}m".
pub fn fmt_elapsed(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Last `max_lines` lines of `path` (reading at most the final `max_bytes`),
/// for the job-detail log view. Returns a single "(no output yet)" line when the
/// file is missing or empty.
pub fn tail_file(path: &std::path::Path, max_lines: usize, max_bytes: u64) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let placeholder = || vec!["(no output yet)".to_string()];
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return placeholder(),
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    if len == 0 {
        return placeholder();
    }
    let start = len.saturating_sub(max_bytes);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return placeholder();
    }
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_err() {
        return placeholder();
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
    if lines.len() > max_lines {
        lines = lines.split_off(lines.len() - max_lines);
    }
    if lines.is_empty() {
        placeholder()
    } else {
        lines
    }
}

/// Job statuses that end the working timer (the job will not run further).
/// Must cover every terminal status the orchestrator reports: merged/done/failed/
/// bounced for build jobs, approved/rejected for customer reviews.
fn is_terminal_status(status: &str) -> bool {
    matches!(
        status,
        "merged" | "done" | "failed" | "bounced" | "approved" | "rejected"
    )
}

fn status_glyph(status: &str) -> &'static str {
    match status {
        "running" => "●",
        "merged" => "✓",
        "done" => "✓",
        "approved" => "✓",
        "failed" => "✗",
        "rejected" => "✗",
        "bounced" => "↺",
        "queued" => "·",
        _ => "?",
    }
}

pub fn render(f: &mut ratatui::Frame, s: &AppState) {
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::Line;
    use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

    let area = f.area();

    if s.in_model_config() {
        render_model_config(f, s, area);
        return;
    }

    if s.in_goal_entry() {
        render_goal_entry(f, s, area);
        return;
    }

    // Bottom input bar height grows with the number of input lines (capped).
    let input_lines = s.input_buffer().split('\n').count().max(1) as u16;
    let footer_height = (input_lines + 3).min(12); // label + input lines + hint + border
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(footer_height),
        ])
        .split(area);

    // --- Top status bar: the fixed suffix (iter/gate/open/⏱) always fits; the
    // goal gets the remaining width and is ellipsized so a long goal can't push
    // the counters off-screen. Newlines would break the one-line bar.
    let total = fmt_elapsed(s.total_elapsed());
    // The standby banner carries the park reason: "✓ DONE" only when the run
    // actually finished; otherwise the stop cause (stall/cap) with open/failed
    // counts, so an idle loop with unfinished work is never mislabeled DONE.
    let prefix = if s.standby {
        if s.standby_reason.starts_with("all tasks done") {
            " ✓ DONE · standby  │  ".to_string()
        } else {
            format!(" ⏸ standby: {}  │  ", s.standby_reason)
        }
    } else {
        " ".to_string()
    };
    let suffix = format!(
        "  │  iter {}  │  gate: {}  │  open: {}  │  ⏱ {}",
        s.iter, s.gate, s.open, total
    );
    let avail =
        (chunks[0].width as usize).saturating_sub(prefix.chars().count() + suffix.chars().count());
    let goal = ellipsize(&s.goal.replace('\n', " "), avail);
    let status_text = format!("{prefix}{goal}{suffix}");
    let status_bar = Paragraph::new(status_text).style(
        Style::default()
            .bg(Color::DarkGray)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(status_bar, chunks[0]);

    // --- Main area: the jobs list, or the job-detail view ---
    if s.in_job_detail() {
        render_job_detail(f, s, chunks[1]);
    } else {
        let job_items: Vec<ListItem> = s
            .jobs
            .iter()
            .map(|j| {
                let glyph = status_glyph(&j.status);
                let dur = j.elapsed().map(fmt_elapsed).unwrap_or_default();
                let line = format!(
                    " {} {} [{}]  {}",
                    glyph,
                    j.label,
                    crate::events::fmt_tool_model(&j.tool, &j.model),
                    dur
                );
                ListItem::new(Line::from(line))
            })
            .collect();
        let jobs_list = List::new(job_items)
            .block(
                Block::default()
                    .title(" Jobs ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow)),
            )
            .highlight_style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            );
        let mut jobs_state = ListState::default();
        if !s.jobs.is_empty() {
            jobs_state.select(Some(s.selected_job.min(s.jobs.len() - 1)));
        }
        f.render_stateful_widget(jobs_list, chunks[1], &mut jobs_state);
    }

    // --- Persistent bottom input bar ---
    let footer = chunks[2];
    let fchunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(footer);

    let input = Paragraph::new(s.input_buffer())
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .title(" Add task ")
                .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .style(Style::default().fg(Color::White));
    f.render_widget(input, fchunks[0]);

    let hint = if s.standby {
        " ✓ standby · [enter] submit  [shift+enter] newline  [↑↓] jobs  [ctrl-o] models  [esc] clear  [q] quit"
    } else {
        " [enter] submit  [shift+enter] newline  [↑↓] jobs  [ctrl-o] models  [esc] clear  [q] quit"
    };
    let hint_para = Paragraph::new(Line::from(hint)).style(Style::default().fg(Color::DarkGray));
    f.render_widget(hint_para, fchunks[1]);
}

fn render_goal_entry(f: &mut ratatui::Frame, s: &AppState, area: ratatui::layout::Rect) {
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::Line;
    use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    let title = Paragraph::new(Line::from(
        " Describe what to build — or edit the goal below, then Continue:",
    ))
    .style(
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(title, chunks[1]);

    let input = Paragraph::new(s.input_buffer())
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .title(" Goal ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        );
    f.render_widget(input, chunks[2]);

    let button_style = if s.goal_continue_focused() {
        Style::default()
            .bg(Color::Green)
            .fg(Color::Black)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    };
    let button = Paragraph::new(Line::from(
        "  [ Continue ]   ([enter] start  ·  [shift+enter] newline  ·  [ctrl-c] quit)",
    ))
    .style(button_style)
    .block(
        Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(button, chunks[3]);

    let hint = Paragraph::new(Line::from(
        " [ctrl-o] models — pick tool/model/effort per role",
    ))
    .style(Style::default().fg(Color::DarkGray));
    f.render_widget(hint, chunks[4]);
}

fn render_model_config(f: &mut ratatui::Frame, s: &AppState, area: ratatui::layout::Rect) {
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Paragraph};

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);

    let (sel_row, sel_col) = s.model_selection();
    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        format!(
            " {:<12} {:<10} {:<24} {:<10}",
            "role", "tool", "model", "effort"
        ),
        Style::default().fg(Color::DarkGray),
    ))];
    for (i, row) in s.model_rows().iter().enumerate() {
        let cell = |col: usize, value: &str| -> Span<'static> {
            let width = match col {
                0 => 10,
                1 => 24,
                _ => 10,
            };
            // The selected cell is highlighted; an in-progress edit shows the
            // buffer with a cursor mark instead of the stored value. Text is
            // clipped to the cell so a long value can't shove the next column.
            let editing = i == sel_row && col == sel_col && s.model_edit_buffer().is_some();
            let text = if editing {
                let buf = s.model_edit_buffer().unwrap_or("");
                let shown: String = buf.chars().take(width - 1).collect();
                format!("{shown}▏")
            } else if value.is_empty() {
                "(default)".to_string()
            } else {
                value.chars().take(width).collect()
            };
            let padded = format!("{text:<width$}");
            if i == sel_row && col == sel_col {
                Span::styled(
                    padded,
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::raw(padded)
            }
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {:<12} ", row.role),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            cell(0, &row.tool),
            Span::raw(" "),
            cell(1, &row.model),
            Span::raw(" "),
            cell(2, &row.effort),
        ]));
    }
    let panel = Paragraph::new(lines).block(
        Block::default()
            .title(" Model routing \u{2014} saved to config.json ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow)),
    );
    f.render_widget(panel, chunks[0]);

    let hint =
        " [↑↓←→] move  [enter] edit / cycle tool  [esc] close  ·  empty model/effort = tool default";
    f.render_widget(
        Paragraph::new(Line::from(hint)).style(Style::default().fg(Color::DarkGray)),
        chunks[1],
    );
}

fn render_job_detail(f: &mut ratatui::Frame, s: &AppState, area: ratatui::layout::Rect) {
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::style::{Color, Style};
    use ratatui::text::Line;
    use ratatui::widgets::{Block, Borders, Paragraph};

    let job = s.jobs.get(s.selected_job);
    let parts = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Min(0)])
        .split(area);

    let (title, header_lines) = match job {
        Some(j) => {
            let dur = j.elapsed().map(fmt_elapsed).unwrap_or_default();
            (
                format!(" Job: {} \u{2014} {} ", j.id, j.label),
                vec![Line::from(format!(
                    " status: {} {}   tool: {}   {}",
                    status_glyph(&j.status),
                    j.status,
                    crate::events::fmt_tool_model(&j.tool, &j.model),
                    dur
                ))],
            )
        }
        None => (" Job ".to_string(), vec![Line::from(" (no job selected)")]),
    };

    let header = Paragraph::new(header_lines).block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow)),
    );
    f.render_widget(header, parts[0]);

    // Log tail.
    let lines: Vec<Line> = match job.and_then(|j| j.log_path.as_deref()) {
        Some(path) => tail_file(path, 400, 32 * 1024)
            .into_iter()
            .map(Line::from)
            .collect(),
        None => vec![Line::from("(no output yet)")],
    };
    let body = parts[1];
    // Show the lines that fit, honoring log_scroll as an offset from the bottom.
    let visible = body.height.saturating_sub(2) as usize; // minus the borders
    let total = lines.len();
    let scroll = (s.log_scroll as usize).min(total.saturating_sub(visible.max(1)));
    let end = total.saturating_sub(scroll);
    let start = end.saturating_sub(visible);
    let shown: Vec<Line> = lines[start..end].to_vec();
    let log = Paragraph::new(shown).block(
        Block::default()
            .title(" log \u{2014} [\u{2191}\u{2193}] scroll  [esc] back ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(log, body);
}
