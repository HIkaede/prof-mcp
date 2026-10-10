use std::{path::Path, process::Command};

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    for name in ["HEAD", "refs", "packed-refs"] {
        if let Some(path) = git(&["rev-parse", "--git-path", name])
            && Path::new(&path).exists()
        {
            println!("cargo::rerun-if-changed={path}");
        }
    }
    let hash = git(&["rev-parse", "HEAD"])
        .filter(|hash| hash.len() >= 12 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(|hash| hash[..12].to_owned())
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo::rustc-env=PROF_MCP_GIT_HASH={hash}");
}
