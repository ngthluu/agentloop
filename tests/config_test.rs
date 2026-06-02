use agentloop::config::Config;
use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};

static CFG_CTR: AtomicU32 = AtomicU32::new(0);

fn write_cfg(body: &str) -> tempfile_path::TempCfg {
    let n = CFG_CTR.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("alcfg-{}-{}", std::process::id(), n));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("config.yaml");
    let mut f = std::fs::File::create(&p).unwrap();
    f.write_all(body.as_bytes()).unwrap();
    tempfile_path::TempCfg { path: p }
}

mod tempfile_path {
    pub struct TempCfg { pub path: std::path::PathBuf }
}

const SAMPLE: &str = r#"
caps: { max_iterations: 7, max_parallel: 2, item_timeout_sec: 30, total_budget_sec: 300, max_attempts: 3 }
routing:
  planner: { tool: claude, model: opus, effort: high, flags: "--dangerously-skip-permissions" }
  build:   { tool: codex,  model: gpt-5, effort: high, flags: "" }
defaults: { role: build }
"#;

#[test]
fn loads_and_resolves() {
    let c = write_cfg(SAMPLE);
    let cfg = Config::load(&c.path).unwrap();

    assert_eq!(cfg.resolve_role("planner").as_deref(), Some("planner"));
    assert_eq!(cfg.resolve_role("nonexistent").as_deref(), Some("build")); // -> defaults.role
    assert_eq!(cfg.role_field("planner", "tool").as_deref(), Some("claude"));
    assert_eq!(cfg.role_field("planner", "model").as_deref(), Some("opus"));
    assert_eq!(cfg.role_field("build", "flags"), None); // empty string -> None
    assert_eq!(cfg.max_iterations(), 7);
    assert_eq!(cfg.max_parallel(), 2);
    assert_eq!(cfg.max_attempts(), 3);
}

#[test]
fn caps_default_when_absent() {
    let c = write_cfg("routing: {}\ndefaults: {}\n");
    let cfg = Config::load(&c.path).unwrap();
    assert_eq!(cfg.max_iterations(), 25);
    assert_eq!(cfg.item_timeout_sec(), 1200);
    assert_eq!(cfg.resolve_role("anything"), None); // no defaults.role
}
