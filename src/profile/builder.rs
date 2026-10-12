use std::{
    cmp::Reverse,
    collections::{HashMap, hash_map::Entry},
    fs::File,
    io::{BufRead, BufReader},
    path::PathBuf,
};

use blake3::Hasher;

use crate::{
    error::{ApiError, ProfileError},
    profile::{
        ContextCallTree, ContextNode, FrameId, FrameStats, FrameTable, PostingTable, Profile,
        RecursionStats, SourceMeta, StackTable, folded::parse_line,
    },
};

#[derive(Clone, Copy, Debug)]
pub struct BuildLimits {
    pub max_file_bytes: u64,
    pub max_line_bytes: usize,
    pub max_depth: usize,
    pub max_total_weight: u64,
    pub max_frames: usize,
    pub max_model_bytes: usize,
    pub max_cct_nodes: usize,
    pub max_stack_frames: usize,
}
impl Default for BuildLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 512 * 1024 * 1024,
            max_line_bytes: 8 * 1024 * 1024,
            max_depth: 4096,
            max_total_weight: (1_u64 << 53) - 1,
            max_frames: u32::MAX as usize,
            max_model_bytes: 256 * 1024 * 1024,
            max_cct_nodes: 1_000_000,
            max_stack_frames: 4_000_000,
        }
    }
}

pub struct ProfileBuilder {
    limits: BuildLimits,
}
impl ProfileBuilder {
    pub fn new(limits: BuildLimits) -> Self {
        Self { limits }
    }
    pub fn from_file(&self, path: PathBuf) -> Result<Profile, ProfileError> {
        let file = File::open(&path).map_err(|source| ProfileError::Io {
            path: path.clone(),
            source,
        })?;
        self.from_reader(BufReader::new(file), path)
    }
    pub fn from_reader<R: BufRead>(
        &self,
        mut reader: R,
        canonical_path: PathBuf,
    ) -> Result<Profile, ProfileError> {
        let mut model_bytes = 0usize;
        charge_model(&mut model_bytes, 256, self.limits.max_model_bytes)?;
        check_model_limit("cct_nodes", 1, self.limits.max_cct_nodes)?;
        let mut stack_frames = 0usize;
        let mut interner: HashMap<Box<str>, FrameId> = HashMap::new();
        let mut aggregated: HashMap<Box<[FrameId]>, u64> = HashMap::new();
        let mut hasher = Hasher::new();
        let mut line = Vec::new();
        let mut line_no = 0;
        let mut bytes_read = 0_u64;
        loop {
            line.clear();
            let read = match read_bounded_line(&mut reader, &mut line, self.limits.max_line_bytes) {
                Ok(read) => read,
                Err(source) if source.kind() == std::io::ErrorKind::InvalidData => return Err(ApiError::new("invalid_folded_line", format!("line {} exceeds maximum length", line_no + 1), serde_json::json!({"line":line_no + 1, "preview": crate::profile::folded::preview(&line)}), "Regenerate the profile with shorter folded lines.").into()),
                Err(source) => return Err(ProfileError::Io { path: canonical_path.clone(), source }),
            };
            if read == 0 {
                break;
            }
            bytes_read = bytes_read.checked_add(read as u64).ok_or_else(|| {
                ApiError::new(
                    "profile_too_large",
                    "Profile size overflowed while reading",
                    serde_json::json!({}),
                    "Use a smaller profile.",
                )
            })?;
            if bytes_read > self.limits.max_file_bytes {
                return Err(ApiError::new(
                    "profile_too_large",
                    "Profile exceeds configured maximum size while reading",
                    serde_json::json!({"max_bytes": self.limits.max_file_bytes}),
                    "Use a smaller profile or raise --max-file-size-mib.",
                )
                .into());
            }
            line_no += 1;
            hasher.update(&line);
            if let Some((names, weight)) = parse_line(
                line_no,
                &line,
                self.limits.max_depth.min(u16::MAX as usize + 1),
            )? {
                let ids = names
                    .into_iter()
                    .map(|name| {
                        if let Some(id) = interner.get(name) {
                            Ok(*id)
                        } else {
                            let id = u32::try_from(interner.len()).map_err(|_| {
                                ApiError::new(
                                    "too_many_frames",
                                    "Profile has too many distinct frames",
                                    serde_json::json!({}),
                                    "Split the profile into a smaller input.",
                                )
                            })?;
                            if interner.len() >= self.limits.max_frames {
                                return Err(ApiError::new(
                                    "too_many_frames",
                                    "Profile has too many distinct frames",
                                    serde_json::json!({"max_frames": self.limits.max_frames}),
                                    "Split the profile into a smaller input.",
                                ));
                            }
                            charge_model(
                                &mut model_bytes,
                                192usize.saturating_add(name.len().saturating_mul(2)),
                                self.limits.max_model_bytes,
                            )?;
                            let owned: Box<str> = name.into();
                            interner.insert(owned, id);
                            Ok(id)
                        }
                    })
                    .collect::<Result<Vec<_>, ApiError>>()?;
                let entry = match aggregated.entry(ids.into_boxed_slice()) {
                    Entry::Occupied(entry) => entry.into_mut(),
                    Entry::Vacant(entry) => {
                        stack_frames = stack_frames.saturating_add(entry.key().len());
                        check_model_limit(
                            "stack_frames",
                            stack_frames,
                            self.limits.max_stack_frames,
                        )?;
                        charge_model(
                            &mut model_bytes,
                            64usize.saturating_add(entry.key().len().saturating_mul(12)),
                            self.limits.max_model_bytes,
                        )?;
                        entry.insert(0)
                    }
                };
                *entry = entry.checked_add(weight).ok_or_else(|| {
                    ApiError::new(
                        "weight_overflow",
                        "A duplicate stack weight overflowed",
                        serde_json::json!({"line":line_no}),
                        "Use a profile with smaller aggregate weight.",
                    )
                })?;
            }
        }
        drop(line);
        drop(reader);
        let frame_count = interner.len();
        charge_model(
            &mut model_bytes,
            frame_count.saturating_mul(12),
            self.limits.max_model_bytes,
        )?;
        let mut frame_names: Vec<_> = interner.into_iter().collect();
        frame_names.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let mut remap = vec![0u32; frame_count];
        for (id, (_, old_id)) in frame_names.iter().enumerate() {
            remap[*old_id as usize] = id as u32;
        }
        u32::try_from(stack_frames).map_err(|_| offset_overflow())?;
        let mut stacks: Vec<_> = aggregated.into_iter().collect();
        for (frames, _) in &mut stacks {
            for frame in frames.iter_mut() {
                *frame = remap[*frame as usize];
            }
        }
        drop(remap);
        stacks.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let total_weight = stacks.iter().try_fold(0_u64, |total, stack| {
            total.checked_add(stack.1).ok_or_else(|| {
                ApiError::new(
                    "weight_overflow",
                    "Profile total weight overflowed",
                    serde_json::json!({}),
                    "Use a profile with smaller aggregate weight.",
                )
            })
        })?;
        if total_weight > self.limits.max_total_weight {
            return Err(ApiError::new(
                "weight_overflow",
                "Profile total weight exceeds JSON-safe maximum",
                serde_json::json!({"max_total_weight": self.limits.max_total_weight}),
                "Split the profile into smaller inputs.",
            )
            .into());
        }
        charge_model(
            &mut model_bytes,
            frame_count.saturating_mul(32),
            self.limits.max_model_bytes,
        )?;
        let max_depth = stacks.iter().map(|s| s.0.len()).max().unwrap_or(0);
        let mut offsets = Vec::with_capacity(stacks.len() + 1);
        let mut flat_frames = Vec::with_capacity(stack_frames);
        let mut weights = Vec::with_capacity(stacks.len());
        offsets.push(0);
        for (frames, weight) in stacks {
            flat_frames.extend_from_slice(&frames);
            offsets.push(u32::try_from(flat_frames.len()).map_err(|_| offset_overflow())?);
            weights.push(weight);
        }
        let stacks = StackTable {
            offsets: offsets.into_boxed_slice(),
            frames: flat_frames.into_boxed_slice(),
            weights: weights.into_boxed_slice(),
        };
        let name_bytes = frame_names.iter().try_fold(0u32, |total, (name, _)| {
            let len = u32::try_from(name.len()).map_err(|_| offset_overflow())?;
            total.checked_add(len).ok_or_else(offset_overflow)
        })?;
        let mut name_text = String::with_capacity(name_bytes as usize);
        let mut name_offsets = Vec::with_capacity(frame_count + 1);
        name_offsets.push(0);
        for (name, _) in frame_names {
            name_text.push_str(&name);
            name_offsets.push(u32::try_from(name_text.len()).map_err(|_| offset_overflow())?);
        }
        let frames = FrameTable {
            name_text: name_text.into_boxed_str(),
            name_offsets: name_offsets.into_boxed_slice(),
        };
        let mut nodes = vec![ContextNode {
            frame: None,
            total_weight: 0,
        }];
        let mut parents = Vec::new();
        let mut node_path = vec![0u32];
        let mut previous = &[][..];
        let mut profile = Profile {
            source: SourceMeta {
                fingerprint: hasher.finalize().to_hex().to_string(),
            },
            total_weight,
            max_depth,
            frames,
            stacks,
            frame_stats: vec![FrameStats::default(); frame_count].into_boxed_slice(),
            frame_to_stacks: PostingTable::default(),
            top_self: Box::new([]),
            top_inclusive: Box::new([]),
            recursive_frames: Box::new([]),
            heaviest_stack_weights: [0; 2],
            cct: ContextCallTree {
                root: 0,
                nodes: Box::new([]),
                child_offsets: Box::new([]),
                children: Box::new([]),
            },
        };
        let mut recursive = Vec::<RecursionStats>::new();
        let mut occurrences = vec![0u32; frame_count];
        let mut counts = vec![0u32; frame_count];
        let mut seen = vec![u32::MAX; frame_count];
        for (stack_index, stack) in profile.stacks.iter().enumerate() {
            let sid = u32::try_from(stack_index).map_err(|_| offset_overflow())?;
            // ID-sorted stacks visit prefixes in the same order as the old trie.
            let common = previous
                .iter()
                .zip(stack.frames)
                .take_while(|(a, b)| a == b)
                .count();
            node_path.truncate(common + 1);
            for &node in &node_path {
                nodes[node as usize].total_weight += stack.weight;
            }
            for &frame in &stack.frames[common..] {
                check_model_limit(
                    "cct_nodes",
                    nodes.len().saturating_add(1),
                    self.limits.max_cct_nodes,
                )?;
                charge_model(&mut model_bytes, 256, self.limits.max_model_bytes)?;
                let child = u32::try_from(nodes.len()).map_err(|_| offset_overflow())?;
                parents.push(*node_path.last().unwrap());
                nodes.push(ContextNode {
                    frame: Some(frame),
                    total_weight: stack.weight,
                });
                node_path.push(child);
            }
            previous = stack.frames;
            for frame in stack.frames.iter().copied() {
                if seen[frame as usize] != sid {
                    seen[frame as usize] = sid;
                    let stats = &mut profile.frame_stats[frame as usize];
                    stats.inclusive_weight += stack.weight;
                    stats.stack_count += 1;
                    counts[frame as usize] += 1;
                    occurrences[frame as usize] = 1;
                } else {
                    let count = &mut occurrences[frame as usize];
                    *count += 1;
                    if recursive.is_empty() {
                        recursive = (0..frame_count)
                            .map(|frame| RecursionStats {
                                frame: frame as u32,
                                ..RecursionStats::default()
                            })
                            .collect();
                    }
                    let recursion = &mut recursive[frame as usize];
                    if *count == 2 {
                        recursion.affected_weight += stack.weight;
                    }
                    recursion.max_occurrences = recursion.max_occurrences.max(*count);
                }
            }
            let leaf = *stack.frames.last().expect("parser disallows empty stack");
            profile.frame_stats[leaf as usize].self_weight += stack.weight;
        }
        drop(occurrences);
        drop(node_path);
        let offsets = prefix_offsets(&counts)?;
        let posting_count = *offsets.last().expect("sentinel") as usize;
        // Position fields and the temporary global ranking coexist with the frozen stack table.
        charge_model(
            &mut model_bytes,
            posting_count
                .saturating_mul(4)
                .saturating_add(profile.stacks.len().saturating_mul(4))
                .saturating_add(frame_count.saturating_mul(8))
                .saturating_add(4),
            self.limits.max_model_bytes,
        )?;
        let mut entries = vec![0u32; posting_count];
        let mut positions = vec![0u32; posting_count];
        counts.copy_from_slice(&offsets[..frame_count]);
        let mut cursors = counts;
        seen.fill(u32::MAX);
        let mut ranked_stacks: Vec<_> = (0..profile.stacks.len())
            .map(|id| u32::try_from(id).map_err(|_| offset_overflow()))
            .collect::<Result<_, _>>()?;
        ranked_stacks
            .sort_unstable_by_key(|&id| (Reverse(profile.stacks.weights[id as usize]), id));
        profile.heaviest_stack_weights = [10, 50].map(|limit| {
            ranked_stacks
                .iter()
                .take(limit)
                .map(|&id| profile.stacks.weights[id as usize])
                .sum()
        });
        for sid in ranked_stacks {
            let stack = profile.stacks.stack(sid);
            for (position, &frame) in stack.frames.iter().enumerate() {
                let position = u16::try_from(position).map_err(|_| {
                    ApiError::new(
                        "stack_too_deep",
                        "Stack position exceeds the model's u16 limit",
                        serde_json::json!({"max_depth": 65536}),
                        "Use stacks with at most 65536 frames.",
                    )
                })?;
                let frame = frame as usize;
                if seen[frame] != sid {
                    seen[frame] = sid;
                    let cursor = cursors[frame] as usize;
                    entries[cursor] = sid;
                    positions[cursor] = u32::from(position) | (u32::from(position) << 16);
                    cursors[frame] += 1;
                } else {
                    let packed = &mut positions[(cursors[frame] - 1) as usize];
                    *packed = (*packed & 0xffff) | (u32::from(position) << 16);
                }
            }
        }
        drop(cursors);
        drop(seen);
        let mut position_offsets = Vec::with_capacity(frame_count + 1);
        let mut constant_positions = Vec::with_capacity(frame_count);
        let mut variable_count = 0;
        position_offsets.push(0);
        for range in offsets.windows(2) {
            let start = range[0] as usize;
            let end = range[1] as usize;
            let constant = positions[start..end].first().copied().unwrap_or(0);
            if positions[start..end]
                .iter()
                .all(|&position| position == constant)
            {
                constant_positions.push(constant);
            } else {
                constant_positions.push(0);
                positions.copy_within(start..end, variable_count);
                variable_count += end - start;
            }
            position_offsets.push(u32::try_from(variable_count).map_err(|_| offset_overflow())?);
        }
        positions.truncate(variable_count);
        profile.frame_to_stacks = PostingTable {
            offsets: offsets.into_boxed_slice(),
            entries: entries.into_boxed_slice(),
            position_offsets: position_offsets.into_boxed_slice(),
            constant_positions: constant_positions.into_boxed_slice(),
            variable_positions: positions.into_boxed_slice(),
        };
        recursive.retain(|row| row.max_occurrences > 1);
        recursive.sort_unstable_by(|a, b| {
            b.affected_weight
                .cmp(&a.affected_weight)
                .then(a.frame.cmp(&b.frame))
        });
        recursive.truncate(5);
        profile.recursive_frames = recursive.into_boxed_slice();
        let mut child_counts = vec![0u32; nodes.len()];
        for &parent in &parents {
            child_counts[parent as usize] += 1;
        }
        let child_offsets = prefix_offsets(&child_counts)?;
        child_counts.copy_from_slice(&child_offsets[..nodes.len()]);
        let mut children = vec![0; parents.len()];
        let mut cursors = child_counts;
        for (index, &parent) in parents.iter().enumerate() {
            children[cursors[parent as usize] as usize] =
                u32::try_from(index + 1).map_err(|_| offset_overflow())?;
            cursors[parent as usize] += 1;
        }
        drop(parents);
        drop(cursors);
        for range in child_offsets.windows(2) {
            children[range[0] as usize..range[1] as usize].sort_unstable_by(|a, b| {
                let an = &nodes[*a as usize];
                let bn = &nodes[*b as usize];
                bn.total_weight
                    .cmp(&an.total_weight)
                    .then(an.frame.cmp(&bn.frame))
                    .then(a.cmp(b))
            });
        }
        profile.cct = ContextCallTree {
            root: 0,
            nodes: nodes.into_boxed_slice(),
            child_offsets: child_offsets.into_boxed_slice(),
            children: children.into_boxed_slice(),
        };
        let mut top_self: Vec<_> = (0..frame_count as u32).collect();
        top_self.sort_unstable_by_key(|&id| {
            (Reverse(profile.frame_stats[id as usize].self_weight), id)
        });
        let mut top_inclusive: Vec<_> = (0..frame_count as u32).collect();
        top_inclusive.sort_unstable_by_key(|&id| {
            let stats = &profile.frame_stats[id as usize];
            (
                Reverse(stats.inclusive_weight),
                Reverse(stats.self_weight),
                id,
            )
        });
        profile.top_self = top_self.into_boxed_slice();
        profile.top_inclusive = top_inclusive.into_boxed_slice();
        Ok(profile)
    }
}

fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    line: &mut Vec<u8>,
    max: usize,
) -> std::io::Result<usize> {
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            return Ok(line.len());
        }
        let end = chunk
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(chunk.len(), |index| index + 1);
        if line.len().saturating_add(end) > max {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "folded line exceeds maximum",
            ));
        }
        let ended_with_newline = chunk.get(end - 1) == Some(&b'\n');
        line.extend_from_slice(&chunk[..end]);
        reader.consume(end);
        if ended_with_newline {
            return Ok(line.len());
        }
    }
}

// Conservative charges cover build scratch, container slack, names, and indexes.
fn charge_model(used: &mut usize, amount: usize, limit: usize) -> Result<(), ApiError> {
    *used = used.saturating_add(amount);
    check_model_limit("model_bytes", *used, limit)
}

fn check_model_limit(resource: &str, actual: usize, limit: usize) -> Result<(), ApiError> {
    if actual > limit {
        return Err(ApiError::new(
            "profile_model_too_large",
            "Profile exceeds the expanded model budget",
            serde_json::json!({"resource":resource, "actual":actual, "limit":limit}),
            "Split the profile into smaller inputs or reduce distinct call paths.",
        ));
    }
    Ok(())
}

fn offset_overflow() -> ApiError {
    ApiError::new(
        "profile_model_too_large",
        "Profile exceeds the compact index address space",
        serde_json::json!({"resource":"index_entries", "limit":u32::MAX}),
        "Split the profile into smaller inputs.",
    )
}

fn prefix_offsets(counts: &[u32]) -> Result<Vec<u32>, ApiError> {
    let mut offsets = Vec::with_capacity(counts.len() + 1);
    offsets.push(0u32);
    for &count in counts {
        offsets.push(
            offsets
                .last()
                .unwrap()
                .checked_add(count)
                .ok_or_else(offset_overflow)?,
        );
    }
    Ok(offsets)
}

#[cfg(test)]
mod compact_tests {
    use super::{BuildLimits, ProfileBuilder, prefix_offsets};

    #[test]
    fn bounded_weights_match_full_sort() {
        for len in [0, 1, 9, 10, 11, 49, 50, 51, 257] {
            for mut weights in [
                vec![7; len],
                (0..len).map(|i| 1 + (i as u64 * 37) % 101).collect(),
            ] {
                for _ in 0..2 {
                    let mut reference = weights.clone();
                    reference.sort_unstable_by(|a, b| b.cmp(a));
                    assert_eq!(
                        ProfileBuilder::new(BuildLimits::default())
                            .from_reader(
                                std::io::Cursor::new(
                                    weights
                                        .iter()
                                        .enumerate()
                                        .map(|(id, weight)| format!("frame{id} {weight}\n"))
                                        .collect::<String>()
                                ),
                                "/weights.folded".into()
                            )
                            .unwrap()
                            .heaviest_stack_weights(),
                        [
                            reference.iter().take(10).sum::<u64>(),
                            reference.iter().take(50).sum::<u64>()
                        ]
                    );
                    weights.reverse();
                }
            }
        }
    }

    #[test]
    fn offsets_check_overflow_and_keep_empty_ranges() {
        assert_eq!(prefix_offsets(&[]).unwrap(), [0]);
        assert_eq!(prefix_offsets(&[0, 2, 0, 1]).unwrap(), [0, 0, 2, 2, 3]);
        assert_eq!(prefix_offsets(&[u32::MAX]).unwrap(), [0, u32::MAX]);
        assert_eq!(
            prefix_offsets(&[u32::MAX, 1]).unwrap_err().code,
            "profile_model_too_large"
        );
    }
}
