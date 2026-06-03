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

#[derive(Clone)]
pub struct Pending {
    pub item_id: String,
    pub label: String,
    pub text: String,
    pub context: String,
}

#[derive(PartialEq, Clone, Copy)]
enum Focus {
    Jobs,
    Inbox,
}

#[derive(PartialEq, Clone, Copy)]
enum View {
    GoalEntry,
    List,
    JobDetail,
}

pub struct AppState {
    pub goal: String,
    pub jobs: Vec<Job>,
    pub inbox: Vec<Pending>,
    pub selected: usize,
    pub iter: u32,
    pub gate: String,
    pub open: i64,
    pub standby: bool,
    input: String,
    focus: Focus,
    view: View,
    goal_focus_continue: bool,
    selected_job: usize,
    log_scroll: u16,
    started: std::time::Instant,
}

impl AppState {
    pub fn new(goal: String) -> Self {
        Self {
            goal: goal.clone(),
            jobs: vec![],
            inbox: vec![],
            selected: 0,
            iter: 0,
            gate: "init".into(),
            open: 0,
            standby: false,
            input: goal,
            focus: Focus::Inbox,
            view: View::GoalEntry,
            goal_focus_continue: false,
            selected_job: 0,
            log_scroll: 0,
            started: std::time::Instant::now(),
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
                    let terminal =
                        matches!(status.as_str(), "merged" | "done" | "failed" | "bounced");
                    if terminal && j.frozen.is_none() {
                        j.frozen = j.started.map(|s| s.elapsed());
                    }
                    j.status = status;
                }
            }
            Event::QuestionRaised {
                item_id,
                label,
                text,
                context,
            } => {
                if !self.inbox.iter().any(|p| p.item_id == item_id) {
                    self.inbox.push(Pending {
                        item_id,
                        label,
                        text,
                        context,
                    });
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
            Event::EnteredStandby => {
                self.standby = true;
            }
            Event::Shutdown => {}
        }
    }

    /// Map a key to an optional Command. Returns None when the key only changes UI state.
    pub fn on_key(&mut self, k: KeyEvent) -> Option<Command> {
        match self.view {
            View::GoalEntry => self.on_key_goal_entry(k),
            View::JobDetail => self.on_key_job_detail(k),
            View::List => self.on_key_list(k),
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
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Jobs => Focus::Inbox,
                    Focus::Inbox => Focus::Jobs,
                };
                None
            }
            KeyCode::Up => {
                match self.focus {
                    Focus::Jobs => {
                        if self.selected_job > 0 {
                            self.selected_job -= 1;
                        }
                    }
                    Focus::Inbox => {
                        if self.selected > 0 {
                            self.selected -= 1;
                        }
                    }
                }
                None
            }
            KeyCode::Down => {
                match self.focus {
                    Focus::Jobs => {
                        if self.selected_job + 1 < self.jobs.len() {
                            self.selected_job += 1;
                        }
                    }
                    Focus::Inbox => {
                        if self.selected + 1 < self.inbox.len() {
                            self.selected += 1;
                        }
                    }
                }
                None
            }
            KeyCode::Enter => {
                // Non-empty input submits; empty input runs the focused pane's action.
                if self.input.trim().is_empty() {
                    if self.focus == Focus::Jobs && self.selected_job < self.jobs.len() {
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

    /// Submit the current input, routing by focus/selection. Clears the input.
    fn submit(&mut self) -> Option<Command> {
        let text = self.input.trim().to_string();
        if text.is_empty() {
            return None;
        }
        if self.focus == Focus::Inbox && !self.inbox.is_empty() {
            let idx = self.selected.min(self.inbox.len().saturating_sub(1));
            let p = self.inbox.remove(idx);
            self.selected = 0;
            self.input.clear();
            Some(Command::AnswerQuestion {
                item_id: p.item_id,
                text,
            })
        } else {
            self.input.clear();
            Some(Command::AddTask { request: text })
        }
    }

    pub fn input_buffer(&self) -> &str {
        &self.input
    }

    pub fn focus_is_jobs(&self) -> bool {
        self.focus == Focus::Jobs
    }

    pub fn in_job_detail(&self) -> bool {
        self.view == View::JobDetail
    }

    pub fn in_goal_entry(&self) -> bool {
        self.view == View::GoalEntry
    }

    pub fn goal_continue_focused(&self) -> bool {
        self.goal_focus_continue
    }

    /// Label shown above the input: what a submission will do right now.
    pub fn input_target_label(&self) -> String {
        if self.focus == Focus::Inbox && !self.inbox.is_empty() {
            let idx = self.selected.min(self.inbox.len().saturating_sub(1));
            format!("Answering {}", self.inbox[idx].item_id)
        } else {
            "Add task".to_string()
        }
    }

    /// Wall-clock time since the session (TUI) started.
    pub fn total_elapsed(&self) -> std::time::Duration {
        self.started.elapsed()
    }
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

fn status_glyph(status: &str) -> &'static str {
    match status {
        "running" => "●",
        "merged" => "✓",
        "done" => "✓",
        "failed" => "✗",
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

    // --- Top status bar ---
    let total = fmt_elapsed(s.total_elapsed());
    let status_text = if s.standby {
        format!(
            " ✓ DONE · standby  │  {}  │  iter {}  │  gate: {}  │  open: {}  │  ❓{}  │  ⏱ {}",
            s.goal,
            s.iter,
            s.gate,
            s.open,
            s.inbox.len(),
            total
        )
    } else {
        format!(
            " {}  │  iter {}  │  gate: {}  │  open: {}  │  ❓{}  │  ⏱ {}",
            s.goal,
            s.iter,
            s.gate,
            s.open,
            s.inbox.len(),
            total
        )
    };
    let status_bar = Paragraph::new(status_text).style(
        Style::default()
            .bg(Color::DarkGray)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(status_bar, chunks[0]);

    // --- Main area: jobs (top) + inbox (bottom), or the job-detail view ---
    if s.in_job_detail() {
        render_job_detail(f, s, chunks[1]);
    } else {
        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(chunks[1]);

        let job_items: Vec<ListItem> = s
            .jobs
            .iter()
            .map(|j| {
                let glyph = status_glyph(&j.status);
                let dur = j.elapsed().map(fmt_elapsed).unwrap_or_default();
                let line = format!(" {} {} [{}/{}]  {}", glyph, j.label, j.tool, j.model, dur);
                ListItem::new(Line::from(line))
            })
            .collect();
        let jobs_border = if s.focus_is_jobs() {
            Color::Yellow
        } else {
            Color::Blue
        };
        let jobs_highlight = if s.focus_is_jobs() {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let jobs_list = List::new(job_items)
            .block(
                Block::default()
                    .title(" Jobs ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(jobs_border)),
            )
            .highlight_style(jobs_highlight);
        let mut jobs_state = ListState::default();
        if !s.jobs.is_empty() {
            jobs_state.select(Some(s.selected_job.min(s.jobs.len() - 1)));
        }
        f.render_stateful_widget(jobs_list, main_chunks[0], &mut jobs_state);

        let inbox_items: Vec<ListItem> = s
            .inbox
            .iter()
            .map(|p| {
                ListItem::new(Line::from(format!(
                    " \u{2753} {} \u{2014} {}",
                    p.label, p.text
                )))
            })
            .collect();
        let inbox_border = if s.focus_is_jobs() {
            Color::Magenta
        } else {
            Color::Yellow
        };
        let inbox_highlight = if !s.focus_is_jobs() {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let inbox_list = List::new(inbox_items)
            .block(
                Block::default()
                    .title(" Inbox ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(inbox_border)),
            )
            .highlight_style(inbox_highlight);
        let mut inbox_state = ListState::default();
        if !s.inbox.is_empty() {
            inbox_state.select(Some(s.selected.min(s.inbox.len() - 1)));
        }
        f.render_stateful_widget(inbox_list, main_chunks[1], &mut inbox_state);
    }

    // --- Persistent bottom input bar ---
    let footer = chunks[2];
    let fchunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(footer);

    let title = format!(" {} ", s.input_target_label());
    let input = Paragraph::new(s.input_buffer())
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .title(title)
                .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .style(Style::default().fg(Color::White));
    f.render_widget(input, fchunks[0]);

    let hint = if s.standby {
        " ✓ standby · [enter] submit  [shift+enter] newline  [tab] pane  [↑↓] nav  [esc] clear  [q] quit"
    } else {
        " [enter] submit  [shift+enter] newline  [tab] pane  [↑↓] nav  [esc] clear  [q] quit"
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
                    " status: {} {}   tool: {}/{}   {}",
                    status_glyph(&j.status),
                    j.status,
                    j.tool,
                    j.model,
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
