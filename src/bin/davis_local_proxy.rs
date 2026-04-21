#[tokio::main]
async fn main() -> anyhow::Result<()> {
    davis_zero_claw::run_local_proxy().await
}
