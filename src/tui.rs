use crossterm::event::{KeyCode, KeyEvent};
use crate::events::{Command, Event};

#[derive(Clone)]
pub struct Job {
    pub id: String,
    pub label: String,
    pub tool: String,
    pub model: String,
    pub status: String,
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
        }
    }

    pub fn apply(&mut self, ev: Event) {
        match ev {
            Event::JobDispatched { id, label, tool, model } => {
                if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
                    j.label = label;
                    j.tool = tool;
                    j.model = model;
                    j.status = "running".into();
                } else {
                    self.jobs.push(Job { id, label, tool, model, status: "running".into() });
                }
            }
            Event::JobStatus { id, status } => {
                if let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) {
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
            Mode::Normal => match k.code {
                KeyCode::Char('q') => Some(Command::Quit),
                KeyCode::Char('a') => {
                    self.mode = Mode::AddingTask;
                    self.input.clear();
                    None
                }
                KeyCode::Up => {
                    if self.selected > 0 {
                        self.selected -= 1;
                    }
                    None
                }
                KeyCode::Down => {
                    if self.selected + 1 < self.inbox.len() {
                        self.selected += 1;
                    }
                    None
                }
                KeyCode::Enter => {
                    if !self.inbox.is_empty() {
                        self.mode = Mode::Answering;
                        self.input.clear();
                    }
                    None
                }
                _ => None,
            },
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
