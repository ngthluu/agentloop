mod common;
use agentloop::config::Config;
use agentloop::events::{EventLineReporter, Reporter};
use agentloop::orchestrator;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

fn git(repo: &Path, args: &[&str]) {
    assert!(Command::new("git").arg("-C").arg(repo).args(args).status().unwrap().success());
}

/// Workspace whose two items both write `shared.txt`, forcing a conflict on the second
/// merge; a RESOLVER prompt resolves it by committing the in-progress merge.
fn init_ws_conflict_stub() -> PathBuf {
    let ws = std::env::temp_dir().join(format!(
        "alres-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    ));
    let st = ws.join(".agentloop/state");
    std::fs::create_dir_all(&st).unwrap();
    std::fs::create_dir_all(ws.join(".agentloop/results")).unwrap();
    std::fs::create_dir_all(ws.join(".agentloop/logs")).unwrap();
    git(&ws, &["init", "-q"]);
    git(&ws, &["config", "user.email", "t@t"]);
    git(&ws, &["config", "user.name", "t"]);
    std::fs::write(ws.join("seed.txt"), "seed").unwrap();
    git(&ws, &["add", "-A"]);
    git(&ws, &["commit", "-qm", "init"]);
    std::fs::write(st.join("goal.md"), "make shared").unwrap();
    std::fs::write(st.join("master.md"), "# status").unwrap();
    std::fs::write(st.join("backlog.json"), r#"{"items":[]}"#).unwrap();

    let stub = r##"#!/bin/bash
tool="$1"; shift
ws="$WS"; st="$ws/.agentloop/state"; res="$ws/.agentloop/results"
prompt="$*"
case "$prompt" in
  *RESOLVER*)
    echo resolved > "$PWD/shared.txt"
    git add shared.txt
    git commit --no-edit -q
    ;;
  *PLANNER*)
    n=$(cat "$ws/.pn" 2>/dev/null || echo 0); n=$((n+1)); echo "$n" > "$ws/.pn"
    if [ "$n" -eq 1 ]; then
      echo '{"items":[{"id":"it-1","title":"a","desc":"d","role":"build","deps":[],"status":"ready","attempts":0,"acceptance":"ok"},{"id":"it-2","title":"b","desc":"d","role":"build","deps":[],"status":"ready","attempts":0,"acceptance":"ok"}]}' > "$st/backlog.json"
      printf '#!/bin/bash\ntest -f "$PWD/shared.txt"\n' > "$ws/.agentloop/verify.sh"; chmod +x "$ws/.agentloop/verify.sh"
    fi
    echo "# m" > "$st/master.md"
    ;;
  *WORKER*)
    id=$(echo "$prompt" | grep -oE 'it-[0-9]+' | head -1)
    echo "$id" > "$PWD/shared.txt"
    git add -A; git commit -qm "w $id" >/dev/null 2>&1
    echo "{\"status\":\"done\",\"summary\":\"s\",\"files_changed\":[\"shared.txt\"]}" > "$res/$id.json"
    ;;
esac
exit 0
"##;
    let stub_path = ws.join("stub.sh");
    std::fs::write(&stub_path, stub).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&stub_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    ws
}

#[tokio::test]
async fn merge_conflict_is_resolved_by_an_agent_not_bounced() {
    let ws = init_ws_conflict_stub();

    let cfg: Config = serde_yaml::from_str(
        r#"
caps: { max_iterations: 6, max_parallel: 2, item_timeout_sec: 30, total_budget_sec: 300, max_attempts: 3 }
routing:
  planner:  { tool: claude, model: opus,   effort: high,   flags: "" }
  build:    { tool: codex,  model: gpt-5,  effort: high,   flags: "" }
  resolver: { tool: claude, model: sonnet, effort: medium, flags: "" }
defaults: { role: build }
"#,
    )
    .unwrap();

    std::env::set_var("FAKE_AGENT", "1");
    std::env::set_var("FAKE_AGENT_BIN", ws.join("stub.sh"));
    std::env::set_var("WS", &ws);

    let reporter: Arc<dyn Reporter> = Arc::new(EventLineReporter);
    let rc = orchestrator::run(&cfg, &ws, reporter).await.unwrap();

    assert_eq!(rc, 0, "loop reaches DONE after resolving the conflict");
    assert!(ws.join("shared.txt").exists(), "shared.txt is present on main");
    assert_eq!(
        agentloop::state::open_count(&ws.join(".agentloop/state/backlog.json")).unwrap(),
        0,
        "no open items remain"
    );
    assert_eq!(
        std::fs::read_to_string(ws.join("shared.txt")).unwrap().trim(),
        "resolved",
        "the resolver's resolution landed on main (not a clean retry)"
    );

    std::env::remove_var("FAKE_AGENT");
    std::env::remove_var("FAKE_AGENT_BIN");
    std::env::remove_var("WS");
    let _ = std::fs::remove_dir_all(&ws);
}
