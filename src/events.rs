use chrono::Local;

/// Progress sink. Phase 1 uses EventLineReporter (stderr lines, mirroring the
/// non-TTY behavior of lib/progress.sh). Phases 2-3 add a TUI implementation.
pub trait Reporter: Send + Sync {
    /// A job (planner or worker) has been dispatched.
    fn dispatch(&self, id: &str, label: &str, tool: &str, model: &str);
    /// A job changed status (done/failed/merged/bounced/...).
    fn status(&self, id: &str, status: &str, tool: &str, model: &str);
    /// End-of-iteration summary line.
    fn iteration(&self, n: u32, merged: u32, gate: &str, open: i64);
}

pub struct EventLineReporter;

fn hms() -> String {
    Local::now().format("%H:%M:%S").to_string()
}

impl Reporter for EventLineReporter {
    fn dispatch(&self, id: &str, label: &str, tool: &str, model: &str) {
        eprintln!("{}  dispatch {:<10} {}/{}  {}", hms(), id, tool, model, label);
    }
    fn status(&self, id: &str, status: &str, tool: &str, model: &str) {
        eprintln!("{}  {:<9} {:<10} {}/{}", hms(), status, id, tool, model);
    }
    fn iteration(&self, n: u32, merged: u32, gate: &str, open: i64) {
        eprintln!("iter {n}: merged={merged} gate={gate} open={open}");
    }
}
