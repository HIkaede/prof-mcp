use clap::Parser;

#[derive(Clone, Debug, Parser)]
#[command(
    name = "prof-mcp",
    about = "Register folded stack profiles or serve read-only MCP queries"
)]
pub struct Config {
    /// Folded profile to register. Without it prof-mcp serves MCP over stdio.
    #[arg(value_name = "PROFILE")]
    pub profile: Option<std::path::PathBuf>,
    /// Alias to register for PROFILE.
    #[arg(long, value_name = "ALIAS", requires = "profile")]
    pub name: Option<String>,
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
