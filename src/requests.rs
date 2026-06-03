use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub ts: i64,
    pub text: String,
    pub status: String, // "pending" | "consumed"
}

fn path(ws: &Path) -> PathBuf {
    ws.join(".agentloop/state/requests.jsonl")
}

fn read_all(ws: &Path) -> Result<Vec<Request>> {
    let p = path(ws);
    if !p.exists() {
        return Ok(vec![]);
    }
    let text = std::fs::read_to_string(&p)?;
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        out.push(serde_json::from_str(line)?);
    }
    Ok(out)
}

fn write_all(ws: &Path, reqs: &[Request]) -> Result<()> {
    let p = path(ws);
    let dir = p.parent().unwrap();
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".requests.{}.tmp", std::process::id()));
    let mut buf = String::new();
    for r in reqs {
        buf.push_str(&serde_json::to_string(r)?);
        buf.push('\n');
    }
    std::fs::write(&tmp, buf)?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

pub fn append(ws: &Path, text: &str) -> Result<()> {
    let mut all = read_all(ws)?;
    all.push(Request {
        ts: chrono::Local::now().timestamp(),
        text: text.to_string(),
        status: "pending".into(),
    });
    write_all(ws, &all)
}

pub fn pending(ws: &Path) -> Result<Vec<String>> {
    Ok(read_all(ws)?
        .into_iter()
        .filter(|r| r.status == "pending")
        .map(|r| r.text)
        .collect())
}

pub fn mark_all_consumed(ws: &Path) -> Result<()> {
    let mut all = read_all(ws)?;
    for r in all.iter_mut() {
        if r.status == "pending" {
            r.status = "consumed".into();
        }
    }
    write_all(ws, &all)
}

/// A manager-prompt section listing pending requests, or "" if none.
pub fn prompt_block(ws: &Path) -> Result<String> {
    let p = pending(ws)?;
    if p.is_empty() {
        return Ok(String::new());
    }
    let mut s = String::from(
        "\n\nPENDING USER REQUESTS (fold these into the backlog this round, then they are consumed):\n",
    );
    for (i, t) in p.iter().enumerate() {
        s.push_str(&format!("  {}. {}\n", i + 1, t));
    }
    Ok(s)
}
