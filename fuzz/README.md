# Parser and collapse fuzzing

`folded` checks deterministic parsing, accounting and configured bounds.
`collapse` checks deterministic success/errors, atomic output on parse failure,
and successful output reparsing. Both use 64 KiB input, 4 KiB lines, depth 64 and
total weight 1,000,000 to exercise boundaries cheaply. Folded parsing also bounds
unique frames at 4096. Seeds cover UTF-8 errors, CRLF, whitespace, missing final
newline, numeric extremes, Unicode, escapes, recursion, long lines, literal path parentheses, unknown-name collisions and mixed events.

Run coverage-guided libFuzzer with stable Rust on x86_64 Linux:

```bash
export RUSTFLAGS='-Cpasses=sancov-module -Cllvm-args=-sanitizer-coverage-level=4 -Cllvm-args=-sanitizer-coverage-inline-8bit-counters -Cllvm-args=-sanitizer-coverage-pc-table -Cllvm-args=-sanitizer-coverage-trace-compares'
cargo +stable build --locked --manifest-path fuzz/Cargo.toml --target x86_64-unknown-linux-gnu
mkdir -p /tmp/prof-fuzz-folded /tmp/prof-fuzz-collapse
cp fuzz/corpus/folded/* /tmp/prof-fuzz-folded/
cp fuzz/corpus/collapse/* /tmp/prof-fuzz-collapse/
fuzz/target/x86_64-unknown-linux-gnu/debug/folded /tmp/prof-fuzz-folded -runs=20000 -max_len=65537 -timeout=3 -rss_limit_mb=512
fuzz/target/x86_64-unknown-linux-gnu/debug/collapse /tmp/prof-fuzz-collapse -runs=20000 -max_len=65537 -timeout=3 -rss_limit_mb=512
```

The explicit target keeps coverage instrumentation out of host build scripts.
When ccache's default directory is read-only, set `CCACHE_DISABLE=1`. Omit
`-runs` for continuous mutation. A nightly toolchain with cargo-fuzz also runs
these standard targets with its sanitizers, using `cargo +nightly fuzz run folded`
or `collapse`.

Local validation ran each target for 20,000 mutations (seed 1), with coverage
counters active and no crashes, timeouts or RSS-limit failures. Peak reported RSS
was at most 34 MiB. This stable run instruments coverage without address
sanitization; libFuzzer reports missing sanitizer diagnostic hooks accordingly.
CI repeats bounded coverage-guided runs. Generated query invariants are exercised
separately by `cargo test --test invariants --test recursion`.
