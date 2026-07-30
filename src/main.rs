use anyhow::Result;
use clap::Parser;
use prof_mcp::{config::Config, registry, server::run_stdio};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::parse();
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.log_level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    if let Some(profile) = config.profile.clone() {
        let registration = registry::register(
            &std::env::current_dir()?,
            &profile,
            config.name.as_deref(),
            config.max_file_size_bytes(),
        )
        .map_err(anyhow::Error::msg)?;
        println!(
            "registered alias={} fingerprint={} bytes={}",
            registration.alias, registration.fingerprint, registration.byte_len
        );
        return Ok(());
    }
    run_stdio(config).await
}
