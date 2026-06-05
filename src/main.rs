// agentloop relies on POSIX process groups and signals (nix, command-group)
// throughout the spawn/kill paths; without this guard a Windows build fails
// with a confusing pile of trait errors instead of one clear message.
#[cfg(not(unix))]
compile_error!("agentloop currently supports Unix (macOS/Linux) only");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    agentloop::cli::run().await
}
