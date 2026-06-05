use agentloop::config::Config;
use agentloop::preflight;

fn cfg_with(routing: &str) -> Config {
    serde_json::from_str(&format!(
        r#"{{"routing": {routing}, "defaults": {{"role":"builder"}}}}"#
    ))
    .unwrap()
}

#[test]
fn required_tools_maps_each_tool_to_its_roles() {
    let cfg = cfg_with(
        r#"{"manager":{"tool":"claude","model":"haiku"},"builder":{"tool":"codex"},"customer":{"tool":"claude"}}"#,
    );
    let req = preflight::required_tools(&cfg);
    assert_eq!(
        req.get("claude").unwrap(),
        &vec!["customer".to_string(), "manager".to_string()]
    );
    assert_eq!(req.get("codex").unwrap(), &vec!["builder".to_string()]);
}

#[test]
fn check_fails_when_a_configured_tool_is_not_installed() {
    let cfg = cfg_with(r#"{"manager":{"tool":"claude","model":"haiku"}}"#);
    let err = preflight::check_with_path(&cfg, "/nonexistent-dir").unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("claude"), "names the missing tool: {msg}");
    assert!(
        msg.contains("manager"),
        "names the roles that need it: {msg}"
    );
    assert!(msg.contains("install"), "tells the user to install: {msg}");
}

#[test]
fn check_passes_when_tools_are_executable_on_path() {
    let dir = std::env::temp_dir().join(format!(
        "preflight-bin-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    for bin in ["claude", "codex"] {
        let p = dir.join(bin);
        std::fs::write(&p, "#!/bin/bash\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    let cfg = cfg_with(r#"{"manager":{"tool":"claude"},"builder":{"tool":"codex"}}"#);
    preflight::check_with_path(&cfg, dir.to_str().unwrap()).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn empty_routing_is_an_error() {
    let cfg = cfg_with("{}");
    let err = preflight::check_with_path(&cfg, "/usr/bin").unwrap_err();
    assert!(format!("{err:#}").contains("routes no roles"));
}
