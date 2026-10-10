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
(report 3.5, criteria (a)-(c)).

## Scope boundary

The compiler is one level above execution, management, supervision and
verification: it answers how to plan and what to plan, not how to build
once planning is settled. It is not a multi-agent system or a software
factory.

v0 specifically does not:

- call language models (agents propose with `pc propose`, decide with
  `pc accept`/`pc reject` and read slices with `pc context`; the core is
  model-free),
- execute anything or track task status/assignment,
- model people (`docs/adr/0003-no-human-executors.md`),
- schedule resource capacity.

State is text files in the repo (`docs/adr/0002-text-files.md`). The repo
is private and unlicensed pending a later decision
(`docs/adr/0001-license-deferred.md`).

## Layout

```
crates/pc-ir/     IR types, TOML+JSON serde, JSON Schemas (plan + patch)
crates/pc-core/   Passes P1-P5, P6 selection input, roll-ups, P7 CPM, P8 frontier, P9 impact/diff, step-7 proposal loop, diagnostics
crates/pc-select/ P6 exact ILP method selection with greedy fallback
crates/pc-store/  plan loader, plan writer, evidence JSONL append/read
crates/pc-cli/    pc binary (check, compile, explain, record, frontier, impact, diff, propose, accept, reject, context, schema)
schema/           generated JSON Schemas (committed; CI fails if stale)
examples/self-plan/  this project's own plan (dogfood fixture)
examples/transcription-substitution/  synthetic substitution study (before plan, decided plan, evidence, changes)
tests/fixtures/   golden valid plans + one negative fixture per diagnostic
docs/             ir.md, passes.md, evidence/ (step-8 dogfood log), adr/
```

Adapters (`pc-adapter-*`) arrive after the MVP.

## Use

```sh
pc check [path]    # run P1-P5; exit 0 clean, 1 diagnostics, 2 usage
pc check [path] --json   # full report: diagnostics, counts, trace
pc compile [path]  # effective totals with predicted-vs-measured columns
pc explain <id> [path]   # per-decision trail plus capability provenance
pc record <evidence.json> [path]  # append one observation to evidence.jsonl
pc frontier [path]  # rank unresolved work (P8)
pc impact <change.json> [path]  # preview a hypothetical change (P9)
pc diff <a> <b>    # compare two plans: structure, selection, totals (P9)
pc propose <patch.json> [path]  # stage tasks, methods, capabilities as proposed
pc accept <method> [path]  # flip a proposed/derived method to accepted
pc reject <method> [path]  # flip a proposed/derived method to rejected
pc context <task> [path] [--budget N]  # budget-fitted slice for the next decomposition
pc schema [--patch] [--out file]   # emit the canonical JSON Schema (plan or patch)
```

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
