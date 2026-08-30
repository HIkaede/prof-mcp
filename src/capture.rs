//! Thin Linux capture orchestration: perf -> folded text -> registry.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::File,
    io::{BufRead, BufReader, BufWriter, Write},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail};

use crate::{config::Config, registry};

pub fn run(
    config: &Config,
    name: Option<&str>,
    sample_period_us: Option<u64>,
    command: &[OsString],
) -> Result<registry::Registration> {
    if sample_period_us == Some(0) {
        bail!("--sample-period-us must be a positive integer");
    }
    if command.is_empty() {
        bail!("capture requires a command after --");
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = (config, name, sample_period_us, command);
        bail!("capture is only supported on Linux");
    }

    #[cfg(target_os = "linux")]
    {
        let temporary = tempfile::tempdir().context("could not create capture directory")?;
        let perf_data = temporary.path().join("perf.data");
        let status = Command::new("perf")
            .arg("record")
            .arg("-g")
            .arg("-o")
            .arg(&perf_data)
            .arg("--")
            .args(command)
            .status()
            .context("could not start perf record")?;
        if !status.success() {
            bail!("perf record failed: {status}");
        }

        let folded = temporary.path().join("capture.folded");
        let file = File::create(&folded).context("could not store folded capture")?;
        let mut script = Command::new("perf")
            .args(["script", "-i"])
            .arg(&perf_data)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("could not start perf script")?;
        let stdout = script
            .stdout
            .take()
            .context("perf script stdout unavailable")?;
        let mut writer = BufWriter::new(file);
        let collapse_result = collapse_perf_script(BufReader::new(stdout), &mut writer);
        writer.flush().context("could not flush folded capture")?;
        let status = script.wait().context("could not wait for perf script")?;
        collapse_result?;
        if !status.success() {
            bail!("perf script failed: {status}");
        }

        registry::register(
            &std::env::current_dir()?,
            &folded,
            name,
            config.max_file_size_bytes(),
            sample_period_us,
        )
        .map_err(anyhow::Error::msg)
    }
}

fn collapse_perf_script<R: BufRead, W: Write>(reader: R, writer: &mut W) -> Result<()> {
    let mut collapsed = BTreeMap::<String, u64>::new();
    let mut process = None;
    let mut period = 1_u64;
    let mut stack = Vec::<Vec<String>>::new();
    let mut event_filter = None;

    for (line_no, result) in reader.lines().enumerate() {
        let line =
            result.with_context(|| format!("could not read perf script line {}", line_no + 1))?;
        if line.starts_with('#') {
            continue;
        }
        if line.trim().is_empty() {
            finish_sample(&mut collapsed, &mut process, &mut period, &mut stack)?;
            continue;
        }
        if let Some((name, sample_period, event)) = parse_event_header(&line) {
            if process.is_some() {
                finish_sample(&mut collapsed, &mut process, &mut period, &mut stack)?;
            }
            if event_filter
                .as_deref()
                .is_some_and(|selected| selected != event)
            {
                process = None;
                continue;
            }
            event_filter = Some(event);
            process = Some(name.replace(' ', "_"));
            period = sample_period;
            continue;
        }
        if process.is_some()
            && let Some(frames) = parse_stack_line(
                &line,
                process
                    .as_deref()
                    .is_some_and(|name| name.starts_with("java")),
            )
        {
            stack.push(frames);
        }
    }
    finish_sample(&mut collapsed, &mut process, &mut period, &mut stack)?;

    for (stack, weight) in collapsed {
        writeln!(writer, "{stack} {weight}").context("could not write folded capture")?;
    }
    Ok(())
}

fn finish_sample(
    collapsed: &mut BTreeMap<String, u64>,
    process: &mut Option<String>,
    period: &mut u64,
    stack: &mut Vec<Vec<String>>,
) -> Result<()> {
    let Some(process) = process.take() else {
        stack.clear();
        return Ok(());
    };
    if stack.is_empty() {
        return Ok(());
    }
    let mut frames = vec![process];
    for line in stack.drain(..).rev() {
        frames.extend(line);
    }
    let key = frames.join(";");
    let entry = collapsed.entry(key).or_insert(0);
    *entry = entry
        .checked_add(*period)
        .context("perf sample period overflow")?;
    *period = 1;
    Ok(())
}

fn parse_event_header(line: &str) -> Option<(String, u64, String)> {
    if line.starts_with(char::is_whitespace) {
        return None;
    }
    let end = line.trim_end();
    let event_colon = end.rfind(':')?;
    let event_prefix = &end[..event_colon];
    let (left, event) = event_prefix.rsplit_once(char::is_whitespace)?;
    let (before_timestamp, period) = match left.trim_end().rsplit_once(char::is_whitespace) {
        Some((prefix, candidate)) if candidate.parse::<u64>().is_ok() => {
            (prefix, candidate.parse().ok()?)
        }
        _ => (left.trim_end(), 1),
    };
    let timestamp_colon = before_timestamp.rfind(':')?;
    let before_timestamp = before_timestamp[..timestamp_colon].trim_end();
    let (fields_text, timestamp) = before_timestamp.rsplit_once(char::is_whitespace)?;
    timestamp.parse::<f64>().ok()?;
    let fields: Vec<_> = fields_text.split_whitespace().collect();
    let pid_index = fields.iter().position(|field| is_pid_field(field))?;
    let process = fields[..pid_index].join(" ");
    if process.is_empty() {
        return None;
    }
    Some((process, period, event.trim().to_owned()))
}

fn is_pid_field(field: &str) -> bool {
    let valid = |value: &str| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit());
    field
        .split_once('/')
        .map_or_else(|| valid(field), |(pid, tid)| valid(pid) && valid(tid))
}

fn parse_stack_line(line: &str, java_process: bool) -> Option<Vec<String>> {
    let line = line.trim_start();
    let module_start = line.rfind(" (")?;
    if !line.ends_with(')') {
        return None;
    }
    let frame = &line[..module_start];
    let module = &line[module_start + 2..line.len() - 1];
    let (pc, raw) = frame.split_once(char::is_whitespace)?;
    if pc.is_empty()
        || !pc
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return None;
    }
    let mut frames = Vec::new();
    for rawfunc in raw.trim().split("->") {
        if rawfunc.starts_with('(') {
            continue;
        }
        let mut function = rawfunc.trim().to_owned();
        if let Some(offset) = function.rfind("+0x")
            && function[offset + 3..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            function.truncate(offset);
        }
        if function == "[unknown]" {
            function = if module == "[unknown]" {
                "[unknown]".into()
            } else {
                format!("[{}]", module.rsplit('/').next().unwrap_or(module))
            };
        }
        function = function.replace(';', ":");
        if java_process && function.starts_with('L') && function.contains(':') {
            function.remove(0);
        }
        let go_method = function
            .find(".(")
            .is_some_and(|open| function[open + 2..].contains(")."));
        if let Some(paren) = function.find('(').filter(|&paren| {
            !go_method && !function[paren + 1..].starts_with("anonymous namespace")
        }) {
            function.truncate(paren);
        }
        function.retain(|character| character != '\'' && character != '"');
        if !function.is_empty() {
            frames.push(function);
        }
    }
    (!frames.is_empty()).then_some(frames)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::collapse_perf_script;

    #[test]
    fn collapses_default_perf_script_and_filters_other_events() {
        let input = b"# header\nworker 12 1.0: 2 cpu/cycles/P:\n  7 leaf+0x4 (/tmp/a)\n  8 caller (/tmp/a)\n\nworker 12 2.0: 9 instructions:\n  7 ignored (/tmp/a)\n\nworker 12 3.0: 3 cpu/cycles/P:\n  7 leaf (/tmp/a)\n  8 caller (/tmp/a)\n";
        let mut output = Vec::new();
        collapse_perf_script(Cursor::new(input), &mut output).unwrap();
        assert_eq!(output, b"worker;caller;leaf 5\n");
    }

    #[test]
    fn accepts_pid_tid_and_process_names_with_spaces() {
        let input = b"V8 WorkerThread 24636/25607 [000] 1.0: 4 cycles:\n  7 main (/tmp/a)\n\n";
        let mut output = Vec::new();
        collapse_perf_script(Cursor::new(input), &mut output).unwrap();
        assert_eq!(output, b"V8_WorkerThread;main 4\n");
    }
}
