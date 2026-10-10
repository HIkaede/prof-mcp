//! Thin Linux capture orchestration: perf -> folded text -> registry.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::File,
    io::{BufRead, BufReader, BufWriter, Read, Write},
    process::{Child, Command, Stdio},
};

use anyhow::{Context, Result, bail};

use crate::{config::Config, profile::BuildLimits, registry};

pub fn run(
    config: &Config,
    name: Option<&str>,
    command: &[OsString],
) -> Result<registry::Registration> {
    if command.is_empty() {
        bail!("capture requires a command after --");
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = (config, name, command);
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
        let mut script = PerfScript(
            Command::new("perf")
                .args(["script", "-i"])
                .arg(&perf_data)
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .context("could not start perf script")?,
        );
        let stdout = script
            .0
            .stdout
            .take()
            .context("perf script stdout unavailable")?;
        let mut writer = BufWriter::new(file);
        collapse_perf_script(
            BufReader::new(stdout),
            &mut writer,
            BuildLimits {
                max_file_bytes: config.max_file_size_bytes(),
                ..BuildLimits::default()
            },
        )?;
        writer.flush().context("could not flush folded capture")?;
        let status = script.0.wait().context("could not wait for perf script")?;
        if !status.success() {
            bail!("perf script failed: {status}");
        }

        registry::register(
            &std::env::current_dir()?,
            &folded,
            name,
            config.max_file_size_bytes(),
        )
        .map_err(anyhow::Error::msg)
    }
}

// The stream may still be producing output when parsing or file writes fail.
// Always terminate and reap perf before leaving the temporary capture directory.
#[cfg(target_os = "linux")]
struct PerfScript(Child);
#[cfg(target_os = "linux")]
impl Drop for PerfScript {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn collapse_perf_script<R: BufRead, W: Write>(
    mut reader: R,
    writer: &mut W,
    limits: BuildLimits,
) -> Result<()> {
    let mut collapsed = BTreeMap::<String, u64>::new();
    let mut process = None;
    let mut period = 1_u64;
    let mut stack = Vec::<String>::new();
    let mut event_filter = None;
    let mut input_bytes = 0_u64;
    let mut total_weight = 0_u64;
    let mut sample_bytes = 0_usize;
    let mut line_bytes = Vec::new();
    let mut line_no = 0;

    loop {
        line_bytes.clear();
        // Read at most one byte past either limit, including for a stream with
        // no newline. BufRead::lines would allocate an unbounded String first.
        let remaining = limits.max_file_bytes - input_bytes;
        let bound = remaining
            .saturating_add(1)
            .min((limits.max_line_bytes as u64).saturating_add(1));
        let read = reader
            .by_ref()
            .take(bound)
            .read_until(b'\n', &mut line_bytes)
            .context("could not read perf script")?;
        if read == 0 {
            break;
        }
        line_no += 1;
        if read as u64 > remaining {
            bail!("perf script exceeds input byte limit");
        }
        input_bytes += read as u64;
        if read > limits.max_line_bytes {
            bail!("perf script line {line_no} exceeds line byte limit");
        }
        let line = std::str::from_utf8(&line_bytes)
            .context("perf script contains non-UTF-8 bytes")?
            .trim_end_matches(['\r', '\n']);
        if line.trim_start().starts_with('#') {
            continue;
        }
        if line.trim().is_empty() {
            finish_sample(
                &mut collapsed,
                &mut process,
                period,
                &mut stack,
                &mut total_weight,
                limits,
            )?;
            continue;
        }
        if let Some((name, sample_period, event)) = parse_event_header(line) {
            finish_sample(
                &mut collapsed,
                &mut process,
                period,
                &mut stack,
                &mut total_weight,
                limits,
            )?;
            if event_filter
                .as_deref()
                .is_some_and(|selected| selected != event)
            {
                bail!("perf script contains multiple event types at line {line_no}");
            }
            if sample_period == 0 {
                bail!("perf sample weight must be positive");
            }
            event_filter = Some(event);
            let name = encode_frame(&name);
            sample_bytes = name.len();
            process = Some(name);
            period = sample_period;
            continue;
        }
        if process.is_some()
            && let Some(frame) = parse_stack_line(line)
        {
            if stack.len().saturating_add(2) > limits.max_depth {
                bail!("perf stack exceeds maximum depth");
            }
            sample_bytes = sample_bytes
                .checked_add(frame.len() + 1)
                .context("perf stack size overflow")?;
            if sample_bytes.saturating_add(period.to_string().len() + 2) > limits.max_line_bytes {
                bail!("folded capture exceeds line byte limit");
            }
            stack.push(frame);
        } else {
            bail!("invalid perf script line {line_no}");
        }
    }
    finish_sample(
        &mut collapsed,
        &mut process,
        period,
        &mut stack,
        &mut total_weight,
        limits,
    )?;
    if collapsed.is_empty() {
        bail!("perf script contains no samples with stack frames");
    }

    // Check the complete serialized output before writing anything. Encoding
    // can expand symbols, and combining weights can add decimal digits.
    let mut output_bytes = 0_u64;
    for (stack, weight) in &collapsed {
        let len = stack.len().saturating_add(weight.to_string().len() + 2);
        if len > limits.max_line_bytes {
            bail!("folded capture exceeds line byte limit");
        }
        output_bytes = output_bytes
            .checked_add(len as u64)
            .context("folded capture size overflow")?;
        if output_bytes > limits.max_file_bytes {
            bail!("folded capture exceeds output byte limit");
        }
    }
    for (stack, weight) in collapsed {
        writeln!(writer, "{stack} {weight}").context("could not write folded capture")?;
    }
    Ok(())
}

fn finish_sample(
    collapsed: &mut BTreeMap<String, u64>,
    process: &mut Option<String>,
    period: u64,
    stack: &mut Vec<String>,
    total_weight: &mut u64,
    limits: BuildLimits,
) -> Result<()> {
    let Some(process) = process.take() else {
        stack.clear();
        return Ok(());
    };
    if stack.is_empty() {
        bail!("perf sample has no stack frames");
    }
    *total_weight = total_weight
        .checked_add(period)
        .filter(|sum| *sum <= limits.max_total_weight)
        .context("perf sample total weight exceeds maximum")?;
    let key_len = process.len() + stack.iter().map(String::len).sum::<usize>() + stack.len();
    let mut key = String::with_capacity(key_len);
    key.push_str(&process);
    for frame in stack.drain(..).rev() {
        key.push(';');
        key.push_str(&frame);
    }
    let entry = collapsed.entry(key).or_insert(0);
    *entry = entry
        .checked_add(period)
        .context("perf sample period overflow")?;
    Ok(())
}

fn parse_event_header(line: &str) -> Option<(String, u64, String)> {
    let event_prefix = line.trim_end().strip_suffix(':')?;
    let (left, event) = event_prefix.rsplit_once(char::is_whitespace)?;
    let left = left.trim_end();
    let (before_timestamp, period) = if left.ends_with(':') {
        (left, 1)
    } else {
        let (prefix, period) = left.rsplit_once(char::is_whitespace)?;
        (prefix.trim_end(), period.parse::<u64>().ok()?)
    };
    let before_timestamp = before_timestamp.strip_suffix(':')?.trim_end();
    let (fields_text, timestamp) = before_timestamp.rsplit_once(char::is_whitespace)?;
    let timestamp = timestamp.parse::<f64>().ok()?;
    if !timestamp.is_finite() || timestamp < 0.0 {
        return None;
    }
    let (mut process, mut pid) = fields_text.trim_end().rsplit_once(char::is_whitespace)?;
    if pid.starts_with('[') && pid.ends_with(']') {
        (process, pid) = process.trim_end().rsplit_once(char::is_whitespace)?;
    }
    if !is_pid_field(pid) {
        return None;
    }
    let process = process.trim();
    if process.is_empty() || event.is_empty() {
        return None;
    }
    Some((process.to_owned(), period, event.to_owned()))
}

fn is_pid_field(field: &str) -> bool {
    let valid = |value: &str| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit());
    field
        .split_once('/')
        .map_or_else(|| valid(field), |(pid, tid)| valid(pid) && valid(tid))
}

// Percent escaping keeps folded separators unambiguous without conflating
// a literal colon or percent sequence with the original symbol identity.
fn encode_frame(frame: &str) -> String {
    let extra = frame
        .bytes()
        .filter(|byte| matches!(byte, b'%' | b';' | b'[' | b']'))
        .count()
        .saturating_mul(2);
    let mut encoded = String::with_capacity(frame.len().saturating_add(extra));
    for ch in frame.chars() {
        match ch {
            '%' => encoded.push_str("%25"),
            ';' => encoded.push_str("%3B"),
            '[' => encoded.push_str("%5B"),
            ']' => encoded.push_str("%5D"),
            ch => encoded.push(ch),
        }
    }
    encoded
}

fn parse_stack_line(line: &str) -> Option<String> {
    let line = line.trim_start();
    if !line.ends_with(')') {
        return None;
    }
    let balanced_start = {
        let mut nesting = 0_usize;
        line.char_indices()
            .rev()
            .find_map(|(index, ch)| {
                if ch == ')' {
                    nesting += 1;
                } else if ch == '(' {
                    nesting -= 1;
                    if nesting == 0 {
                        return Some(index);
                    }
                }
                None
            })
            .and_then(|index| index.checked_sub(1))
            .filter(|start| line.as_bytes()[*start] == b' ')
    };
    // A path can contain nested prefixes or unpaired parentheses; a symbol's
    // complete parenthesized argument still belongs before the DSO field.
    let mut path_start = None;
    let mut path_depth = 0_isize;
    for (index, ch) in line.char_indices() {
        if ch == ' '
            && path_depth == 0
            && (line[index..].starts_with(" (/") || line[index..].starts_with(" (["))
        {
            path_start = Some(index);
        }
        if path_start.is_some() {
            path_depth += match ch {
                '(' => 1,
                ')' => -1,
                _ => 0,
            };
        }
    }
    let module_start = match (path_start, balanced_start) {
        (Some(path), Some(balanced)) if path < balanced && path_depth == 0 => balanced,
        (Some(path), _) => path,
        (_, balanced) => balanced?,
    };
    let frame = &line[..module_start];
    let module = &line[module_start + 2..line.len() - 1];
    let (pc, raw) = frame.split_once(char::is_whitespace)?;
    let pc = pc.strip_prefix("0x").unwrap_or(pc);
    if pc.is_empty() || !pc.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut function = raw.trim().to_owned();
    if let Some((symbol, offset)) = function.rsplit_once("+0x")
        && !offset.is_empty()
        && offset.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        function = symbol.to_owned();
    }
    if function.is_empty() || module.is_empty() {
        return None;
    }
    // Literal brackets are escaped, so this reserved marker cannot be a symbol name.
    let function = if function == "[unknown]" {
        format!("[unknown@0x{pc}]")
    } else {
        encode_frame(&function)
    };
    Some(format!("{function} [{}]", encode_frame(module)))
}

#[cfg(test)]
mod tests;
