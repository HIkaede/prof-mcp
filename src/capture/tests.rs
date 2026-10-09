use std::io::Cursor;

use super::{BuildLimits, collapse_perf_script, parse_stack_line};

#[test]
fn reject_mixed_events() {
    for (first, second) in [("cycles", "instructions"), ("instructions", "cycles")] {
        let input = format!(
            "worker 12 1.0: 2 {first}:\n  7 leaf (/tmp/a)\n\nworker 12 2.0: 9 {second}:\n  7 leaf (/tmp/a)\n"
        );
        let mut output = Vec::new();
        let error = collapse_perf_script(Cursor::new(input), &mut output, BuildLimits::default())
            .unwrap_err();
        assert!(error.to_string().contains("multiple event types"));
        assert!(output.is_empty());
    }
}

#[test]
fn parse_process_names() {
    let input = b"V8 WorkerThread 24636/25607 [000] 1.0: 4 cycles:\n  7 main (/tmp/a)\n\n";
    let mut output = Vec::new();
    collapse_perf_script(Cursor::new(input), &mut output, BuildLimits::default()).unwrap();
    assert_eq!(output, b"V8 WorkerThread;main [/tmp/a] 4\n");
}

#[test]
fn process_whitespace() {
    let input = "a  b 12 1.0: 1 cycles:\n  7 leaf (/tmp/a)\n\na b 12 [000] 2.0: 2 cycles:\n  7 leaf (/tmp/a)\n\na\tb 12/13 [001] 3.0: 3 cycles:\n  7 leaf (/tmp/a)\n";
    let mut output = Vec::new();
    collapse_perf_script(Cursor::new(input), &mut output, BuildLimits::default()).unwrap();
    assert_eq!(
        output,
        b"a\tb;leaf [/tmp/a] 3\na  b;leaf [/tmp/a] 1\na b;leaf [/tmp/a] 2\n"
    );
}

#[test]
fn preserve_symbol_names() {
    for (raw, expected) in [
        ("overload(int)+0x4", "overload(int)"),
        ("overload(double)", "overload(double)"),
        ("Type::operator->()", "Type::operator->()"),
        ("Type::operator()(int)", "Type::operator()(int)"),
        (
            "(anonymous namespace)::work(int)",
            "(anonymous namespace)::work(int)",
        ),
        ("quoted<'x', \"y\">()", "quoted<'x', \"y\">()"),
        ("pkg.(*Type).Method(int)", "pkg.(*Type).Method(int)"),
        ("Lpkg/Type;::method(I)V", "Lpkg/Type%3B::method(I)V"),
        ("name%3B;tail", "name%253B%3Btail"),
        ("literal+0x", "literal+0x"),
    ] {
        let line = format!("  7 {raw} (/tmp/a)");
        assert_eq!(
            parse_stack_line(&line).unwrap(),
            format!("{expected} [/tmp/a]"),
            "{raw}"
        );
    }
}

#[test]
fn reject_malformed_samples() {
    for input in [
        "not perf data\n",
        "worker 12 1.0: 1 cycles:\n  7 foo @(/tmp/lib(foo.so)\n",
        "worker 12 1.0: 1 cycles:\n  broken stack line\n",
        "worker 12 1.0: 1 cycles:\n\n",
        "worker 12 1.0: 0 cycles:\n  7 leaf (/tmp/a)\n",
        "worker 12 1.0: 18446744073709551616 cycles:\n  7 leaf (/tmp/a)\n",
    ] {
        let mut output = Vec::new();
        assert!(
            collapse_perf_script(Cursor::new(input), &mut output, BuildLimits::default()).is_err(),
            "{input}"
        );
        assert!(output.is_empty());
    }
}

#[test]
fn enforce_capture_limits() {
    let input = "worker 12 1.0: 3 cycles:\n  7 leaf (/tmp/a)\n  8 root (/tmp/a)\n\n";
    for limits in [
        BuildLimits {
            max_line_bytes: 8,
            ..BuildLimits::default()
        },
        BuildLimits {
            max_depth: 2,
            ..BuildLimits::default()
        },
        BuildLimits {
            max_file_bytes: input.len() as u64 - 1,
            ..BuildLimits::default()
        },
        BuildLimits {
            max_total_weight: 2,
            ..BuildLimits::default()
        },
    ] {
        let mut output = Vec::new();
        assert!(
            collapse_perf_script(Cursor::new(input), &mut output, limits).is_err(),
            "{limits:?}"
        );
        assert!(output.is_empty());
    }
}

#[test]
fn parse_header_variants() {
    let input = b"    worker 2 123 [001] 1.0: cycles:\n  7 leaf (/tmp/a)\n\nworker 2 123 [001] 2.0: 2 cycles:\n  7 leaf (/tmp/a)\n";
    let mut output = Vec::new();
    collapse_perf_script(Cursor::new(input), &mut output, BuildLimits::default()).unwrap();
    assert_eq!(output, b"worker 2;leaf [/tmp/a] 3\n");
}

#[test]
fn bound_folded_output() {
    let input = format!(
        "worker 12 1.0: 1 cycles:\n  7 {} (/tmp/a)\n",
        ";".repeat(100)
    );
    let mut output = Vec::new();
    let error = collapse_perf_script(
        Cursor::new(&input),
        &mut output,
        BuildLimits {
            max_file_bytes: input.len() as u64,
            ..BuildLimits::default()
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("output byte limit"));
    assert!(output.is_empty());

    let sample = format!(
        "{} 12 1.0: 9 cycles:\n  7 {} (/tmp/a)\n\n",
        "p".repeat(30),
        "f".repeat(30)
    );
    let input = sample.repeat(2);
    let mut output = Vec::new();
    let error = collapse_perf_script(
        Cursor::new(input),
        &mut output,
        BuildLimits {
            max_line_bytes: 64,
            ..BuildLimits::default()
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("line byte limit"));
    assert!(output.is_empty());
}

#[test]
fn reject_write_errors_and_overflow() {
    struct BrokenWriter;
    impl std::io::Write for BrokenWriter {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("injected write failure"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let input = "worker 12 1.0: 1 cycles:\n  7 leaf (/tmp/a)\n";
    let error = collapse_perf_script(
        Cursor::new(input),
        &mut BrokenWriter,
        BuildLimits::default(),
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("injected write failure"));
    let input = "worker 12 1.0: 18446744073709551615 cycles:\n  7 leaf (/tmp/a)\n\nworker 12 2.0: 1 cycles:\n  7 leaf (/tmp/a)\n";
    let mut output = Vec::new();
    assert!(
        collapse_perf_script(
            Cursor::new(input),
            &mut output,
            BuildLimits {
                max_total_weight: u64::MAX,
                ..BuildLimits::default()
            }
        )
        .is_err()
    );
    assert!(output.is_empty());
}

#[test]
fn preserve_frame_identity() {
    let input = "worker 12 1.0: 1 cycles:\n  7 foo (/a/lib.so)\n\nworker 12 2.0: 1 cycles:\n  7 foo (/b/lib.so)\n\nworker 12 3.0: 1 cycles:\n  7 [unknown] ([unknown])\n\nworker 12 4.0: 1 cycles:\n  8 [unknown] ([unknown])\n";
    let mut output = Vec::new();
    collapse_perf_script(Cursor::new(input), &mut output, BuildLimits::default()).unwrap();
    let text = String::from_utf8(output).unwrap();
    assert_eq!(text.lines().count(), 4);
    assert!(text.contains("foo [/a/lib.so]"));
    assert!(text.contains("foo [/b/lib.so]"));
    assert!(text.contains("[unknown@0x7] [%5Bunknown%5D]"));
    assert!(text.contains("[unknown@0x8] [%5Bunknown%5D]"));
    assert_ne!(
        parse_stack_line("  7 foo [bar] (/a)").unwrap(),
        parse_stack_line("  7 foo (/a [bar])").unwrap()
    );
    assert!(parse_stack_line("  7 foo ()").is_none());
}

#[test]
fn preserve_deleted_modules() {
    for pc in ["7", "8"] {
        assert_eq!(
            parse_stack_line(&format!("  {pc} [unknown] (/tmp/lib (deleted))")),
            Some(format!("[unknown@0x{pc}] [/tmp/lib (deleted)]"))
        );
    }
    assert_eq!(
        parse_stack_line("  7 operator() (int) (/tmp/lib (copy).so)"),
        Some("operator() (int) [/tmp/lib (copy).so]".into())
    );
    for module in ["libfoo.so (deleted)", "lib (copy).so", "[kernel.kallsyms]"] {
        assert_eq!(
            parse_stack_line(&format!("  7 foo ({module})")),
            Some(format!("foo [{}]", super::encode_frame(module)))
        );
    }
    assert!(parse_stack_line("  7 foo (/tmp/lib (deleted").is_none());
}

#[test]
fn preserve_path_parentheses() {
    for module in [
        "/tmp/lib(foo.so",
        "/tmp/lib)foo.so",
        "/tmp/lib (foo.so",
        "/tmp/lib foo).so",
        "/tmp/lib (copy).so",
        "/tmp/lib(foo.so (deleted)",
        "/tmp/dir (/nested)/lib.so",
        "/tmp/dir ([nested])/lib.so",
    ] {
        let input = format!("worker 12 1.0: 1 cycles:\n  7 foo ({module})\n");
        let mut output = Vec::new();
        collapse_perf_script(Cursor::new(input), &mut output, BuildLimits::default()).unwrap();
        assert_eq!(
            output,
            format!("worker;foo [{}] 1\n", super::encode_frame(module)).as_bytes()
        );
    }
}

#[test]
fn symbol_path_prefixes() {
    for symbol in ["foo (/argument)", "foo ([argument])"] {
        for module in [
            "/tmp/lib.so",
            "lib.so",
            "[kernel.kallsyms]",
            "/tmp/lib(foo.so",
            "/tmp/lib)foo.so",
            "/tmp/dir (/nested)/lib.so",
        ] {
            assert_eq!(
                parse_stack_line(&format!("  7 {symbol} ({module})")),
                Some(format!(
                    "{} [{}]",
                    super::encode_frame(symbol),
                    super::encode_frame(module)
                ))
            );
        }
    }
}

#[test]
fn unknown_names_are_distinct() {
    let frames = [
        ("7 [unknown]", "[unknown@0x7]"),
        ("8 [unknown]", "[unknown@0x8]"),
        ("9 [unknown]@0x7", "%5Bunknown%5D@0x7"),
        ("a [unknown@0x7]", "%5Bunknown@0x7%5D"),
        ("b %5Bunknown@0x7%5D", "%255Bunknown@0x7%255D"),
    ];
    let input = frames
        .iter()
        .enumerate()
        .map(|(index, (frame, _))| {
            format!(
                "worker 12 1.0: {} cycles:\n  {frame} (/tmp/a)\n\n",
                index + 1
            )
        })
        .collect::<String>();
    let mut output = Vec::new();
    collapse_perf_script(Cursor::new(input), &mut output, BuildLimits::default()).unwrap();
    let text = String::from_utf8(output).unwrap();
    assert_eq!(text.lines().count(), frames.len());
    for (index, (_, name)) in frames.iter().enumerate() {
        assert!(
            text.lines()
                .any(|line| line == format!("worker;{name} [/tmp/a] {}", index + 1))
        );
    }
}
