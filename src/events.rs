use chrono::Local;
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

/// Progress sink. Phase 1 uses EventLineReporter (stderr lines, mirroring the
/// non-TTY behavior of lib/progress.sh). Phase 2 adds ChannelReporter (TUI).
pub trait Reporter: Send + Sync {
    /// A job (planner or worker) has been dispatched. `log` is the job's log file.
    fn dispatch(&self, id: &str, label: &str, tool: &str, model: &str, log: Option<&Path>);
    /// A job changed status (done/failed/merged/bounced/...).
    fn status(&self, id: &str, status: &str, tool: &str, model: &str);
    /// End-of-iteration summary line.
    fn iteration(&self, n: u32, merged: u32, gate: &str, open: i64);
    /// An agent raised a question for the user. Default: no-op.
    fn question(&self, _item_id: &str, _label: &str, _text: &str, _context: &str) {}
    /// The loop entered the standby state (Phase 3). Default: no-op.
    fn standby(&self) {}
}

pub struct EventLineReporter;

fn hms() -> String {
    Local::now().format("%H:%M:%S").to_string()
}

impl Reporter for EventLineReporter {
    fn dispatch(&self, id: &str, label: &str, tool: &str, model: &str, _log: Option<&Path>) {
        eprintln!("{}  dispatch {:<10} {}/{}  {}", hms(), id, tool, model, label);
    }
    fn status(&self, id: &str, status: &str, tool: &str, model: &str) {
        eprintln!("{}  {:<9} {:<10} {}/{}", hms(), status, id, tool, model);
    }
    fn iteration(&self, n: u32, merged: u32, gate: &str, open: i64) {
        eprintln!("iter {n}: merged={merged} gate={gate} open={open}");
    }
    fn question(&self, item_id: &str, _label: &str, text: &str, _context: &str) {
        eprintln!("{}  question  {:<10} {}", hms(), item_id, text);
    }
    fn standby(&self) {
        eprintln!("=== standby: waiting for input ===");
    }
}

/// Orchestrator -> UI.
#[derive(Debug, Clone)]
pub enum Event {
    JobDispatched {
        id: String,
        label: String,
        tool: String,
        model: String,
        log_path: Option<PathBuf>,
    },
    JobStatus {
        id: String,
        status: String,
    },
    QuestionRaised {
        item_id: String,
        label: String,
        text: String,
        context: String,
    },
    Iteration {
        n: u32,
        merged: u32,
        gate: String,
        open: i64,
    },
    EnteredStandby,
    Shutdown,
}

/// UI -> orchestrator.
#[derive(Debug, Clone)]
pub enum Command {
    AnswerQuestion { item_id: String, text: String },
    AddTask { request: String },
    Quit,
}

/// Reporter that forwards progress to the TUI over a channel.
pub struct ChannelReporter {
    tx: mpsc::UnboundedSender<Event>,
}

impl ChannelReporter {
    pub fn new(tx: mpsc::UnboundedSender<Event>) -> Self {
        Self { tx }
    }
}

impl Reporter for ChannelReporter {
    fn dispatch(&self, id: &str, label: &str, tool: &str, model: &str, log: Option<&Path>) {
        let _ = self.tx.send(Event::JobDispatched {
            id: id.into(),
            label: label.into(),
            tool: tool.into(),
            model: model.into(),
            log_path: log.map(|p| p.to_path_buf()),
        });
    }
    fn status(&self, id: &str, status: &str, _tool: &str, _model: &str) {
        let _ = self.tx.send(Event::JobStatus {
            id: id.into(),
            status: status.into(),
        });
    }
    fn iteration(&self, n: u32, merged: u32, gate: &str, open: i64) {
        let _ = self.tx.send(Event::Iteration {
            n,
            merged,
            gate: gate.into(),
            open,
        });
    }
    fn question(&self, item_id: &str, label: &str, text: &str, context: &str) {
        let _ = self.tx.send(Event::QuestionRaised {
            item_id: item_id.into(),
            label: label.into(),
            text: text.into(),
            context: context.into(),
        });
    }
    fn standby(&self) {
        let _ = self.tx.send(Event::EnteredStandby);
    }
}
