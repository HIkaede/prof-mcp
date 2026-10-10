# Repository instructions

## Documentation changes

Do not create or modify documentation unless the user explicitly requests it.

## Scope

`prof-mcp` is a Rust 2024, MSRV 1.88, workspace-local MCP server for folded
stack profiles, with a thin Linux CLI capture pipeline. Keep the implementation
small and preserve the existing read-only query boundary.

## Runtime contract

- `prof-mcp serve` is the MCP stdio entry point.
- MCP queries discover the nearest ancestor `.prof-mcp/manifest.json` on every
  query. A server restart must not be required after registration changes.
- The server exposes exactly these tools, in this order:
  `profile_summary`, `profile_find_symbols`, `profile_top`, `profile_tree`,
  `profile_callers`, `profile_callees`, `profile_paths`, and `profile_diff`.
- Single-profile tools use the active alias by default; diffs require explicit
  baseline and candidate aliases.
- Keep response schema version string `"2"` stable. Any truncation must report
  structured `truncation_reasons`; bounded responses must remain deterministic.
- Percentages, rankings, and diffs describe observed profile data. They are not
  causal performance conclusions, and code must not invent missing DSO/source
  metadata or treat overlapping context observations as a partition.
- Protocol output stays on stdout; diagnostics and logs stay on stderr.
- Tool results are capped at 64 KiB of compact JSON and manifests at 4 MiB.
  Over-limit cases return structured `query_too_large` / `registry_too_large`
  errors; never silently shorten exact frame names or statistics.

## Registry and safety invariants

- Workspace state lives under `.prof-mcp/`; generated profile blobs remain
  ignored and the registry's small `.gitignore` remains visible.
- Profile registration stores exact input bytes under their lowercase BLAKE3
  fingerprint and updates aliases through the manifest. Preserve byte
  deduplication, active-alias validity, and atomic persistence.
- Validate profile size, manifest schema, aliases, fingerprints, source names,
  and registry paths before use. Reject symlinks, non-regular files, and paths
  that escape `.prof-mcp`.
- Registry mutations (`register`, `use`, `remove`, and `gc`) are serialized by the
  persistent advisory lock. Preserve recoverable lock contention. The lock
  requires Unix file identity; claim only Unix/Linux support until non-Unix
  conditional compilation and a CI build check exist.
- `remove` deletes only a manifest alias. Removing the active alias requires an
  existing replacement alias, and the last alias cannot be removed. Blob
  deletion remains the responsibility of `gc`.
- `gc` may remove only unreferenced, regular, fingerprint-named profile blobs;
  it must not rewrite the manifest or active alias. Keep `--dry-run`
  deterministic.
- Query failures must remain structured API errors. Do not turn malformed or
  untrusted profile/registry input into a panic or an arbitrary filesystem
  operation.

## Integration side effects

- Direct invocation with no arguments prints help without persistent side effects;
  an explicit profile argument remains the registration shorthand.
- `capture` is Linux-only and must pass the target command directly to system
  `perf`; it must not invoke a shell. It parses `perf script` output in Rust,
  then reuses `registry::register`, preserving size, parser, alias, and
  atomic-write checks.
- `setup` only updates the Codex MCP registration. It must be idempotent,
  support `--dry-run`, refuse conflicting custom registrations, and avoid a
  partial successful setup.
- Agent guidance is documentation, not a runtime source of truth. Setup must
  preserve global agent instructions.

## Code ownership

- `src/profile/`: folded-input parsing, model construction, and limits.
- `src/registry/`: workspace discovery, manifest validation, locking, and
  atomic persistence.
- `src/capture.rs`: direct Linux perf/folding orchestration only; no profile
  parsing or alternate registry writes.
- `src/query/`: deterministic profile analysis and bounded query semantics.
- `src/server/`: MCP routing, input/output schemas, and response shaping.
- `src/setup.rs`: Codex integration and its rollback/idempotency behavior.

Keep changes in the narrowest responsible module. Update tests when a
user-visible CLI, MCP tool, schema, limit, or persistence rule changes. Avoid
new dependencies or abstractions unless the current modules cannot express
the required behavior clearly.

## Required validation

Run from the repository root. `.github/workflows/ci.yml` is authoritative if
it differs from this list:

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo test --locked --release --test stdio
python3 eval/test_query.py
python3 eval/score.py --self-check
cargo +1.88.0 test --locked --all-targets --all-features
```

CI also runs a short fuzz smoke (see the `fuzz` job); run it when changing
folded parsing or capture collapse. Changes to MCP responses, registry safety,
setup, or filesystem behavior require focused tests in addition to the full
suite. Capture changes require a deterministic fake-perf pipeline test and,
when available, a real Linux perf smoke test.
