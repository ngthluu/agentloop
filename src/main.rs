#[tokio::main]
async fn main() -> anyhow::Result<()> {
    agentloop::cli::run().await
}
