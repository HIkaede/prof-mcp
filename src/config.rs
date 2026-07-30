use std::path::PathBuf;

use clap::Parser;

#[derive(Clone, Debug, Parser)]
#[command(
    name = "rmdb-prof-mcp",
    about = "Read-only folded stack profile MCP server"
)]
pub struct Config {
    /// One or more roots that profile paths must stay within.
    #[arg(long, required = true, value_name = "PATH")]
    pub root: Vec<PathBuf>,
    /// Maximum accepted profile file size in MiB.
    #[arg(long, default_value_t = 512)]
    pub max_file_size_mib: u64,
    /// Number of parsed profiles retained by the transparent LRU cache.
    #[arg(long, default_value_t = 8)]
    pub cache_capacity: usize,
    /// tracing filter; RUST_LOG may also be used by tracing-subscriber.
    #[arg(long, default_value = "warn")]
    pub log_level: String,
}

impl Config {
    pub fn max_file_size_bytes(&self) -> u64 {
        self.max_file_size_mib.saturating_mul(1024 * 1024)
    }
}
