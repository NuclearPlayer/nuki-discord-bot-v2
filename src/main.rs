#[tokio::main]
async fn main() -> anyhow::Result<()> {
    nuki::run().await
}
