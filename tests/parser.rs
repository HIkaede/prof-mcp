mod support;

use std::{io::Cursor, path::PathBuf};

use rmdb_prof_mcp::profile::{BuildLimits, ProfileBuilder};

fn parse(
    input: &[u8],
) -> Result<rmdb_prof_mcp::profile::Profile, rmdb_prof_mcp::error::ProfileError> {
    ProfileBuilder::new(BuildLimits::default()).from_reader(
        Cursor::new(input),
        PathBuf::from("/fixture.folded"),
        input.len() as u64,
        None,
    )
}

#[test]
fn accepts_spaces_crlf_empty_lines_trailing_ascii_whitespace_and_aggregates() {
    let profile = parse(b"\r\nroot;frame with spaces 3 \t\r\nroot;frame with spaces 7\n").unwrap();
    assert_eq!(profile.total_weight, 10);
    assert_eq!(profile.stacks.len(), 1);
    assert_eq!(
        profile.frame_name(profile.frame_id("frame with spaces").unwrap()),
        "frame with spaces"
    );
}

#[test]
fn final_contiguous_delimiter_is_not_part_of_a_frame() {
    let profile = parse(b"root;A  \t37\n").unwrap();
    assert_eq!(profile.frame_id("A"), Some(1));
    let error = parse(b"root;   1\n").unwrap_err();
    assert_eq!(
        rmdb_prof_mcp::error::ApiError::from(error).code,
        "invalid_folded_line"
    );
}

#[test]
fn parser_error_matrix_has_recoverable_codes_and_preview() {
    for (input, code) in [
        (b"root;A\n".as_slice(), "invalid_folded_line"),
        (b"root;A nope\n", "invalid_weight"),
        (b"root;A 0\n", "invalid_weight"),
        (b"root;;A 1\n", "invalid_folded_line"),
        (b"root;A 18446744073709551616\n", "weight_overflow"),
    ] {
        let error = parse(input).unwrap_err();
        let api: rmdb_prof_mcp::error::ApiError = error.into();
        assert_eq!(api.code, code);
        assert!(api.details["preview"].is_string());
        assert!(!api.retry_hint.is_empty());
    }
}

#[test]
fn limits_and_fingerprint_are_enforced_and_stable() {
    let depth_error = ProfileBuilder::new(BuildLimits {
        max_depth: 2,
        ..BuildLimits::default()
    })
    .from_reader(Cursor::new(b"a;b;c 1\n"), PathBuf::from("/x"), 8, None)
    .unwrap_err();
    assert_eq!(
        rmdb_prof_mcp::error::ApiError::from(depth_error).code,
        "stack_too_deep"
    );
    let line_error = ProfileBuilder::new(BuildLimits {
        max_line_bytes: 3,
        ..BuildLimits::default()
    })
    .from_reader(Cursor::new(b"a 1\n"), PathBuf::from("/x"), 4, None)
    .unwrap_err();
    assert_eq!(
        rmdb_prof_mcp::error::ApiError::from(line_error).code,
        "invalid_folded_line"
    );
    let too_large_total = ProfileBuilder::new(BuildLimits {
        max_total_weight: 2,
        ..BuildLimits::default()
    })
    .from_reader(Cursor::new(b"a 3\n"), PathBuf::from("/x"), 4, None)
    .unwrap_err();
    assert_eq!(
        rmdb_prof_mcp::error::ApiError::from(too_large_total).code,
        "weight_overflow"
    );
    let streaming_size = ProfileBuilder::new(BuildLimits {
        max_file_bytes: 3,
        ..BuildLimits::default()
    })
    .from_reader(Cursor::new(b"a 1\n"), PathBuf::from("/x"), 3, None)
    .unwrap_err();
    assert_eq!(
        rmdb_prof_mcp::error::ApiError::from(streaming_size).code,
        "profile_too_large"
    );
    assert_eq!(
        parse(b"a 1\n").unwrap().source.fingerprint,
        parse(b"a 1\n").unwrap().source.fingerprint
    );
    assert_ne!(
        parse(b"a 1\n").unwrap().source.fingerprint,
        parse(b"a 2\n").unwrap().source.fingerprint
    );
}
