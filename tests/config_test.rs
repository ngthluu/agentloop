use agentloop::config::Config;
use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};

static CFG_CTR: AtomicU32 = AtomicU32::new(0);

fn temp_path(name: &str) -> std::path::PathBuf {
    let n = CFG_CTR.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir()
        .join(format!("alcfg-{}-{}", std::process::id(), n))
        .join(name)
}

fn write_cfg(name: &str, body: &str) -> std::path::PathBuf {
    let p = temp_path(name);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    p
}

const SAMPLE_JSON: &str = r#"
{
  "caps": {
    "max_iterations": 7,
    "max_parallel": 2,
    "item_timeout_sec": 30,
    "total_budget_sec": 300,
    "max_attempts": 3
  },
  "routing": {
    "manager": { "tool": "claude", "model": "opus", "effort": "high" },
    "builder": { "tool": "codex", "model": "gpt-5", "effort": "high" }
  },
  "defaults": { "role": "builder" }
}
"#;

#[test]
fn loads_json_and_resolves_roles() {
    let path = write_cfg("config.json", SAMPLE_JSON);
    let cfg = Config::load(&path).unwrap();

    assert_eq!(cfg.resolve_role("manager").as_deref(), Some("manager"));
    assert_eq!(cfg.resolve_role("nonexistent").as_deref(), Some("builder"));
    assert_eq!(cfg.role_field("manager", "tool").as_deref(), Some("claude"));
    assert_eq!(cfg.role_field("manager", "model").as_deref(), Some("opus"));
    assert_eq!(cfg.role_field("manager", "flags"), None);
    assert_eq!(cfg.max_iterations(), 7);
    assert_eq!(cfg.max_parallel(), 2);
    assert_eq!(cfg.max_attempts(), 3);
}

#[test]
fn yaml_config_fails_with_migration_message() {
    let path = write_cfg(
        "config.yaml",
        "routing:\n  builder: { tool: codex, model: gpt-5, effort: high }\ndefaults: { role: builder }\n",
    );

    let err = Config::load(&path).unwrap_err().to_string();
    assert!(
        err.contains("config must be JSON; migrate config.yaml to config.json"),
        "unexpected error: {err}"
    );
}

#[test]
fn ensure_default_creates_global_json() {
    let path = temp_path("nested/agentloop/config.json");
    Config::ensure_default_config(&path).unwrap();

    assert!(path.exists());
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.trim_start().starts_with('{'), "default config is JSON");
    let cfg = Config::load(&path).unwrap();
    assert_eq!(cfg.resolve_role("missing").as_deref(), Some("builder"));
    assert_eq!(cfg.role_field("builder", "tool").as_deref(), Some("codex"));
    assert_eq!(cfg.role_field("builder", "flags"), None);
}

#[test]
fn caps_default_when_absent() {
    let path = write_cfg("config.json", r#"{ "routing": {}, "defaults": {} }"#);
    let cfg = Config::load(&path).unwrap();

    assert_eq!(cfg.max_iterations(), 25);
    assert_eq!(cfg.item_timeout_sec(), 1200);
    assert_eq!(cfg.resolve_role("anything"), None);
}
