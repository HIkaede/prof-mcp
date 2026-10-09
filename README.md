# prof-mcp

A workspace-local stdio MCP server that lets agents query folded stack profiles
with bounded, structured, deterministic results. On Linux, `capture` runs
`perf` and registers the result in one step.

[中文说明](docs/README.zh-CN.md)

## Quick start

Put `prof-mcp` on `PATH`, then register it with Codex:

```bash
prof-mcp setup            # add --dry-run to preview
```

`setup` only edits the Codex MCP entry (equivalent to `command = "prof-mcp"`,
`args = ["serve"]`). It is idempotent and refuses to overwrite a same-named
entry that has custom arguments, environment, cwd or timeouts.

Register profiles from your workspace:

```bash
prof-mcp register ./perf.folded                    # alias defaults from file name
prof-mcp register ./baseline.folded --name baseline
producer | prof-mcp register - --name streamed     # stdin
prof-mcp capture --name candidate -- ./my-program args...   # Linux, needs perf
```

`prof-mcp PROFILE` is shorthand for `register`. Running with no arguments prints
help. The server finds the nearest ancestor `.prof-mcp/manifest.json` on every
query, so no restart is needed after registering. Until a profile exists,
queries return `workspace_not_registered`.

## Managing the registry

```bash
prof-mcp list
prof-mcp use baseline
prof-mcp remove candidate                          # alias only; blob stays until gc
prof-mcp remove baseline --new-active candidate    # required when removing the active alias
prof-mcp gc --dry-run
prof-mcp gc                                        # deletes only unreferenced blobs
```

Profiles are stored under `.prof-mcp/profiles/<blake3>.folded` as the exact input
bytes, so identical inputs are deduplicated. The last alias cannot be removed.
Mutations hold a persistent advisory lock, and manifest writes are atomic. The
generated registry data is git-ignored, and its small `.gitignore` stays visible.

## Tools

Eight tools, in this order:

| Tool | Purpose |
| --- | --- |
| `profile_summary` | Alias, fingerprint, total weight, profile shape |
| `profile_find_symbols` | Substring or regex search to exact frames and stats |
| `profile_top` | Rank frames by `self` or `inclusive`; `frame` restricts to stacks containing it |
| `profile_tree` | Bounded context tree |
| `profile_callers` / `profile_callees` | Caller or callee chains for an exact frame |
| `profile_paths` | Heaviest complete stacks through a frame |
| `profile_diff` | Compare two aliases (both required) |

Single-profile tools default to the active alias. A suggested order for agents:

1. `profile_summary` to confirm the alias, fingerprint and total weight.
2. `profile_find_symbols` to get an exact name or frame ID (IDs are per profile).
3. `profile_callers`, `profile_callees` or `profile_paths` for call context.
4. `profile_top` with `frame` and `metric:"self"` for self ranking in that scope.
5. `profile_diff` for two explicit aliases.

High inclusive weight is not proof of an optimization win, and a diff does not
show causation or statistical significance. Results describe the observed
profile only.

Truncated tree, callers and callees results return continuations. Copy a
returned `data.continuations[i].continuation` object unchanged into the next
request's `continuation` field and omit `profile` and `frame`. The descriptor
carries the alias, fingerprint, tool and cursor. If the alias now points to
different bytes, restart without it. Do not build cursors by hand.

`profile_paths` can crop what it displays without changing which paths are
selected or their weights:

```json
{"frame":{"frame_name":"foo"},"frame_window":{"mode":"head","lines":10}}
{"frame":{"frame_name":"foo"},"frame_window":{"mode":"tail","lines":10}}
{"frame":{"frame_name":"foo"},"frame_window":{"mode":"around_target","before":5,"after":5}}
```

Input schemas reject unknown fields. Responses use schema version `"2"`, and any
`truncated=true` carries structured `truncation_reasons`.

Clients must pass `structuredContent` to the agent. It holds the full evidence
and error-recovery hints. The text `content` is only a summary of at most
2048 bytes. How a given host (including Codex) maps the two is unverified.

## Limits

- Input profile: 512 MiB by default (`--max-file-size-mib`), 8 MiB per line,
  4096 frames per stack, total weight at most `2^53 - 1`.
- Tool result: 64 KiB of compact JSON. Larger results return `query_too_large`.
  Lower the row, node or frame limits, or narrow the query.
- Manifest: 4 MiB (`registry_too_large`). Missing-alias errors list at most
  100 aliases.
- Per-tool row, node and depth bounds are in the generated input schemas.

## Folded format and capture

A line is `frame;frame weight` with a positive integer weight. Frames are opaque
and case-sensitive. Weights are unitless (`weight_semantics`: `opaque`).

`capture` passes your command straight to `perf record -g` (no shell) and folds
`perf script` output in Rust. It keeps full signatures and `[module]` suffixes,
and it drops only a trailing `+0xHEX` offset. Unknown symbols become
`[unknown@0xADDR]`. Literal `% ; [ ]` are percent-escaped (`%25 %3B %5B %5D`),
so query the encoded names as returned. Mixed event types, malformed samples,
limit violations and perf failures fail the capture without touching existing
aliases.

## Platform support

Registry operations currently require Unix. `capture` additionally requires
Linux. CI validates Linux only, and other platforms are not verified.

## Development

Development follows the stable toolchain from `rust-toolchain.toml`. The minimum
supported Rust version is 1.88, and CI tests both.

```bash
cargo build --locked --release
cargo fmt --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo test --locked --release --test stdio     # real-binary MCP smoke test
```

- Fuzzing: [fuzz/README.md](fuzz/README.md)
- Native perf comparison and agent evaluation: [eval/README.md](eval/README.md)
- MCP Inspector: `inspector.config.json` is for debugging from the repo root.
  Point its `command` at an absolute binary path to inspect another workspace.

The crate is pre-1.0. Its public Rust modules are implementation details.

## License

[MIT](LICENSE)
