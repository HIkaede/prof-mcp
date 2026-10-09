#![no_main]
use libfuzzer_sys::fuzz_target;
use prof_mcp::{
    capture::collapse_perf_script,
    profile::{BuildLimits, ProfileBuilder},
};
use std::{io::Cursor, path::PathBuf};

fuzz_target!(|input: &[u8]| {
    let limits = BuildLimits {
        max_file_bytes: 65536,
        max_line_bytes: 4096,
        max_depth: 64,
        max_frames: u32::MAX as usize,
        max_total_weight: 1_000_000,
    };
    let collapse = || {
        let mut output = Vec::new();
        let result = collapse_perf_script(Cursor::new(input), &mut output, limits);
        (result, output)
    };
    let (left, output) = collapse();
    let (right, repeated) = collapse();
    assert_eq!(output, repeated);
    match (left, right) {
        (Ok(()), Ok(())) => {
            assert!(output.len() as u64 <= limits.max_file_bytes);
            let parsed = ProfileBuilder::new(limits)
                .from_reader(
                    Cursor::new(&output),
                    PathBuf::from("capture.folded"),
                    output.len() as u64,
                    None,
                )
                .unwrap();
            assert!(parsed.total_weight <= limits.max_total_weight);
        }
        (Err(left), Err(right)) => {
            assert_eq!(left.to_string(), right.to_string());
            assert!(output.is_empty());
        }
        _ => panic!("nondeterministic collapse result"),
    }
});
