use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub fn task_dir(ws: &Path, task_id: &str) -> PathBuf {
    ws.join(".agentloop/state/tasks").join(task_id)
}

pub fn ensure_task_dir(ws: &Path, task_id: &str) -> Result<PathBuf> {
    let dir = task_dir(ws, task_id);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    Ok(dir)
}

pub fn builders_path(ws: &Path, task_id: &str) -> PathBuf {
    task_dir(ws, task_id).join("builders.json")
}

pub fn customer_path(ws: &Path, task_id: &str) -> PathBuf {
    task_dir(ws, task_id).join("customer.json")
}

pub fn read_builders(ws: &Path, task_id: &str) -> Result<Value> {
    read_json(&builders_path(ws, task_id))
}

pub fn write_builders(ws: &Path, task_id: &str, v: &Value) -> Result<()> {
    ensure_task_dir(ws, task_id)?;
    write_atomic(&builders_path(ws, task_id), v)
}

pub fn builder_plan_valid(ws: &Path, task_id: &str) -> bool {
    if task_id.is_empty() {
        return false;
    }

    let dir = task_dir(ws, task_id);
    let design = std::fs::read_to_string(dir.join("design.md")).unwrap_or_default();
    if design.trim().is_empty() {
        return false;
    }

    matches!(read_builders(ws, task_id), Ok(v) if builders_items_valid(&v, task_id))
}

pub fn item<'a>(v: &'a Value, id: &str) -> Option<&'a Value> {
    v["items"].as_array()?.iter().find(|i| i["id"] == json!(id))
}

pub fn ready_builders(ws: &Path, task_id: &str, max_parallel: usize) -> Result<Vec<String>> {
    let v = read_builders(ws, task_id)?;
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
            Some(id) => id,
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
        let deps_ok = match it.get("deps").and_then(|deps| deps.as_array()) {
            Some(deps) => deps
                .iter()
                .all(|dep| dep.as_str().map(|s| done.contains(s)).unwrap_or(false)),
            None => true,
        };
        if deps_ok {
            out.push(id.to_string());
        }
    }

    out.truncate(max_parallel);
    Ok(out)
}

pub fn open_builder_count(ws: &Path, task_id: &str) -> Result<i64> {
    let v = read_builders(ws, task_id)?;
    let empty = vec![];
    let n = v["items"]
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .filter(|i| {
            matches!(
                i["status"].as_str(),
                Some("ready") | Some("in_progress") | Some("blocked")
            )
        })
        .count();
    Ok(n as i64)
}

pub fn all_builders_done(ws: &Path, task_id: &str) -> Result<bool> {
    let v = read_builders(ws, task_id)?;
    let Some(items) = v["items"].as_array() else {
        return Ok(false);
    };
    Ok(!items.is_empty() && items.iter().all(|i| i["status"] == "done"))
}

pub fn set_builder_status(
    ws: &Path,
    task_id: &str,
    builder_id: &str,
    status: &str,
    note: &str,
) -> Result<()> {
    let mut v = read_builders(ws, task_id)?;
    if let Some(items) = v["items"].as_array_mut() {
        for it in items.iter_mut() {
            if it["id"] == json!(builder_id) {
                it["status"] = json!(status);
                if !note.is_empty() {
                    it["notes"] = json!(note);
                }
            }
        }
    }
    write_builders(ws, task_id, &v)
}

pub fn increment_builder_attempts(ws: &Path, task_id: &str, builder_id: &str) -> Result<()> {
    let mut v = read_builders(ws, task_id)?;
    if let Some(items) = v["items"].as_array_mut() {
        for it in items.iter_mut() {
            if it["id"] == json!(builder_id) {
                let cur = it
                    .get("attempts")
                    .and_then(|attempts| attempts.as_u64())
                    .unwrap_or(0);
                it["attempts"] = json!(cur + 1);
            }
        }
    }
    write_builders(ws, task_id, &v)
}

pub fn write_customer(ws: &Path, task_id: &str, v: &Value) -> Result<()> {
    ensure_task_dir(ws, task_id)?;
    write_atomic(&customer_path(ws, task_id), v)
}

pub fn customer_approved(ws: &Path, task_id: &str) -> bool {
    matches!(
        read_json(&customer_path(ws, task_id)),
        Ok(v) if v.get("status").and_then(|status| status.as_str()) == Some("approved")
    )
}

fn read_json(path: &Path) -> Result<Value> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

fn write_atomic(path: &Path, v: &Value) -> Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let tmp = dir.join(format!(".task-state.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(v)?)
        .with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename {}", path.display()))?;
    Ok(())
}

fn builders_items_valid(v: &Value, task_id: &str) -> bool {
    let Some(items) = v.get("items").and_then(|items| items.as_array()) else {
        return false;
    };

    let mut seen = HashSet::new();
    !items.is_empty()
        && items.iter().all(|item| {
            let Some(id) = item.get("id").and_then(|id| id.as_str()) else {
                return false;
            };
            builder_item_valid(item, task_id) && seen.insert(id.to_string())
        })
}

fn builder_item_valid(item: &Value, task_id: &str) -> bool {
    non_empty_str(item, "id")
        && non_empty_str(item, "title")
        && non_empty_str(item, "desc")
        && non_empty_str(item, "acceptance")
        && item
            .get("id")
            .and_then(|id| id.as_str())
            .map(|id| id.starts_with(&format!("{task_id}-")))
            .unwrap_or(false)
        && item.get("deps").and_then(|deps| deps.as_array()).is_some()
        && matches!(
            item.get("status").and_then(|status| status.as_str()),
            Some("ready" | "in_progress" | "done" | "failed" | "blocked")
        )
        && item
            .get("attempts")
            .and_then(|attempts| attempts.as_u64())
            .is_some()
}

fn non_empty_str(item: &Value, key: &str) -> bool {
    item.get(key)
        .and_then(|value| value.as_str())
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}
