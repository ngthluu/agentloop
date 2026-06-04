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
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
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
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    writeln!(f, "{ev}").with_context(|| format!("write {}", path.display()))?;
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

/// Human-readable troubleshooting report: every bounced/failed/rejected event
/// ever recorded, plus what is failed right now in the backlog and in each
/// task's builder plan.
pub fn report(ws: &Path) -> String {
    let mut out = String::new();
    let events = read_events(ws);
    out.push_str(&format!("=== agentloop report — {} ===\n", ws.display()));
    if events.is_empty() {
        out.push_str("(no events recorded yet — events.jsonl is written by runs from this version on)\n");
    }

    let pick = |status: &str| -> Vec<&Value> {
        events
            .iter()
            .filter(|e| e["kind"] == "status" && e["status"] == status)
            .collect()
    };
    for (title, status) in [
        ("BOUNCED", "bounced"),
        ("FAILED", "failed"),
        ("REJECTED (customer)", "rejected"),
    ] {
        let evs = pick(status);
        out.push_str(&format!("\n{title} events: {}\n", evs.len()));
        for e in evs {
            out.push_str(&format!(
                "  {}  {:<24} {}\n",
                e["ts"].as_str().unwrap_or(""),
                e["id"].as_str().unwrap_or(""),
                e["reason"].as_str().unwrap_or("")
            ));
        }
    }

    let redesigns: Vec<&Value> = events.iter().filter(|e| e["kind"] == "task").collect();
    out.push_str(&format!(
        "\nTASK redesign/failure events: {}\n",
        redesigns.len()
    ));
    for e in redesigns {
        out.push_str(&format!(
            "  {}  {:<24} {:<8} {}\n",
            e["ts"].as_str().unwrap_or(""),
            e["id"].as_str().unwrap_or(""),
            e["status"].as_str().unwrap_or(""),
            e["reason"].as_str().unwrap_or("")
        ));
    }

    let bk = ws.join(".agentloop/state/backlog.json");
    if let Ok(v) = crate::state::read(&bk) {
        let empty = vec![];
        let failed: Vec<&Value> = v["items"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter(|i| i["status"] == "failed")
            .collect();
        out.push_str(&format!(
            "\nbacklog items currently failed: {}\n",
            failed.len()
        ));
        for i in failed {
            out.push_str(&format!(
                "  {}  {}\n    note: {}\n",
                i["id"].as_str().unwrap_or(""),
                i["title"].as_str().unwrap_or(""),
                i["notes"].as_str().unwrap_or("").lines().next().unwrap_or("")
            ));
        }
    }

    out.push_str("\nbuilders currently failed:\n");
    let mut none = true;
    if let Ok(entries) = std::fs::read_dir(ws.join(".agentloop/state/tasks")) {
        let mut names: Vec<String> = entries
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        for task_id in names {
            let Ok(b) = crate::task_state::read_builders(ws, &task_id) else {
                continue;
            };
            let empty = vec![];
            for i in b["items"].as_array().unwrap_or(&empty) {
                if i["status"] == "failed" {
                    none = false;
                    out.push_str(&format!(
                        "  {}/{} (attempts {})  {}\n",
                        task_id,
                        i["id"].as_str().unwrap_or(""),
                        i["attempts"].as_u64().unwrap_or(0),
                        i["notes"].as_str().unwrap_or("").lines().next().unwrap_or("")
                    ));
                }
            }
        }
    }
    if none {
        out.push_str("  (none)\n");
    }
    out
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
    }).with_context(|| format!("archive {} -> {}", path.display(), dest.display()))?;
    Ok(())
}
