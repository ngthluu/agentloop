use chrono::Local;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Progress sink. Phase 1 uses EventLineReporter (stderr lines, mirroring the
/// non-TTY behavior of lib/progress.sh). Phase 2 adds ChannelReporter (TUI).
pub trait Reporter: Send + Sync {
    /// A job has been dispatched. `log` is the job's log file.
    fn dispatch(&self, id: &str, label: &str, tool: &str, model: &str, log: Option<&Path>);
    /// A job changed status (done/failed/merged/bounced/...). `note` is the
    /// human-readable reason for failures/bounces ("" when there is none).
    fn status(&self, id: &str, status: &str, tool: &str, model: &str, note: &str);
    /// End-of-iteration summary line.
    fn iteration(&self, n: u32, merged: u32, gate: &str, open: i64);
    /// The loop entered the standby state (Phase 3) for `reason`
    /// (done / stall / cap — shown in the TUI status bar). Default: no-op.
    fn standby(&self, _reason: &str) {}
}

pub struct EventLineReporter;

fn hms() -> String {
    Local::now().format("%H:%M:%S").to_string()
}

impl Reporter for EventLineReporter {
    fn dispatch(&self, id: &str, label: &str, tool: &str, model: &str, _log: Option<&Path>) {
        eprintln!(
            "{}  dispatch {:<10} {}/{}  {}",
            hms(),
            id,
            tool,
            model,
            label
        );
    }
    fn status(&self, id: &str, status: &str, tool: &str, model: &str, note: &str) {
        if note.is_empty() {
            eprintln!("{}  {:<9} {:<10} {}/{}", hms(), status, id, tool, model);
        } else {
            eprintln!(
                "{}  {:<9} {:<10} {}/{}  {}",
                hms(),
                status,
                id,
                tool,
                model,
                note
            );
        }
    }
    fn iteration(&self, n: u32, merged: u32, gate: &str, open: i64) {
        eprintln!("iter {n}: merged={merged} gate={gate} open={open}");
    }
    fn standby(&self, reason: &str) {
        eprintln!("=== standby: {reason} — waiting for input ===");
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
    Iteration {
        n: u32,
        merged: u32,
        gate: String,
        open: i64,
    },
    EnteredStandby { reason: String },
    Shutdown,
}

/// UI -> orchestrator.
#[derive(Debug, Clone)]
pub enum Command {
    StartRun { goal: String },
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
    fn status(&self, id: &str, status: &str, _tool: &str, _model: &str, _note: &str) {
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
    fn standby(&self, reason: &str) {
        let _ = self.tx.send(Event::EnteredStandby {
            reason: reason.into(),
        });
    }
}

/// Decorator that persists every dispatch/status/iteration to
/// `.agentloop/state/events.jsonl` (via crate::history) before forwarding, so
/// bounces and failures survive after the TUI exits and are queryable with
/// `agentloop --report`.
pub struct RecordingReporter {
    ws: PathBuf,
    inner: Arc<dyn Reporter>,
}

impl RecordingReporter {
    pub fn new(ws: PathBuf, inner: Arc<dyn Reporter>) -> Self {
        Self { ws, inner }
    }
}

impl Reporter for RecordingReporter {
    fn dispatch(&self, id: &str, label: &str, tool: &str, model: &str, log: Option<&Path>) {
        // status="running" marks the start of a job's life in the event log.
        crate::history::record(&self.ws, "dispatch", id, "running", label);
        self.inner.dispatch(id, label, tool, model, log);
    }
    fn status(&self, id: &str, status: &str, tool: &str, model: &str, note: &str) {
        crate::history::record(&self.ws, "status", id, status, note);
        self.inner.status(id, status, tool, model, note);
    }
    fn iteration(&self, n: u32, merged: u32, gate: &str, open: i64) {
        crate::history::record(
            &self.ws,
            "iteration",
            &format!("iter-{n}"),
            gate,
            &format!("merged={merged} open={open}"),
        );
        self.inner.iteration(n, merged, gate, open);
    }
    fn standby(&self, reason: &str) {
        // Not recorded: standby is a UI session state, not a job lifecycle event.
        self.inner.standby(reason);
    }
}
