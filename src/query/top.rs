//! Self/inclusive ranking of exact folded-frame identities.

use crate::output::{FrameRow, ResponseBudget, TopData, TopOutput};
use hashbrown::HashMap;

use super::{
    ApiError, FrameSelector, FrameStats, Profile, TopSort, check_limit, compile_regex, envelope,
    frame_order_stats, metric_weight, resolve_selector, row_limit_reason,
};

#[derive(Default)]
pub struct TopWorkspace {
    stats: Vec<FrameStats>,
    seen_stacks: Vec<u32>,
    touched: Vec<u32>,
}

impl TopWorkspace {
    pub fn retained_bytes(&self) -> usize {
        self.stats.capacity() * std::mem::size_of::<FrameStats>()
            + (self.seen_stacks.capacity() + self.touched.capacity()) * 4
    }

    pub fn release_oversized(&mut self) {
        if self.retained_bytes() > 8 * 1024 * 1024 {
            *self = Self::default();
        }
    }

    fn subset(&mut self, profile: &Profile, postings: &[u32]) {
        // Clear before resizing: previous touched IDs may exceed the next profile's size.
        for &frame in &self.touched {
            self.stats[frame as usize] = FrameStats::default();
        }
        self.touched.clear();
        self.stats
            .resize(profile.frames.len(), FrameStats::default());
        if profile.recursive_frames.is_empty() {
            for &sid in postings {
                let stack = profile.stacks.stack(sid);
                for &frame in stack.frames {
                    let stats = &mut self.stats[frame as usize];
                    if stats.inclusive_weight == 0 {
                        self.touched.push(frame);
                    }
                    stats.inclusive_weight += stack.weight;
                    stats.stack_count += 1;
                }
                self.stats[*stack.frames.last().expect("non-empty stack") as usize].self_weight +=
                    stack.weight;
            }
        } else {
            self.seen_stacks.resize(profile.frames.len(), u32::MAX);
            for &sid in postings {
                let stack = profile.stacks.stack(sid);
                for &frame in stack.frames {
                    let index = frame as usize;
                    let stats = &mut self.stats[index];
                    if stats.inclusive_weight == 0 {
                        self.touched.push(frame);
                        self.seen_stacks[index] = u32::MAX;
                    }
                    if self.seen_stacks[index] != sid {
                        self.seen_stacks[index] = sid;
                        stats.inclusive_weight += stack.weight;
                        stats.stack_count += 1;
                    }
                }
                self.stats[*stack.frames.last().expect("non-empty stack") as usize].self_weight +=
                    stack.weight;
            }
        }
    }
}

pub fn top<'p>(
    profile: &'p Profile,
    metric: TopSort,
    limit: usize,
    frame: Option<&FrameSelector>,
    name_regex: Option<&str>,
) -> Result<TopOutput<'p>, ApiError> {
    top_with_workspace(
        profile,
        metric,
        limit,
        frame,
        name_regex,
        &mut TopWorkspace::default(),
    )
}

pub fn top_with_workspace<'p>(
    profile: &'p Profile,
    metric: TopSort,
    limit: usize,
    frame: Option<&FrameSelector>,
    name_regex: Option<&str>,
    workspace: &mut TopWorkspace,
) -> Result<TopOutput<'p>, ApiError> {
    check_limit(limit, 1, 200, "limit")?;
    let frame_id = frame
        .map(|selector| resolve_selector(profile, selector))
        .transpose()?;
    let regex = name_regex.map(compile_regex).transpose()?;
    let scope_weight = frame_id.map_or(profile.total_weight, |id| {
        profile.frame_stats[id as usize].inclusive_weight
    });
    let scoped_postings = frame_id.and_then(|id| {
        let stacks = profile.frame_to_stacks.ranked(id as usize);
        // A posting contains each matching stack once, including recursive frames.
        (stacks.len() != profile.stacks.len()).then_some(stacks)
    });
    let one = scoped_postings
        .filter(|stacks| stacks.len() == 1 && profile.recursive_frames.is_empty())
        .map(|stacks| profile.stacks.stack(stacks[0]));
    let one_leaf = one.and_then(|stack| stack.frames.last().copied());
    let one_stats = one.map(|stack| {
        [
            FrameStats {
                self_weight: 0,
                inclusive_weight: stack.weight,
                stack_count: 1,
            },
            FrameStats {
                self_weight: stack.weight,
                inclusive_weight: stack.weight,
                stack_count: 1,
            },
        ]
    });
    let sparse_stats = scoped_postings
        .filter(|_| one.is_none())
        .filter(|stacks| {
            stacks.len() == 1
                || (stacks.len() < profile.frames.len() / 8
                    && stacks
                        .iter()
                        .map(|posting| profile.stacks.stack(*posting).frames.len())
                        .sum::<usize>()
                        < profile.frames.len() / 8)
        })
        .map(|stacks| sparse_subset_stats(profile, stacks));
    let dense = scoped_postings.filter(|_| one.is_none() && sparse_stats.is_none());
    if let Some(stacks) = dense {
        workspace.subset(profile, stacks);
    }
    let stats: &[FrameStats] = if dense.is_some() {
        &workspace.stats
    } else {
        &profile.frame_stats
    };
    let stats_for = |id: u32| {
        if let Some(pair) = &one_stats {
            return &pair[usize::from(Some(id) == one_leaf)];
        }
        sparse_stats
            .as_ref()
            .map_or(&stats[id as usize], |rows| &rows[&id].0)
    };
    let is_subset = scoped_postings.is_some();
    let order = match metric {
        TopSort::SelfWeight => &profile.top_self,
        TopSort::Inclusive => &profile.top_inclusive,
    };
    let sparse_ids = sparse_stats
        .as_ref()
        .map(|rows| rows.keys().copied().collect::<Vec<_>>());
    let candidates = one
        .map(|stack| stack.frames)
        .or(sparse_ids.as_deref())
        .unwrap_or(order);
    let (ids, available) = if dense.is_some() {
        let mut available = workspace.touched.len();
        if let Some(regex) = &regex {
            available = 0;
            for index in 0..workspace.touched.len() {
                if regex.is_match(profile.frame_name(workspace.touched[index])) {
                    workspace.touched.swap(index, available);
                    available += 1;
                }
            }
        }
        // Keep every touched ID for the next reset, including filtered and unreturned rows.
        let matches = &mut workspace.touched[..available];
        let order = |a: &u32, b: &u32| frame_order_stats(stats, *a, *b, metric);
        if matches.len() > limit {
            matches.select_nth_unstable_by(limit, order);
        }
        let count = matches.len().min(limit);
        matches[..count].sort_unstable_by(order);
        (matches[..count].to_vec(), available)
    } else if !is_subset && regex.is_none() {
        (order[..limit.min(order.len())].to_vec(), order.len())
    } else {
        let mut ids = Vec::with_capacity(limit);
        let mut available = 0;
        for &id in candidates {
            if stats_for(id).inclusive_weight > 0
                && regex
                    .as_ref()
                    .is_none_or(|re| re.is_match(profile.frame_name(id)))
            {
                available += 1;
                if is_subset || ids.len() < limit {
                    ids.push(id);
                }
            }
        }
        if is_subset {
            let order = |a: &u32, b: &u32| {
                let an = stats_for(*a);
                let bn = stats_for(*b);
                metric_weight(bn, metric)
                    .cmp(&metric_weight(an, metric))
                    .then_with(|| bn.self_weight.cmp(&an.self_weight))
                    .then(a.cmp(b))
            };
            if ids.len() > limit {
                ids.select_nth_unstable_by(limit, order);
            }
            ids.truncate(limit);
            ids.sort_unstable_by(order);
        }
        (ids, available)
    };
    let truncation_reasons = row_limit_reason(limit, available);
    let mut bytes = ResponseBudget::new();
    let rows: Vec<_> = ids
        .iter()
        .map(|id| {
            let stats = stats_for(*id);
            let weight = metric_weight(stats, metric);
            bytes.row_name(profile.frame_name(*id))?;
            Ok(FrameRow {
                frame_id: *id,
                name: profile.frame_name(*id),
                self_weight: stats.self_weight,
                inclusive_weight: stats.inclusive_weight,
                stack_count: stats.stack_count,
                profile_percent: crate::output::percent(weight, profile.total_weight),
                scope_percent: crate::output::percent(weight, scope_weight),
            })
        })
        .collect::<Result<_, ApiError>>()?;
    Ok(envelope(
        profile,
        scope_weight,
        truncation_reasons,
        Vec::new(),
        TopData {
            metric,
            frame: frame_id,
            rows,
        },
    ))
}

fn sparse_subset_stats(profile: &Profile, postings: &[u32]) -> HashMap<u32, (FrameStats, u32)> {
    let mut result = HashMap::new();
    for posting in postings {
        let stack_id = *posting;
        let stack = profile.stacks.stack(stack_id);
        for &frame in stack.frames {
            let (stats, seen) = result
                .entry(frame)
                .or_insert((FrameStats::default(), u32::MAX));
            if *seen != stack_id {
                *seen = stack_id;
                stats.inclusive_weight += stack.weight;
                stats.stack_count += 1;
            }
        }
        result
            .get_mut(stack.frames.last().expect("non-empty stack"))
            .unwrap()
            .0
            .self_weight += stack.weight;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{BuildLimits, ProfileBuilder};

    fn profile(input: &str) -> Profile {
        ProfileBuilder::new(BuildLimits::default())
            .from_reader(std::io::Cursor::new(input), "/workspace.folded".into())
            .unwrap()
    }

    fn check(profile: &Profile, name: &str, workspace: &mut TopWorkspace) {
        let id = profile.frame_id(name).unwrap();
        let mut expected = vec![FrameStats::default(); profile.frames.len()];
        for stack in profile
            .stacks
            .iter()
            .filter(|stack| stack.frames.contains(&id))
        {
            let unique: std::collections::HashSet<_> = stack.frames.iter().copied().collect();
            for frame in unique {
                expected[frame as usize].inclusive_weight += stack.weight;
                expected[frame as usize].stack_count += 1;
            }
            expected[*stack.frames.last().unwrap() as usize].self_weight += stack.weight;
        }
        for metric in [TopSort::SelfWeight, TopSort::Inclusive] {
            for filter in [None, Some("[ab]")] {
                let selector = FrameSelector {
                    frame_name: Some(name.into()),
                    ..Default::default()
                };
                let actual =
                    top_with_workspace(profile, metric, 3, Some(&selector), filter, workspace)
                        .unwrap();
                let mut ids: Vec<_> = (0..profile.frames.len() as u32)
                    .filter(|&id| {
                        expected[id as usize].inclusive_weight > 0
                            && filter.is_none_or(|_| profile.frame_name(id).contains(['a', 'b']))
                    })
                    .collect();
                ids.sort_unstable_by(|&a, &b| frame_order_stats(&expected, a, b, metric));
                let available = ids.len();
                assert_eq!(actual.truncated, available > 3);
                if available > 3 {
                    match actual.truncation_reasons.as_slice() {
                        [
                            crate::output::TruncationReason::RowLimit {
                                limit,
                                returned,
                                available: count,
                                omitted,
                            },
                        ] => {
                            assert_eq!(
                                (*limit, *returned, *count, *omitted),
                                (3, 3, available, available - 3)
                            );
                        }
                        reasons => panic!("unexpected row-limit reasons: {reasons:?}"),
                    }
                } else {
                    assert!(actual.truncation_reasons.is_empty());
                }
                ids.truncate(3);
                assert_eq!(
                    actual
                        .data
                        .rows
                        .iter()
                        .map(|row| row.frame_id)
                        .collect::<Vec<_>>(),
                    ids
                );
                for row in actual.data.rows {
                    let stats = &expected[row.frame_id as usize];
                    assert_eq!(
                        (row.self_weight, row.inclusive_weight, row.stack_count),
                        (stats.self_weight, stats.inclusive_weight, stats.stack_count)
                    );
                }
            }
        }
    }

    #[test]
    fn touched_reset_handles_scope_profile_resize_recursion_and_errors() {
        let recursive = profile("a;b;a 3\na;c 5\nx;c 7\n");
        let small = profile("a;a 11\na;b 2\nx 13\n");
        let nonrecursive = profile("a;b;c 3\na;d;e 5\nx;b;f 7\n");
        let mut workspace = TopWorkspace::default();
        for (profile, name) in [
            (&recursive, "a"),
            (&nonrecursive, "a"),
            (&nonrecursive, "b"),
            (&small, "a"),
            (&recursive, "c"),
            (&nonrecursive, "c"),
            (&recursive, "a"),
        ] {
            check(profile, name, &mut workspace);
        }
        let selector = FrameSelector {
            frame_name: Some("a".into()),
            ..Default::default()
        };
        assert!(
            top_with_workspace(
                &recursive,
                TopSort::SelfWeight,
                3,
                Some(&selector),
                Some("["),
                &mut workspace
            )
            .is_err()
        );
        check(&small, "a", &mut workspace);
        top_with_workspace(
            &profile(""),
            TopSort::Inclusive,
            3,
            None,
            None,
            &mut workspace,
        )
        .unwrap();
        check(&nonrecursive, "a", &mut workspace);
    }

    #[test]
    fn one_scope_never_allocates_dense_workspace() {
        let input = format!(
            "{} 7\nx 3\n",
            (0..512)
                .map(|i| format!("f{i}"))
                .collect::<Vec<_>>()
                .join(";")
        );
        let deep = profile(&input);
        let mut workspace = TopWorkspace::default();
        check(&deep, "f256", &mut workspace);
        assert_eq!(workspace.retained_bytes(), 0);
        check(&profile("a;b 3\nb;c 5\n"), "a", &mut workspace);
        assert_eq!(workspace.retained_bytes(), 0);
        let mut workspace = TopWorkspace::default();
        check(&profile("a;b 1\na;c 2\nx 3\n"), "a", &mut workspace);
        assert!(workspace.seen_stacks.is_empty());
    }

    #[test]
    fn invalid_regex_is_checked_before_workspace_allocation() {
        let profile = profile("a;b 1\na;c 2\nx 3\n");
        let selector = FrameSelector {
            frame_name: Some("a".into()),
            ..Default::default()
        };
        let mut workspace = TopWorkspace::default();
        assert!(
            top_with_workspace(
                &profile,
                TopSort::SelfWeight,
                3,
                Some(&selector),
                Some("["),
                &mut workspace
            )
            .is_err()
        );
        assert_eq!(workspace.retained_bytes(), 0);
    }

    #[test]
    fn oversized_workspace_is_released() {
        let mut workspace = TopWorkspace::default();
        workspace.stats.resize(400_000, FrameStats::default());
        assert!(workspace.retained_bytes() > 8 * 1024 * 1024);
        workspace.release_oversized();
        assert_eq!(workspace.retained_bytes(), 0);
    }
}
