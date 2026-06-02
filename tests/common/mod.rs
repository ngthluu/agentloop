use std::path::{Path, PathBuf};
use std::process::Command;

fn git(repo: &Path, args: &[&str]) {
    assert!(Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .status()
        .unwrap()
        .success());
}

pub fn init_ws_with_stub() -> PathBuf {
    let ws = std::env::temp_dir().join(format!(
        "alloop-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
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
    std::fs::write(st.join("goal.md"), "make one file").unwrap();
    std::fs::write(st.join("master.md"), "# status").unwrap();
    std::fs::write(st.join("backlog.json"), r#"{"items":[]}"#).unwrap();

    let stub = r##"#!/bin/bash
tool="$1"; shift
ws_state="$WS/.agentloop/state"; res="$WS/.agentloop/results"
prompt="$*"
case "$prompt" in
  *PLANNER*)
    n=$(cat "$WS/.plan_n" 2>/dev/null || echo 0); n=$((n+1)); echo "$n" > "$WS/.plan_n"
    if [ "$n" -eq 1 ]; then
      echo '{"items":[{"id":"it-1","title":"f","desc":"d","role":"build","deps":[],"status":"ready","attempts":0,"acceptance":"file exists"}]}' > "$ws_state/backlog.json"
      printf '#!/bin/bash\ntest -f "$PWD/made.txt"\n' > "$WS/.agentloop/verify.sh"; chmod +x "$WS/.agentloop/verify.sh"
    else
      if [ -f "$res/it-1.json" ]; then
        python3 -c "import json; p='$ws_state/backlog.json'; d=json.load(open(p)); [i.__setitem__('status','done') for i in d['items']]; json.dump(d,open(p,'w'))"
      fi
    fi
    echo "# updated" > "$ws_state/master.md"
    ;;
  *WORKER*)
    echo made > "$PWD/made.txt"; git add -A; git commit -qm "worker" 2>/dev/null
    echo '{"status":"done","summary":"made file","files_changed":["made.txt"]}' > "$res/it-1.json"
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
