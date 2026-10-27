use anyhow::Result;
use clap::{CommandFactory, Parser};
use prof_mcp::{
    capture,
    config::{Cli, Command, Config},
    registry,
    server::run_stdio,
    setup,
};
use tracing_subscriber::EnvFilter;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::from(&cli);
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    match cli.command {
        Some(Command::Setup { dry_run }) => setup::run(dry_run),
        Some(Command::Serve { .. }) => run_stdio(config).await,
        Some(Command::Register { profile, name }) => register(&config, &profile, name.as_deref()),
        Some(Command::List) => {
            let status = registry::status(&std::env::current_dir()?).map_err(anyhow::Error::msg)?;
            emit(format_args!("registry={}", status.registry_root.display()))?;
            emit(format_args!("active={}", status.active))?;
            for profile in status.profiles {
                emit(format_args!(
                    "{}{} fingerprint={} bytes={} source={}",
                    if profile.alias == status.active {
                        "* "
                    } else {
                        "  "
                    },
                    profile.alias,
                    profile.fingerprint,
                    profile.byte_len,
                    // Hand-edited manifests can carry control bytes; never print them raw.
                    profile.source_name.escape_debug()
                ))?;
            }
            Ok(())
        }
        Some(Command::Use { alias }) => {
            let status = registry::set_active(&std::env::current_dir()?, &alias)
                .map_err(anyhow::Error::msg)?;
            emit(format_args!("active alias={}", status.active))?;
            Ok(())
        }
        Some(Command::Remove { alias, new_active }) => {
            let removal =
                registry::remove(&std::env::current_dir()?, &alias, new_active.as_deref())
                    .map_err(anyhow::Error::msg)?;
            emit(format_args!(
                "removed alias={} active={}",
                removal.alias, removal.active
            ))?;
            Ok(())
        }
        Some(Command::Capture { name, command }) => {
            let registration =
                capture::run(&config, name.as_deref(), &command).map_err(anyhow::Error::msg)?;
            print_registration(&registration)
        }
        Some(Command::Gc { dry_run }) => {
            let report =
                registry::gc(&std::env::current_dir()?, dry_run).map_err(anyhow::Error::msg)?;
            emit(format_args!("registry={}", report.registry_root.display()))?;
            emit(format_args!("dry_run={}", report.dry_run))?;
            for path in report.removed {
                emit(format_args!(
                    "{} {}",
                    if dry_run { "would_remove" } else { "removed" },
                    path
                ))?;
            }
            for path in report.skipped {
                warn(format_args!("skipped {path}"));
            }
            Ok(())
        }
        None => {
            Cli::command().print_help()?;
            emit(format_args!(""))?;
            Ok(())
        }
    }
}

fn register(config: &Config, profile: &std::path::Path, name: Option<&str>) -> Result<()> {
    let workspace = std::env::current_dir()?;
    let registration = if profile == std::path::Path::new("-") {
        registry::register_reader(
            &workspace,
            std::io::stdin().lock(),
            "stdin.folded",
            name,
            config.max_file_size_bytes(),
        )
    } else {
        registry::register(&workspace, profile, name, config.max_file_size_bytes())
    }
    .map_err(anyhow::Error::msg)?;
    print_registration(&registration)
}

fn print_registration(registration: &registry::Registration) -> Result<()> {
    if registration.replaced {
        warn(format_args!(
            "alias {} now points at a different profile",
            registration.alias
        ));
    }
    emit(format_args!(
        "registered alias={} fingerprint={} bytes={}",
        registration.alias, registration.fingerprint, registration.byte_len
    ))
}

/// Write one line to stdout. A closed pipe ends output quietly instead of panicking.
fn emit(line: std::fmt::Arguments<'_>) -> Result<()> {
    use std::io::Write;
    let mut stdout = std::io::stdout().lock();
    let written = stdout
        .write_fmt(line)
        .and_then(|()| stdout.write_all(b"\n"));
    match written {
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        other => Ok(other?),
    }
}

/// Diagnostic output only: a closed stderr must not panic the CLI either.
fn warn(line: std::fmt::Arguments<'_>) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "{line}");
}
