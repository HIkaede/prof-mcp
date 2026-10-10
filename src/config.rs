use clap::{Parser, Subcommand};

#[derive(Clone, Debug, Parser)]
#[command(
    name = "prof-mcp",
    version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("PROF_MCP_GIT_HASH"), ")"),
    about = "Install, register, and query folded stack profiles"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
    /// Shorthand for `prof-mcp register PROFILE`.
    #[arg(value_name = "PROFILE")]
    pub profile: Option<std::path::PathBuf>,
    /// Alias for the PROFILE shorthand.
    #[arg(long, value_name = "ALIAS", requires = "profile")]
    pub name: Option<String>,
    /// Maximum accepted profile file size in MiB.
    #[arg(long, default_value_t = 512, global = true)]
    pub max_file_size_mib: u64,
}

#[derive(Clone, Debug, Subcommand)]
pub enum Command {
    /// Register the prof-mcp MCP server in Codex.
    Setup {
        /// Print the intended changes without writing them.
        #[arg(long)]
        dry_run: bool,
    },
    /// Start the stdio MCP server.
    Serve {
        /// Maximum estimated memory (MiB) retained by the profile cache; 0 disables caching.
        #[arg(long, default_value_t = 512)]
        max_cache_mib: u64,
    },
    /// Validate and register one folded profile in the current workspace.
    Register {
        /// Folded profile path, or - to read stdin.
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
    /// Remove one registered alias without deleting its profile blob.
    Remove {
        #[arg(value_name = "ALIAS")]
        alias: String,
        /// Replacement active alias when removing the current active alias.
        #[arg(long, value_name = "ALIAS")]
        new_active: Option<String>,
    },
    /// Run system perf, fold its output, and register the result.
    Capture {
        #[arg(long, value_name = "ALIAS")]
        name: Option<String>,
        /// Command to profile, after `--`.
        #[arg(required = true, last = true, value_name = "COMMAND")]
        command: Vec<std::ffi::OsString>,
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
    pub max_file_size_mib: u64,
    pub max_cache_mib: u64,
}

impl From<&Cli> for Config {
    fn from(cli: &Cli) -> Self {
        let max_cache_mib = match cli.command.as_ref() {
            Some(Command::Serve { max_cache_mib }) => *max_cache_mib,
            _ => 512,
        };
        Self {
            max_file_size_mib: cli.max_file_size_mib,
            max_cache_mib,
        }
    }
}

impl Config {
    pub fn max_file_size_bytes(&self) -> u64 {
        self.max_file_size_mib.saturating_mul(1024 * 1024)
    }

    pub fn max_cache_bytes(&self) -> usize {
        usize::try_from(self.max_cache_mib.saturating_mul(1024 * 1024)).unwrap_or(usize::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::Parser;

    #[test]
    fn serve_cache_budget_defaults_and_accepts_zero() {
        for (args, expected) in [
            (vec!["prof-mcp", "serve"], 512),
            (vec!["prof-mcp", "serve", "--max-cache-mib", "0"], 0),
            (vec!["prof-mcp", "serve", "--max-cache-mib", "64"], 64),
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            assert_eq!(super::Config::from(&cli).max_cache_mib, expected);
        }
    }

    #[test]
    fn cache_byte_conversion_saturates() {
        let cli = Cli::try_parse_from([
            "prof-mcp",
            "serve",
            "--max-cache-mib",
            &u64::MAX.to_string(),
        ])
        .unwrap();
        assert_eq!(super::Config::from(&cli).max_cache_bytes(), usize::MAX);
    }
}
