use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct Question {
    pub question: String,
    #[serde(default)]
    pub context: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Answer {
    pub question: String,
    pub answer: String,
    pub ts: i64,
}

fn qpath(ws: &Path, id: &str) -> std::path::PathBuf {
    ws.join(format!(".agentloop/questions/{id}.json"))
}

fn apath(ws: &Path, id: &str) -> std::path::PathBuf {
    ws.join(format!(".agentloop/answers/{id}.json"))
}

pub fn read_question(ws: &Path, id: &str) -> Result<Question> {
    let text = std::fs::read_to_string(qpath(ws, id))
        .with_context(|| format!("no question for {id}"))?;
    serde_json::from_str(&text).context("parse question json")
}

pub fn has_question(ws: &Path, id: &str) -> bool {
    qpath(ws, id).exists()
}

pub fn record_answer(ws: &Path, id: &str, question: &str, answer: &str) -> Result<()> {
    let a = Answer {
        question: question.into(),
        answer: answer.into(),
        ts: chrono::Local::now().timestamp(),
    };
    std::fs::create_dir_all(ws.join(".agentloop/answers"))?;
    std::fs::write(apath(ws, id), serde_json::to_vec_pretty(&a)?)?;
    Ok(())
}

pub fn read_answer(ws: &Path, id: &str) -> Result<Answer> {
    let text = std::fs::read_to_string(apath(ws, id))
        .with_context(|| format!("no answer for {id}"))?;
    serde_json::from_str(&text).context("parse answer json")
}

/// A prompt block describing the prior question + the user's answer, or "" if none.
pub fn prior_qa_block(ws: &Path, id: &str) -> Result<String> {
    match read_answer(ws, id) {
        Ok(a) => Ok(format!(
            "\n\nEARLIER YOU ASKED THE USER A QUESTION; HERE IS THEIR ANSWER:\n  Q: {}\n  A: {}\nProceed using this answer.",
            a.question, a.answer
        )),
        Err(_) => Ok(String::new()),
    }
}

/// Archive the question file under logs/ so it isn't re-raised.
pub fn consume_question(ws: &Path, id: &str) -> Result<()> {
    let q = qpath(ws, id);
    if q.exists() {
        let dest = ws.join(format!(".agentloop/logs/answered-{id}.json"));
        std::fs::rename(&q, &dest).or_else(|_| std::fs::remove_file(&q))?;
    }
    Ok(())
}
