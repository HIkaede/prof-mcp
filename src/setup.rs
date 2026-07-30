//! Idempotent Codex integration setup.

use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

const START: &str = "<!-- PROF_MCP_START -->";
const END: &str = "<!-- PROF_MCP_END -->";
const GUIDANCE: &str = r#"<!-- PROF_MCP_START -->
## prof-mcp

- For code-level performance attribution, use prof-mcp when `.prof-mcp/manifest.json` exists. If absent, register a folded profile with `prof-mcp register PATH`.
- Start with summary and exact symbol lookup, then use callers, callees, paths, and focused top-self. Do not infer optimization targets from inclusive weight alone.
- Respect scope, truncation reasons, aliases, and fingerprints. Diffs are descriptive, not causal.
- Use SVG for overview and folded data for complete audit. prof-mcp does not run perf or parse `perf.data`.
<!-- PROF_MCP_END -->
"#;

pub fn run(dry_run: bool) -> Result<()> {
    let codex_home = codex_home()?;
    let agents = codex_home.join("AGENTS.md");
    let command = preferred_command()?;
    let desired = format!("command={command}; args=serve --mcp");
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

    let original_agents = match fs::read_to_string(&agents) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| format!("Could not read {}", agents.display()));
        }
    };
    let updated = update_marked_block(&original_agents, GUIDANCE)?;
    if dry_run {
        println!("would configure Codex MCP: {desired}");
        println!("would update {}", agents.display());
        return Ok(());
    }

    fs::create_dir_all(&codex_home)
        .with_context(|| format!("Could not create {}", codex_home.display()))?;

    // Codex configuration and AGENTS.md live in separate persistence layers.
    // Apply the MCP change first, then roll it back best-effort if the local
    // guidance write fails. This avoids reporting a completed setup when only
    // one half was updated.
    let mcp_changed = ensure_codex_registration(&registration, &command)?;
    if let Err(error) = atomic_write(&agents, updated.as_bytes()) {
        if mcp_changed && let Err(rollback) = restore_codex_registration(&registration) {
            return Err(error).context(format!(
                "Could not update {}; additionally could not restore the previous Codex MCP registration: {rollback:#}",
                agents.display()
            ));
        }
        return Err(error);
    }

    println!("configured Codex MCP: {desired}");
    println!("updated {}", agents.display());
    let override_path = codex_home.join("AGENTS.override.md");
    if override_path.is_file()
        && fs::metadata(&override_path).is_ok_and(|metadata| metadata.len() > 0)
    {
        eprintln!(
            "warning: {} overrides AGENTS.md; copy the prof-mcp block there or remove the override",
            override_path.display()
        );
    }
    Ok(())
}

fn codex_home() -> Result<PathBuf> {
    if let Some(path) = env::var_os("CODEX_HOME") {
        return Ok(PathBuf::from(path));
    }
    let home = env::var_os("HOME").context("Neither CODEX_HOME nor HOME is set")?;
    Ok(PathBuf::from(home).join(".codex"))
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
        && transport.args == ["serve", "--mcp"]
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
    if let Err(error) = run_codex(&["mcp", "add", "prof-mcp", "--", command, "serve", "--mcp"]) {
        if let Err(rollback) = restore_codex_registration(current) {
            return Err(error).context(format!(
                "Could not install prof-mcp and could not restore the previous registration: {rollback:#}"
            ));
        }
        return Err(error);
    }
    let verified = matches!(
        classify_registration(codex_registration()?, command)?,
        ExistingRegistration::Desired
    );
    if !verified {
        if let Err(rollback) = restore_codex_registration(current) {
            bail!(
                "Codex accepted prof-mcp setup but did not report the requested command/args; \
                 rollback also failed: {rollback:#}"
            );
        }
        bail!(
            "Codex accepted prof-mcp setup but did not report command={command} with args=serve --mcp"
        );
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

fn update_marked_block(existing: &str, block: &str) -> Result<String> {
    let starts = existing
        .match_indices(START)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let ends = existing
        .match_indices(END)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    match (starts.as_slice(), ends.as_slice()) {
        ([start], [end]) if start <= end => {
            let after = *end + END.len();
            let mut updated = String::with_capacity(existing.len() + block.len());
            updated.push_str(&existing[..*start]);
            updated.push_str(block.trim_end());
            updated.push_str(&existing[after..]);
            if !updated.ends_with('\n') {
                updated.push('\n');
            }
            Ok(updated)
        }
        ([], []) => {
            let mut updated = existing.trim_end().to_owned();
            if !updated.is_empty() {
                updated.push_str("\n\n");
            }
            updated.push_str(block);
            Ok(updated)
        }
        _ => bail!("Codex AGENTS.md must contain zero or one complete prof-mcp marker block"),
    }
}

fn atomic_write(destination: &Path, bytes: &[u8]) -> Result<()> {
    let existing_mode = match fs::symlink_metadata(destination) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!(
                "Refusing to replace symlinked AGENTS.md at {}",
                destination.display()
            );
        }
        Ok(metadata) if !metadata.file_type().is_file() => {
            bail!(
                "Refusing to replace non-file AGENTS.md at {}",
                destination.display()
            );
        }
        Ok(metadata) => Some(metadata.permissions()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Could not inspect {}", destination.display()));
        }
    };
    let parent = destination
        .parent()
        .context("AGENTS.md path has no parent directory")?;
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .context("AGENTS.md file name is not valid UTF-8")?;
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = parent.join(format!(
        ".{file_name}.tmp.{}.{}",
        std::process::id(),
        unique
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .with_context(|| format!("Could not create {}", temp.display()))?;
    if let Err(error) = file.write_all(bytes) {
        let _ = fs::remove_file(&temp);
        return Err(error).with_context(|| format!("Could not write {}", temp.display()));
    }
    if let Some(mode) = existing_mode
        && let Err(error) = fs::set_permissions(&temp, mode)
    {
        let _ = fs::remove_file(&temp);
        return Err(error)
            .with_context(|| format!("Could not preserve mode for {}", destination.display()));
    }
    fs::rename(&temp, destination)
        .with_context(|| format!("Could not replace {}", destination.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marked_block_is_idempotent_and_preserves_other_guidance() {
        let original = "# Personal\n\nKeep this.\n";
        let once = update_marked_block(original, GUIDANCE).unwrap();
        let twice = update_marked_block(&once, GUIDANCE).unwrap();
        assert_eq!(once, twice);
        assert!(once.starts_with(original.trim_end()));
        assert_eq!(once.matches(START).count(), 1);
        assert_eq!(once.matches(END).count(), 1);
    }

    #[test]
    fn json_registration_classification_rejects_extra_user_settings() {
        let desired = CodexServer {
            name: "prof-mcp".into(),
            enabled: true,
            transport: CodexTransport {
                kind: "stdio".into(),
                command: "prof-mcp".into(),
                args: vec!["serve".into(), "--mcp".into()],
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
    }

    #[test]
    fn agents_update_rejects_duplicate_markers() {
        let duplicate = format!("{GUIDANCE}\n{GUIDANCE}");
        assert!(update_marked_block(&duplicate, GUIDANCE).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn agents_atomic_write_rejects_symlink_and_preserves_mode() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("target");
        let agents = temp.path().join("AGENTS.md");
        fs::write(&target, "target").unwrap();
        symlink(&target, &agents).unwrap();
        assert!(atomic_write(&agents, b"replacement").is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "target");

        fs::remove_file(&agents).unwrap();
        fs::write(&agents, "old").unwrap();
        fs::set_permissions(&agents, fs::Permissions::from_mode(0o640)).unwrap();
        atomic_write(&agents, b"new").unwrap();
        assert_eq!(fs::read_to_string(&agents).unwrap(), "new");
        assert_eq!(
            fs::metadata(&agents).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }
}
