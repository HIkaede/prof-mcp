//! Idempotent Codex integration setup.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

pub fn run(dry_run: bool) -> Result<()> {
    let command = preferred_command()?;
    let desired = format!("command={command}; args=serve");
    let current = codex_registration()?;
    let registration = classify_registration(current, &command)?;
    if matches!(registration, ExistingRegistration::Conflict(_)) {
        let detail = match &registration {
            ExistingRegistration::Conflict(detail) => detail,
            _ => unreachable!(),
        };
        bail!(
            "A conflicting Codex MCP registration named prof-mcp already exists: {detail}. Reconfigure or remove it manually before running setup."
        );
    }

    if dry_run {
        println!("would configure Codex MCP: {desired}");
        return Ok(());
    }
    ensure_codex_registration(&registration, &command)?;
    println!("configured Codex MCP: {desired}");
    Ok(())
}

fn preferred_command() -> Result<String> {
    let current = env::current_exe().context("Could not resolve the prof-mcp executable")?;
    if let Some(path) = find_on_path("prof-mcp")
        && same_file(&current, &path)
    {
        return Ok("prof-mcp".into());
    }
    Ok(current.display().to_string())
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|directory| directory.join(name))
            .find(|candidate| candidate.is_file())
    })
}

fn same_file(left: &Path, right: &Path) -> bool {
    fs::canonicalize(left).ok() == fs::canonicalize(right).ok()
}

#[derive(Clone, Debug, Deserialize)]
struct CodexServer {
    name: String,
    enabled: bool,
    transport: CodexTransport,
    #[serde(default)]
    startup_timeout_sec: Option<serde_json::Value>,
    #[serde(default)]
    tool_timeout_sec: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize)]
struct CodexTransport {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: Option<serde_json::Value>,
    #[serde(default)]
    env_vars: Vec<String>,
    #[serde(default)]
    cwd: Option<String>,
}

#[derive(Clone, Debug)]
enum ExistingRegistration {
    None,
    Desired,
    Legacy { command: String },
    Conflict(String),
}

fn codex_registration() -> Result<Option<CodexServer>> {
    let output = Command::new("codex")
        .args(["mcp", "list", "--json"])
        .output()
        .context("Could not run `codex`; install Codex or add it to PATH")?;
    if !output.status.success() {
        bail!(
            "`codex mcp list --json` failed with {}: {}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout).trim(),
            if output.stderr.is_empty() {
                String::new()
            } else {
                format!(" {}", String::from_utf8_lossy(&output.stderr).trim())
            }
        );
    }
    let registrations: Vec<CodexServer> = serde_json::from_slice(&output.stdout)
        .context("`codex mcp list --json` returned invalid JSON")?;
    Ok(registrations
        .into_iter()
        .find(|entry| entry.name == "prof-mcp"))
}

fn classify_registration(
    current: Option<CodexServer>,
    command: &str,
) -> Result<ExistingRegistration> {
    let Some(current) = current else {
        return Ok(ExistingRegistration::None);
    };
    let transport = &current.transport;
    if current.enabled
        && transport.kind == "stdio"
        && transport.command == command
        && transport.args == ["serve"]
        && transport.env.is_none()
        && transport.env_vars.is_empty()
        && transport.cwd.is_none()
        && current.startup_timeout_sec.is_none()
        && current.tool_timeout_sec.is_none()
    {
        return Ok(ExistingRegistration::Desired);
    }
    if current.enabled
        && transport.kind == "stdio"
        && transport.args.is_empty()
        && transport.env.is_none()
        && transport.env_vars.is_empty()
        && transport.cwd.is_none()
        && current.startup_timeout_sec.is_none()
        && current.tool_timeout_sec.is_none()
        && legacy_command_matches(&transport.command, command)
    {
        return Ok(ExistingRegistration::Legacy {
            command: transport.command.clone(),
        });
    }
    Ok(ExistingRegistration::Conflict(format!(
        "enabled={}, transport={}, command={}, args={:?}, cwd={:?}, env_present={}, env_vars={}, startup_timeout_set={}, tool_timeout_set={}",
        current.enabled,
        transport.kind,
        transport.command,
        transport.args,
        transport.cwd,
        transport.env.is_some(),
        transport.env_vars.len(),
        current.startup_timeout_sec.is_some(),
        current.tool_timeout_sec.is_some(),
    )))
}

fn legacy_command_matches(existing: &str, command: &str) -> bool {
    existing == command
        || existing == "prof-mcp"
        || find_on_path("prof-mcp")
            .is_some_and(|installed| same_file(Path::new(existing), &installed))
}

fn ensure_codex_registration(current: &ExistingRegistration, command: &str) -> Result<bool> {
    if matches!(current, ExistingRegistration::Desired) {
        return Ok(false);
    }
    if matches!(current, ExistingRegistration::Legacy { .. }) {
        run_codex(&["mcp", "remove", "prof-mcp"])?;
    }
    let install = (|| -> Result<()> {
        run_codex(&["mcp", "add", "prof-mcp", "--", command, "serve"])?;
        if !matches!(
            classify_registration(codex_registration()?, command)?,
            ExistingRegistration::Desired
        ) {
            bail!(
                "Codex accepted prof-mcp setup but did not report command={command} with args=serve"
            );
        }
        Ok(())
    })();
    if let Err(error) = install {
        if let Err(rollback) = restore_codex_registration(current) {
            return Err(error).context(format!(
                "Could not install prof-mcp and could not restore the previous registration: {rollback:#}"
            ));
        }
        return Err(error);
    }
    Ok(true)
}

fn restore_codex_registration(previous: &ExistingRegistration) -> Result<()> {
    // The only mutation callers make before restoration is removal/addition of
    // this named entry. Start from a known absent state, then reconstruct the
    // lossless legacy command representation confirmed by `codex mcp list --json`.
    let _ = run_codex(&["mcp", "remove", "prof-mcp"]);
    if let ExistingRegistration::Legacy { command } = previous {
        run_codex(&["mcp", "add", "prof-mcp", "--", command])?;
    }
    Ok(())
}

fn run_codex(args: &[&str]) -> Result<()> {
    let status = Command::new("codex")
        .args(args)
        .status()
        .context("Could not run `codex`; install Codex or add it to PATH")?;
    if !status.success() {
        bail!("`codex {}` failed with {status}", args.join(" "));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reject_custom_registration() {
        let desired = CodexServer {
            name: "prof-mcp".into(),
            enabled: true,
            transport: CodexTransport {
                kind: "stdio".into(),
                command: "prof-mcp".into(),
                args: vec!["serve".into()],
                env: None,
                env_vars: Vec::new(),
                cwd: None,
            },
            startup_timeout_sec: None,
            tool_timeout_sec: None,
        };
        assert!(matches!(
            classify_registration(Some(desired.clone()), "prof-mcp").unwrap(),
            ExistingRegistration::Desired
        ));
        let mut legacy = desired.clone();
        legacy.transport.args.clear();
        assert!(matches!(
            classify_registration(Some(legacy), "prof-mcp").unwrap(),
            ExistingRegistration::Legacy { .. }
        ));
        let mut custom = desired;
        custom.transport.cwd = Some("/workspace".into());
        assert!(matches!(
            classify_registration(Some(custom), "prof-mcp").unwrap(),
            ExistingRegistration::Conflict(_)
        ));
        let remote = serde_json::from_value(serde_json::json!({
            "name": "prof-mcp",
            "enabled": true,
            "transport": {"type": "streamable_http", "url": "https://example.test/mcp"}
        }))
        .unwrap();
        assert!(matches!(
            classify_registration(Some(remote), "prof-mcp").unwrap(),
            ExistingRegistration::Conflict(_)
        ));
    }
}
