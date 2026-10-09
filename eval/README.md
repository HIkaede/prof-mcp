# Capture and agent evaluation

These development tools exercise the current MCP and capture boundary. They use Python 3's standard
library and run separately from the shipped MCP server. Generated answers,
query logs and scores live in the ignored local `results/` directory.

## Native perf comparison

`native.py` loads a specified perf `stackcollapse.py` and drives its actual
`process_event` handler with deterministic event dictionaries. It probes C, C++,
Rust, Unicode, operators, recursion, long names, DSO distinctions, unknown
addresses, escaping, process names, periods and mixed events. Assertions check
the harness's empty and duplicate-event behavior.

```bash
mkdir -p eval/results
python3 eval/native.py /usr/libexec/perf-core/scripts/python/stackcollapse.py > eval/results/native.json
python3 eval/native.py /tmp/stackcollapse-v6.6.py /tmp/stackcollapse-v5.15.py
```

Historical formatter sources are available from
[Linux v6.6](https://github.com/torvalds/linux/blob/v6.6/tools/perf/scripts/python/stackcollapse.py)
and [Linux v5.15](https://github.com/torvalds/linux/blob/v5.15/tools/perf/scripts/python/stackcollapse.py).
The probe stubs perf bridge imports and checks formatting/aggregation. A real
Linux pipeline smoke uses:

```bash
prof-mcp capture --name smoke -- /usr/bin/python3 -c 'sum(i*i for i in range(2000000))'
```

The native scripts tested merge DSO distinctions and unknown addresses, replace
periods with sample counts, and merge mixed events. These counterexamples support
retaining the Rust collapse step. Native formatter probes and real capture are
separate checks.

The final native CLI spike used system perf `7.2.9-300.fc45.x86_64` with actual
`perf.data`, rather than only calling the Python formatter:

```bash
perf record -e cpu-clock -g -o /tmp/native.data -- /usr/bin/python3 -c 'sum(i*i for i in range(2000000))'
perf script report stackcollapse -i /tmp/native.data
perf report -i /tmp/native.data --stdio --no-children --percent-limit 0 \
  -g folded,0,caller,function,period
perf report -i /tmp/native.data --stdio --no-children --percent-limit 0 \
  -g folded,0,caller,address,period -s comm,dso,symbol
```

The `stackcollapse` command succeeded and emitted total weight 407 for 407
samples, while the recorded period total was 101750000. Repeating the recording
with `-e cpu-clock,task-clock` produced 750 samples; `stackcollapse` accepted them
and emitted total weight 750 without event separation or rejection.

`perf report` does support period weights: the function-mode folded rows summed
to 101750000. Its output is a report with headings and weight-before-stack rows,
not registerable folded text. More importantly, neither function nor address
mode carries the DSO identity of every frame in the semicolon-separated chain.
Sorting by `comm,dso,symbol` supplies a histogram entry's DSO, not all ancestor
DSOs; address mode also splits known functions by instruction offset. Changing
column order or filtering report headings cannot recover the missing identities.
For mixed events, report prints separate sections rather than rejecting the input.

Decision: retain Rust collapse over `perf script` records. The tested native CLI
paths do not supply the complete identity, escaping and event-validation contract.
This closes the replacement investigation; reopen it only with a concrete native
pipeline that passes these counterexamples. Raw spike output stays local.

## Blinded agent trials

The [12 tasks](tasks.json) cover self/inclusive rankings, symbol resolution,
callers, callees, paths, recursion, diff, a large profile, C++ identities, tree
continuation and malformed-input recovery. Small fixtures live in `corpus/`.
`make_large.py OUTPUT` generates 20,000 stacks with a unique 1000-weight hotspot.

Create separate raw and MCP workspaces, copy fixtures, and generate `large.folded`.
Register baseline/candidate/cpp/tree/large only in the MCP workspace. Give fresh
agents the same task file and restrict each agent's profile evidence to one arm:

```bash
python3 eval/make_large.py /tmp/large.folded
python3 eval/raw.py /tmp/raw-workspace 'print(open("baseline.folded").read())'
python3 eval/query.py /tmp/mcp-workspace -- --list
python3 eval/query.py /tmp/mcp-workspace profile_top '{"profile":"baseline","sort":"self","limit":1}'
```

Raw agents author their own folded aggregation through `raw.py`. MCP agents
inspect tool schemas and query aliases through `query.py`. Both wrappers log
returned evidence bytes. The MCP client bounds exchanges and reaps children.
Agents use fresh contexts without implementation, roadmap or other trial answers.
Repeat both arms before exposing their results to each other. The malformed task
allows explicit CLI registration; tree tasks start with a two-node budget and
follow a returned continuation.

Save local artifacts as `results/{raw,mcp}-{1,2}-answers.json` and matching
`-queries.jsonl` files. `score.py` checks expected answer fields and numeric
tolerance, then summarizes correctness, evidence calls, returned bytes and
raw-arm authored snippet bytes:

```bash
python3 eval/score.py
```

Extra answer notes are ignored for field comparisons. Malformed-input scoring
checks empty-frame rejection and valid-profile recovery; raw validation and CLI
output have no shared API error identifier. Exact `error_code` is not scored.
The bytes/4 metric is a volume proxy rather than tokenizer-measured usage.
Evidence calls exclude orchestration, handshakes, discovery and registration;
record these separately. Review self/inclusive and truncation mistakes alongside
scores. A 20,000-stack fixture and a few trials establish a bounded local sample.

CI checks the scorer/fixtures and transport independently of local results:

```bash
python3 eval/score.py --self-check
python3 eval/test_query.py
```

## Contract decisions

The repeated local trials covered all tasks with the eight tools and avoided
custom folded parsing in the MCP arm. Raw agents also answered correctly and
used fewer evidence calls and returned bytes by batching their own aggregation.
Keep the eight-tool algebra and `profile_top.focus`; these trials do not establish
a benefit from another primitive or a public rename.
