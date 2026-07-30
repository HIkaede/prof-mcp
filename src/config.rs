use clap::{Parser, Subcommand};

#[derive(Clone, Debug, Parser)]
#[command(
    name = "prof-mcp",
    version,
    about = "Install, register, and query folded stack profiles"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
    /// Backward-compatible shorthand for `prof-mcp register PROFILE`.
    #[arg(value_name = "PROFILE")]
    pub profile: Option<std::path::PathBuf>,
    /// Alias for the backward-compatible PROFILE form.
    #[arg(long, value_name = "ALIAS", requires = "profile")]
    pub name: Option<String>,
    /// Maximum accepted profile file size in MiB.
    #[arg(long, default_value_t = 512, global = true)]
    pub max_file_size_mib: u64,
    /// Number of parsed profiles retained by the transparent LRU cache.
    #[arg(long, default_value_t = 8, global = true)]
    pub cache_capacity: usize,
    /// tracing filter; RUST_LOG may also be used by tracing-subscriber.
    #[arg(long, default_value = "warn", global = true)]
    pub log_level: String,
}

#[derive(Clone, Debug, Subcommand)]
pub enum Command {
    /// Install prof-mcp into Codex and update global AGENTS.md guidance.
    Setup {
        /// Print the intended changes without writing them.
        #[arg(long)]
        dry_run: bool,
    },
    /// Start the stdio MCP server.
    Serve {
        /// Use the MCP stdio transport.
        #[arg(long)]
        mcp: bool,
    },
    /// Validate and register one folded profile in the current workspace.
    Register {
        #[arg(value_name = "PROFILE")]
        profile: std::path::PathBuf,
        #[arg(long, value_name = "ALIAS")]
        name: Option<String>,
    },
    /// List the registry root, active alias, and registered profiles.
    List,
    /// Select an existing alias as the active profile.
    Use {
        #[arg(value_name = "ALIAS")]
        alias: String,
    },
    /// Remove unreferenced folded blobs from the nearest workspace registry.
    Gc {
        /// Print the deletion plan without removing files.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Clone, Debug)]
pub struct Config {
    pub profile: Option<std::path::PathBuf>,
    pub name: Option<String>,
    pub max_file_size_mib: u64,
    pub cache_capacity: usize,
    pub log_level: String,
}

impl From<&Cli> for Config {
    fn from(cli: &Cli) -> Self {
        Self {
            profile: None,
            name: None,
            max_file_size_mib: cli.max_file_size_mib,
            cache_capacity: cli.cache_capacity,
            log_level: cli.log_level.clone(),
        }
    }
}

impl Config {
    pub fn max_file_size_bytes(&self) -> u64 {
        self.max_file_size_mib.saturating_mul(1024 * 1024)
    }
}
