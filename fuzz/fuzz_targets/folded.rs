#![no_main]
use libfuzzer_sys::fuzz_target;
use prof_mcp::{
    profile::{BuildLimits, ProfileBuilder},
    query,
};
use std::{io::Cursor, path::PathBuf};

fuzz_target!(|input: &[u8]| {
    let limits = BuildLimits {
        max_file_bytes: 65536,
        max_line_bytes: 4096,
        max_depth: 64,
        max_frames: 4096,
        max_total_weight: 1_000_000,
    };
    let parse = || {
        ProfileBuilder::new(limits).from_reader(
            Cursor::new(input),
            PathBuf::from("fuzz.folded"),
            input.len() as u64,
            None,
        )
    };
    match (parse(), parse()) {
        (Ok(left), Ok(right)) => {
            assert_eq!(query::summary(&left), query::summary(&right));
            assert_eq!(
                left.frame_stats
                    .iter()
                    .map(|stats| stats.self_weight)
                    .sum::<u64>(),
                left.total_weight
            );
            assert!(
                left.frame_stats
                    .iter()
                    .all(|stats| stats.inclusive_weight <= left.total_weight)
            );
            assert!(left.max_depth <= limits.max_depth);
            assert!(left.source.byte_len <= limits.max_file_bytes);
        }
        (Err(left), Err(right)) => assert_eq!(left.to_string(), right.to_string()),
        _ => panic!("nondeterministic parser result"),
    }
});
