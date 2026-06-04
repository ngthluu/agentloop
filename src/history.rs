use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Append-only status-transition history. One JSON object per line; never
/// truncated or rewritten, so bounces/failures survive after the TUI exits.
pub fn events_path(ws: &Path) -> PathBuf {
    ws.join(".agentloop/state/events.jsonl")
}

/// Append one event. Best-effort: history must never break the loop.
pub fn record(ws: &Path, kind: &str, id: &str, status: &str, reason: &str) {
    if let Err(e) = try_record(ws, kind, id, status, reason) {
        eprintln!("history: record failed for {id}: {e:#}");
    }
}

/// First line of `reason`, capped at `max` chars (full output lives in the job
/// logs / gate.log; events.jsonl stays one skimmable line per event).
fn one_line(reason: &str, max: usize) -> String {
    let first = reason.lines().next().unwrap_or("");
    let mut s: String = first.chars().take(max).collect();
    if first.chars().count() > max || reason.lines().count() > 1 {
        s.push('…');
    }
    s
}

fn try_record(ws: &Path, kind: &str, id: &str, status: &str, reason: &str) -> Result<()> {
    let path = events_path(ws);
    let dir = path.parent().context("events path has no parent")?;
    std::fs::create_dir_all(dir)?;
    let ev = json!({
        "ts": chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        "kind": kind,
        "id": id,
        "status": status,
        "reason": one_line(reason, 500),
    });
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(f, "{ev}")?;
    Ok(())
}

/// All recorded events, oldest first. Missing file -> empty; unparseable lines skipped.
pub fn read_events(ws: &Path) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(events_path(ws)) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Move `path` into `dir` with a timestamp prefix so repeats never overwrite.
/// Missing source is a no-op. This is how loop artifacts are retired: archived
/// for troubleshooting, never deleted.
pub fn archive_file(path: &Path, dir: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .context("archive: source has no file name")?
        .to_string();
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let mut dest = dir.join(format!("{stamp}-{name}"));
    let mut n = 1u32;
    while dest.exists() {
        dest = dir.join(format!("{stamp}-{n}-{name}"));
        n += 1;
    }
    std::fs::rename(path, &dest).or_else(|_| {
        std::fs::copy(path, &dest)
            .map(|_| ())
            .and_then(|_| std::fs::remove_file(path))
    })?;
    Ok(())
}
