# planning-compiler

A deterministic compiler for work, in Rust. You describe a plan as data -
objectives, tasks with alternative methods, capabilities with typed
contracts and estimates, recorded evidence - and the compiler validates it,
chooses how to do it, and explains every decision.

**Status: steps 0-1.** Workspace skeleton, IR v0, and `pc check` (passes
P1-P2). See `docs/` for semantics and the sequenced plan. The `self-plan`
fixture (`examples/self-plan`) is this project's own implementation plan.

## Scope boundary

The compiler is one level above execution, management, supervision and
verification: it answers how to plan and what to plan, not how to build
once planning is settled. It is not a multi-agent system or a software
factory.

v0 specifically does not:

- call language models (agents propose through the CLI; the core is
  model-free),
- execute anything or track task status/assignment,
- model people (`docs/adr/0003-no-human-executors.md`),
- optimize, schedule, rank or explain yet (`compile`, `frontier`,
  `explain`, `impact`, `propose`, `record`, `context` arrive in steps 2-7).

State is text files in the repo (`docs/adr/0002-text-files.md`). The repo
is private and unlicensed pending a later decision
(`docs/adr/0001-license-deferred.md`).

## Layout

```
crates/pc-ir/     IR types, TOML+JSON serde, JSON Schema
crates/pc-core/   Pass trait + manager, P1-P2, diagnostics, trace
crates/pc-store/  plan loader, evidence JSONL
crates/pc-cli/    pc binary (check, schema)
schema/           generated JSON Schema (committed; CI fails if stale)
examples/self-plan/  this project's own plan (dogfood fixture)
tests/fixtures/   golden valid plans + one negative fixture per diagnostic
docs/             ir.md, passes.md, adr/
```

`pc-select` (exact ILP selection) arrives in step 3; adapters
(`pc-adapter-*`) arrive after the MVP.

## Use

```sh
pc check [path]    # run P1-P2; exit 0 clean, 1 diagnostics, 2 usage
pc check [path] --json   # full report: diagnostics, counts, trace
pc schema [--out file]   # emit the canonical JSON Schema
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
```

See `CONTRIBUTING.md` for the full loop and `docs/passes.md` for the
diagnostic catalogue.
