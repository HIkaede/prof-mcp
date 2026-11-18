use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{BufReader, Cursor, Read, Write},
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    config::Config,
    profile::{BuildLimits, ProfileBuilder, StackId},
    query::{
        engine::{FactWeight, MetricScope, QueryPlan, QueryResult, query_weighted},
        selection::{Metric, Predicate, Selection},
    },
};

#[cfg(target_os = "linux")]
mod perf;
#[cfg(target_os = "linux")]
pub use perf::import_perf;

#[cfg(not(target_os = "linux"))]
pub fn import_perf(_: &Path, _: &Path, _: &Config) -> Result<()> {
    bail!("perf.data sample import is only supported on Linux")
}

const VERSION: u32 = 1;
const MAX_SNAPSHOT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_RECORDS: usize = 250_000;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SampleSnapshot {
    version: u32,
    source: Option<PerfSource>,
    event: String,
    records: Vec<SampleRecord>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PerfSource {
    path: String,
    blake3: String,
    perf_version: String,
    event_attributes: String,
    recording_header: String,
    record_stats: Option<String>,
    loss: Option<PerfLoss>,
    buildids: String,
    script_fields: String,
    symbolization: String,
    callchain: bool,
    max_stack: usize,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SampleRecord {
    comm: Option<String>,
    pid: Option<u32>,
    tid: Option<u32>,
    cpu: Option<u32>,
    timestamp_ns: Option<u64>,
    period: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sampled_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stack_source: Option<StackSource>,
    stack: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum StackSource {
    SampledIpOnly,
    PerfCallchain,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PerfLoss {
    lost_record_notifications: u64,
    lost_samples: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ThreadFilter {
    pid: u32,
    tid: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SampleQuery {
    snapshot_id: Option<String>,
    metric: Metric,
    event: Option<String>,
    start_ns: Option<u64>,
    end_ns: Option<u64>,
    thread: Option<ThreadFilter>,
    predicate: Option<Predicate>,
    plan: QueryPlan,
}

#[derive(Debug, Serialize)]
pub struct SampleQueryResult {
    #[serde(flatten)]
    projection: QueryResult,
    sample: SampleInfo,
    slice: SampleSlice,
}

#[derive(Debug, Serialize)]
struct SampleSlice {
    start_ns: Option<u64>,
    end_ns: Option<u64>,
    thread: Option<ThreadFilter>,
}

#[derive(Debug, Serialize)]
struct SampleInfo {
    event: String,
    records: usize,
    thread_identity: Option<&'static str>,
    timestamp_bounds_ns: Option<[u64; 2]>,
    timestamp: Dimension,
    thread: Dimension,
    cpu: Dimension,
    period: Dimension,
    stack_completeness: &'static str,
    perf_reported_loss: Option<PerfLoss>,
    perf_callchain_records: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Availability {
    Unknown,
    Absent,
    Partial,
    Available,
}

#[derive(Debug, Serialize)]
struct Dimension {
    source: Availability,
    retained: Availability,
    queryable: bool,
}

impl SampleSnapshot {
    fn validate(&self, limits: BuildLimits) -> Result<()> {
        ensure!(
            self.version == VERSION,
            "unsupported sample snapshot version"
        );
        ensure!(
            !self.event.is_empty()
                && self.event.len() <= 1024
                && !self.event.chars().any(char::is_whitespace),
            "sample snapshot must identify one event stream"
        );
        ensure!(
            (1..=MAX_RECORDS).contains(&self.records.len()),
            "sample snapshot must contain 1 to {MAX_RECORDS} records"
        );
        for record in &self.records {
            ensure!(
                (1..=limits.max_depth).contains(&record.stack.len()),
                "invalid sample stack depth"
            );
            for frame in &record.stack {
                ensure!(
                    !frame.is_empty()
                        && frame.trim_matches(|ch: char| ch.is_ascii_whitespace() || ch == '\u{b}')
                            == frame
                        && !frame.contains([';', '\r', '\n']),
                    "sample frames must be normalized folded labels"
                );
            }
        }
        Ok(())
    }

    fn info(&self) -> SampleInfo {
        let dimension = |present, supported| {
            let retained = match present {
                0 => Availability::Absent,
                value if value == self.records.len() => Availability::Available,
                _ => Availability::Partial,
            };
            Dimension {
                source: if self.source.is_some() {
                    retained
                } else {
                    Availability::Unknown
                },
                retained,
                queryable: supported && retained == Availability::Available,
            }
        };
        let timestamp = dimension(
            self.records
                .iter()
                .filter(|row| row.timestamp_ns.is_some())
                .count(),
            true,
        );
        let thread = dimension(
            self.records
                .iter()
                .filter(|row| row.pid.is_some() && row.tid.is_some())
                .count(),
            true,
        );
        let timestamp_bounds_ns = timestamp.queryable.then(|| {
            self.records
                .iter()
                .filter_map(|row| row.timestamp_ns)
                .fold([u64::MAX, 0], |[first, last], time| {
                    [first.min(time), last.max(time)]
                })
        });
        SampleInfo {
            event: self.event.clone(),
            records: self.records.len(),
            thread_identity: (thread.retained != Availability::Absent)
                .then_some("recording_scoped_pid_tid"),
            timestamp_bounds_ns,
            timestamp,
            thread,
            cpu: dimension(
                self.records.iter().filter(|row| row.cpu.is_some()).count(),
                false,
            ),
            period: dimension(
                self.records
                    .iter()
                    .filter(|row| row.period.is_some())
                    .count(),
                true,
            ),
            stack_completeness: "unknown",
            perf_reported_loss: self.source.as_ref().and_then(|source| source.loss),
            perf_callchain_records: self.source.as_ref().map(|_| {
                self.records
                    .iter()
                    .filter(|row| matches!(row.stack_source, Some(StackSource::PerfCallchain)))
                    .count()
            }),
        }
    }
}

fn snapshot_limit(config: &Config) -> u64 {
    MAX_SNAPSHOT_BYTES.min(config.max_file_size_bytes())
}

pub fn query_file(
    input: &Path,
    mut request: SampleQuery,
    config: &Config,
) -> Result<SampleQueryResult> {
    let limit = snapshot_limit(config);
    let metadata = fs::metadata(input).context("could not inspect sample snapshot")?;
    ensure!(metadata.is_file(), "sample snapshot must be a regular file");
    ensure!(
        metadata.len() <= limit,
        "sample snapshot exceeds input byte limit"
    );
    let file = File::open(input).context("could not open sample snapshot")?;
    let mut bytes = Vec::new();
    BufReader::new(file)
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .context("could not read sample snapshot")?;
    ensure!(
        bytes.len() as u64 <= limit,
        "sample snapshot exceeds input byte limit"
    );
    let snapshot: SampleSnapshot =
        serde_json::from_slice(&bytes).context("invalid sample snapshot")?;
    let fingerprint = blake3::hash(&bytes).to_hex().to_string();
    let mut snapshot_hash = blake3::Hasher::new_derive_key("prof-mcp/sample-records-v1");
    drop(bytes);
    serde_json::to_writer(&mut snapshot_hash, &snapshot)
        .context("could not hash sample snapshot")?;
    let limits = super::build_limits(config);
    snapshot.validate(limits)?;
    let info = snapshot.info();

    ensure!(
        request
            .event
            .as_ref()
            .is_none_or(|event| event == &snapshot.event),
        "event is not in this single-stream sample snapshot"
    );
    if let (Some(start), Some(end)) = (request.start_ns, request.end_ns) {
        ensure!(
            start <= end,
            "sample time range must have start_ns <= end_ns"
        );
    }
    if (request.start_ns.is_some() || request.end_ns.is_some()) && !info.timestamp.queryable {
        bail!("capability_unavailable: time is missing from some sample records");
    }
    if request.thread.is_some() && !info.thread.queryable {
        bail!("capability_unavailable: PID or TID is missing from some sample records");
    }
    ensure!(
        request.plan.metric == Metric::OpaqueWeight || request.plan.metric == request.metric,
        "sample metric conflicts with projection metric"
    );
    match request.metric {
        Metric::OpaqueWeight => bail!("sample queries require sample_count or sample_period"),
        Metric::SamplePeriod if !info.period.queryable => {
            bail!("capability_unavailable: period is missing from some sample records");
        }
        _ => {}
    }

    let mut stacks = BTreeMap::<&[String], (u64, FactWeight)>::new();
    let mut profile_weight = 0_u64;
    for record in &snapshot.records {
        let weight = match request.metric {
            Metric::SamplePeriod => record.period.expect("period availability checked"),
            _ => 1,
        };
        profile_weight = profile_weight
            .checked_add(weight)
            .filter(|total| *total <= limits.max_total_weight)
            .context("sample total weight exceeds maximum")?;
        let (full_count, selected) = stacks.entry(&record.stack).or_insert((
            0,
            FactWeight {
                weight: 0,
                fact_count: 0,
            },
        ));
        *full_count += 1;
        if request
            .start_ns
            .is_none_or(|start| record.timestamp_ns >= Some(start))
            && request
                .end_ns
                .is_none_or(|end| record.timestamp_ns < Some(end))
            && request.thread.as_ref().is_none_or(|thread| {
                record.pid == Some(thread.pid) && record.tid == Some(thread.tid)
            })
        {
            selected.weight += weight;
            selected.fact_count += 1;
        }
    }

    // The shared builder assigns the same lexical order to full root-to-leaf paths.
    // It receives positive sample counts so zero periods still have stack identities.
    let mut folded = Vec::new();
    for (labels, (full_count, _)) in &stacks {
        let mut labels = labels.iter();
        folded.extend(labels.next().expect("nonempty stack").as_bytes());
        for label in labels {
            folded.push(b';');
            folded.extend(label.as_bytes());
        }
        writeln!(&mut folded, " {full_count}")?;
    }
    let mut profile = ProfileBuilder::new(limits).from_reader(Cursor::new(folded), input.into())?;
    profile.source.fingerprint = fingerprint;
    let _ = profile
        .source
        .snapshot_id
        .set(snapshot_hash.finalize().to_hex().to_string());
    let predicate = request.predicate.unwrap_or_default();
    let selection = if let Some(snapshot_id) = request.snapshot_id {
        ensure!(
            snapshot_id == profile.snapshot_id(),
            "snapshot_mismatch: sample snapshot ID changed"
        );
        Selection::new(&profile, Predicate::default())?.refine(&profile, predicate)?
    } else {
        request.plan.refine = request.plan.alias_refinement(&profile)?;
        Selection::new(&profile, predicate)?
    };
    let weights: BTreeMap<StackId, FactWeight> = stacks
        .into_values()
        .enumerate()
        .filter_map(|(id, (_, selected))| {
            (selected.fact_count > 0).then_some((id as StackId, selected))
        })
        .collect();
    let slice = SampleSlice {
        start_ns: request.start_ns,
        end_ns: request.end_ns,
        thread: request.thread,
    };
    request.plan.metric = request.metric;
    let projection = query_weighted(
        &profile,
        selection,
        request.plan,
        MetricScope {
            profile_weight,
            event_stream_id: Some(snapshot.event),
            scope_key: Some(serde_json::to_string(&slice)?),
        },
        &weights,
    )?;
    Ok(SampleQueryResult {
        projection,
        sample: info,
        slice,
    })
}
