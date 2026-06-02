use agentloop::worker::resolver_prompt;
use serde_json::json;

#[test]
fn resolver_prompt_mentions_branch_title_and_commit() {
    let ws = std::path::Path::new("/tmp/ws");
    let item = json!({"id":"it-7","title":"add auth","desc":"wire login","role":"build"});
    let p = resolver_prompt(ws, &item);

    assert!(p.contains("RESOLVER"), "identifies the role");
    assert!(p.contains("item/it-7"), "names the branch");
    assert!(p.contains("add auth"), "includes the item title");
    assert!(p.contains("commit"), "instructs to commit the merge");
}
