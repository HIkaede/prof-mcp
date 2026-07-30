use std::{cmp::Ordering, collections::BTreeMap};

use hashbrown::{HashMap, HashSet};
use regex::Regex;
use serde_json::{Value, json};

use crate::{
    error::ApiError,
    output::{
        SCHEMA_VERSION, envelope, frame_row, frame_row_with_percent_weight, percent, profile_meta,
    },
    profile::{ContextNode, FrameId, FrameStats, NodeId, Profile, StackRecord},
};

#[derive(Clone, Debug, Default)]
pub struct FrameSelector {
    pub frame_id: Option<u32>,
    pub frame_name: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopSort {
    SelfWeight,
    Inclusive,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffSort {
    Regression,
    Improvement,
    Absolute,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchMode {
    Contains,
    Regex,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameWindow {
    Head { lines: usize },
    Tail { lines: usize },
    AroundTarget { before: usize, after: usize },
}

pub const DEFAULT_MAX_TOTAL_FRAMES: usize = 500;
pub const MAX_TOTAL_FRAMES: usize = 5_000;

pub fn summary(profile: &Profile) -> Value {
    let mut ids: Vec<_> = (0..profile.frames.len() as u32).collect();
    ids.sort_by(|a, b| frame_order(profile, *a, *b, TopSort::SelfWeight));
    let top_self: Vec<_> = ids
        .iter()
        .take(5)
        .map(|id| {
            frame_row_with_percent_weight(
                profile,
                *id,
                &profile.frame_stats[*id as usize],
                profile.total_weight,
                profile.frame_stats[*id as usize].self_weight,
            )
        })
        .collect();
    ids.sort_by(|a, b| frame_order(profile, *a, *b, TopSort::Inclusive));
    let top_inclusive: Vec<_> = ids
        .iter()
        .take(5)
        .map(|id| {
            frame_row(
                profile,
                *id,
                &profile.frame_stats[*id as usize],
                profile.total_weight,
            )
        })
        .collect();
    let unknown_frame_weight = profile
        .frame_id("[unknown]")
        .map(|id| profile.frame_stats[id as usize].inclusive_weight)
        .unwrap_or(0);
    envelope(
        profile,
        profile.total_weight,
        Vec::new(),
        vec!["Weight unit is opaque; it is not assumed to be time or cycles.".into()],
        json!({"total_weight":profile.total_weight,"frame_count":profile.frames.len(),"unique_stack_count":profile.stacks.len(),"max_depth":profile.max_depth,"unknown_frame_weight":unknown_frame_weight,"top_self":top_self,"top_inclusive":top_inclusive}),
    )
}

pub fn find_symbols(
    profile: &Profile,
    query: &str,
    mode: MatchMode,
    limit: usize,
) -> Result<Value, ApiError> {
    check_limit(limit, 1, 100, "limit")?;
    let regex = match mode {
        MatchMode::Contains => None,
        MatchMode::Regex => Some(compile_regex(query)?),
    };
    let mut ids: Vec<_> = (0..profile.frames.len() as u32)
        .filter(|id| match &regex {
            Some(regex) => regex.is_match(profile.frame_name(*id)),
            None => profile.frame_name(*id).contains(query),
        })
        .collect();
    ids.sort_by(|a, b| frame_order(profile, *a, *b, TopSort::Inclusive));
    let available = ids.len();
    let truncation_reasons = row_limit_reason(limit, available);
    ids.truncate(limit);
    let warnings = if ids.is_empty() {
        vec!["No exact frame identities matched; try a broader contains query or profile_find_symbols regex.".into()]
    } else {
        Vec::new()
    };
    let rows: Vec<_> = ids
        .iter()
        .map(|id| {
            let row = frame_row(
                profile,
                *id,
                &profile.frame_stats[*id as usize],
                profile.total_weight,
            );
            let mut value = serde_json::to_value(row).expect("frame row serializes");
            value
                .as_object_mut()
                .expect("frame row is an object")
                .insert("context_hint".into(), symbol_context(profile, *id));
            value
        })
        .collect();
    Ok(envelope(
        profile,
        profile.total_weight,
        truncation_reasons,
        warnings,
        json!({"query":query,"mode":match mode {MatchMode::Contains=>"contains",MatchMode::Regex=>"regex"},"symbol_metadata":"folded_frames_and_observed_context_only","matches":rows}),
    ))
}

pub fn top(
    profile: &Profile,
    sort: TopSort,
    limit: usize,
    focus: Option<&FrameSelector>,
    name_regex: Option<&str>,
) -> Result<Value, ApiError> {
    check_limit(limit, 1, 200, "limit")?;
    let focused_id = focus
        .map(|selector| resolve_selector(profile, selector))
        .transpose()?;
    let stack_ids: Vec<_> = match focused_id {
        Some(id) => profile.frame_to_stacks[id as usize].clone(),
        None => (0..profile.stacks.len() as u32).collect(),
    };
    let scope_weight = stack_ids
        .iter()
        .map(|id| profile.stacks[*id as usize].weight)
        .sum();
    let stats = subset_stats(profile, &stack_ids);
    let regex = name_regex.map(compile_regex).transpose()?;
    let mut ids: Vec<_> = (0..profile.frames.len() as u32)
        .filter(|id| {
            stats[*id as usize].inclusive_weight > 0
                && regex
                    .as_ref()
                    .is_none_or(|re| re.is_match(profile.frame_name(*id)))
        })
        .collect();
    ids.sort_by(|a, b| frame_order_stats(profile, &stats, *a, *b, sort));
    let available = ids.len();
    let truncation_reasons = row_limit_reason(limit, available);
    ids.truncate(limit);
    let rows: Vec<_> = ids
        .iter()
        .map(|id| {
            frame_row_with_percent_weight(
                profile,
                *id,
                &stats[*id as usize],
                scope_weight,
                metric_weight(&stats[*id as usize], sort),
            )
        })
        .collect();
    Ok(envelope(
        profile,
        scope_weight,
        truncation_reasons,
        Vec::new(),
        json!({"sort":match sort {TopSort::SelfWeight=>"self",TopSort::Inclusive=>"inclusive"},"focus":focused_id,"rows":rows}),
    ))
}

pub fn tree(
    profile: &Profile,
    root_node_id: NodeId,
    expected_fingerprint: Option<&str>,
    max_depth: usize,
    max_nodes: usize,
    min_scope_percent: f64,
) -> Result<Value, ApiError> {
    check_budget(max_depth, max_nodes, min_scope_percent)?;
    if root_node_id != profile.cct.root {
        let expected = expected_fingerprint.ok_or_else(|| {
            ApiError::new(
                "profile_changed",
                "Non-root tree continuation requires profile_fingerprint",
                json!({"root_node_id":root_node_id}),
                "Pass the fingerprint returned by the previous profile_tree result.",
            )
        })?;
        if expected != profile.source.fingerprint {
            return Err(ApiError::new(
                "profile_changed",
                "Profile fingerprint no longer matches this tree continuation",
                json!({"expected_fingerprint":expected,"current_fingerprint":profile.source.fingerprint}),
                "Restart at root_node_id 0 after reloading the profile.",
            ));
        }
    }
    let root = profile
        .cct
        .nodes
        .get(root_node_id as usize)
        .ok_or_else(|| {
            ApiError::new(
                "invalid_node_id",
                format!("Unknown CCT node id: {root_node_id}"),
                json!({"root_node_id":root_node_id}),
                "Start with root_node_id 0 or use a node_id returned for this profile fingerprint.",
            )
        })?;
    let mut budget = max_nodes;
    let mut reason_stats = BTreeMap::new();
    let mut continuations = Vec::new();
    let mut continuation_count = 0usize;
    let mut render = RenderState {
        scope: root.total_weight,
        max_depth,
        min_percent: min_scope_percent,
        budget: &mut budget,
        reason_stats: &mut reason_stats,
        continuations: &mut continuations,
        continuation_count: &mut continuation_count,
        continuation_limit: 128,
    };
    let (node, _) = render_cct(profile, root_node_id, 0, &mut render);
    let continuations_truncated = continuation_count > continuations.len();
    let truncation_reasons =
        tree_reason_values(&reason_stats, max_depth, max_nodes, min_scope_percent);
    Ok(envelope(
        profile,
        root.total_weight,
        truncation_reasons,
        Vec::new(),
        json!({
            "root":node,
            "continuations":continuations,
            "continuations_truncated":continuations_truncated,
            "continuation_limit":128,
            "continuations_available":continuation_count,
            "continuations_omitted":continuation_count.saturating_sub(continuations.len())
        }),
    ))
}

pub fn callers(
    profile: &Profile,
    selector: &FrameSelector,
    max_depth: usize,
    max_nodes: usize,
    min_scope_percent: f64,
) -> Result<Value, ApiError> {
    directional(
        profile,
        selector,
        max_depth,
        max_nodes,
        min_scope_percent,
        true,
    )
}
pub fn callees(
    profile: &Profile,
    selector: &FrameSelector,
    max_depth: usize,
    max_nodes: usize,
    min_scope_percent: f64,
) -> Result<Value, ApiError> {
    directional(
        profile,
        selector,
        max_depth,
        max_nodes,
        min_scope_percent,
        false,
    )
}

fn directional(
    profile: &Profile,
    selector: &FrameSelector,
    max_depth: usize,
    max_nodes: usize,
    min_scope_percent: f64,
    callers: bool,
) -> Result<Value, ApiError> {
    check_budget(max_depth, max_nodes, min_scope_percent)?;
    let frame = resolve_selector(profile, selector)?;
    let mut root = TempNode::new(frame);
    let mut scope = 0;
    for stack_id in &profile.frame_to_stacks[frame as usize] {
        let stack = &profile.stacks[*stack_id as usize];
        let positions: Vec<_> = stack
            .frames
            .iter()
            .enumerate()
            .filter_map(|(index, id)| (*id == frame).then_some(index))
            .collect();
        let anchor = if callers {
            *positions.last().expect("index points to frame")
        } else {
            positions[0]
        };
        scope += stack.weight;
        root.total_weight += stack.weight;
        let walk: Vec<_> = if callers {
            (0..anchor).rev().map(|index| stack.frames[index]).collect()
        } else {
            ((anchor + 1)..stack.frames.len())
                .map(|index| stack.frames[index])
                .collect()
        };
        if walk.is_empty() {
            root.self_weight += stack.weight;
        } else {
            root.insert(&walk, stack.weight);
        }
    }
    let mut budget = max_nodes;
    let mut reason_stats = BTreeMap::new();
    let mut continuations = Vec::new();
    let mut continuation_count = 0usize;
    let mut render = RenderState {
        scope,
        max_depth,
        min_percent: min_scope_percent,
        budget: &mut budget,
        reason_stats: &mut reason_stats,
        continuations: &mut continuations,
        continuation_count: &mut continuation_count,
        continuation_limit: 0,
    };
    let (node, _) = render_temp(profile, &root, 0, &mut render);
    let truncation_reasons =
        tree_reason_values(&reason_stats, max_depth, max_nodes, min_scope_percent);
    Ok(envelope(
        profile,
        scope,
        truncation_reasons,
        Vec::new(),
        json!({"frame":frame_row(profile, frame, &profile.frame_stats[frame as usize], scope),"root":node}),
    ))
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
    paths_with_window_budget(profile, selector, limit, window, DEFAULT_MAX_TOTAL_FRAMES)
}

pub fn paths_with_window_budget(
    profile: &Profile,
    selector: &FrameSelector,
    limit: usize,
    window: Option<FrameWindow>,
    max_total_frames: usize,
) -> Result<Value, ApiError> {
    check_limit(limit, 1, 50, "limit")?;
    check_frame_window(window)?;
    check_limit(max_total_frames, 1, MAX_TOTAL_FRAMES, "max_total_frames")?;
    let frame = resolve_selector(profile, selector)?;
    let stack_ids = &profile.frame_to_stacks[frame as usize];
    let scope: u64 = stack_ids
        .iter()
        .map(|id| profile.stacks[*id as usize].weight)
        .sum();
    let mut stacks: Vec<&StackRecord> = stack_ids
        .iter()
        .map(|id| &profile.stacks[*id as usize])
        .collect();
    stacks.sort_by(|a, b| {
        b.weight.cmp(&a.weight).then_with(|| {
            frame_sequence(profile, &a.frames).cmp(&frame_sequence(profile, &b.frames))
        })
    });
    let available = stacks.len();
    let mut truncation_reasons = row_limit_reason(limit, available);
    stacks.truncate(limit);
    let selected_paths = stacks.len();
    let mut window_cropped_paths = 0usize;
    let mut window_omitted_before = 0usize;
    let mut window_omitted_after = 0usize;
    let mut budget_remaining = max_total_frames;
    let mut budget_available = 0usize;
    let mut budget_returned = 0usize;
    let mut budget_cropped_paths = 0usize;
    let mut rows = Vec::new();
    for stack in stacks {
        let positions: Vec<_> = stack
            .frames
            .iter()
            .enumerate()
            .filter_map(|(index, id)| (*id == frame).then_some(index))
            .collect();
        let total_depth = stack.frames.len();
        let (requested_frame_start, requested_frame_end) =
            frame_window_range(total_depth, &positions, window);
        let requested_frames = requested_frame_end - requested_frame_start;
        budget_available += requested_frames;
        if requested_frame_start != 0 || requested_frame_end != total_depth {
            window_cropped_paths += 1;
            window_omitted_before += requested_frame_start;
            window_omitted_after += total_depth - requested_frame_end;
        }
        if budget_remaining == 0 {
            continue;
        }
        let (frame_start, frame_end) = if requested_frames <= budget_remaining {
            (requested_frame_start, requested_frame_end)
        } else {
            budget_cropped_paths += 1;
            budget_range(
                requested_frame_start,
                requested_frame_end,
                &positions,
                window,
                budget_remaining,
            )
        };
        let returned_frames = frame_end - frame_start;
        budget_remaining -= returned_frames;
        budget_returned += returned_frames;
        let display_target_positions: Vec<_> = positions
            .iter()
            .filter(|position| **position >= frame_start && **position < frame_end)
            .map(|position| *position - frame_start)
            .collect();
        rows.push(json!({
            "frames":frame_sequence(profile,&stack.frames[frame_start..frame_end]),
            "weight":stack.weight,
            "profile_percent":percent(stack.weight,profile.total_weight),
            "scope_percent":percent(stack.weight,scope),
            "target_positions":positions,
            "display_target_positions":display_target_positions,
            "total_depth":total_depth,
            "requested_frame_start":requested_frame_start,
            "requested_frame_end":requested_frame_end,
            "frame_start":frame_start,
            "frame_end":frame_end,
            "omitted_before":frame_start,
            "omitted_after":total_depth-frame_end,
            "budget_omitted_before":frame_start-requested_frame_start,
            "budget_omitted_after":requested_frame_end-frame_end,
        }));
    }
    if window_cropped_paths > 0 {
        truncation_reasons.push(json!({
            "kind":"frame_window",
            "mode":match window {
                Some(FrameWindow::Head { .. })=>"head",
                Some(FrameWindow::Tail { .. })=>"tail",
                Some(FrameWindow::AroundTarget { .. })=>"around_target",
                None=>"none"
            },
            "cropped_paths":window_cropped_paths,
            "total_omitted_before":window_omitted_before,
            "total_omitted_after":window_omitted_after
        }));
    }
    let budget_omitted = budget_available - budget_returned;
    let total_frame_budget = json!({
        "limit":max_total_frames,
        "available":budget_available,
        "returned":budget_returned,
        "omitted":budget_omitted,
        "selected_paths":selected_paths,
        "returned_paths":rows.len(),
        "omitted_paths":selected_paths-rows.len(),
        "cropped_paths":budget_cropped_paths,
    });
    if budget_omitted > 0 {
        let mut reason = total_frame_budget
            .as_object()
            .expect("static object")
            .clone();
        reason.insert("kind".into(), Value::String("total_frame_budget".into()));
        truncation_reasons.push(Value::Object(reason));
    }
    Ok(envelope(
        profile,
        scope,
        truncation_reasons,
        Vec::new(),
        json!({"through":frame,"paths":rows,"total_frame_budget":total_frame_budget}),
    ))
}

fn check_frame_window(window: Option<FrameWindow>) -> Result<(), ApiError> {
    match window {
        None => Ok(()),
        Some(FrameWindow::Head { lines }) | Some(FrameWindow::Tail { lines }) => {
            check_limit(lines, 1, 4096, "frame_window.lines")
        }
        Some(FrameWindow::AroundTarget { before, after }) => {
            if before > 4096 || after > 4096 || (before == 0 && after == 0) {
                return Err(ApiError::new(
                    "invalid_budget",
                    "frame_window around_target requires before/after <= 4096 and at least one non-zero value",
                    json!({"before":before,"after":after}),
                    "Use a bounded non-empty display window.",
                ));
            }
            Ok(())
        }
    }
}

fn frame_window_range(
    total_depth: usize,
    positions: &[usize],
    window: Option<FrameWindow>,
) -> (usize, usize) {
    match window {
        None => (0, total_depth),
        Some(FrameWindow::Head { lines }) => (0, lines.min(total_depth)),
        Some(FrameWindow::Tail { lines }) => (total_depth.saturating_sub(lines), total_depth),
        Some(FrameWindow::AroundTarget { before, after }) => {
            let first = *positions.first().expect("selected frame occurs in stack");
            let last = *positions.last().expect("selected frame occurs in stack");
            (
                first.saturating_sub(before),
                (last.saturating_add(after).saturating_add(1)).min(total_depth),
            )
        }
    }
}

fn budget_range(
    requested_start: usize,
    requested_end: usize,
    positions: &[usize],
    window: Option<FrameWindow>,
    remaining: usize,
) -> (usize, usize) {
    debug_assert!(remaining > 0);
    debug_assert!(requested_end - requested_start > remaining);
    match window {
        Some(FrameWindow::Head { .. }) => (requested_start, requested_start + remaining),
        Some(FrameWindow::Tail { .. }) => (requested_end - remaining, requested_end),
        None | Some(FrameWindow::AroundTarget { .. }) => {
            let anchor = *positions.first().expect("selected frame occurs in stack");
            let max_start = requested_end - remaining;
            let centered_start = anchor.saturating_sub(remaining / 2);
            let start = centered_start.clamp(requested_start, max_start);
            (start, start + remaining)
        }
    }
}

pub fn diff(
    baseline: &Profile,
    candidate: &Profile,
    metric: TopSort,
    sort: DiffSort,
    limit: usize,
    name_regex: Option<&str>,
) -> Result<Value, ApiError> {
    check_limit(limit, 1, 200, "limit")?;
    let regex = name_regex.map(compile_regex).transpose()?;
    let mut names: Vec<_> = baseline
        .frames
        .iter()
        .map(|f| f.name.to_string())
        .chain(candidate.frames.iter().map(|f| f.name.to_string()))
        .collect();
    names.sort();
    names.dedup();
    let mut rows: Vec<_> = names.into_iter().filter_map(|name| {
        if regex.as_ref().is_some_and(|re| !re.is_match(&name)) { return None; }
        let b = baseline.frame_id(&name).map(|id| &baseline.frame_stats[id as usize]); let c = candidate.frame_id(&name).map(|id| &candidate.frame_stats[id as usize]);
        let bw = b.map(|s| metric_weight(s, metric)).unwrap_or(0); let cw = c.map(|s| metric_weight(s, metric)).unwrap_or(0);
        let bp = percent(bw, baseline.total_weight); let cp = percent(cw, candidate.total_weight); let delta = cp - bp;
        Some(json!({"name":name,"baseline_weight":bw,"candidate_weight":cw,"baseline_percent":bp,"candidate_percent":cp,"delta_pp":delta}))
    }).collect::<Vec<Value>>();
    rows.sort_by(|a, b| {
        let ad = a["delta_pp"].as_f64().unwrap_or(0.0);
        let bd = b["delta_pp"].as_f64().unwrap_or(0.0);
        let primary = match sort {
            DiffSort::Regression => bd.total_cmp(&ad),
            DiffSort::Improvement => ad.total_cmp(&bd),
            DiffSort::Absolute => bd.abs().total_cmp(&ad.abs()),
        };
        primary.then_with(|| a["name"].as_str().cmp(&b["name"].as_str()))
    });
    let available = rows.len();
    let truncation_reasons = row_limit_reason(limit, available);
    rows.truncate(limit);
    Ok(
        json!({"schema_version":SCHEMA_VERSION,"baseline":profile_meta(baseline),"candidate":profile_meta(candidate),"scope_weight":{"baseline":baseline.total_weight,"candidate":candidate.total_weight},"truncated":!truncation_reasons.is_empty(),"truncation_reasons":truncation_reasons,"warnings":["Percentage-point changes do not prove causality or statistical significance."],"data":{"metric":match metric {TopSort::SelfWeight=>"self",TopSort::Inclusive=>"inclusive"},"sort":match sort {DiffSort::Regression=>"regression",DiffSort::Improvement=>"improvement",DiffSort::Absolute=>"absolute"},"rows":rows}}),
    )
}

fn resolve_selector(profile: &Profile, selector: &FrameSelector) -> Result<FrameId, ApiError> {
    match (&selector.frame_id, &selector.frame_name) {
        (Some(_), Some(_)) | (None, None) => Err(ApiError::new(
            "invalid_frame_selector",
            "Frame selector must set exactly one of frame_id or frame_name",
            json!({}),
            "Use an exact frame_id from profile_find_symbols or one exact frame_name.",
        )),
        (Some(id), None) => {
            if (*id as usize) < profile.frames.len() {
                Ok(*id)
            } else {
                Err(ApiError::new(
                    "frame_not_found",
                    format!("Frame id not found: {id}"),
                    json!({"frame_id":id}),
                    "Call profile_find_symbols to discover valid frame ids.",
                ))
            }
        }
        (None, Some(name)) => profile.frame_id(name).ok_or_else(|| {
            ApiError::new(
                "frame_not_found",
                format!("Frame not found: {name}"),
                json!({"frame_name":name}),
                "Call profile_find_symbols to discover exact frame names.",
            )
        }),
    }
}
fn compile_regex(pattern: &str) -> Result<Regex, ApiError> {
    if pattern.len() > 4096 {
        return Err(ApiError::new(
            "invalid_regex",
            "Regex exceeds 4 KiB limit",
            json!({"length":pattern.len()}),
            "Use a shorter regex.",
        ));
    }
    Regex::new(pattern).map_err(|error| {
        ApiError::new(
            "invalid_regex",
            format!("Invalid regex: {error}"),
            json!({"pattern":pattern}),
            "Fix the regex syntax and retry.",
        )
    })
}
fn check_limit(value: usize, min: usize, max: usize, field: &str) -> Result<(), ApiError> {
    if value < min || value > max {
        Err(ApiError::new(
            "invalid_budget",
            format!("{field} must be between {min} and {max}"),
            json!({field:value,"min":min,"max":max}),
            "Use a documented output budget.",
        ))
    } else {
        Ok(())
    }
}
fn check_budget(depth: usize, nodes: usize, percent: f64) -> Result<(), ApiError> {
    check_limit(depth, 0, 16, "max_depth")?;
    check_limit(nodes, 1, 512, "max_nodes")?;
    if !(0.0..=100.0).contains(&percent) || !percent.is_finite() {
        Err(ApiError::new(
            "invalid_budget",
            "min_scope_percent must be finite and between 0 and 100",
            json!({"min_scope_percent":percent}),
            "Use a documented tree budget.",
        ))
    } else {
        Ok(())
    }
}

fn row_limit_reason(limit: usize, available: usize) -> Vec<Value> {
    if available > limit {
        vec![json!({
            "kind":"row_limit",
            "limit":limit,
            "returned":limit,
            "available":available,
            "omitted":available-limit
        })]
    } else {
        Vec::new()
    }
}

fn tree_reason_values(
    stats: &BTreeMap<&'static str, TruncationStats>,
    max_depth: usize,
    max_nodes: usize,
    min_scope_percent: f64,
) -> Vec<Value> {
    stats
        .iter()
        .map(|(kind, stats)| match *kind {
            "depth_limit" => json!({"kind":"depth_limit","max_depth":max_depth,"omitted_children":stats.count,"omitted_weight":stats.weight}),
            "node_budget" => json!({"kind":"node_budget","max_nodes":max_nodes,"omitted_children":stats.count,"omitted_weight":stats.weight}),
            "min_scope_percent" => {
                json!({"kind":"min_scope_percent","threshold":min_scope_percent,"omitted_children":stats.count,"omitted_weight":stats.weight})
            }
            _ => json!({"kind":kind,"omitted_children":stats.count,"omitted_weight":stats.weight}),
        })
        .collect()
}

fn frame_order(profile: &Profile, a: FrameId, b: FrameId, sort: TopSort) -> Ordering {
    frame_order_stats(profile, &profile.frame_stats, a, b, sort)
}
fn frame_order_stats(
    profile: &Profile,
    stats: &[FrameStats],
    a: FrameId,
    b: FrameId,
    sort: TopSort,
) -> Ordering {
    let aw = metric_weight(&stats[a as usize], sort);
    let bw = metric_weight(&stats[b as usize], sort);
    bw.cmp(&aw)
        .then_with(|| {
            stats[b as usize]
                .self_weight
                .cmp(&stats[a as usize].self_weight)
        })
        .then_with(|| profile.frame_name(a).cmp(profile.frame_name(b)))
        .then(a.cmp(&b))
}
fn metric_weight(stats: &FrameStats, sort: TopSort) -> u64 {
    match sort {
        TopSort::SelfWeight => stats.self_weight,
        TopSort::Inclusive => stats.inclusive_weight,
    }
}
fn subset_stats(profile: &Profile, stack_ids: &[u32]) -> Vec<FrameStats> {
    let mut result = vec![FrameStats::default(); profile.frames.len()];
    for stack_id in stack_ids {
        let stack = &profile.stacks[*stack_id as usize];
        let mut seen = HashSet::new();
        for frame in &stack.frames {
            if seen.insert(*frame) {
                result[*frame as usize].inclusive_weight += stack.weight;
                result[*frame as usize].stack_count += 1;
            }
        }
        let leaf = *stack.frames.last().expect("non-empty stack");
        result[leaf as usize].self_weight += stack.weight;
    }
    result
}
fn frame_sequence(profile: &Profile, frames: &[FrameId]) -> Vec<String> {
    frames
        .iter()
        .map(|id| profile.frame_name(*id).to_owned())
        .collect()
}

fn symbol_context(profile: &Profile, frame: FrameId) -> Value {
    let mut callers: HashMap<FrameId, u64> = HashMap::new();
    let mut callees: HashMap<FrameId, u64> = HashMap::new();
    for stack_id in &profile.frame_to_stacks[frame as usize] {
        let stack = &profile.stacks[*stack_id as usize];
        let mut stack_callers = HashSet::new();
        let mut stack_callees = HashSet::new();
        for (position, candidate) in stack.frames.iter().enumerate() {
            if *candidate != frame {
                continue;
            }
            if position > 0 {
                stack_callers.insert(stack.frames[position - 1]);
            }
            if position + 1 < stack.frames.len() {
                stack_callees.insert(stack.frames[position + 1]);
            }
        }
        for caller in stack_callers {
            *callers.entry(caller).or_default() += stack.weight;
        }
        for callee in stack_callees {
            *callees.entry(callee).or_default() += stack.weight;
        }
    }
    json!({
        "top_callers":context_rows(profile,callers),
        "top_callees":context_rows(profile,callees)
    })
}

fn context_rows(profile: &Profile, weights: HashMap<FrameId, u64>) -> Vec<Value> {
    let mut rows: Vec<_> = weights.into_iter().collect();
    rows.sort_by(|(left_id, left_weight), (right_id, right_weight)| {
        right_weight
            .cmp(left_weight)
            .then_with(|| {
                profile
                    .frame_name(*left_id)
                    .cmp(profile.frame_name(*right_id))
            })
            .then(left_id.cmp(right_id))
    });
    rows.into_iter()
        .take(3)
        .map(|(frame_id, weight)| {
            json!({"frame_id":frame_id,"name":profile.frame_name(frame_id),"weight":weight})
        })
        .collect()
}

#[derive(Default)]
struct TempNode {
    frame: FrameId,
    self_weight: u64,
    total_weight: u64,
    children: HashMap<FrameId, TempNode>,
}

struct RenderState<'a> {
    scope: u64,
    max_depth: usize,
    min_percent: f64,
    budget: &'a mut usize,
    reason_stats: &'a mut BTreeMap<&'static str, TruncationStats>,
    continuations: &'a mut Vec<Value>,
    continuation_count: &'a mut usize,
    continuation_limit: usize,
}

#[derive(Clone, Copy, Default)]
struct TruncationStats {
    count: usize,
    weight: u64,
}

impl RenderState<'_> {
    fn note_truncation(&mut self, reason: &'static str, weight: u64) {
        let stats = self.reason_stats.entry(reason).or_default();
        stats.count = stats.count.saturating_add(1);
        stats.weight = stats.weight.saturating_add(weight);
    }
}

impl TempNode {
    fn new(frame: FrameId) -> Self {
        Self {
            frame,
            ..Self::default()
        }
    }
    fn insert(&mut self, frames: &[FrameId], weight: u64) {
        let frame = frames[0];
        let child = self
            .children
            .entry(frame)
            .or_insert_with(|| TempNode::new(frame));
        child.total_weight += weight;
        if frames.len() == 1 {
            child.self_weight += weight;
        } else {
            child.insert(&frames[1..], weight);
        }
    }
}

fn ordered_children<'a>(
    profile: &Profile,
    children: impl Iterator<Item = (FrameId, &'a TempNode)>,
) -> Vec<(FrameId, &'a TempNode)> {
    let mut children: Vec<_> = children.collect();
    children.sort_by(|(a, an), (b, bn)| {
        bn.total_weight
            .cmp(&an.total_weight)
            .then_with(|| profile.frame_name(*a).cmp(profile.frame_name(*b)))
            .then(a.cmp(b))
    });
    children
}
fn render_temp(
    profile: &Profile,
    node: &TempNode,
    depth: usize,
    state: &mut RenderState<'_>,
) -> (Value, bool) {
    *state.budget -= 1;
    let children = ordered_children(profile, node.children.iter().map(|(id, node)| (*id, node)));
    let mut rendered = Vec::new();
    let mut omitted_count = 0;
    let mut omitted_weight = 0;
    let mut truncated = false;
    for (_id, child) in children {
        let reason = if depth >= state.max_depth {
            Some("depth_limit")
        } else if percent(child.total_weight, state.scope) < state.min_percent {
            Some("min_scope_percent")
        } else if *state.budget == 0 {
            Some("node_budget")
        } else {
            None
        };
        if let Some(reason) = reason {
            state.note_truncation(reason, child.total_weight);
            omitted_count += 1;
            omitted_weight += child.total_weight;
            truncated = true;
        } else {
            let (value, child_truncated) = render_temp(profile, child, depth + 1, state);
            truncated |= child_truncated;
            rendered.push(value);
        }
    }
    (
        json!({"node_id":Value::Null,"frame_id":node.frame,"name":profile.frame_name(node.frame),"self_weight":node.self_weight,"total_weight":node.total_weight,"profile_percent":percent(node.total_weight,profile.total_weight),"scope_percent":percent(node.total_weight,state.scope),"omitted_children":omitted_count,"omitted_weight":omitted_weight,"children":rendered}),
        truncated,
    )
}
fn render_cct(
    profile: &Profile,
    node_id: NodeId,
    depth: usize,
    state: &mut RenderState<'_>,
) -> (Value, bool) {
    *state.budget -= 1;
    let node = &profile.cct.nodes[node_id as usize];
    let children = ordered_cct(profile, node);
    let mut rendered = Vec::new();
    let mut omitted_count = 0;
    let mut omitted_weight = 0;
    let mut truncated = false;
    for child_id in children {
        let child = &profile.cct.nodes[child_id as usize];
        let reason = if depth >= state.max_depth {
            Some("depth_limit")
        } else if percent(child.total_weight, state.scope) < state.min_percent {
            Some("min_scope_percent")
        } else if *state.budget == 0 {
            Some("node_budget")
        } else {
            None
        };
        if let Some(reason) = reason {
            state.note_truncation(reason, child.total_weight);
            *state.continuation_count += 1;
            omitted_count += 1;
            omitted_weight += child.total_weight;
            truncated = true;
            if state.continuations.len() < state.continuation_limit {
                let frame_id = child.frame.expect("non-root child has a frame");
                state.continuations.push(json!({
                    "node_id":child_id,
                    "frame_id":frame_id,
                    "name":profile.frame_name(frame_id),
                    "reason":reason,
                    "profile_fingerprint":profile.source.fingerprint,
                    "total_weight":child.total_weight,
                    "profile_percent":percent(child.total_weight,profile.total_weight),
                    "scope_percent":percent(child.total_weight,state.scope)
                }));
            }
        } else {
            let (value, child_truncated) = render_cct(profile, child_id, depth + 1, state);
            truncated |= child_truncated;
            rendered.push(value);
        }
    }
    let name = node.frame.map(|id| profile.frame_name(id));
    (
        json!({"node_id":node_id,"frame_id":node.frame,"name":name,"self_weight":node.self_weight,"total_weight":node.total_weight,"profile_percent":percent(node.total_weight,profile.total_weight),"scope_percent":percent(node.total_weight,state.scope),"omitted_children":omitted_count,"omitted_weight":omitted_weight,"children":rendered}),
        truncated,
    )
}
fn ordered_cct(profile: &Profile, node: &ContextNode) -> Vec<NodeId> {
    let mut children: Vec<_> = node.children.values().copied().collect();
    children.sort_by(|a, b| {
        let an = &profile.cct.nodes[*a as usize];
        let bn = &profile.cct.nodes[*b as usize];
        bn.total_weight
            .cmp(&an.total_weight)
            .then_with(|| {
                profile
                    .frame_name(an.frame.expect("not root"))
                    .cmp(profile.frame_name(bn.frame.expect("not root")))
            })
            .then(a.cmp(b))
    });
    children
}
