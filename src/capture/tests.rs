use std::io::Cursor;

use super::{BuildLimits, collapse_perf_script, parse_stack_line};

#[test]
fn collapses_default_perf_script_and_filters_other_events() {
    let input = b"# header\nworker 12 1.0: 2 cpu/cycles/P:\n  7 leaf+0x4 (/tmp/a)\n  8 caller (/tmp/a)\n\nworker 12 2.0: 9 instructions:\n  7 ignored (/tmp/a)\n\nworker 12 3.0: 3 cpu/cycles/P:\n  7 leaf (/tmp/a)\n  8 caller (/tmp/a)\n";
    let mut output = Vec::new();
    collapse_perf_script(Cursor::new(input), &mut output, BuildLimits::default()).unwrap();
    assert_eq!(output, b"worker;caller;leaf 5\n");
}

#[test]
fn accepts_pid_tid_and_process_names_with_spaces() {
    let input = b"V8 WorkerThread 24636/25607 [000] 1.0: 4 cycles:\n  7 main (/tmp/a)\n\n";
    let mut output = Vec::new();
    collapse_perf_script(Cursor::new(input), &mut output, BuildLimits::default()).unwrap();
    assert_eq!(output, b"V8_WorkerThread;main 4\n");
}

#[test]
fn stack_symbols_preserve_signatures_operators_quotes_and_language_names() {
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
        assert_eq!(parse_stack_line(&line).unwrap(), expected, "{raw}");
    }
}

#[test]
fn collapse_rejects_invalid_nonempty_input_and_unframed_samples() {
    for input in [
        "not perf data\n",
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
fn collapse_enforces_input_line_depth_total_bytes_and_weight_budgets() {
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
fn indented_headers_default_period_and_numeric_process_names_are_supported() {
    let input = b"    worker 2 123 [001] 1.0: cycles:\n  7 leaf (/tmp/a)\n\nworker 2 123 [001] 2.0: 2 cycles:\n  7 leaf (/tmp/a)\n";
    let mut output = Vec::new();
    collapse_perf_script(Cursor::new(input), &mut output, BuildLimits::default()).unwrap();
    assert_eq!(output, b"worker_2;leaf 3\n");
}

#[test]
fn encoded_output_and_aggregated_weights_obey_serialized_limits() {
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
fn writer_failures_are_returned_and_total_weight_overflow_is_rejected() {
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
