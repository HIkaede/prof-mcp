use anyhow::{Result, bail};
use clap::Parser;
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
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(&config.log_level));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    match cli.command {
        Some(Command::Setup { dry_run }) => setup::run(dry_run),
        Some(Command::Serve { mcp }) => {
            if !mcp {
                bail!("prof-mcp serve requires --mcp");
            }
            run_stdio(config).await
        }
        Some(Command::Register {
            profile,
            name,
            sample_period_us,
        }) => register(&config, &profile, name.as_deref(), sample_period_us),
        Some(Command::List) => {
            let status = registry::status(&std::env::current_dir()?).map_err(anyhow::Error::msg)?;
            println!("registry={}", status.registry_root.display());
            println!("active={}", status.active);
            for profile in status.profiles {
                println!(
                    "{}{} fingerprint={} bytes={} source={}",
                    if profile.alias == status.active {
                        "* "
                    } else {
                        "  "
                    },
                    profile.alias,
                    profile.fingerprint,
                    profile.byte_len,
                    profile.source_name
                );
            }
            Ok(())
        }
        Some(Command::Use { alias }) => {
            let status = registry::set_active(&std::env::current_dir()?, &alias)
                .map_err(anyhow::Error::msg)?;
            println!("active alias={}", status.active);
            Ok(())
        }
        Some(Command::Remove { alias, new_active }) => {
            let removal =
                registry::remove(&std::env::current_dir()?, &alias, new_active.as_deref())
                    .map_err(anyhow::Error::msg)?;
            println!("removed alias={} active={}", removal.alias, removal.active);
            Ok(())
        }
        Some(Command::Capture {
            name,
            sample_period_us,
            command,
        }) => {
            let registration = capture::run(&config, name.as_deref(), sample_period_us, &command)
                .map_err(anyhow::Error::msg)?;
            print_registration(&registration);
            Ok(())
        }
        Some(Command::Gc { dry_run }) => {
            let report =
                registry::gc(&std::env::current_dir()?, dry_run).map_err(anyhow::Error::msg)?;
            println!("registry={}", report.registry_root.display());
            println!("dry_run={}", report.dry_run);
            for path in report.removed {
                println!(
                    "{} {}",
                    if dry_run { "would_remove" } else { "removed" },
                    path
                );
            }
            for path in report.skipped {
                eprintln!("skipped {path}");
            }
            Ok(())
        }
        None => match cli.profile {
            Some(profile) => register(&config, &profile, cli.name.as_deref(), cli.sample_period_us),
            None => setup::run(false),
        },
    }
}

fn register(
    config: &Config,
    profile: &std::path::Path,
    name: Option<&str>,
    sample_period_us: Option<u64>,
) -> Result<()> {
    if sample_period_us == Some(0) {
        bail!("--sample-period-us must be a positive integer");
    }
    let registration = registry::register(
        &std::env::current_dir()?,
        profile,
        name,
        config.max_file_size_bytes(),
        sample_period_us,
    )
    .map_err(anyhow::Error::msg)?;
    print_registration(&registration);
    Ok(())
}

fn print_registration(registration: &registry::Registration) {
    println!(
        "registered alias={} fingerprint={} bytes={} sample_period_us={}",
        registration.alias,
        registration.fingerprint,
        registration.byte_len,
        registration
            .sample_period_us
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".into())
    );
}
