mod support;

use std::{io::Cursor, path::PathBuf};

use prof_mcp::profile::{BuildLimits, ProfileBuilder};

fn parse(input: &[u8]) -> Result<prof_mcp::profile::Profile, prof_mcp::error::ProfileError> {
    ProfileBuilder::new(BuildLimits::default()).from_reader(
        Cursor::new(input),
        PathBuf::from("/fixture.folded"),
        input.len() as u64,
        None,
    )
}

#[test]
fn parse_folded_whitespace() {
    let profile = parse(b"\r\nroot;frame with spaces 3 \t\r\nroot;frame with spaces 7\n").unwrap();
    assert_eq!(profile.total_weight, 10);
    assert_eq!(profile.stacks.len(), 1);
    assert_eq!(
        profile.frame_name(profile.frame_id("frame with spaces").unwrap()),
        "frame with spaces"
    );
}

#[test]
fn trim_weight_delimiter() {
    let profile = parse(b"root;A  \t37\n").unwrap();
    assert_eq!(profile.frame_id("A"), Some(1));
    let error = parse(b"root;   1\n").unwrap_err();
    assert_eq!(
        prof_mcp::error::ApiError::from(error).code,
        "invalid_folded_line"
    );
}

#[test]
fn parser_errors_and_previews() {
    for (input, code) in [
        (b"root;A\n".as_slice(), "invalid_folded_line"),
        (b"root;A nope\n", "invalid_weight"),
        (b"root;A 0\n", "invalid_weight"),
        (b"root;;A 1\n", "invalid_folded_line"),
        (b"root;A 18446744073709551616\n", "weight_overflow"),
    ] {
        let error = parse(input).unwrap_err();
        let api: prof_mcp::error::ApiError = error.into();
        assert_eq!(api.code, code);
        assert!(api.details["preview"].is_string());
        assert!(!api.retry_hint.is_empty());
    }
}

#[test]
fn parser_limits_and_fingerprint() {
    let depth_error = ProfileBuilder::new(BuildLimits {
        max_depth: 2,
        ..BuildLimits::default()
    })
    .from_reader(Cursor::new(b"a;b;c 1\n"), PathBuf::from("/x"), 8, None)
    .unwrap_err();
    assert_eq!(
        prof_mcp::error::ApiError::from(depth_error).code,
        "stack_too_deep"
    );
    let line_error = ProfileBuilder::new(BuildLimits {
        max_line_bytes: 3,
        ..BuildLimits::default()
    })
    .from_reader(Cursor::new(b"a 1\n"), PathBuf::from("/x"), 4, None)
    .unwrap_err();
    assert_eq!(
        prof_mcp::error::ApiError::from(line_error).code,
        "invalid_folded_line"
    );
    let too_large_total = ProfileBuilder::new(BuildLimits {
        max_total_weight: 2,
        ..BuildLimits::default()
    })
    .from_reader(Cursor::new(b"a 3\n"), PathBuf::from("/x"), 4, None)
    .unwrap_err();
    assert_eq!(
        prof_mcp::error::ApiError::from(too_large_total).code,
        "weight_overflow"
    );
    let streaming_size = ProfileBuilder::new(BuildLimits {
        max_file_bytes: 3,
        ..BuildLimits::default()
    })
    .from_reader(Cursor::new(b"a 1\n"), PathBuf::from("/x"), 3, None)
    .unwrap_err();
    assert_eq!(
        prof_mcp::error::ApiError::from(streaming_size).code,
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

#[test]
fn parse_unicode_frames() {
    for (name, delimiter) in [
        ("函数", " "),
        ("🙂", "\t"),
        ("é", "  \t"),
        ("函数🙂", "\t "),
    ] {
        let input = format!("root;foo;{name}{delimiter}3\r\n");
        let profile = parse(input.as_bytes()).unwrap();
        assert_eq!(profile.total_weight, 3);
        assert!(profile.frame_id(name).is_some(), "{name}");
        assert_eq!(profile.stacks[0].frames.len(), 3);
    }
    for input in ["root;函数 nope\n", "root;函数 0\n"] {
        assert_eq!(
            prof_mcp::error::ApiError::from(parse(input.as_bytes()).unwrap_err()).code,
            "invalid_weight"
        );
    }
}

#[test]
fn model_limits() {
    let input = b"a;b 2\na;b 3\n";
    let limits = BuildLimits {
        max_model_bytes: 2000,
        max_cct_nodes: 3,
        max_stack_frames: 2,
        ..BuildLimits::default()
    };
    let profile = ProfileBuilder::new(limits)
        .from_reader(Cursor::new(input), PathBuf::from("/x"), 0, None)
        .unwrap();
    assert_eq!(profile.total_weight, 5);
    assert_eq!(profile.stacks.len(), 1);
    assert_eq!(profile.cct.nodes.len(), 3);
    assert!(profile.estimated_size_bytes() > input.len());

    for (limits, resource) in [
        (
            BuildLimits {
                max_model_bytes: 450,
                ..BuildLimits::default()
            },
            "model_bytes",
        ),
        (
            BuildLimits {
                max_cct_nodes: 2,
                ..BuildLimits::default()
            },
            "cct_nodes",
        ),
        (
            BuildLimits {
                max_stack_frames: 1,
                ..BuildLimits::default()
            },
            "stack_frames",
        ),
    ] {
        let error = ProfileBuilder::new(limits)
            .from_reader(Cursor::new(input), PathBuf::from("/x"), 0, None)
            .unwrap_err();
        let error = prof_mcp::error::ApiError::from(error);
        assert_eq!(error.code, "profile_model_too_large");
        assert_eq!(error.details["resource"], resource);
        assert!(
            error.details["actual"].as_u64().unwrap() > error.details["limit"].as_u64().unwrap()
        );
    }
}
