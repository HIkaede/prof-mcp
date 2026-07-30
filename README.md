# prof-mcp

`prof-mcp` is a workspace-local, read-only stdio MCP server for folded stack
profiles. It does not read `perf.data`, run profilers, render SVG, or execute
shell commands.

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

`prof-mcp PROFILE` remains a compatibility shorthand for
`prof-mcp register PROFILE`. Registration validates the complete input and
stores its exact bytes in `.prof-mcp/profiles/<blake3>.folded`. Re-registering
an alias replaces it and makes it active; byte-identical inputs are deduplicated.

The registry contains a CodeGraph-style `.gitignore`: generated registry data
stays ignored while the small `.gitignore` remains visible.

Inspect or select aliases:

```bash
prof-mcp list
prof-mcp use baseline
```

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
folded input.

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
to the returned frame array. Tree responses expose bounded `continuations`
which can be queried using their `node_id` and the existing fingerprint guard.

For a real folded profile used during validation, the exact selector
`DeletedTupleCandidatesConflictWithInsert` had inclusive weight
`52,552,000,975` (22.9398% of total `229,086,571,313`) and zero self weight.
An `around_target` path window returned absolute `target_positions: [14]` and
display-relative `display_target_positions: [5]`. Registering the same profile
as `baseline` and `candidate`, then running an inclusive diff for that exact
name, returned equal weights and `delta_pp: 0`.

`prof-mcp` is intentionally not an SVG viewer, TUI, HTTP service, SQL/DuckDB
interface, native `perf.data` parser, or profiler runner. Use SVG for global
shape and the registered folded input for complete audit.

Run local gates with:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

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
