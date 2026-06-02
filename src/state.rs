use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::Path;

pub fn read(path: &Path) -> Result<Value> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

/// Atomic write: temp file in the same dir, then rename.
fn write_atomic(path: &Path, v: &Value) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = dir.join(format!(".state.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(v)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn backlog_valid(path: &Path) -> bool {
    matches!(read(path), Ok(v) if v.get("items").map(|i| i.is_array()).unwrap_or(false))
}

pub fn ready_items(path: &Path, max_parallel: usize) -> Result<Vec<String>> {
    let v = read(path)?;
    let empty = vec![];
    let items = v["items"].as_array().unwrap_or(&empty);
    let done: HashSet<&str> = items.iter()
        .filter(|i| i["status"] == "done")
        .filter_map(|i| i["id"].as_str())
        .collect();
    let mut out = Vec::new();
    for it in items {
        if it["status"] != "ready" { continue; }
        let deps_ok = match it.get("deps").and_then(|d| d.as_array()) {
            Some(deps) => deps.iter().all(|d| d.as_str().map(|s| done.contains(s)).unwrap_or(false)),
            None => true, // missing deps key == no deps
        };
        if deps_ok {
            if let Some(id) = it["id"].as_str() { out.push(id.to_string()); }
        }
    }
    out.truncate(max_parallel);
    Ok(out)
}

pub fn open_count(path: &Path) -> Result<i64> {
    let v = read(path)?;
    let empty = vec![];
    let n = v["items"].as_array().unwrap_or(&empty).iter()
        .filter(|i| matches!(i["status"].as_str(), Some("ready") | Some("in_progress") | Some("blocked")))
        .count();
    Ok(n as i64)
}

pub fn set_status(path: &Path, id: &str, status: &str, note: &str) -> Result<()> {
    let mut v = read(path)?;
    if let Some(items) = v["items"].as_array_mut() {
        for it in items.iter_mut() {
            if it["id"] == json!(id) {
                it["status"] = json!(status);
                if !note.is_empty() { it["notes"] = json!(note); }
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
