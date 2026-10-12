use std::{hint::black_box, path::PathBuf, time::Instant};

use prof_mcp::{
    profile::{BuildLimits, ProfileBuilder},
    query::{self, DiffSort, FrameSelector, MatchMode, TopSort},
};
use serde_json::{Value, json};

#[path = "support/allocation.rs"]
mod allocation;
#[global_allocator]
static ALLOCATOR: allocation::Allocator = allocation::Allocator;

fn json_value(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap()
}

fn result_value<T: serde::Serialize, E: serde::Serialize>(value: Result<T, E>) -> Value {
    match value {
        Ok(output) => json_value(output),
        Err(error) => json!({"isError":true,"error":error}),
    }
}

fn measure(name: &str, repeats: usize, mut query: impl FnMut() -> Value) {
    let mut compute = 0.0;
    let mut encode = 0.0;
    let mut bytes = 0;
    let mut is_error = false;
    let mut peak = 0;
    let mut allocations = 0;
    let mut retained = 0;
    let mut cold_seconds = 0.0;
    let mut cold_peak = 0;
    let mut warm_peak = 0;
    for index in 0..repeats {
        let allocation_start = allocation::begin();
        let started = Instant::now();
        let value = black_box(query());
        let elapsed = started.elapsed().as_secs_f64();
        compute += elapsed;
        if index == 0 {
            cold_seconds = elapsed;
        }
        let started = Instant::now();
        let encoded = black_box(serde_json::to_vec(&value).unwrap());
        encode += started.elapsed().as_secs_f64();
        bytes = encoded.len();
        is_error = value
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let measured = allocation::end(allocation_start);
        peak = peak.max(measured.0);
        if index == 0 {
            cold_peak = measured.0;
        } else {
            warm_peak = warm_peak.max(measured.0);
        }
        allocations += measured.1;
        drop(encoded);
        drop(value);
        retained = retained.max(allocation::end(allocation_start).2);
    }
    println!(
        "{}",
        json!({"operation":name,"compute_shape_seconds":compute / repeats as f64,"encode_seconds":encode / repeats as f64,"response_bytes":bytes,"isError":is_error,"peak_allocated_bytes":peak,"allocation_calls":allocations / repeats,"retained_after_query_bytes":retained,"cold_compute_shape_seconds":cold_seconds,"warm_compute_shape_seconds":(repeats > 1).then(|| (compute - cold_seconds) / (repeats - 1) as f64),"cold_peak_allocated_bytes":cold_peak,"warm_peak_allocated_bytes":warm_peak})
    );
}

fn measure_typed<T, E>(name: &str, repeats: usize, mut query: impl FnMut() -> Result<T, E>) {
    let mut seconds = 0.0;
    let mut cold_seconds = 0.0;
    let mut cold_peak = 0;
    let mut warm_peak = 0;
    let mut calls = 0;
    let mut is_error = false;
    for index in 0..repeats {
        let allocation_start = allocation::begin();
        let started = Instant::now();
        let value = black_box(query());
        let elapsed = started.elapsed().as_secs_f64();
        seconds += elapsed;
        let measured = allocation::end(allocation_start);
        calls += measured.1;
        if index == 0 {
            cold_seconds = elapsed;
            cold_peak = measured.0;
        } else {
            warm_peak = warm_peak.max(measured.0);
        }
        is_error = value.is_err();
        drop(value);
    }
    println!(
        "{}",
        json!({"operation":name,"typed_compute_seconds":seconds / repeats as f64,
        "cold_typed_compute_seconds":cold_seconds,"warm_typed_compute_seconds":(repeats > 1).then(|| (seconds - cold_seconds) / (repeats - 1) as f64),
        "cold_peak_allocated_bytes":cold_peak,"warm_peak_allocated_bytes":warm_peak,"allocation_calls":calls / repeats,"isError":is_error})
    );
}

fn rss_values(status: &str) -> Option<(usize, usize)> {
    let field = |name: &str| {
        status.lines().find_map(|line| {
            line.strip_prefix(name)?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
    };
    Some((field("VmRSS:")?, field("VmHWM:")?))
}

fn query_rss(name: &str, warm: bool, mut query: impl FnMut() -> Value) {
    if warm {
        drop(black_box(query()));
    }
    let before = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|text| rss_values(&text));
    let reset = std::fs::write("/proc/self/clear_refs", "5");
    let allocation_start = allocation::begin();
    let started = Instant::now();
    let value = black_box(query());
    let encoded = black_box(serde_json::to_vec(&value).unwrap());
    let seconds = started.elapsed().as_secs_f64();
    let measured = allocation::end(allocation_start);
    let after = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|text| rss_values(&text));
    let growth = if reset.is_ok() {
        before
            .zip(after)
            .map(|(start, end)| end.1.saturating_sub(start.0))
    } else {
        None
    };
    println!(
        "{}",
        json!({"operation":name,"warm":warm,"query_seconds":seconds,
        "hwm_reset":reset.is_ok(),"hwm_reset_error":reset.err().map(|error| error.to_string()),
        "loaded_rss_kib":before.map(|value|value.0),"post_query_hwm_kib":after.map(|value|value.1),
        "query_rss_growth_kib":growth,"peak_allocated_bytes":measured.0,"allocation_calls":measured.1,
        "response_bytes":encoded.len(),"isError":value.get("isError").and_then(Value::as_bool).unwrap_or(false)})
    );
}

#[cfg(test)]
mod measurement_tests {
    #[test]
    fn rss_status_fields() {
        assert_eq!(
            super::rss_values("Name:	test\nVmHWM:	42 kB\nVmRSS:	35 kB\n"),
            Some((35, 42))
        );
        assert_eq!(super::rss_values("VmRSS:	35 kB\n"), None);
    }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().expect("folded path"));
    let repeats: usize = args.next().expect("repeats").parse()?;
    anyhow::ensure!(repeats > 0, "repeats must be positive");
    let mode = args.next();
    let allocation_start = allocation::begin();
    let started = Instant::now();
    let profile = ProfileBuilder::new(BuildLimits::default()).from_file(path)?;
    let seconds = started.elapsed().as_secs_f64();
    let build_allocation = allocation::end(allocation_start);
    let memory = profile.memory_breakdown();
    println!(
        "{}",
        json!({"operation":"build","seconds":seconds,"cct_physical_nodes":profile.cct().nodes().len(),"total_weight":profile.total_weight(),"max_depth":profile.max_depth(),"estimated_model_bytes":memory.total_bytes,"peak_allocated_bytes":build_allocation.0,"allocation_calls":build_allocation.1,"retained_allocated_bytes":build_allocation.2,
        "memory_breakdown":{
            "frame_name_bytes":memory.frame_name_bytes,"frame_offsets_bytes":memory.frame_offsets_bytes,
            "stack_offsets_bytes":memory.stack_offsets_bytes,
            "stack_frames_bytes":memory.stack_frames_bytes,"stack_weights_bytes":memory.stack_weights_bytes,
            "frame_stats_bytes":memory.frame_stats_bytes,"posting_offsets_bytes":memory.posting_offsets_bytes,
            "posting_entries_bytes":memory.posting_entries_bytes,"posting_positions_bytes":memory.posting_positions_bytes,"cct_nodes_bytes":memory.cct_nodes_bytes,
            "cct_offsets_bytes":memory.cct_offsets_bytes,"cct_edges_bytes":memory.cct_edges_bytes,
            "top_rankings_bytes":memory.top_rankings_bytes,"recursion_bytes":memory.recursion_bytes,
            "metadata_bytes":memory.metadata_bytes,"total_bytes":memory.total_bytes
        }})
    );
    if mode.as_deref() == Some("--build-only") {
        return Ok(());
    }
    if matches!(mode.as_deref(), Some("--query-rss" | "--query-rss-warm")) {
        let workload: Value = serde_json::from_reader(std::fs::File::open(
            args.next().expect("selector JSON path"),
        )?)?;
        let operation = args.next().expect("query operation");
        let mut workspace = query::TopWorkspace::default();
        let (tool, kind) = operation.split_once('_').unwrap_or((&operation, ""));
        let selector = workload
            .get(kind)
            .and_then(Value::as_str)
            .map(|name| FrameSelector {
                frame_name: Some(name.into()),
                ..Default::default()
            });
        anyhow::ensure!(
            kind.is_empty() || selector.is_some(),
            "unknown selector kind"
        );
        anyhow::ensure!(
            matches!(tool, "top" | "callers" | "callees" | "tree"),
            "unsupported RSS query tool"
        );
        query_rss(
            &operation,
            mode.as_deref() == Some("--query-rss-warm"),
            || match tool {
                "top" => {
                    let value = query::top_with_workspace(
                        &profile,
                        TopSort::SelfWeight,
                        20,
                        selector.as_ref(),
                        None,
                        &mut workspace,
                    );
                    workspace.release_oversized();
                    result_value(value)
                }
                "callers" => result_value(query::callers(
                    &profile,
                    selector.as_ref().expect("direction selector"),
                    4,
                    64,
                    0.0,
                    None,
                )),
                "callees" => result_value(query::callees(
                    &profile,
                    selector.as_ref().expect("direction selector"),
                    4,
                    64,
                    0.0,
                    None,
                )),
                _ => result_value(query::tree(&profile, [0, 0], None, 4, 64, 0.0)),
            },
        );
        return Ok(());
    }
    if mode.as_deref() == Some("--single-model") {
        let workload: Value = serde_json::from_reader(std::fs::File::open(
            args.next().expect("selector JSON path"),
        )?)?;
        measure("summary", repeats, || {
            result_value(query::summary(&profile))
        });
        measure("find_symbols", repeats, || {
            result_value(query::find_symbols(&profile, "", MatchMode::Contains, 20))
        });
        measure("top", repeats, || {
            result_value(query::top(&profile, TopSort::SelfWeight, 20, None, None))
        });
        for (depth, nodes) in [(1, 8), (4, 64), (16, 64)] {
            measure(&format!("tree_{depth}_{nodes}"), repeats, || {
                result_value(query::tree(&profile, [0, 0], None, depth, nodes, 0.0))
            });
        }
        let mut typed_workspace = query::TopWorkspace::default();
        let mut shaped_workspace = query::TopWorkspace::default();
        for (kind, name) in workload.as_object().expect("selector object") {
            let selector = FrameSelector {
                frame_name: Some(name.as_str().expect("frame name").into()),
                ..Default::default()
            };
            measure(&format!("top_{kind}"), repeats, || {
                result_value(query::top(
                    &profile,
                    TopSort::SelfWeight,
                    20,
                    Some(&selector),
                    None,
                ))
            });
            measure_typed(&format!("top_typed_{kind}"), repeats, || {
                let value = query::top_with_workspace(
                    &profile,
                    TopSort::SelfWeight,
                    20,
                    Some(&selector),
                    None,
                    &mut typed_workspace,
                );
                typed_workspace.release_oversized();
                value
            });
            measure(&format!("top_reused_{kind}"), repeats, || {
                let value = query::top_with_workspace(
                    &profile,
                    TopSort::SelfWeight,
                    20,
                    Some(&selector),
                    None,
                    &mut shaped_workspace,
                );
                shaped_workspace.release_oversized();
                result_value(value)
            });
            println!(
                "{}",
                json!({"operation":format!("top_workspace_{kind}"),"typed_retained_bytes":typed_workspace.retained_bytes(),"shaped_retained_bytes":shaped_workspace.retained_bytes()})
            );
            measure_typed(&format!("callers_typed_{kind}"), repeats, || {
                query::callers(&profile, &selector, 4, 64, 0.0, None)
            });
            measure_typed(&format!("callees_typed_{kind}"), repeats, || {
                query::callees(&profile, &selector, 4, 64, 0.0, None)
            });
            measure(&format!("callers_{kind}"), repeats, || {
                result_value(query::callers(&profile, &selector, 4, 64, 0.0, None))
            });
            measure(&format!("callees_{kind}"), repeats, || {
                result_value(query::callees(&profile, &selector, 4, 64, 0.0, None))
            });
            measure(&format!("paths_{kind}"), repeats, || {
                result_value(query::paths_with_window_budget(
                    &profile, &selector, 10, None, 500,
                ))
            });
        }
        measure("diff", repeats, || {
            result_value(query::diff(
                &profile,
                &profile,
                TopSort::SelfWeight,
                DiffSort::Absolute,
                20,
                None,
            ))
        });
        return Ok(());
    }
    let focus = FrameSelector {
        frame_name: Some("focus".into()),
        ..Default::default()
    };
    let leaf = FrameSelector {
        frame_name: Some("work42".into()),
        ..Default::default()
    };
    measure("summary", repeats, || {
        json_value(query::summary(&profile).unwrap())
    });
    measure("find_symbols", repeats, || {
        json_value(query::find_symbols(&profile, "work", MatchMode::Contains, 20).unwrap())
    });
    measure("top", repeats, || {
        json_value(query::top(&profile, TopSort::SelfWeight, 20, None, None).unwrap())
    });
    measure("tree", repeats, || {
        json_value(query::tree(&profile, [0, 0], None, 4, 64, 0.1).unwrap())
    });
    measure("callers", repeats, || {
        json_value(query::callers(&profile, &leaf, 4, 64, 0.1, None).unwrap())
    });
    measure("callees", repeats, || {
        json_value(query::callees(&profile, &focus, 4, 64, 0.1, None).unwrap())
    });
    measure("paths", repeats, || {
        json_value(
            prof_mcp::query::paths_with_window_budget(
                &profile,
                &leaf,
                10,
                None,
                prof_mcp::query::DEFAULT_MAX_TOTAL_FRAMES,
            )
            .unwrap(),
        )
    });
    measure("diff", repeats, || {
        json_value(
            query::diff(
                &profile,
                &profile,
                TopSort::SelfWeight,
                DiffSort::Absolute,
                20,
                None,
            )
            .unwrap(),
        )
    });
    let candidate_path = PathBuf::from(mode.expect("candidate folded path"));
    let candidate = ProfileBuilder::new(BuildLimits::default()).from_file(candidate_path)?;
    let dense = FrameSelector {
        frame_name: Some("scope_dense".into()),
        ..Default::default()
    };
    let root = FrameSelector {
        frame_name: Some("root".into()),
        ..Default::default()
    };
    measure("top_scoped_dense", repeats, || {
        json_value(query::top(&candidate, TopSort::SelfWeight, 20, Some(&dense), None).unwrap())
    });
    measure("top_scoped_sparse", repeats, || {
        json_value(query::top(&profile, TopSort::SelfWeight, 20, Some(&leaf), None).unwrap())
    });
    measure("paths_heap", repeats, || {
        json_value(query::paths_with_window_budget(&profile, &root, 10, None, 500).unwrap())
    });
    measure("diff_changed", repeats, || {
        json_value(
            query::diff(
                &profile,
                &candidate,
                TopSort::SelfWeight,
                DiffSort::Absolute,
                20,
                None,
            )
            .unwrap(),
        )
    });
    Ok(())
}
