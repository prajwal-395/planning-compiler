# planning-compiler

A deterministic compiler for work, in Rust. You describe a plan as data -
objectives, tasks with alternative methods, capabilities with typed
contracts and estimates, recorded evidence - and the compiler validates it,
chooses how to do it, and explains every decision.

**Status: steps 0-7.** Workspace skeleton, IR v0, `pc check` (passes P1-P5:
validation, capability matching, shared-subtask merging, evidence
estimation), method selection (P6) over evidence-folded estimates,
selected-plan analysis with `pc compile` and `pc explain` (P7) including
predicted-versus-measured columns and `pc record`, frontier ranking
(P8) with change preview and plan comparison (P9), and the agent proposal
loop (step 7): `pc propose` stages tasks, methods and capabilities as
proposed, `pc accept`/`pc reject` decide them, and `pc context` renders the
budget-fitted slice for the next decomposition. See
`docs/` for semantics and the sequenced plan. The `self-plan`
fixture (`examples/self-plan`) is this project's own implementation plan;
`examples/transcription-substitution` is the synthetic substitution study
(report 3.5, criteria (a)-(c)); `examples/ren-native` is the first real
external-project planning cycle (Ren native rendering throughput).

## Scope boundary

The compiler is one level above execution, management, supervision and
verification: it answers how to plan and what to plan, not how to build
once planning is settled. It is not a multi-agent system or a software
factory.

v0 specifically does not:

- call language models (agents propose with `pc propose`, decide with
  `pc accept`/`pc reject` and read slices with `pc context`; the core is
  model-free),
- execute anything or own task status/assignment (`pc sync` only mirrors
  explicitly mapped external lifecycle state),
- model people (`docs/adr/0003-no-human-executors.md`),
- schedule resource capacity.

State is text files in the repo (`docs/adr/0002-text-files.md`). The repo
is private and unlicensed pending a later decision
(`docs/adr/0001-license-deferred.md`).

## Layout

```
crates/pc-ir/     IR types, TOML+JSON serde, JSON Schemas (plan + patch)
crates/pc-core/   Passes P1-P5, P6 selection input, roll-ups, P7 CPM, P8 frontier, P9 impact/diff, step-7 proposal loop, diagnostics
crates/pc-select/ P6 exact ILP (effort) or makespan-search method selection with greedy fallback
crates/pc-store/  plan loader (plus `LoadedPlan::check`, shared P1-P5 run), plan writer, evidence JSONL append/read
crates/pc-adapter-tasks-axi/  tasks-axi `backlog.md` import/export (Firstmate integration: holds, kinds, repos, priorities, date gates and bodies ride along as IR passthrough)
crates/pc-adapter-beads/      Beads issue JSONL import/export (source records ride along as IR passthrough)
crates/pc-sync/   explicit-mapping reconciliation of external task lifecycle state (`pc sync`)
crates/pc-cli/    pc binary (init, check, compile, explain, record, frontier, capabilities, impact, diff, propose, accept, reject, context, import-tasks-axi, export-tasks-axi, import-beads, export-beads, sync, schema)
crates/pc-mcp/    pc-mcp binary: read-only MCP stdio server (check, compile, frontier, context, impact, diff)
schema/           generated JSON Schemas (committed; CI fails if stale)
examples/self-plan/  this project's own plan (dogfood fixture)
examples/transcription-substitution/  synthetic substitution study (before plan, decided plan, evidence, changes)
examples/ren-native/  real Ren native-throughput planning cycle (plan, pending evidence, decisions)
tests/fixtures/   golden valid plans + one negative fixture per diagnostic
docs/             ir.md, passes.md, sync.md, evidence/ (self-plan continuous-validation log), adr/
```

Adapters (`pc-adapter-*`) convert external backlogs into the IR. The first
one is `pc-adapter-tasks-axi`, the Firstmate integration: backlog items map
to tasks, `blocked-by:` maps to order edges, and fields with no IR
counterpart ride along as passthrough so the backlog round-trips
byte-identical (`tests/fixtures/tasks-axi/backlog.md`). `pc-adapter-beads`
maps Beads issues to tasks, blocking dependencies to order edges, and keeps
each complete source JSONL record as passthrough (`tests/fixtures/beads/issues.jsonl`).
A blocking dependency on an issue not in the file (e.g. `external:`) adds no
edge; the import names it on an `unlinked dependency:` line and the record
still round-trips untouched.

## Use

```sh
pc init [path]     # scaffold .plan/plan.toml (or <path>/plan.toml) that passes pc check; refuses to overwrite
pc check [path]    # run P1-P5; exit 0 clean, 1 diagnostics, 2 usage
pc check [path] --json   # full report: diagnostics, counts, trace, contract notes
pc compile [path]  # effective totals with predicted-vs-measured columns
pc explain <id> [path]   # per-decision trail plus capability provenance
pc record <evidence.json> [path]  # append one observation to evidence.jsonl
pc decision <event.json> [path]  # append one decision and its prediction to decisions.jsonl
pc outcome <event.json> [path]  # append one observed result to outcomes.jsonl
pc decisions [path] / pc outcomes [path]  # print recorded events as JSON
pc evaluate [--decision id] [path]  # initial -> recommendation -> final -> outcome per decision
pc metrics [path] / pc calibration [path]  # classification counts / prediction error, latest outcome per decision
pc history [path]  # all decision and outcome events in time order
pc frontier [path]  # rank unresolved work (P8)
pc capabilities [path]  # capability inventory with evidence coverage; flags unmeasured
pc impact <change.json> [path]  # preview a hypothetical change (P9)
pc diff <a> <b>    # compare two plans: structure, selection, totals (P9)
pc propose <patch.json> [path]  # stage tasks, methods, capabilities as proposed
pc accept <method> [path]  # flip a proposed/derived method to accepted
pc reject <method> [path]  # flip a proposed/derived method to rejected
pc context <task> [path] [--budget N]  # budget-fitted slice for the next decomposition
pc import-tasks-axi <backlog.md> [dir]  # import a tasks-axi backlog into <dir>/plan.toml (refuses to overwrite)
pc export-tasks-axi <backlog.md> [path]  # export a plan back into backlog.md (byte-identical when unedited)
pc import-beads <issues.jsonl> [dir]  # import Beads issues into <dir>/plan.toml (refuses to overwrite)
pc export-beads <issues.jsonl> [path]  # export imported issue records to JSONL, preserving source fields
pc sync (--source <github|tasks-axi|beads> [--input file] | --all) [--dry-run] [path]  # reconcile mapped external task state (docs/sync.md)
pc schema [--patch|--decision|--outcome|--metrics] [--out file]   # emit the canonical JSON Schema (plan, patch, event, or run metrics)
```

`pc check`, `pc compile`, `pc frontier` and `pc context` also append one run
observation to `metrics.jsonl` beside the plan (`PC_METRICS=off` skips it);
see "Compiler runs" in `docs/ir.md`.

Without a `[path]`, commands use `.plan/` when it holds a plan file and the
current directory otherwise, so a bare `pc check` finds what a bare `pc init`
made. A bare `pc init` refuses when the current directory already holds a plan.

For tool-only agents, `pc-mcp` (`cargo run -q -p pc-mcp`) is a read-only
MCP stdio server whose tools return the same JSON as the matching
`pc <verb> --json`. `check` takes a plan `path`; `compile` and `frontier`
take `path` plus optional `selection_time_budget_ms`; `context` takes `task`,
`path` and optional `budget`; `impact` takes a `change` object (as in
`change.json`), `path` and optional `selection_time_budget_ms`; `diff` takes
plans `a` and `b` plus optional `selection_time_budget_ms`. Write verbs are
not exposed yet; they will be added in a later change.

Every command is non-interactive with `--json` output in the style of the
`axi` tools. Diagnostics look like a compiler:

```
error[E008]: method `voz-mfa` cannot satisfy task `transcribe-footage`: capability `voz-mfa@1` does not produce `transcript.segment_confidence`
  --> plan.toml:27
```

## Develop

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny --workspace check licenses advisories bans sources
cargo run -q -p pc-cli -- schema --out schema/plan.schema.json
cargo run -q -p pc-cli -- schema --patch --out schema/patch.schema.json
```

See `CONTRIBUTING.md` for the full loop and `docs/passes.md` for the
diagnostic catalogue.
