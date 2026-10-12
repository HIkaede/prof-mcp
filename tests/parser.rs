mod support;

use std::{io::Cursor, path::PathBuf};

use prof_mcp::profile::{BuildLimits, ProfileBuilder};

fn parse(input: &[u8]) -> Result<prof_mcp::profile::Profile, prof_mcp::error::ProfileError> {
    ProfileBuilder::new(BuildLimits::default())
        .from_reader(Cursor::new(input), PathBuf::from("/fixture.folded"))
}

#[test]
fn parse_folded_whitespace() {
    let profile = parse(b"\r\nroot;frame with spaces 3 \t\r\nroot;frame with spaces 7\n").unwrap();
    assert_eq!(profile.total_weight(), 10);
    assert_eq!(profile.stacks().len(), 1);
    assert_eq!(
        profile.frame_name(profile.frame_id("frame with spaces").unwrap()),
        "frame with spaces"
    );
}

#[test]
fn randomized_builder_hashing_preserves_frame_ids_and_stack_order() {
    let input = b"root;zeta;leaf 2\nroot;alpha 3\nroot;zeta;leaf 5\nroot;alpha;zeta 7\n";
    let expected = parse(input).unwrap();
    for _ in 0..16 {
        let actual = parse(input).unwrap();
        assert_eq!(actual.frames().name_text(), expected.frames().name_text());
        assert_eq!(
            actual.frames().name_offsets(),
            expected.frames().name_offsets()
        );
        assert_eq!(actual.stacks().offsets(), expected.stacks().offsets());
        assert_eq!(actual.stacks().frames(), expected.stacks().frames());
        assert_eq!(actual.stacks().weights(), expected.stacks().weights());
        assert_eq!(actual.cct().child_offsets(), expected.cct().child_offsets());
        assert_eq!(actual.cct().child_ids(), expected.cct().child_ids());
    }
    assert_eq!(expected.stacks().weights(), &[3, 7, 7]);
}

#[test]
fn trim_weight_delimiter() {
    let profile = parse(b"root;A  \t37\n").unwrap();
    assert_eq!(profile.frame_id("A"), Some(0));
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
    .from_reader(Cursor::new(b"a;b;c 1\n"), PathBuf::from("/x"))
    .unwrap_err();
    assert_eq!(
        prof_mcp::error::ApiError::from(depth_error).code,
        "stack_too_deep"
    );
    let line_error = ProfileBuilder::new(BuildLimits {
        max_line_bytes: 3,
        ..BuildLimits::default()
    })
    .from_reader(Cursor::new(b"a 1\n"), PathBuf::from("/x"))
    .unwrap_err();
    assert_eq!(
        prof_mcp::error::ApiError::from(line_error).code,
        "invalid_folded_line"
    );
    let too_large_total = ProfileBuilder::new(BuildLimits {
        max_total_weight: 2,
        ..BuildLimits::default()
    })
    .from_reader(Cursor::new(b"a 3\n"), PathBuf::from("/x"))
    .unwrap_err();
    assert_eq!(
        prof_mcp::error::ApiError::from(too_large_total).code,
        "weight_overflow"
    );
    let streaming_size = ProfileBuilder::new(BuildLimits {
        max_file_bytes: 3,
        ..BuildLimits::default()
    })
    .from_reader(Cursor::new(b"a 1\n"), PathBuf::from("/x"))
    .unwrap_err();
    assert_eq!(
        prof_mcp::error::ApiError::from(streaming_size).code,
        "profile_too_large"
    );
    assert_eq!(
        parse(b"a 1\n").unwrap().fingerprint(),
        parse(b"a 1\n").unwrap().fingerprint()
    );
    assert_ne!(
        parse(b"a 1\n").unwrap().fingerprint(),
        parse(b"a 2\n").unwrap().fingerprint()
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
        assert_eq!(profile.total_weight(), 3);
        assert!(profile.frame_id(name).is_some(), "{name}");
        assert_eq!(profile.stacks().stack(0).frames.len(), 3);
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
        .from_reader(Cursor::new(input), PathBuf::from("/x"))
        .unwrap();
    assert_eq!(profile.total_weight(), 5);
    assert_eq!(profile.stacks().len(), 1);
    assert_eq!(profile.cct().nodes().len(), 3);
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
            .from_reader(Cursor::new(input), PathBuf::from("/x"))
            .unwrap_err();
        let error = prof_mcp::error::ApiError::from(error);
        assert_eq!(error.code, "profile_model_too_large");
        assert_eq!(error.details["resource"], resource);
        assert!(
            error.details["actual"].as_u64().unwrap() > error.details["limit"].as_u64().unwrap()
        );
    }
}

#[test]
fn frozen_ids_are_canonical_across_input_order() {
    let a = parse(b"zeta;root 2\nalpha;zeta 3\nalpha 5\n").unwrap();
    let b = parse(b"alpha 5\nalpha;zeta 3\nzeta;root 2\n").unwrap();
    assert_eq!(
        a.frames().iter().collect::<Vec<_>>(),
        ["alpha", "root", "zeta"]
    );
    assert_eq!(a.frames().name_text(), b.frames().name_text());
    assert_eq!(a.stacks().frames(), b.stacks().frames());
    assert_eq!(a.stacks().weights(), b.stacks().weights());
    assert_eq!(a.postings().entries(), b.postings().entries());
}

#[test]
fn configurable_depth_cannot_overflow_posting_positions() {
    let limits = BuildLimits {
        max_depth: 65537,
        ..BuildLimits::default()
    };
    let build = |depth| {
        let input = format!("{} 1\n", vec!["a"; depth].join(";"));
        ProfileBuilder::new(limits).from_reader(Cursor::new(input.as_bytes()), PathBuf::from("/x"))
    };
    let profile = build(65536).unwrap();
    assert_eq!(profile.postings().positions(0, 0), (0, u16::MAX));
    assert_eq!(
        prof_mcp::error::ApiError::from(build(65537).unwrap_err()).code,
        "stack_too_deep"
    );
}

#[test]
fn posting_positions_compress_constants_and_compact_variable_ranges() {
    let profile = parse(b"root;A;A;leaf 9\nroot;B;A;leaf 8\nroot;B;leaf 7\n").unwrap();
    let postings = profile.postings();
    assert_eq!(postings.position_offsets(), [0, 2, 2, 5, 5]);
    assert_eq!(postings.constant_positions(), [0, 1 | (1 << 16), 0, 0]);
    assert_eq!(postings.variable_positions().len(), 5);
    let a = profile.frame_id("A").unwrap() as usize;
    assert_eq!(postings.positions(a, 0), (1, 2));
    assert_eq!(postings.positions(a, 1), (2, 2));
    let leaf = profile.frame_id("leaf").unwrap() as usize;
    assert_eq!(postings.positions(leaf, 0), (3, 3));
    assert_eq!(postings.positions(leaf, 1), (3, 3));
    assert_eq!(postings.positions(leaf, 2), (2, 2));
    let root = profile.frame_id("root").unwrap() as usize;
    for index in 0..postings[root].len() {
        assert_eq!(postings.positions(root, index), (0, 0));
    }
}
