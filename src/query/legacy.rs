// Keep the public JSON query interface; MCP uses typed queries without this adapter.
use serde::Serialize;
use serde_json::Value;

use super::{ApiError, DiffSort, FrameSelector, FrameWindow, MatchMode, Profile, TopSort, typed};

fn value(output: impl Serialize) -> Value {
    let mut value = serde_json::to_value(output).expect("query output serializes");
    for field in ["profile", "baseline", "candidate"] {
        if let Some(profile) = value.get_mut(field).and_then(Value::as_object_mut) {
            profile.remove("alias");
        }
    }
    value
}

pub fn summary(profile: &Profile) -> Value {
    let mut result = match typed::summary(profile) {
        Ok(output) => value(output),
        Err(error) => return serde_json::to_value(error).expect("API error serializes"),
    };
    result["data"]
        .as_object_mut()
        .expect("summary data")
        .remove("registry");
    result
}

pub fn find_symbols(
    profile: &Profile,
    query: &str,
    mode: MatchMode,
    limit: usize,
) -> Result<Value, ApiError> {
    typed::find_symbols(profile, query, mode, limit).map(value)
}

pub fn top(
    profile: &Profile,
    metric: TopSort,
    limit: usize,
    frame: Option<&FrameSelector>,
    name_regex: Option<&str>,
) -> Result<Value, ApiError> {
    typed::top(profile, metric, limit, frame, name_regex).map(value)
}

pub fn diff(
    baseline: &Profile,
    candidate: &Profile,
    metric: TopSort,
    sort: DiffSort,
    limit: usize,
    name_regex: Option<&str>,
) -> Result<Value, ApiError> {
    typed::diff(baseline, candidate, metric, sort, limit, name_regex).map(value)
}

fn direction_value(output: impl Serialize) -> Value {
    let mut result = value(output);
    for row in result["data"]["continuations"]
        .as_array_mut()
        .expect("continuation rows")
    {
        let row = row.as_object_mut().expect("continuation row");
        let mut token = row.remove("continuation").expect("continuation");
        let mut cursor = token["cursor"].take();
        if token["tool"] != "profile_tree" {
            cursor.as_array_mut().expect("direction cursor").remove(0);
            row.insert("node_path".into(), cursor);
        }
        row.insert("profile_fingerprint".into(), token["fingerprint"].take());
    }
    result
}

pub fn tree(
    profile: &Profile,
    root_node_id: u32,
    expected_fingerprint: Option<&str>,
    max_depth: usize,
    max_nodes: usize,
    min_scope_percent: f64,
) -> Result<Value, ApiError> {
    typed::tree(
        profile,
        root_node_id,
        expected_fingerprint,
        max_depth,
        max_nodes,
        min_scope_percent,
    )
    .map(direction_value)
}

pub fn callers(
    profile: &Profile,
    selector: &FrameSelector,
    max_depth: usize,
    max_nodes: usize,
    min_scope_percent: f64,
    continuation: Option<(&[u32], &str)>,
) -> Result<Value, ApiError> {
    typed::callers(
        profile,
        selector,
        max_depth,
        max_nodes,
        min_scope_percent,
        continuation,
    )
    .map(direction_value)
}

pub fn callees(
    profile: &Profile,
    selector: &FrameSelector,
    max_depth: usize,
    max_nodes: usize,
    min_scope_percent: f64,
    continuation: Option<(&[u32], &str)>,
) -> Result<Value, ApiError> {
    typed::callees(
        profile,
        selector,
        max_depth,
        max_nodes,
        min_scope_percent,
        continuation,
    )
    .map(direction_value)
}

pub fn paths(profile: &Profile, selector: &FrameSelector, limit: usize) -> Result<Value, ApiError> {
    paths_with_window(profile, selector, limit, None)
}

pub fn paths_with_window(
    profile: &Profile,
    selector: &FrameSelector,
    limit: usize,
    window: Option<FrameWindow>,
) -> Result<Value, ApiError> {
    paths_with_window_budget(
        profile,
        selector,
        limit,
        window,
        super::DEFAULT_MAX_TOTAL_FRAMES,
    )
}

pub fn paths_with_window_budget(
    profile: &Profile,
    selector: &FrameSelector,
    limit: usize,
    window: Option<FrameWindow>,
    max_total_frames: usize,
) -> Result<Value, ApiError> {
    let mut result = value(typed::paths_with_window_budget(
        profile,
        selector,
        limit,
        window,
        max_total_frames,
    )?);
    for row in result["data"]["paths"].as_array_mut().expect("path rows") {
        let row = row.as_object_mut().expect("path row");
        for (before, after) in [
            ("omitted_before", "omitted_after"),
            ("budget_omitted_before", "budget_omitted_after"),
            ("requested_frame_start", "requested_frame_end"),
        ] {
            if row[before] == 0 && row[after] == 0 {
                row.remove(before);
                row.remove(after);
            }
        }
    }
    Ok(result)
}
