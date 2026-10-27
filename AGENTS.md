# Repository instructions

## Documentation changes

Do not create or modify documentation unless the user explicitly requests it.

## Project positioning

`prof-mcp` is a workspace-local MCP server for querying folded stack profiles.
It operates on Linux. The implementation stays small and maintains a clear
read-only query boundary.

## Architecture: Read-Only Query Boundary

MCP queries are read-only. Profile registration and lifecycle management
(register, update, remove, gc) happen through the CLI, not through MCP.

Each MCP query discovers the nearest ancestor `.prof-mcp/manifest.json`
without requiring a server restart. This separates profile registration
from query execution.

The read-only boundary is non-negotiable. If a requirement cannot be
accommodated within this constraint, reject the requirement rather than
expanding the boundary.

## Scope: Query, Not Analysis

Tools expose measurements: symbol locations, frequency rankings, call chains,
and comparisons. Profile data describes observed execution, not causal
performance conclusions. Code must not invent missing metadata or treat
overlapping observations as a partition.

The server does not offer analysis, recommendations, or performance judgments.
It surfaces what the profiler measured.

## Complexity gate

Before introducing a new dependency, abstraction, or module, justify why
existing code cannot express the behavior clearly.

If a requirement conflicts with the read-only boundary, decline it.

## Engineering practices

User-visible behavior changes—CLI interface, MCP tool set, response format,
limits—must include corresponding test coverage. See `.github/workflows/ci.yml`
for the validation baseline.

