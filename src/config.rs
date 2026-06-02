use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

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
    pub flags: Option<String>,
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
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read config {}", path.display()))?;
        serde_yaml::from_str(&text).context("parse config yaml")
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
            "flags" => r.flags.clone(),
            _ => None,
        };
        v.filter(|s| !s.is_empty())
    }

    pub fn max_iterations(&self) -> u32 { self.caps.max_iterations.unwrap_or(25) }
    pub fn max_parallel(&self) -> u32 { self.caps.max_parallel.unwrap_or(3) }
    pub fn item_timeout_sec(&self) -> u64 { self.caps.item_timeout_sec.unwrap_or(1200) }
    pub fn total_budget_sec(&self) -> u64 { self.caps.total_budget_sec.unwrap_or(21600) }
    pub fn max_attempts(&self) -> u32 { self.caps.max_attempts.unwrap_or(3) }
}
