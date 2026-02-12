use tracing_subscriber;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    tracing::info!("fractalaw v{}", env!("CARGO_PKG_VERSION"));
    Ok(())
}
