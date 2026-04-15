#[tokio::main]
async fn main() -> anyhow::Result<()> {
    davis_zero_claw::cli::run().await
}
