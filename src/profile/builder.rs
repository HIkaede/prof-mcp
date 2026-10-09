use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::PathBuf,
};

use blake3::Hasher;
use hashbrown::{HashMap, HashSet, hash_map::Entry};

use crate::{
    error::{ApiError, ProfileError},
    profile::{
        ContextCallTree, ContextNode, Frame, FrameId, FrameStats, Profile, SourceMeta, StackRecord,
        folded::parse_line,
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
    pub fn from_file(
        &self,
        path: PathBuf,
        byte_len: u64,
        modified_unix_ms: Option<u64>,
    ) -> Result<Profile, ProfileError> {
        let file = File::open(&path).map_err(|source| ProfileError::Io {
            path: path.clone(),
            source,
        })?;
        self.from_reader(BufReader::new(file), path, byte_len, modified_unix_ms)
    }
    pub fn from_reader<R: BufRead>(
        &self,
        mut reader: R,
        canonical_path: PathBuf,
        _byte_len: u64,
        modified_unix_ms: Option<u64>,
    ) -> Result<Profile, ProfileError> {
        let mut model_bytes = 0usize;
        charge_model(&mut model_bytes, 256, self.limits.max_model_bytes)?;
        check_model_limit("cct_nodes", 1, self.limits.max_cct_nodes)?;
        let mut stack_frames = 0usize;
        let mut interner: HashMap<Box<str>, FrameId> = HashMap::new();
        let mut frame_names = Vec::<Box<str>>::new();
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
            if let Some((names, weight)) = parse_line(line_no, &line, self.limits.max_depth)? {
                let ids = names
                    .into_iter()
                    .map(|name| {
                        if let Some(id) = interner.get(name) {
                            Ok(*id)
                        } else {
                            let id = u32::try_from(frame_names.len()).map_err(|_| {
                                ApiError::new(
                                    "too_many_frames",
                                    "Profile has too many distinct frames",
                                    serde_json::json!({}),
                                    "Split the profile into a smaller input.",
                                )
                            })?;
                            if frame_names.len() >= self.limits.max_frames {
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
                            interner.insert(owned.clone(), id);
                            frame_names.push(owned);
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
        let mut stacks: Vec<_> = aggregated
            .into_iter()
            .map(|(frames, weight)| StackRecord { frames, weight })
            .collect();
        stacks.sort_by(|a, b| a.frames.cmp(&b.frames));
        let total_weight = stacks.iter().try_fold(0_u64, |total, stack| {
            total.checked_add(stack.weight).ok_or_else(|| {
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
        let frame_count = frame_names.len();
        let mut profile = Profile {
            source: SourceMeta {
                canonical_path,
                fingerprint: hasher.finalize().to_hex().to_string(),
                byte_len: bytes_read,
                modified_unix_ms,
            },
            total_weight,
            max_depth: stacks.iter().map(|s| s.frames.len()).max().unwrap_or(0),
            frames: frame_names.into_iter().map(|name| Frame { name }).collect(),
            frame_by_name: interner,
            stacks,
            frame_stats: vec![FrameStats::default(); frame_count],
            frame_to_stacks: vec![Vec::new(); frame_count],
            cct: ContextCallTree {
                root: 0,
                nodes: vec![ContextNode {
                    frame: None,
                    parent: None,
                    self_weight: 0,
                    total_weight: 0,
                    children: HashMap::new(),
                }],
            },
        };
        for (stack_index, stack) in profile.stacks.iter().enumerate() {
            let sid = u32::try_from(stack_index).expect("bounded by address space");
            let mut node = profile.cct.root;
            profile.cct.nodes[node as usize].total_weight += stack.weight;
            for frame in stack.frames.iter().copied() {
                let child =
                    if let Some(child) = profile.cct.nodes[node as usize].children.get(&frame) {
                        *child
                    } else {
                        check_model_limit(
                            "cct_nodes",
                            profile.cct.nodes.len().saturating_add(1),
                            self.limits.max_cct_nodes,
                        )?;
                        charge_model(&mut model_bytes, 256, self.limits.max_model_bytes)?;
                        let child = u32::try_from(profile.cct.nodes.len())
                            .expect("bounded by address space");
                        profile.cct.nodes.push(ContextNode {
                            frame: Some(frame),
                            parent: Some(node),
                            self_weight: 0,
                            total_weight: 0,
                            children: HashMap::new(),
                        });
                        profile.cct.nodes[node as usize]
                            .children
                            .insert(frame, child);
                        child
                    };
                node = child;
                profile.cct.nodes[node as usize].total_weight += stack.weight;
            }
            profile.cct.nodes[node as usize].self_weight += stack.weight;
            let mut distinct = HashSet::new();
            for frame in stack.frames.iter().copied() {
                if distinct.insert(frame) {
                    let stats = &mut profile.frame_stats[frame as usize];
                    stats.inclusive_weight += stack.weight;
                    stats.stack_count += 1;
                    profile.frame_to_stacks[frame as usize].push(sid);
                }
            }
            let leaf = *stack.frames.last().expect("parser disallows empty stack");
            profile.frame_stats[leaf as usize].self_weight += stack.weight;
        }
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

// Conservative charges cover container slack, both name copies, and reverse indexes.
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
