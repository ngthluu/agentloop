use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;

pub fn read(path: &Path) -> Result<Value> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

/// Atomic, durable write: temp file in the same dir, fsync, then rename. The
/// fsync matters: without it a crash/power loss can persist the rename before
/// the data blocks, leaving a present-but-empty backlog.json — the worst state
/// file to tear. The temp name carries a per-process counter so concurrent
/// writers inside one process never collide on the temp path.
pub(crate) fn write_atomic(path: &Path, v: &Value) -> Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = dir.join(format!(
        ".state.{}.{}.tmp",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, serde_json::to_vec_pretty(v)?)?;
    if let Ok(f) = std::fs::File::open(&tmp) {
        let _ = f.sync_all();
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Whether `id` is safe to use as a git refname component and a filesystem path
/// segment. Task/builder ids are LLM-generated; without this check an id like
/// `../../escape` becomes a path-traversal write and `b ad~id` an invalid git ref.
pub fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 100
        && !id.starts_with(['-', '.'])
        && !id.contains("..")
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

pub fn backlog_valid(path: &Path) -> bool {
    let Ok(v) = read(path) else {
        return false;
    };
    let Some(items) = v.get("items").and_then(|i| i.as_array()) else {
        return false;
    };
    // Every item's id must be present and safe: ids become git branch names and
    // filesystem paths, so an invalid id here would poison dispatch later.
    items.iter().all(|it| {
        it.get("id")
            .and_then(|i| i.as_str())
            .map(safe_id)
            .unwrap_or(false)
    })
}

/// Ids that should be dispatched this round: items whose deps are all `done` and
/// that are either `ready` or a manager dependency-`blocked` item with NO pending
/// user question. Including the latter is what makes the loop fully autonomous —
/// the manager uses `blocked` for sequencing, so such items run as soon as their
/// deps complete instead of stalling the loop. A `blocked` item that carries a real
/// user question (`.agentloop/questions/<id>.json`) is left for the user to answer.
pub fn ready_items(path: &Path, ws: &Path, max_parallel: usize) -> Result<Vec<String>> {
    let v = read(path)?;
    let empty = vec![];
    let items = v["items"].as_array().unwrap_or(&empty);
    let done: HashSet<&str> = items
        .iter()
        .filter(|i| i["status"] == "done")
        .filter_map(|i| i["id"].as_str())
        .collect();
    let mut out = Vec::new();
    for it in items {
        let id = match it["id"].as_str() {
            Some(i) => i,
            None => continue,
        };
        let dispatchable = match it["status"].as_str() {
            Some("ready") => true,
            Some("blocked") => !crate::inbox::has_question(ws, id),
            _ => false,
        };
        if !dispatchable {
            continue;
        }
        let deps_ok = match it.get("deps").and_then(|d| d.as_array()) {
            Some(deps) => deps
                .iter()
                .all(|d| d.as_str().map(|s| done.contains(s)).unwrap_or(false)),
            None => true, // missing deps key == no deps
        };
        if deps_ok {
            out.push(id.to_string());
        }
    }
    out.truncate(max_parallel);
    Ok(out)
}

/// Items that still hold the run open: anything not terminally `done` or
/// `failed`. Counting by exclusion is deliberate — an unknown status written by
/// a confused manager (or a newer binary's state) must keep the loop alive and
/// get surfaced, not silently vanish from accounting and produce a false DONE.
pub fn open_count(path: &Path) -> Result<i64> {
    let v = read(path)?;
    let empty = vec![];
    let n = v["items"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter(|i| !matches!(i["status"].as_str(), Some("done") | Some("failed")))
        .count();
    Ok(n as i64)
}

/// Number of `failed` items. Failed tasks are not dispatchable (so they are
/// not "open"), but unresolved failures must keep the loop alive: the manager
/// is required to reshape or drop them before the run can be DONE.
pub fn failed_count(path: &Path) -> Result<i64> {
    let v = read(path)?;
    let empty = vec![];
    let n = v["items"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter(|i| i["status"] == "failed")
        .count();
    Ok(n as i64)
}

/// Semantic fingerprint of loop-relevant state: every backlog item's
/// (id, status, attempts, deps) plus every task-local builder's
/// (id, status, attempts). Any change here is cap-bounded forward motion
/// (attempt and redesign caps), so the stall detector treats an unchanged
/// fingerprint across iterations as a dead loop. Notes/titles are excluded
/// on purpose: agents rephrase them without making progress.
pub fn progress_fingerprint(bk: &Path, ws: &Path) -> String {
    let mut parts: Vec<String> = Vec::new();
    let empty = vec![];
    if let Ok(v) = read(bk) {
        for it in v["items"].as_array().unwrap_or(&empty) {
            let deps = it["deps"]
                .as_array()
                .map(|d| {
                    d.iter()
                        .filter_map(|x| x.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_default();
            parts.push(format!(
                "{}={}:{}:{}",
                it["id"], it["status"], it["attempts"], deps
            ));
        }
    }
    if let Ok(entries) = std::fs::read_dir(ws.join(".agentloop/state/tasks")) {
        let mut dirs: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        dirs.sort();
        for dir in dirs {
            let Ok(text) = std::fs::read_to_string(dir.join("builders.json")) else {
                continue;
            };
            let Ok(v) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let task = dir
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            for it in v["items"].as_array().unwrap_or(&empty) {
                parts.push(format!(
                    "{task}/{}={}:{}",
                    it["id"], it["status"], it["attempts"]
                ));
            }
        }
    }
    parts.sort();
    parts.join("|")
}

pub fn set_status(path: &Path, id: &str, status: &str, note: &str) -> Result<()> {
    let mut v = read(path)?;
    if let Some(items) = v["items"].as_array_mut() {
        for it in items.iter_mut() {
            if it["id"] == json!(id) {
                it["status"] = json!(status);
                if !note.is_empty() {
                    it["notes"] = json!(note);
                }
            }
        }
    }
    write_atomic(path, &v)
}

pub fn increment_attempts(path: &Path, id: &str) -> Result<()> {
    let mut v = read(path)?;
    if let Some(items) = v["items"].as_array_mut() {
        for it in items.iter_mut() {
            if it["id"] == json!(id) {
                let cur = it.get("attempts").and_then(|a| a.as_u64()).unwrap_or(0);
                it["attempts"] = json!(cur + 1);
            }
        }
    }
    write_atomic(path, &v)
}

/// Convenience accessor used by the orchestrator.
pub fn item<'a>(v: &'a Value, id: &str) -> Option<&'a Value> {
    v["items"].as_array()?.iter().find(|i| i["id"] == json!(id))
}

/// Remove deps that reference ids not present in the backlog at all — they can
/// never be satisfied (the `done` set can never include them), so the item would
/// sit open forever. Only open items (ready/in_progress/blocked) are repaired.
/// Returns the removed (item_id, dep_id) pairs; each repaired item gets a note.
pub fn strip_unknown_deps(path: &Path) -> Result<Vec<(String, String)>> {
    let mut v = read(path)?;
    let empty = vec![];
    let ids: HashSet<String> = v["items"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter_map(|i| i["id"].as_str().map(str::to_string))
        .collect();
    let mut removed: Vec<(String, String)> = Vec::new();
    if let Some(items) = v["items"].as_array_mut() {
        for it in items.iter_mut() {
            if !matches!(
                it["status"].as_str(),
                Some("ready") | Some("in_progress") | Some("blocked")
            ) {
                continue;
            }
            let Some(id) = it["id"].as_str().map(str::to_string) else {
                continue;
            };
            let Some(deps) = it.get_mut("deps").and_then(|d| d.as_array_mut()) else {
                continue;
            };
            let mut gone: Vec<String> = Vec::new();
            deps.retain(|d| match d.as_str() {
                Some(dep) if !ids.contains(dep) => {
                    gone.push(dep.to_string());
                    false
                }
                _ => true,
            });
            if !gone.is_empty() {
                it["notes"] = json!(format!("removed deps on unknown ids: {}", gone.join(", ")));
                removed.extend(gone.into_iter().map(|g| (id.clone(), g)));
            }
        }
    }
    if !removed.is_empty() {
        write_atomic(path, &v)?;
    }
    Ok(removed)
}

/// Clamp any item `notes` longer than `max_bytes`. Self-heal for a backlog
/// poisoned by a pre-cap run (or any future unbounded write): oversized notes
/// are inlined into manager/architect prompts and push the spawn argv past
/// ARG_MAX (E2BIG) on every dispatch — a crash loop, since backlog.json
/// persists across runs. Returns the ids whose notes were clamped.
pub fn clamp_oversized_notes(path: &Path, max_bytes: usize) -> Result<Vec<String>> {
    let mut v = read(path)?;
    let mut clamped = Vec::new();
    if let Some(items) = v["items"].as_array_mut() {
        for it in items.iter_mut() {
            let Some(notes) = it["notes"].as_str() else {
                continue;
            };
            if notes.len() > max_bytes {
                let id = it["id"].as_str().unwrap_or("?").to_string();
                it["notes"] = json!(crate::limits::clamp_str(notes, max_bytes));
                clamped.push(id);
            }
        }
    }
    if !clamped.is_empty() {
        write_atomic(path, &v)?;
    }
    Ok(clamped)
}

/// Lines describing every `failed` item (id, title, note head), or "" when there
/// are none. Failed items hold the run open (`loop_done` requires failed == 0)
/// but are not dispatchable, so the manager MUST see all of them — a failed leaf
/// task with no dependents would otherwise never be surfaced and the run could
/// never finish.
pub fn failed_items_report(path: &Path) -> Result<String> {
    let v = read(path)?;
    let empty = vec![];
    let mut out = String::new();
    for it in v["items"].as_array().unwrap_or(&empty) {
        if it["status"] != "failed" {
            continue;
        }
        let id = it["id"].as_str().unwrap_or("?");
        let title = it["title"].as_str().unwrap_or("");
        let note = crate::limits::clamp_str(it["notes"].as_str().unwrap_or(""), 512);
        out.push_str(&format!("  - {id} ({title}): {note}\n"));
    }
    Ok(out)
}

/// Lines describing open items that depend on `failed` items (they can never run
/// until the manager reshapes them), or "" when there are none. Used to build the
/// manager-prompt repair section.
pub fn failed_dep_report(path: &Path) -> Result<String> {
    let v = read(path)?;
    let empty = vec![];
    let items = v["items"].as_array().unwrap_or(&empty);
    let failed: HashSet<&str> = items
        .iter()
        .filter(|i| i["status"] == "failed")
        .filter_map(|i| i["id"].as_str())
        .collect();
    let mut out = String::new();
    for it in items {
        if !matches!(
            it["status"].as_str(),
            Some("ready") | Some("in_progress") | Some("blocked")
        ) {
            continue;
        }
        let Some(id) = it["id"].as_str() else {
            continue;
        };
        let bad: Vec<&str> = it
            .get("deps")
            .and_then(|d| d.as_array())
            .map(|deps| {
                deps.iter()
                    .filter_map(|d| d.as_str())
                    .filter(|d| failed.contains(d))
                    .collect()
            })
            .unwrap_or_default();
        if !bad.is_empty() {
            out.push_str(&format!("  - {id} depends on failed {}\n", bad.join(", ")));
        }
    }
    Ok(out)
}
