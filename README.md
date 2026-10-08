# prof-mcp

`prof-mcp` is a workspace-local, read-only stdio MCP server for folded stack
profiles, plus a thin Linux CLI capture pipeline. The MCP server does not read
`perf.data`, render SVG, or execute shell commands; `capture` directly invokes
the system `perf` and collapses its script output in Rust.

Run it once after placing `prof-mcp` on `PATH`. Direct invocation idempotently
installs the Codex MCP entry and a small managed block in the global Codex
`AGENTS.md`:

```bash
prof-mcp
```

Preview this operation without changing Codex or `AGENTS.md`:

```bash
prof-mcp setup --dry-run
```

Setup uses `codex mcp list --json` and installs only a standard enabled stdio
entry. It safely migrates the old no-argument prof-mcp entry, but refuses a
same-named entry with custom arguments, environment, working directory, or
timeouts rather than overwriting user configuration. Remove or reconfigure
that entry yourself, then run setup again. There is intentionally no `--force`.

The resulting Codex configuration is equivalent to:

```toml
[mcp_servers.prof-mcp]
command = "prof-mcp"
args = ["serve", "--mcp"]
```

Register profiles from an agent workspace:

```bash
cd /path/to/workspace
prof-mcp register ./perf.folded
prof-mcp register ./baseline.folded --name baseline
prof-mcp register ./candidate.folded --name candidate
```

Capture and register a profile in one step on Linux:

```bash
prof-mcp capture --name candidate --sample-period-us 1000 -- ./my-program args...
```

`capture` runs `perf record -g`, streams `perf script` output through the
built-in Rust collapse step, then sends the folded file through the same
validated registration path. The sampling-period option is recorded as profile
metadata; the capture command otherwise uses perf's normal sampling
configuration. It neither sets perf's sampling period nor rescales folded
weights. Every query reports `profile.weight_semantics` with `unit: "opaque"`
and `basis: "folded_input"`; a declared `sample_period_us` appears there as
metadata. Diffs retain each side's own declaration. The raw weights are never
labeled as elapsed microseconds.

Capture preserves complete function signatures, operators, quotes and language
names. It removes only a terminal `+0xHEX` address offset. Literal `%` and `;`
in captured frame names become `%25` and `%3B`, respectively, so folded
separators remain unambiguous; query these encoded names exactly as returned.
Parenthesized names, Java descriptors and Go receivers are retained. Plain
`->` is retained inside a symbol rather than treated as an inline-stack marker.

The collapse step bounds both the raw script stream and folded output by
`--max-file-size-mib` (default 512 MiB), each input/output line by 8 MiB, each
stack by 4096 frames including the process root, and total selected weight by
`2^53 - 1`. It keeps the first observed event type and filters other event
types. Empty output, malformed selected samples, limit violations and perf/I/O
failures fail capture without replacing an existing alias. A failed stream
terminates and reaps `perf script`. Perf's human-readable output varies by
version; unsupported formats fail explicitly. See the
[upstream perf script documentation](https://github.com/torvalds/linux/blob/master/tools/perf/Documentation/perf-script.txt)
for event fields and callchain output.

`prof-mcp PROFILE` remains a compatibility shorthand for
`prof-mcp register PROFILE`. Registration validates the complete input and
stores its exact bytes in `.prof-mcp/profiles/<blake3>.folded`. Re-registering
an alias replaces it and makes it active; byte-identical inputs are deduplicated.
Registry updates use a persistent advisory lock, so a killed process releases
its lock automatically rather than requiring lock-file cleanup. The current
file-identity backend is available on Unix platforms; on non-Unix platforms
registry mutations (`register`, `use`, and `gc`) fail closed rather than risk
locking two replaced lock files independently.

The registry contains a CodeGraph-style `.gitignore`: generated registry data
stays ignored while the small `.gitignore` remains visible.

Inspect or select aliases:

```bash
prof-mcp list
prof-mcp use baseline
prof-mcp remove candidate
prof-mcp remove baseline --new-active candidate
prof-mcp gc --dry-run
prof-mcp gc
```

`remove` deletes only an alias from the manifest. Removing the active alias
requires `--new-active`; the last alias cannot be removed. Profile blobs remain
until `gc` finds them unreferenced.

`gc` discovers the nearest registry, reports a deterministic deletion plan
with `--dry-run`, and removes only unreferenced, regular,
fingerprint-named blobs. It never rewrites the manifest or active alias;
unexpected files under `profiles/` are skipped and reported.

Codex starts `prof-mcp serve --mcp`. The server discovers the nearest ancestor
`.prof-mcp/manifest.json` on every query, so registrations made after startup
are visible. No registry is needed for tool listing; queries return structured
`workspace_not_registered` until one exists.

There are exactly eight tools, in order: `profile_summary`,
`profile_find_symbols`, `profile_top`, `profile_tree`, `profile_callers`,
`profile_callees`, `profile_paths`, and `profile_diff`. Single-profile tools
default to the active alias; diff requires explicit baseline and candidate
aliases.

Results retain string schema version `"2"`. Every `truncated=true` has
structured `truncation_reasons`. `profile_summary` reports the registry root,
active alias, total alias count, and up to 100 aliases with the active alias
first; a larger registry reports `registry_profile_limit`. `profile_find_symbols` reports observed immediate
caller/callee context, but never invents DSO or source metadata absent from the
folded input. Context entries are non-exclusive observations across contributing
stacks, so their weights must not be summed as a partition of scope.

Self ranking intentionally retains zero-self frames when they fall within the
requested ranking: inclusive wrappers can still be useful diagnostic context.

`profile_tree` keeps `max_nodes <= 512`. When it omits children, its structured
`truncation_reasons` identify `node_budget`, `depth_limit`, or
`min_scope_percent` with counts and weight; bounded continuation descriptors
include the node id and profile fingerprint for the next request. Row-limited
queries and cropped path windows likewise report explicit reasons. Treat
percentages and diffs as descriptive rather than causal conclusions.

`profile_paths` supports display-only windows:

```json
{"through":{"frame_name":"foo"},"frame_window":{"mode":"head","lines":10}}
{"through":{"frame_name":"foo"},"frame_window":{"mode":"tail","lines":10}}
{"through":{"frame_name":"foo"},"frame_window":{"mode":"around_target","before":5,"after":5}}
```

Windows do not change weights, selection, sorting, or absolute
`target_positions`. Each path also reports `display_target_positions`, relative
to the returned frame array. It also has a cross-path `max_total_frames` hard
budget (default `500`, maximum `5000`): candidate paths are still selected and
ordered first, then complete rows are emitted until the budget is reached. The
first overflowing row is cropped deterministically, later rows are omitted,
and `data.total_frame_budget` always records the requested, returned, and
omitted frame/path counts. `total_frame_budget` becomes a truncation reason
only when it omits frames. Tree responses expose bounded `continuations` which
can be queried using their `node_id` and the existing fingerprint guard.

For a real folded profile used during validation, the exact selector
`DeletedTupleCandidatesConflictWithInsert` had inclusive weight
`52,552,000,975` (22.9398% of total `229,086,571,313`) and zero self weight.
An `around_target` path window returned absolute `target_positions: [14]` and
display-relative `display_target_positions: [5]`. Registering the same profile
as `baseline` and `candidate`, then running an inclusive diff for that exact
name, returned equal weights and `delta_pp: 0`.

`prof-mcp` is intentionally not an SVG viewer, TUI, HTTP service, SQL/DuckDB
interface, or native `perf.data` parser. Use SVG for global shape and the
registered folded input for complete audit.

Build the stripped release binary with the pinned toolchain in
`rust-toolchain.toml`:

```bash
cargo build --locked --release
```

Development uses Rust 1.99.0; the minimum supported version remains Rust 1.88.
The release profile already enables size optimization and LTO. The stdio server
uses Tokio's current-thread runtime; its blocking profile parser still runs on
the blocking pool. Dependency features retain Unicode regex queries, schema
generation, log filtering, and stderr diagnostics. CLI help and errors use
plain text; optional colors and typo suggestions are disabled.

Run local gates with:

```bash
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo +1.88.0 test --locked --all-targets --all-features
```

Tests are organized by boundary: private parser/query/persistence rules live in
module unit tests; `tests/queries/` groups public query behavior by paths, tree,
and symbols/top; `tests/mcp/` checks schemas, independent client types,
errors, and live registration. `tests/cache.rs` checks shared blobs and alias
replacement, while `tests/invariants.rs` enumerates small reordered and split
inputs. Registry/setup tests use real temporary files and directories.

`tests/stdio.rs` starts the actual Cargo-built binary, checks the MCP handshake,
tool order and live registration, and requires protocol-only stdout with debug
logs on stderr. Exchanges and shutdown are bounded; failure kills and reaps the
child. Run focused tests or the release smoke with:

```bash
cargo test --locked --test queries paths::
cargo test --locked --test mcp --test cache --test invariants
cargo test --locked --release --test stdio
```

CI explicitly selects stable and Rust 1.88 so the local toolchain pin cannot
mask the MSRV job. Linux capture tests use fake perf; ordinary tests do not
require profiler permissions. Symlink/setup tests require Unix, and Linux
missing-registry tests use `/dev/shm` to avoid an unrelated `/tmp` registry.

For Inspector, use a config whose command is an absolute `prof-mcp` path and
whose args are `serve --mcp`, then set the server working directory to the
registered workspace:

```bash
npx --yes @modelcontextprotocol/inspector@2.0.0 \
  --cli --config /absolute/path/to/inspector.config.json --server prof-mcp \
  --cwd /path/to/workspace \
  --method tools/list \
  --format json
```

The checked-in `inspector.config.json` is for repository-root debug use; set
its command to your installed absolute binary when inspecting another workspace.

## License

This project is licensed under the [MIT License](LICENSE).
