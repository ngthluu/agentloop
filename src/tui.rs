use crossterm::event::{KeyCode, KeyEvent};
use crate::events::{Command, Event};

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

#[derive(Clone)]
pub struct Pending {
    pub item_id: String,
    pub label: String,
    pub text: String,
    pub context: String,
}

#[derive(PartialEq)]
enum Mode {
    Normal,
    Answering,
    AddingTask,
}

#[derive(PartialEq, Clone, Copy)]
enum Focus {
    Jobs,
    Inbox,
}

#[derive(PartialEq, Clone, Copy)]
enum View {
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
    mode: Mode,
    input: String,
    focus: Focus,
    view: View,
    selected_job: usize,
    log_scroll: u16,
}

impl AppState {
    pub fn new(goal: String) -> Self {
        Self {
            goal,
            jobs: vec![],
            inbox: vec![],
            selected: 0,
            iter: 0,
            gate: "init".into(),
            open: 0,
            standby: false,
            mode: Mode::Normal,
            input: String::new(),
            focus: Focus::Inbox,
            view: View::List,
            selected_job: 0,
            log_scroll: 0,
        }
    }

    pub fn apply(&mut self, ev: Event) {
        match ev {
            Event::JobDispatched { id, label, tool, model, log_path } => {
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
                    let terminal = matches!(status.as_str(), "merged" | "done" | "failed" | "bounced");
                    if terminal && j.frozen.is_none() {
                        j.frozen = j.started.map(|s| s.elapsed());
                    }
                    j.status = status;
                }
            }
            Event::QuestionRaised { item_id, label, text, context } => {
                if !self.inbox.iter().any(|p| p.item_id == item_id) {
                    self.inbox.push(Pending { item_id, label, text, context });
                }
            }
            Event::Iteration { n, merged: _, gate, open } => {
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
        match self.mode {
            Mode::Normal => {
                // Detail view has its own keys.
                if self.view == View::JobDetail {
                    match k.code {
                        KeyCode::Esc => {
                            self.view = View::List;
                            self.log_scroll = 0;
                        }
                        KeyCode::Up => {
                            self.log_scroll = self.log_scroll.saturating_add(1);
                        }
                        KeyCode::Down => {
                            self.log_scroll = self.log_scroll.saturating_sub(1);
                        }
                        KeyCode::Char('q') => return Some(Command::Quit),
                        _ => {}
                    }
                    return None;
                }
                match k.code {
                    KeyCode::Char('q') => Some(Command::Quit),
                    KeyCode::Char('a') => {
                        self.mode = Mode::AddingTask;
                        self.input.clear();
                        None
                    }
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
                        match self.focus {
                            Focus::Jobs => {
                                if !self.jobs.is_empty() {
                                    self.view = View::JobDetail;
                                    self.log_scroll = 0;
                                }
                            }
                            Focus::Inbox => {
                                if !self.inbox.is_empty() {
                                    self.mode = Mode::Answering;
                                    self.input.clear();
                                }
                            }
                        }
                        None
                    }
                    _ => None,
                }
            }
            Mode::Answering => match k.code {
                KeyCode::Esc => {
                    self.mode = Mode::Normal;
                    self.input.clear();
                    None
                }
                KeyCode::Backspace => {
                    self.input.pop();
                    None
                }
                KeyCode::Char(c) => {
                    self.input.push(c);
                    None
                }
                KeyCode::Enter => {
                    let idx = self.selected.min(self.inbox.len().saturating_sub(1));
                    let p = self.inbox.remove(idx);
                    let text = std::mem::take(&mut self.input);
                    self.selected = 0;
                    self.mode = Mode::Normal;
                    Some(Command::AnswerQuestion { item_id: p.item_id, text })
                }
                _ => None,
            },
            Mode::AddingTask => match k.code {
                KeyCode::Esc => {
                    self.mode = Mode::Normal;
                    self.input.clear();
                    None
                }
                KeyCode::Backspace => {
                    self.input.pop();
                    None
                }
                KeyCode::Char(c) => {
                    self.input.push(c);
                    None
                }
                KeyCode::Enter => {
                    let text = std::mem::take(&mut self.input);
                    self.mode = Mode::Normal;
                    Some(Command::AddTask { request: text })
                }
                _ => None,
            },
        }
    }

    pub fn input_buffer(&self) -> &str {
        &self.input
    }

    pub fn is_editing(&self) -> bool {
        self.mode != Mode::Normal
    }

    pub fn mode_is_adding(&self) -> bool {
        self.mode == Mode::AddingTask
    }

    pub fn mode_is_answering(&self) -> bool {
        self.mode == Mode::Answering
    }

    pub fn focus_is_jobs(&self) -> bool {
        self.focus == Focus::Jobs
    }

    pub fn in_job_detail(&self) -> bool {
        self.view == View::JobDetail
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
    use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};

    let area = f.area();

    // Top status bar (1 line), main content, footer (3 lines)
    let footer_height = if s.is_editing() { 4 } else { 3 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(footer_height),
        ])
        .split(area);

    // --- Top status bar ---
    let status_text = if s.standby {
        format!(
            " ✓ DONE · standby  │  {}  │  iter {}  │  gate: {}  │  open: {}  │  ❓{}",
            s.goal, s.iter, s.gate, s.open, s.inbox.len()
        )
    } else {
        format!(
            " {}  │  iter {}  │  gate: {}  │  open: {}  │  ❓{}",
            s.goal, s.iter, s.gate, s.open, s.inbox.len()
        )
    };
    let status_bar = Paragraph::new(status_text)
        .style(Style::default().bg(Color::DarkGray).fg(Color::White).add_modifier(Modifier::BOLD));
    f.render_widget(status_bar, chunks[0]);

    // --- Main area: jobs (left) + inbox (right) ---
    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chunks[1]);

    // Jobs list
    let job_items: Vec<ListItem> = s
        .jobs
        .iter()
        .map(|j| {
            let glyph = status_glyph(&j.status);
            ListItem::new(Line::from(format!(
                " {} {} [{}/{}]",
                glyph, j.label, j.tool, j.model
            )))
        })
        .collect();
    let jobs_list = List::new(job_items).block(
        Block::default()
            .title(" Jobs ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Blue)),
    );
    f.render_widget(jobs_list, main_chunks[0]);

    // Inbox list
    let inbox_items: Vec<ListItem> = s
        .inbox
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let style = if i == s.selected {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(format!(" ❓ {} — {}", p.label, p.text))).style(style)
        })
        .collect();
    let inbox_list = List::new(inbox_items).block(
        Block::default()
            .title(" Inbox ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Magenta)),
    );
    f.render_widget(inbox_list, main_chunks[1]);

    // --- Footer ---
    let footer_widget = if s.is_editing() {
        // Editing mode: show context + input
        let label = if s.mode_is_answering() {
            let p = s.inbox.get(s.selected);
            match p {
                Some(pending) => format!(
                    " answering {} — {}",
                    pending.item_id, pending.text
                ),
                None => " answering".to_string(),
            }
        } else {
            " add task (sent to planner):".to_string()
        };
        let input_line = format!(" > {}", s.input_buffer());
        let hint = " [enter] submit  [esc] cancel";
        Paragraph::new(vec![
            Line::from(label),
            Line::from(input_line),
            Line::from(hint),
        ])
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .style(Style::default().fg(Color::Yellow))
    } else if s.standby {
        Paragraph::new(vec![
            Line::from(" ✓ DONE · standby — waiting for new input"),
            Line::from(" [a] add task  [q] quit"),
        ])
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::Green)),
        )
        .style(Style::default().fg(Color::Green))
    } else {
        Paragraph::new(Line::from(
            " [↑↓] navigate  [enter] answer  [a] add task  [q] quit",
        ))
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .style(Style::default().fg(Color::DarkGray))
    };
    f.render_widget(footer_widget, chunks[2]);
}
