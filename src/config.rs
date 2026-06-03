use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const DEFAULT_CONFIG_JSON: &str = r#"{
  "caps": {
    "max_iterations": 25,
    "max_parallel": 3,
    "item_timeout_sec": 1200,
    "total_budget_sec": 21600,
    "max_attempts": 3
  },
  "routing": {
    "manager": { "tool": "claude", "model": "opus", "effort": "high" },
    "architect": { "tool": "claude", "model": "opus", "effort": "high" },
    "builder": { "tool": "codex", "model": "gpt-5", "effort": "high" },
    "customer": { "tool": "claude", "model": "sonnet", "effort": "medium" },
    "resolver": { "tool": "claude", "model": "sonnet", "effort": "medium" }
  },
  "defaults": { "role": "builder" }
}
"#;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Caps {
    pub max_iterations: Option<u32>,
    pub max_parallel: Option<u32>,
    pub item_timeout_sec: Option<u64>,
    pub total_budget_sec: Option<u64>,
    pub max_attempts: Option<u32>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Role {
    pub tool: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Defaults {
    pub role: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub caps: Caps,
    #[serde(default)]
    pub routing: BTreeMap<String, Role>,
    #[serde(default)]
    pub defaults: Defaults,
}

impl Config {
    pub fn default_config_path() -> PathBuf {
        if let Some(path) = non_empty_env_path("AGENTLOOP_CONFIG") {
            return path;
        }

        home_dir().join(".agentloop").join("config.json")
    }

    pub fn ensure_default_config(path: &Path) -> Result<PathBuf> {
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create config dir {}", parent.display()))?;
            }
            std::fs::write(path, DEFAULT_CONFIG_JSON)
                .with_context(|| format!("write default config {}", path.display()))?;
        }
        Ok(path.to_path_buf())
    }

    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read config {}", path.display()))?;
        if looks_yaml_like(path, &text) {
            bail!("config must be JSON; migrate config.yaml to config.json");
        }
        serde_json::from_str(&text).context("parse config json")
    }

    /// Role to actually use: the role if present in routing, else defaults.role.
    pub fn resolve_role(&self, role: &str) -> Option<String> {
        if self.routing.contains_key(role) {
            Some(role.to_string())
        } else {
            self.defaults.role.clone()
        }
    }

    /// A role's field, or None if absent or empty (mirrors jq `// empty`).
    pub fn role_field(&self, role: &str, field: &str) -> Option<String> {
        let r = self.routing.get(role)?;
        let v = match field {
            "tool" => r.tool.clone(),
            "model" => r.model.clone(),
            "effort" => r.effort.clone(),
            _ => None,
        };
        v.filter(|s| !s.is_empty())
    }

    pub fn max_iterations(&self) -> u32 {
        self.caps.max_iterations.unwrap_or(25)
    }
    pub fn max_parallel(&self) -> u32 {
        self.caps.max_parallel.unwrap_or(3)
    }
    pub fn item_timeout_sec(&self) -> u64 {
        self.caps.item_timeout_sec.unwrap_or(1200)
    }
    pub fn total_budget_sec(&self) -> u64 {
        self.caps.total_budget_sec.unwrap_or(21600)
    }
    pub fn max_attempts(&self) -> u32 {
        self.caps.max_attempts.unwrap_or(3)
    }
}

fn non_empty_env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).and_then(|value| {
        if value.is_empty() {
            None
        } else {
            Some(PathBuf::from(value))
        }
    })
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn looks_yaml_like(path: &Path, text: &str) -> bool {
    if matches!(
        path.extension().and_then(|s| s.to_str()),
        Some("yaml" | "yml")
    ) {
        return true;
    }

    let trimmed = text.trim_start();
    !trimmed.is_empty() && !trimmed.starts_with('{') && !trimmed.starts_with('[')
}
