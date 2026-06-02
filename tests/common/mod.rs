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

/// Planner: round 1 seeds it-1 + a verify.sh that requires it-1.txt (and it-2.txt when
/// WANT2 is set). Later rounds mark done items done AND, if a pending request exists,
/// add it-2 (ready). Worker: makes <id>.txt + commits + writes result.
#[allow(dead_code)]
pub fn init_ws_with_request_stub() -> PathBuf {
    let ws = init_ws_with_stub();
    let stub = r##"#!/bin/bash
tool="$1"; shift
ws="$WS"; ws_state="$ws/.agentloop/state"; res="$ws/.agentloop/results"
prompt="$*"
case "$prompt" in
  *PLANNER*)
    python3 - "$ws" <<'PY'
import json,sys,os
ws=sys.argv[1]; st=os.path.join(ws,'.agentloop','state'); res=os.path.join(ws,'.agentloop','results')
bkp=os.path.join(st,'backlog.json'); d=json.load(open(bkp))
def ensure(idn,acc):
    if not any(i['id']==idn for i in d['items']):
        d['items'].append({"id":idn,"title":idn,"desc":"d","role":"build","deps":[],"status":"ready","attempts":0,"acceptance":acc})
if not d['items']:
    ensure('it-1','it-1 file')
    open(os.path.join(ws,'.agentloop','verify.sh'),'w').write('#!/bin/bash\n[ -f "$PWD/it-1.txt" ] && { [ -z "$WANT2" ] || [ -f "$PWD/it-2.txt" ]; }\n')
    os.chmod(os.path.join(ws,'.agentloop','verify.sh'),0o755)
for i in d['items']:
    if os.path.exists(os.path.join(res,i['id']+'.json')): i['status']='done'
reqp=os.path.join(st,'requests.jsonl')
pend=[l for l in open(reqp).read().splitlines() if l.strip() and json.loads(l)['status']=='pending'] if os.path.exists(reqp) else []
if pend:
    ensure('it-2','it-2 file')
json.dump(d,open(bkp,'w'))
open(os.path.join(st,'master.md'),'w').write('# updated')
PY
    ;;
  *WORKER*)
    id=$(echo "$prompt" | sed -n 's/.*id:    \([a-z0-9-]*\).*/\1/p' | head -1)
    [ -z "$id" ] && id=it-1
    echo made > "$PWD/$id.txt"; git add -A; git commit -qm "worker $id" 2>/dev/null
    echo "{\"status\":\"done\",\"summary\":\"made $id\",\"files_changed\":[\"$id.txt\"]}" > "$res/$id.json"
    ;;
esac
exit 0
"##;
    std::fs::write(ws.join("stub.sh"), stub).unwrap();
    #[cfg(unix)]
    { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(ws.join("stub.sh"), std::fs::Permissions::from_mode(0o755)).unwrap(); }
    std::fs::write(ws.join(".agentloop/state/goal.md"), "make it-1").unwrap();
    ws
}

/// Stub that, as worker, asks a question the FIRST time (needs_input) and completes
/// the SECOND time (after an answer file exists). Planner seeds one item; marks done
/// when a done result is present.
#[allow(dead_code)]
pub fn init_ws_with_asking_stub() -> PathBuf {
    let ws = init_ws_with_stub();
    let stub = r##"#!/bin/bash
tool="$1"; shift
ws="$WS"; ws_state="$ws/.agentloop/state"; res="$ws/.agentloop/results"
prompt="$*"
case "$prompt" in
  *PLANNER*)
    n=$(cat "$ws/.plan_n" 2>/dev/null || echo 0); n=$((n+1)); echo "$n" > "$ws/.plan_n"
    if [ "$n" -eq 1 ]; then
      echo '{"items":[{"id":"it-1","title":"f","desc":"d","role":"build","deps":[],"status":"ready","attempts":0,"acceptance":"file exists"}]}' > "$ws_state/backlog.json"
      printf '#!/bin/bash\ntest -f "$PWD/made.txt"\n' > "$ws/.agentloop/verify.sh"; chmod +x "$ws/.agentloop/verify.sh"
    elif [ -f "$res/it-1.json" ]; then
      python3 -c "import json; p='$ws_state/backlog.json'; d=json.load(open(p)); [i.__setitem__('status','done') for i in d['items']]; json.dump(d,open(p,'w'))"
    fi
    echo "# updated" > "$ws_state/master.md"
    ;;
  *WORKER*)
    if [ -f "$ws/.agentloop/answers/it-1.json" ]; then
      echo made > "$PWD/made.txt"; git add -A; git commit -qm "worker" 2>/dev/null
      echo '{"status":"done","summary":"made file","files_changed":["made.txt"]}' > "$res/it-1.json"
    else
      mkdir -p "$ws/.agentloop/questions"
      echo '{"question":"make the file?","context":"need confirm"}' > "$ws/.agentloop/questions/it-1.json"
      echo '{"status":"needs_input","summary":"confirm"}' > "$res/it-1.json"
    fi
    ;;
esac
exit 0
"##;
    std::fs::write(ws.join("stub.sh"), stub).unwrap();
    #[cfg(unix)]
    { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(ws.join("stub.sh"), std::fs::Permissions::from_mode(0o755)).unwrap(); }
    ws
}
