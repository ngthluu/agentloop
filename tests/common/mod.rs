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
  *MANAGER*)
    echo '{"items":[{"id":"task-1","title":"f","desc":"d","deps":[],"status":"ready","attempts":0,"acceptance":"file exists"}]}' > "$ws_state/backlog.json"
    printf '#!/bin/bash\ntest -f "$PWD/made.txt"\n' > "$WS/.agentloop/verify.sh"; chmod +x "$WS/.agentloop/verify.sh"
    echo "# updated" > "$ws_state/master.md"
    ;;
  *ARCHITECT*)
    mkdir -p "$ws_state/tasks/task-1"
    echo "Make the file." > "$ws_state/tasks/task-1/design.md"
    echo '{"items":[{"id":"task-1-b1","title":"make file","desc":"write made.txt","deps":[],"status":"ready","attempts":0,"acceptance":"made.txt exists"}]}' > "$ws_state/tasks/task-1/builders.json"
    ;;
  *BUILDER*)
    echo made > "$PWD/made.txt"; git add -A; git commit -qm "worker" 2>/dev/null
    echo '{"status":"done","summary":"made file","files_changed":["made.txt"]}' > "$res/task-1-b1.json"
    ;;
  *"SILLY CUSTOMER"*)
    mkdir -p "$ws_state/tasks/task-1"
    echo '{"status":"approved","summary":"accepted","acceptance_notes":"made.txt exists"}' > "$ws_state/tasks/task-1/customer.json"
    echo '{"status":"approved","summary":"accepted"}' > "$res/task-1-customer.json"
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

/// Manager: seeds task-1 + a verify.sh that requires task-1.txt (and task-2.txt when
/// WANT2 is set). If a pending request exists, add task-2 (ready). Architect writes
/// one builder per task. Builder makes <task-id>.txt + commits + writes result.
#[allow(dead_code)]
pub fn init_ws_with_request_stub() -> PathBuf {
    let ws = init_ws_with_stub();
    let stub = r##"#!/bin/bash
tool="$1"; shift
ws="$WS"; ws_state="$ws/.agentloop/state"; res="$ws/.agentloop/results"
prompt="$*"
case "$prompt" in
  *MANAGER*)
    python3 - "$ws" <<'PY'
import json,sys,os
ws=sys.argv[1]; st=os.path.join(ws,'.agentloop','state'); res=os.path.join(ws,'.agentloop','results')
bkp=os.path.join(st,'backlog.json'); d=json.load(open(bkp))
def ensure(idn,acc):
    if not any(i['id']==idn for i in d['items']):
        d['items'].append({"id":idn,"title":idn,"desc":"d","deps":[],"status":"ready","attempts":0,"acceptance":acc})
if not d['items']:
    ensure('task-1','task-1 file')
    open(os.path.join(ws,'.agentloop','verify.sh'),'w').write('#!/bin/bash\n[ -f "$PWD/task-1.txt" ] && { [ -z "$WANT2" ] || [ -f "$PWD/task-2.txt" ]; }\n')
    os.chmod(os.path.join(ws,'.agentloop','verify.sh'),0o755)
reqp=os.path.join(st,'requests.jsonl')
pend=[l for l in open(reqp).read().splitlines() if l.strip() and json.loads(l)['status']=='pending'] if os.path.exists(reqp) else []
if pend:
    ensure('task-2','task-2 file')
json.dump(d,open(bkp,'w'))
open(os.path.join(st,'master.md'),'w').write('# updated')
PY
    ;;
  *ARCHITECT*)
    task=$(echo "$prompt" | grep -oE 'id: task-[0-9]+' | head -1 | awk '{print $2}')
    [ -z "$task" ] && task=task-1
    mkdir -p "$ws_state/tasks/$task"
    echo "Make $task file." > "$ws_state/tasks/$task/design.md"
    echo "{\"items\":[{\"id\":\"$task-b1\",\"title\":\"make $task\",\"desc\":\"write $task.txt\",\"deps\":[],\"status\":\"ready\",\"attempts\":0,\"acceptance\":\"$task.txt exists\"}]}" > "$ws_state/tasks/$task/builders.json"
    ;;
  *BUILDER*)
    bid=$(echo "$prompt" | sed -n 's/.*id:    \([a-z0-9-]*\).*/\1/p' | head -1)
    task=${bid%-b1}
    [ -z "$task" ] && task=task-1
    echo made > "$PWD/$task.txt"; git add -A; git commit -qm "builder $bid" 2>/dev/null
    echo "{\"status\":\"done\",\"summary\":\"made $task\",\"files_changed\":[\"$task.txt\"]}" > "$res/$bid.json"
    ;;
  *"SILLY CUSTOMER"*)
    task=$(echo "$prompt" | grep -oE 'id: task-[0-9]+' | head -1 | awk '{print $2}')
    [ -z "$task" ] && task=task-1
    mkdir -p "$ws_state/tasks/$task"
    echo "{\"status\":\"approved\",\"summary\":\"accepted $task\",\"acceptance_notes\":\"ok\"}" > "$ws_state/tasks/$task/customer.json"
    echo "{\"status\":\"approved\",\"summary\":\"accepted $task\"}" > "$res/$task-customer.json"
    ;;
esac
exit 0
"##;
    std::fs::write(ws.join("stub.sh"), stub).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(ws.join("stub.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    }
    std::fs::write(ws.join(".agentloop/state/goal.md"), "make task-1").unwrap();
    ws
}

/// Stub that, as builder, asks a question the FIRST time (needs_input) and completes
/// the SECOND time (after an answer file exists). Manager seeds one business task.
#[allow(dead_code)]
pub fn init_ws_with_asking_stub() -> PathBuf {
    let ws = init_ws_with_stub();
    let stub = r##"#!/bin/bash
tool="$1"; shift
ws="$WS"; ws_state="$ws/.agentloop/state"; res="$ws/.agentloop/results"
prompt="$*"
case "$prompt" in
  *MANAGER*)
    echo '{"items":[{"id":"task-1","title":"f","desc":"d","deps":[],"status":"ready","attempts":0,"acceptance":"file exists"}]}' > "$ws_state/backlog.json"
    printf '#!/bin/bash\ntest -f "$PWD/made.txt"\n' > "$ws/.agentloop/verify.sh"; chmod +x "$ws/.agentloop/verify.sh"
    echo "# updated" > "$ws_state/master.md"
    ;;
  *ARCHITECT*)
    mkdir -p "$ws_state/tasks/task-1"
    echo "Make the file." > "$ws_state/tasks/task-1/design.md"
    echo '{"items":[{"id":"task-1-b1","title":"make file","desc":"write made.txt","deps":[],"status":"ready","attempts":0,"acceptance":"made.txt exists"}]}' > "$ws_state/tasks/task-1/builders.json"
    ;;
  *BUILDER*)
    if [ -f "$ws/.agentloop/answers/task-1-b1.json" ]; then
      echo made > "$PWD/made.txt"; git add -A; git commit -qm "worker" 2>/dev/null
      echo '{"status":"done","summary":"made file","files_changed":["made.txt"]}' > "$res/task-1-b1.json"
    else
      mkdir -p "$ws/.agentloop/questions"
      echo '{"question":"make the file?","context":"need confirm"}' > "$ws/.agentloop/questions/task-1-b1.json"
      echo '{"status":"needs_input","summary":"confirm"}' > "$res/task-1-b1.json"
    fi
    ;;
  *"SILLY CUSTOMER"*)
    mkdir -p "$ws_state/tasks/task-1"
    echo '{"status":"approved","summary":"accepted","acceptance_notes":"made.txt exists"}' > "$ws_state/tasks/task-1/customer.json"
    echo '{"status":"approved","summary":"accepted"}' > "$res/task-1-customer.json"
    ;;
esac
exit 0
"##;
    std::fs::write(ws.join("stub.sh"), stub).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(ws.join("stub.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    }
    ws
}
