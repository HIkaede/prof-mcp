# prof-mcp

`prof-mcp` is a workspace-local, read-only stdio MCP server for folded stack profiles. It parses only folded input; it does not read `perf.data`, run profilers, render SVG, or execute shell commands.

Build once, then register profiles from an agent workspace:

```bash
cargo build --release --locked
cd /path/to/workspace
/path/to/prof-mcp ./perf.folded
/path/to/prof-mcp ./baseline.folded --name baseline
/path/to/prof-mcp ./candidate.folded --name candidate
```

Registration validates the complete folded input and stores its exact bytes in `.prof-mcp/profiles/<blake3>.folded`. `.prof-mcp/manifest.json` contains aliases and the active profile, never the original absolute source path. Re-registering an alias replaces it and makes it active; byte-identical inputs are deduplicated.

Configure an MCP client globally with no root argument:

```json
{"mcpServers":{"prof-mcp":{"command":"/path/to/prof-mcp"}}}
```

The running server discovers the nearest ancestor `.prof-mcp/manifest.json` on each query, so profiles registered after startup are visible. No registry is needed for tool listing; queries return structured `workspace_not_registered` until one exists.

There are exactly eight tools, in order: `profile_summary`, `profile_find_symbols`, `profile_top`, `profile_tree`, `profile_callers`, `profile_callees`, `profile_paths`, and `profile_diff`. Single-profile `profile` is an optional alias and defaults to active. Diff always requires `baseline` and `candidate` aliases. Query results use schema version `2` and identify the chosen alias in profile metadata.

Use `profile_find_symbols` before exact frame selectors. For complete heavy paths, `profile_paths` supports display-only windows:

```json
{"through":{"frame_name":"foo"},"frame_window":{"mode":"head","lines":10}}
{"through":{"frame_name":"foo"},"frame_window":{"mode":"tail","lines":10}}
{"through":{"frame_name":"foo"},"frame_window":{"mode":"around_target","before":5,"after":5}}
```

Windows keep full-stack weight, selection, sort order, and absolute recursive `target_positions`; each returned path reports the displayed range and omissions. For up/down context use `profile_callers(max_depth=5)` and `profile_callees(max_depth=5)`.

Run local gates with:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

For Inspector from a registered workspace, invoke the release binary directly (replace the absolute path with your installation):

```bash
npx --yes @modelcontextprotocol/inspector@2.0.0 --cli \
  /absolute/path/to/prof-mcp \
  --method tools/list --format json
```

For a tool call, for example:

```bash
npx --yes @modelcontextprotocol/inspector@2.0.0 --cli \
  /absolute/path/to/prof-mcp \
  --method tools/call --tool-name profile_summary --format json
```

Run either command with the registered workspace as the current directory. `inspector.config.json` uses the repository-relative debug binary and is only for debugging from this repository root.
