use std::{hint::black_box, path::PathBuf, time::Instant};

use prof_mcp::{
    profile::{BuildLimits, ProfileBuilder},
    query::{self, DiffSort, FrameSelector, MatchMode, TopSort},
};
use serde_json::{Value, json};

fn json_value(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap()
}

fn measure(name: &str, repeats: usize, mut query: impl FnMut() -> Value) {
    let mut compute = 0.0;
    let mut encode = 0.0;
    let mut bytes = 0;
    for _ in 0..repeats {
        let started = Instant::now();
        let value = black_box(query());
        compute += started.elapsed().as_secs_f64();
        let started = Instant::now();
        let encoded = black_box(serde_json::to_vec(&value).unwrap());
        encode += started.elapsed().as_secs_f64();
        bytes = encoded.len();
    }
    println!(
        "{}",
        json!({"operation":name,"compute_shape_seconds":compute / repeats as f64,"encode_seconds":encode / repeats as f64,"response_bytes":bytes})
    );
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().expect("folded path"));
    let repeats: usize = args.next().expect("repeats").parse()?;
    anyhow::ensure!(repeats > 0, "repeats must be positive");
    let started = Instant::now();
    let profile = ProfileBuilder::new(BuildLimits::default()).from_file(path)?;
    let seconds = started.elapsed().as_secs_f64();
    let memory = profile.memory_breakdown();
    println!(
        "{}",
        json!({"operation":"build","seconds":seconds,"estimated_model_bytes":memory.total_bytes,
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
        json_value(query::tree(&profile, 0, None, 4, 64, 0.1).unwrap())
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
    let candidate_path = PathBuf::from(args.next().expect("candidate folded path"));
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
