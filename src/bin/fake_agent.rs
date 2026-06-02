// Stand-in for claude/codex when FAKE_AGENT=1. Echoes its argv so tests can
// assert command construction. Honors FAKE_SLEEP (secs) and FAKE_EXIT (code).
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    println!("FAKE_ARGS: {}", args.join(" "));
    if let Ok(s) = std::env::var("FAKE_SLEEP") {
        if let Ok(secs) = s.parse::<u64>() {
            std::thread::sleep(std::time::Duration::from_secs(secs));
        }
    }
    let code = std::env::var("FAKE_EXIT").ok().and_then(|c| c.parse::<i32>().ok()).unwrap_or(0);
    std::process::exit(code);
}
