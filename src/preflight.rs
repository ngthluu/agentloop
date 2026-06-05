use anyhow::{bail, Result};
use std::collections::BTreeMap;
use std::path::Path;

use crate::config::Config;

/// tool -> sorted roles that route to it, from the config
/// (e.g. {"claude": ["customer", "manager"], "codex": ["builder"]}).
pub fn required_tools(cfg: &Config) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (role, r) in &cfg.routing {
        if let Some(tool) = r.tool.as_deref().filter(|t| !t.is_empty()) {
            out.entry(tool.to_string()).or_default().push(role.clone());
        }
    }
    // BTreeMap iteration over routing is already sorted by role.
    out
}

fn is_executable(p: &Path) -> bool {
    let Ok(md) = std::fs::metadata(p) else {
        return false;
    };
    if !md.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        md.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Whether `bin` is an executable file on `path_var` (a PATH-style string).
pub fn on_path(bin: &str, path_var: &str) -> bool {
    std::env::split_paths(path_var).any(|dir| is_executable(&dir.join(bin)))
}

/// Fail fast when a tool the config routes roles to is not installed, naming the
/// missing tool, the roles that need it, and how to install it.
pub fn check_with_path(cfg: &Config, path_var: &str) -> Result<()> {
    let required = required_tools(cfg);
    if required.is_empty() {
        bail!(
            "config routes no roles to any agent tool; set routing.<role>.tool to \"claude\" or \"codex\""
        );
    }
    let missing: Vec<(&String, &Vec<String>)> = required
        .iter()
        .filter(|(tool, _)| !on_path(tool, path_var))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    let mut msg = String::from("missing required agent CLI(s):\n");
    for (tool, roles) in &missing {
        msg.push_str(&format!(
            "  - {tool} (used by roles: {})\n",
            roles.join(", ")
        ));
    }
    msg.push_str("install them (or change the config routing):\n");
    for (tool, _) in &missing {
        match tool.as_str() {
            "claude" => msg.push_str("  claude: npm install -g @anthropic-ai/claude-code\n"),
            "codex" => msg.push_str("  codex:  npm install -g @openai/codex\n"),
            other => msg.push_str(&format!(
                "  {other}: not a known tool — fix routing.<role>.tool in the config\n"
            )),
        }
    }
    bail!(msg);
}

/// Preflight against the real PATH. Skipped for FAKE_AGENT runs (offline tests).
pub fn check(cfg: &Config) -> Result<()> {
    if std::env::var("FAKE_AGENT").as_deref() == Ok("1") {
        return Ok(());
    }
    check_with_path(cfg, &std::env::var("PATH").unwrap_or_default())
}
