use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Both-tools template: strong-reasoning roles on claude, builders on codex.
/// Also the fallback when neither CLI is detected, so preflight can fail fast
/// naming exactly what to install.
pub const DEFAULT_CONFIG_JSON: &str = r#"{
  "caps": {
    "max_iterations": 25,
    "max_parallel": 3,
    "item_timeout_sec": 1200,
    "total_budget_sec": 21600,
    "max_attempts": 3,
    "max_redesigns": 6
  },
  "routing": {
    "manager": { "tool": "claude", "model": "sonnet", "effort": "medium" },
    "architect": { "tool": "claude", "model": "opus", "effort": "xhigh" },
    "builder": { "tool": "codex", "model": "gpt-5.5", "effort": "medium" },
    "customer": { "tool": "claude", "model": "haiku", "effort": "low" },
    "resolver": { "tool": "claude", "model": "sonnet", "effort": "medium" }
  },
  "defaults": { "role": "builder" }
}
"#;

const CONFIG_JSON_CLAUDE_ONLY: &str = r#"{
  "caps": {
    "max_iterations": 25,
    "max_parallel": 3,
    "item_timeout_sec": 1200,
    "total_budget_sec": 21600,
    "max_attempts": 3,
    "max_redesigns": 6
  },
  "routing": {
    "manager": { "tool": "claude", "model": "sonnet", "effort": "medium" },
    "architect": { "tool": "claude", "model": "opus", "effort": "xhigh" },
    "manager": { "tool": "claude", "model": "sonnet", "effort": "medium" },
    "customer": { "tool": "claude", "model": "haiku", "effort": "low" },
    "resolver": { "tool": "claude", "model": "sonnet", "effort": "medium" }
  },
  "defaults": { "role": "builder" }
}
"#;

const CONFIG_JSON_CODEX_ONLY: &str = r#"{
  "caps": {
    "max_iterations": 25,
    "max_parallel": 3,
    "item_timeout_sec": 1200,
    "total_budget_sec": 21600,
    "max_attempts": 3,
    "max_redesigns": 6
  },
  "routing": {
    "manager": { "tool": "codex", "model": "gpt-5.5", "effort": "medium" },
    "architect": { "tool": "codex", "model": "gpt-5.5", "effort": "xhigh" },
    "builder": { "tool": "codex", "model": "gpt-5.5", "effort": "medium" },
    "customer": { "tool": "codex", "model": "gpt-5.5", "effort": "low" },
    "resolver": { "tool": "codex", "model": "gpt-5.5", "effort": "medium" }
  },
  "defaults": { "role": "builder" }
}
"#;

/// The default config template for an environment: every role must route to a
/// CLI the user actually has. With both installed, reasoning-heavy roles go to
/// claude and builders to codex; with one, everything routes to it; with
/// neither, the both-tools template lets preflight name what to install.
pub fn default_config_for(has_claude: bool, has_codex: bool) -> &'static str {
    match (has_claude, has_codex) {
        (true, false) => CONFIG_JSON_CLAUDE_ONLY,
        (false, true) => CONFIG_JSON_CODEX_ONLY,
        _ => DEFAULT_CONFIG_JSON,
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Caps {
    pub max_iterations: Option<u32>,
    pub max_parallel: Option<u32>,
    pub item_timeout_sec: Option<u64>,
    pub total_budget_sec: Option<u64>,
    pub max_attempts: Option<u32>,
    pub max_redesigns: Option<u32>,
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

    /// First-run config seeding against the real PATH: detect which agent CLIs
    /// are installed and write a routing that only references those.
    pub fn ensure_default_config(path: &Path) -> Result<PathBuf> {
        Self::ensure_default_config_with_path(path, &std::env::var("PATH").unwrap_or_default())
    }

    pub fn ensure_default_config_with_path(path: &Path, path_var: &str) -> Result<PathBuf> {
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create config dir {}", parent.display()))?;
            }
            let json = default_config_for(
                crate::preflight::on_path("claude", path_var),
                crate::preflight::on_path("codex", path_var),
            );
            std::fs::write(path, json)
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
    /// Whole-task redesign budget. Deliberately independent of (and higher
    /// than) the per-builder max_attempts: a redesign re-plans the entire task,
    /// and capping it at builder granularity fails tasks prematurely.
    pub fn max_redesigns(&self) -> u32 {
        self.caps.max_redesigns.unwrap_or(6)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_redesigns_defaults_higher_than_max_attempts() {
        // The redesign cap used to reuse max_attempts (3): a few flaky gate runs
        // exhausted it, failed the task, and the manager splintered it into ever
        // smaller fragments. Redesigns are whole-task do-overs and need more room.
        let cfg = Config::default();
        assert_eq!(cfg.max_attempts(), 3);
        assert_eq!(cfg.max_redesigns(), 6);
    }

    #[test]
    fn max_redesigns_is_configurable() {
        let cfg: Config = serde_json::from_str(r#"{"caps":{"max_redesigns":9}}"#).unwrap();
        assert_eq!(cfg.max_redesigns(), 9);
    }

    #[test]
    fn default_config_json_includes_max_redesigns() {
        let cfg: Config = serde_json::from_str(DEFAULT_CONFIG_JSON).unwrap();
        assert_eq!(cfg.max_redesigns(), 6);
    }

    const ROLES: [&str; 5] = ["manager", "architect", "builder", "customer", "resolver"];

    #[test]
    fn default_config_routes_by_available_tools() {
        // Both CLIs: strong-reasoning roles on claude, builders on codex.
        let both: Config = serde_json::from_str(default_config_for(true, true)).unwrap();
        assert_eq!(
            both.role_field("manager", "tool").as_deref(),
            Some("claude")
        );
        assert_eq!(both.role_field("builder", "tool").as_deref(), Some("codex"));

        // Single-CLI environments route every role to the installed tool —
        // a default config must never point at a CLI the user doesn't have.
        let claude: Config = serde_json::from_str(default_config_for(true, false)).unwrap();
        let codex: Config = serde_json::from_str(default_config_for(false, true)).unwrap();
        for role in ROLES {
            assert_eq!(claude.role_field(role, "tool").as_deref(), Some("claude"));
            assert_eq!(codex.role_field(role, "tool").as_deref(), Some("codex"));
        }

        // Neither installed: fall back to the both-tools template so preflight
        // fails fast naming what to install.
        assert_eq!(default_config_for(false, false), DEFAULT_CONFIG_JSON);
    }

    #[test]
    fn no_default_config_uses_the_nonexistent_gpt5_model() {
        // "gpt-5" is not a real codex model (the current one is gpt-5.5); a
        // default that names it makes every codex spawn fail out of the box.
        for json in [
            default_config_for(true, true),
            default_config_for(true, false),
            default_config_for(false, true),
            default_config_for(false, false),
        ] {
            assert!(
                !json.contains(r#""gpt-5""#),
                "stale gpt-5 model in:\n{json}"
            );
        }
    }

    #[test]
    fn all_default_configs_share_the_same_caps_and_roles() {
        let variants = [
            default_config_for(true, true),
            default_config_for(true, false),
            default_config_for(false, true),
        ];
        for json in variants {
            let cfg: Config = serde_json::from_str(json).unwrap();
            assert_eq!(cfg.max_iterations(), 25);
            assert_eq!(cfg.max_parallel(), 3);
            assert_eq!(cfg.item_timeout_sec(), 1200);
            assert_eq!(cfg.total_budget_sec(), 21600);
            assert_eq!(cfg.max_attempts(), 3);
            assert_eq!(cfg.max_redesigns(), 6);
            for role in ROLES {
                assert!(cfg.role_field(role, "tool").is_some(), "missing {role}");
                assert!(
                    cfg.role_field(role, "model").is_some(),
                    "missing {role} model"
                );
                assert!(
                    cfg.role_field(role, "effort").is_some(),
                    "missing {role} effort"
                );
            }
        }
    }

    #[test]
    fn ensure_default_config_detects_installed_tools() {
        let dir = std::env::temp_dir().join(format!(
            "cfg-detect-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // A fake PATH containing only a codex executable.
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("codex"), "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(bin.join("codex"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }

        let path = dir.join("config.json");
        Config::ensure_default_config_with_path(&path, bin.to_str().unwrap()).unwrap();

        let cfg = Config::load(&path).unwrap();
        for role in ROLES {
            assert_eq!(
                cfg.role_field(role, "tool").as_deref(),
                Some("codex"),
                "codex-only env must route {role} to codex"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
