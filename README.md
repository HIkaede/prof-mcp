# prof-mcp

`prof-mcp` captures weighted stacks and exposes bounded, structured, deterministic
queries through a workspace-local stdio MCP server. Linux capture invokes system
`perf` directly and folds its script output in Rust.

Place `prof-mcp` on `PATH`, then explicitly register it with Codex:

```bash
prof-mcp setup
```

Running `prof-mcp` with no arguments prints help. Setup updates only the Codex
MCP registration and preserves agent instructions.

Preview the setup plan:

```bash
prof-mcp setup --dry-run
```

Setup uses `codex mcp list --json` and installs only a standard enabled stdio
entry. It safely migrates the old no-argument prof-mcp entry, but refuses a
same-named entry with custom arguments, environment, working directory, or
timeouts rather than overwriting user configuration. Remove or reconfigure
that entry yourself, then run setup again.

The resulting Codex configuration is equivalent to:

```toml
[mcp_servers.prof-mcp]
command = "prof-mcp"
args = ["serve"]
```

Register profiles from an agent workspace:

```bash
cd /path/to/workspace
prof-mcp register ./perf.folded
prof-mcp register ./baseline.folded --name baseline
prof-mcp register ./candidate.folded --name candidate
producer | prof-mcp register - --name streamed
```

Capture and register a profile in one step on Linux:

```bash
prof-mcp capture --name candidate -- ./my-program args...
```

`capture` runs `perf record -g`, streams `perf script` output through the
built-in Rust collapse step, then sends the folded file through the same
validated registration path. Every query identifies its profile by alias and exact
BLAKE3 fingerprint, with `weight_semantics: {"unit":"opaque","basis":"folded_input"}`.
Weights are the scalar values supplied by the folded input. CLI `list` reports
registry paths, byte lengths and source names.

Capture preserves complete function signatures, operators, quotes and language
names. It removes only a terminal `+0xHEX` address offset. Module paths remain part of opaque frame identities, for example `foo [/lib/libA.so]`.
Unknown symbols use the reserved `[unknown@0xADDRESS]` marker. Literal symbol
brackets are escaped, keeping actual names distinct from this marker. Literal `%`, `;`, `[` and `]`
become `%25`, `%3B`, `%5B` and `%5D`, respectively, so folded
separators remain unambiguous; query these encoded names exactly as returned.
Parenthesized names, Java descriptors and Go receivers are retained. Plain
`->` is retained inside a symbol.

The collapse step bounds both the raw script stream and folded output by
`--max-file-size-mib` (default 512 MiB), each input/output line by 8 MiB, each
stack by 4096 frames including the process root, and total selected weight by
`2^53 - 1`. Mixed event types fail explicitly. Empty output, malformed samples, limit violations and perf/I/O
failures fail capture without replacing an existing alias. A failed stream
terminates and reaps `perf script`. Perf's human-readable output varies by
version; unsupported formats fail explicitly. See the
[upstream perf script documentation](https://github.com/torvalds/linux/blob/master/tools/perf/Documentation/perf-script.txt)
for event fields and callchain output.

`prof-mcp PROFILE` is shorthand for
`prof-mcp register PROFILE`. Registration validates the complete input and
stores its exact bytes in `.prof-mcp/profiles/<blake3>.folded`. Input is bounded
and spooled to a temporary file; parsing and byte comparison stream from disk.
Re-registering
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
fingerprint-named blobs. It preserves the manifest and active alias;
unexpected files under `profiles/` are skipped and reported.

Codex starts `prof-mcp serve`. The server discovers the nearest ancestor
`.prof-mcp/manifest.json` on every query, so registrations made after startup
are visible. Tool listing is available before registration; queries return structured
`workspace_not_registered` until one exists.

There are exactly eight tools, in order: `profile_summary`,
`profile_find_symbols`, `profile_top`, `profile_tree`, `profile_callers`,
`profile_callees`, `profile_paths`, and `profile_diff`. Single-profile tools
default to the active alias; diff requires explicit baseline and candidate
aliases.

Results retain string schema version `"2"`. Every `truncated=true` has
structured `truncation_reasons`. Responses return profile facts, bounds,
warnings, and continuation descriptors. `profile_summary` reports the registry root,
active alias, total alias count, and up to 100 aliases with the active alias
first; a larger registry reports `registry_profile_limit`. `profile_find_symbols`
matches folded frame names by substring or regex. Each exact match includes its
frame ID, name, self and inclusive weights, stack count, and profile/scope
percentages. `profile_callers` and `profile_callees` return caller and callee
chains for an exact frame.

`profile_top` ranks exact frame names by self or inclusive weight. Template
arguments and operator spellings are part of each frame's identity: `foo<int>`
and `foo<double>` have separate rows and statistics. Recursive occurrences of
the same frame contribute each stack's weight once to its inclusive weight.

Self ranking intentionally retains zero-self frames when they fall within the
requested ranking: inclusive wrappers can still be useful diagnostic context.

`profile_tree` keeps `max_nodes <= 512`. When it omits children, its structured
`truncation_reasons` identify `node_budget`, `depth_limit`, or
`min_scope_percent` with counts and weight; bounded continuation descriptors
include the node id and profile fingerprint for the next request. Row-limited
queries and cropped path windows likewise report explicit reasons. Treat
percentages and diffs as descriptions of the observed profile data.

`profile_paths` supports display-only windows:

```json
{"through":{"frame_name":"foo"},"frame_window":{"mode":"head","lines":10}}
{"through":{"frame_name":"foo"},"frame_window":{"mode":"tail","lines":10}}
{"through":{"frame_name":"foo"},"frame_window":{"mode":"around_target","before":5,"after":5}}
```

Windows preserve weights, selection, sorting, and absolute
`target_positions`. Each path also reports `display_target_positions`, relative
to the returned frame array. It also has a cross-path `max_total_frames` hard
budget (default `500`, maximum `5000`): candidate paths are still selected and
ordered first, then complete rows are emitted until the budget is reached. The
first overflowing row is cropped deterministically, later rows are omitted,
and `data.total_frame_budget` always records the requested, returned, and
omitted frame/path counts. `total_frame_budget` becomes a truncation reason
only when it omits frames. Tree responses expose bounded `continuations` which
can be queried using their `node_id` and the existing fingerprint guard.

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
plain text.

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
replacement, while `tests/invariants.rs` generates reordered, split and recursive
profiles and checks accounting, paths, diff and continuation algebra. Registry/setup tests use real temporary files and directories.

`tests/stdio.rs` starts the actual Cargo-built binary, checks the MCP handshake,
tool order and live registration, and requires protocol-only stdout with debug
logs on stderr. Exchanges and shutdown are bounded; failure kills and reaps the
child. Run focused tests or the release smoke with:

```bash
cargo test --locked --test queries paths::
cargo test --locked --test mcp --test cache --test invariants
cargo test --locked --release --test stdio
```

CI explicitly selects stable and Rust 1.88 for current-toolchain and MSRV
coverage. Linux capture tests use fake perf and run with ordinary user
permissions. Symlink/setup tests require Unix, and Linux
missing-registry tests use `/dev/shm` to avoid an unrelated `/tmp` registry.

For Inspector, use a config whose command is an absolute `prof-mcp` path and
whose args are `serve`, then set the server working directory to the
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

Parser and collapse fuzz targets, including bounded mutation commands, live in
[fuzz/README.md](fuzz/README.md). The native perf comparison and repeated agent
workflow evaluation tools are documented in [eval/README.md](eval/README.md).

The package is pre-1.0 (0.5.x); its public Rust modules are implementation
interfaces. MCP responses use schema version `"2"`.

## License

This project is licensed under the [MIT License](LICENSE).
