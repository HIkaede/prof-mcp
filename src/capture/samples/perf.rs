use std::{
    fs::{self, File},
    io::{self, BufRead, BufReader, BufWriter, Read, Seek, Write},
    path::Path,
    process::{ChildStdout, Command, Stdio},
};

use anyhow::{Context, Result, bail, ensure};
use regex::{Captures, Regex};

use super::{
    MAX_RECORDS, PerfLoss, PerfSource, SampleRecord, SampleSnapshot, StackSource, VERSION,
    snapshot_limit,
};
use crate::{
    capture::{PerfScript, encode_frame, parse_stack_location},
    config::Config,
    profile::BuildLimits,
};

const MAX_METADATA_BYTES: u64 = 2 * 1024 * 1024;

pub fn import_perf(input: &Path, output: &Path, config: &Config) -> Result<()> {
    let input = fs::canonicalize(input).context("could not locate perf.data")?;
    let source_path = input.to_str().context("perf.data path must be UTF-8")?;
    let limits = super::super::build_limits(config);
    let source_len = fs::metadata(&input).context("could not inspect perf.data")?;
    ensure!(
        source_len.is_file(),
        "sample import requires a regular perf.data file"
    );
    ensure!(
        source_len.len() <= limits.max_file_bytes,
        "perf.data exceeds input byte limit"
    );

    // Every perf pass and the digest read the same private recording bytes.
    let mut data =
        tempfile::NamedTempFile::new().context("could not create temporary recording")?;
    let mut original = File::open(&input).context("could not open perf.data")?;
    let mut magic = [0; 8];
    original
        .read_exact(&mut magic)
        .context("could not read perf.data magic")?;
    ensure!(
        &magic == b"PERFILE2" || &magic == b"2ELIFREP",
        "sample import requires perf.data"
    );
    original.rewind()?;
    let copied = io::copy(
        &mut original.take(limits.max_file_bytes.saturating_add(1)),
        &mut data,
    )?;
    ensure!(
        copied <= limits.max_file_bytes,
        "perf.data exceeds input byte limit"
    );
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(data.reopen()?)?;

    let perf_version = perf_metadata(&["--version"], None)?;
    let event_attributes = perf_metadata(&["evlist", "-v"], Some(data.path()))?;
    let (event, sample_types) = event_description(&event_attributes)?;
    ensure!(
        ["IP", "TID", "TIME"]
            .iter()
            .all(|field| sample_types.contains(field)),
        "capability_unavailable: sample import requires perf sample_type IP, TID and TIME"
    );
    let has_callchain = sample_types.contains(&"CALLCHAIN") || sample_types.contains(&"STACK_USER");
    let columns = Columns {
        cpu: sample_types.contains(&"CPU"),
        period: sample_types.contains(&"PERIOD"),
    };
    let script_fields = columns.fields();
    let max_stack = limits.max_depth.to_string();
    let mut args = vec![
        "script",
        "--ns",
        "--no-demangle",
        "--no-inline",
        "--no-itrace",
        "--max-stack",
        &max_stack,
        "-F",
        &script_fields,
    ];
    let read = |stdout| parse_samples(stdout, event, columns, limits, snapshot_limit(config));
    let mut records = perf_stdout(&args, Some(data.path()), read)?;
    if has_callchain {
        // Callchain output can omit the sampled location when unwinding fails.
        // Get the sampled IP directly and match records in perf's emitted order.
        args.push("--hide-call-graph");
        let leaves = perf_stdout(&args, Some(data.path()), read)?;
        join_sampled_locations(&mut records, leaves)?;
    } else {
        for record in &mut records {
            record.stack_source = Some(StackSource::SampledIpOnly);
        }
    }
    let record_stats = perf_metadata(&["report", "--stats", "--no-itrace"], Some(data.path())).ok();
    let loss = record_stats
        .as_deref()
        .and_then(|stats| loss_stats(stats, event));
    let snapshot = SampleSnapshot {
        version: VERSION,
        source: Some(PerfSource {
            path: source_path.to_owned(),
            blake3: hasher.finalize().to_hex().to_string(),
            perf_version,
            recording_header: perf_metadata(&["script", "--header-only"], Some(data.path()))?,
            record_stats,
            loss,
            buildids: perf_metadata(&["buildid-list"], Some(data.path()))?,
            script_fields,
            symbolization: "native perf; no demangling, inline expansion or synthesized auxtrace"
                .to_owned(),
            callchain: has_callchain,
            max_stack: limits.max_depth,
            event_attributes: event_attributes.clone(),
        }),
        event: event.to_owned(),
        records,
    };
    snapshot.validate(limits)?;

    let directory = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(directory)
        .context("could not create temporary sample snapshot")?;
    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        serde_json::to_writer(&mut writer, &snapshot).context("could not write sample snapshot")?;
        writer.flush().context("could not flush sample snapshot")?;
    }
    ensure!(
        temporary.as_file().metadata()?.len() <= snapshot_limit(config),
        "sample snapshot exceeds output byte limit"
    );
    temporary
        .persist_noclobber(output)
        .context("could not create sample snapshot")?;
    Ok(())
}

fn perf_stdout<T>(
    args: &[&str],
    input: Option<&Path>,
    decode: impl FnOnce(BufReader<ChildStdout>) -> Result<T>,
) -> Result<T> {
    let mut command = Command::new("perf");
    command
        .env("LC_ALL", "C")
        .env("PERF_CONFIG_NOSYSTEM", "1")
        .env("PERF_CONFIG_NOGLOBAL", "1")
        .env("DEBUGINFOD_URLS", "")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    if let Some(input) = input {
        command.arg("-i").arg(input);
    }
    let mut child = PerfScript(command.spawn().context("could not start perf")?);
    let stdout = child.0.stdout.take().context("perf stdout unavailable")?;
    let decoded = decode(BufReader::new(stdout))?;
    let status = child.0.wait().context("could not wait for perf")?;
    ensure!(status.success(), "perf {} failed: {status}", args[0]);
    Ok(decoded)
}

fn perf_metadata(args: &[&str], input: Option<&Path>) -> Result<String> {
    perf_stdout(args, input, |reader| {
        let mut bytes = String::new();
        reader
            .take(MAX_METADATA_BYTES + 1)
            .read_to_string(&mut bytes)
            .context("could not read perf metadata")?;
        ensure!(
            bytes.len() as u64 <= MAX_METADATA_BYTES,
            "perf metadata exceeds byte limit"
        );
        Ok(bytes.trim_end().to_owned())
    })
}

fn event_description(metadata: &str) -> Result<(&str, Vec<&str>)> {
    let mut lines = metadata
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'));
    let line = lines
        .next()
        .context("perf.data has no event configuration")?;
    ensure!(
        lines.next().is_none(),
        "sample import requires exactly one configured perf event"
    );
    let (event, attributes) = line
        .trim()
        .split_once(": type:")
        .context("unrecognized perf evlist format")?;
    let fields = attributes
        .split_once("sample_type:")
        .context("perf evlist has no sample_type")?
        .1;
    let fields = fields
        .split(',')
        .next()
        .expect("split returns a field")
        .trim();
    Ok((event, fields.split('|').collect()))
}

fn loss_stats(stats: &str, event: &str) -> Option<PerfLoss> {
    let (_, stats) = stats.split_once("Aggregated stats:\n")?;
    let (combined, event_stats) = stats.split_once(&format!("\n{event} stats:\n"))?;
    let stat = |section: &str, field: &str| {
        section
            .lines()
            .find_map(|line| line.trim().strip_prefix(field))
            .map_or(Some(0), |value| {
                value.split_ascii_whitespace().next()?.parse::<u64>().ok()
            })
    };
    // A missing row means zero in the perf stats table. Require the enclosing
    // tables and their sample count to distinguish unknown report formats.
    if stat(combined, "TOTAL events:")? == 0 || stat(event_stats, "SAMPLE events:")? == 0 {
        return None;
    }
    Some(PerfLoss {
        lost_record_notifications: stat(combined, "LOST events:")?,
        lost_samples: stat(event_stats, "LOST_SAMPLES events:")?,
    })
}

#[derive(Clone, Copy)]
struct Columns {
    cpu: bool,
    period: bool,
}

impl Columns {
    fn fields(self) -> String {
        let cpu = if self.cpu { "cpu," } else { "" };
        let period = if self.period { "period," } else { "" };
        format!("comm,pid,tid,{cpu}time,event,{period}ip,sym,dso")
    }
}

fn from_header(fields: &Captures<'_>, event: &str, columns: Columns) -> Result<SampleRecord> {
    ensure!(
        &fields["event"] == event,
        "perf script event does not match the configured event"
    );
    let period = fields
        .name("period")
        .map(|period| period.as_str().parse::<u64>())
        .transpose()?;
    let cpu = fields
        .name("cpu")
        .map(|cpu| cpu.as_str().parse::<u32>())
        .transpose()?;
    ensure!(
        period.is_some() == columns.period && cpu.is_some() == columns.cpu,
        "perf script columns do not match sample_type"
    );
    let nanos = fields["nanos"].parse::<u64>()?;
    let timestamp_ns = fields["seconds"]
        .parse::<u64>()?
        .checked_mul(1_000_000_000)
        .and_then(|seconds| seconds.checked_add(nanos))
        .context("perf timestamp is outside the nanosecond range")?;
    let mut record = SampleRecord {
        comm: Some(fields["comm"].trim().to_owned()),
        pid: Some(fields["pid"].parse()?),
        tid: Some(fields["tid"].parse()?),
        cpu,
        timestamp_ns: Some(timestamp_ns),
        period,
        sampled_ip: None,
        stack_source: None,
        stack: Vec::new(),
    };
    if let Some(leaf) = fields
        .name("leaf")
        .filter(|leaf| !leaf.as_str().trim().is_empty())
    {
        push_frame(&mut record, leaf.as_str())?;
    }
    Ok(record)
}

fn push_frame(record: &mut SampleRecord, line: &str) -> Result<()> {
    let (address, symbol, module) =
        parse_stack_location(line.trim_end()).context("invalid perf frame")?;
    let pc = u64::from_str_radix(address, 16)?;
    let function = if symbol == "[unknown]" {
        format!("[unknown@0x{pc:x}]")
    } else {
        // The first space-bracket delimiter now uniquely separates symbol and DSO.
        encode_frame(symbol).replace('[', "%5B").replace(']', "%5D")
    };
    let frame = format!("{function} [{}]", encode_frame(module));
    if record.stack.is_empty() {
        record.sampled_ip = Some(format!("{pc:x}"));
    }
    record.stack.push(frame);
    Ok(())
}

fn finish(mut record: SampleRecord, records: &mut Vec<SampleRecord>) -> Result<()> {
    ensure!(
        records.len() < MAX_RECORDS,
        "perf script exceeds sample record limit"
    );
    record.stack.reverse();
    records.push(record);
    Ok(())
}

fn join_sampled_locations(records: &mut [SampleRecord], leaves: Vec<SampleRecord>) -> Result<()> {
    ensure!(
        records.len() == leaves.len(),
        "perf callchain and sampled IP exports have different record counts"
    );
    for (record, mut sampled) in records.iter_mut().zip(leaves) {
        ensure!(
            record.timestamp_ns == sampled.timestamp_ns
                && record.pid == sampled.pid
                && record.tid == sampled.tid
                && record.cpu == sampled.cpu
                && record.period == sampled.period
                && record.comm == sampled.comm,
            "perf callchain and sampled IP exports have different record ordering"
        );
        ensure!(
            sampled.stack.len() == 1,
            "perf sampled IP export did not contain one location"
        );
        let leaf = sampled.stack.pop().expect("one location checked");
        record.stack_source = Some(if record.sampled_ip == sampled.sampled_ip {
            *record.stack.last_mut().expect("IP implies a frame") = leaf;
            StackSource::PerfCallchain
        } else {
            // A mismatched callchain cannot establish who called the sampled location.
            record.stack.clear();
            record.stack.push(leaf);
            StackSource::SampledIpOnly
        });
        record.sampled_ip = sampled.sampled_ip;
    }
    Ok(())
}

fn parse_samples(
    mut stdout: impl BufRead,
    event: &str,
    columns: Columns,
    limits: BuildLimits,
    limit: u64,
) -> Result<Vec<SampleRecord>> {
    // perf chooses column order regardless of -F order. Capture version and fields in
    // the snapshot and reject incompatible output instead of guessing positions.
    let header = Regex::new(concat!(
        r"^[ \t]*(?P<comm>.*?)[ \t]+(?P<pid>[0-9]+)/(?P<tid>[0-9]+)[ \t]+",
        r"(?:\[(?P<cpu>[0-9]+)\][ \t]+)?(?P<seconds>[0-9]+)\.(?P<nanos>[0-9]{9}):[ \t]+",
        r"(?:(?P<period>[0-9]+)[ \t]+)?(?P<event>[^ \t]+):(?:[ \t]+(?P<leaf>.*))?$",
    ))
    .expect("fixed perf header regex");
    let mut pending: Option<SampleRecord> = None;
    let mut records = Vec::new();
    let mut bytes = Vec::new();
    let mut remaining = limit;
    let mut line_no = 0;
    loop {
        bytes.clear();
        let bound = (remaining + 1).min(limits.max_line_bytes as u64 + 1);
        let read = stdout
            .by_ref()
            .take(bound)
            .read_until(b'\n', &mut bytes)
            .context("could not read perf script")?;
        if read == 0 {
            break;
        }
        line_no += 1;
        ensure!(
            read <= limits.max_line_bytes,
            "perf script line {line_no} exceeds line byte limit"
        );
        ensure!(
            read as u64 <= remaining,
            "perf script exceeds input byte limit"
        );
        remaining -= read as u64;
        let line = std::str::from_utf8(&bytes)
            .context("perf script must be UTF-8")?
            .trim_end_matches(['\r', '\n']);
        let fields = header.captures(line);
        if line.trim().is_empty() || fields.is_some() {
            if let Some(previous) = pending.take() {
                finish(previous, &mut records)?;
            }
            if let Some(fields) = fields {
                pending = Some(
                    from_header(&fields, event, columns)
                        .with_context(|| format!("invalid perf script header line {line_no}"))?,
                );
            }
        } else if let Some(sample) = pending.as_mut() {
            ensure!(
                sample.stack.len() < limits.max_depth,
                "perf script exceeds sample stack depth limit"
            );
            push_frame(sample, line)
                .with_context(|| format!("invalid perf script line {line_no}"))?;
        } else {
            bail!("invalid perf script line {line_no}");
        }
    }
    if let Some(previous) = pending {
        finish(previous, &mut records)?;
    }
    Ok(records)
}
