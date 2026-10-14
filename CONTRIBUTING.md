# Contributing

Everyone is welcome. By participating you agree to follow the
[Code of Conduct](CODE_OF_CONDUCT.md). Report security issues privately per
[`SECURITY.md`](SECURITY.md), never in a public issue.

Licensed under `MIT OR Apache-2.0` (see `LICENSE-MIT` and `LICENSE-APACHE`).
The crates stay `publish = false` for now, so do not publish to crates.io.

## How to contribute

1. For anything beyond a small fix, open an issue first (bug report or
   feature request templates) so the design can be discussed.
2. Fork, branch, and open a pull request against `main`. The PR template
   lists the checks your change must pass.
3. CI runs `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`,
   `cargo-deny`, and a schema-staleness check on every push and PR. Run the
   loop below locally before pushing.

## Loop

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny --workspace check licenses advisories bans sources
```

`cargo deny` needs the RustSec advisory DB on first run (network).

## Adding a diagnostic

1. Reserve the next `E###` code in `docs/passes.md` with the rule.
2. Implement the check in `crates/pc-core/src/passes.rs` (deterministic:
   sorted iteration, owned snapshots before `ctx.error`).
3. Add one unit test in `crates/pc-core/tests/diagnostics.rs` from the
   smallest mutation of `valid_doc` that triggers exactly that code.
4. Add `tests/fixtures/invalid-E###.toml`; it is picked up automatically
   by the `negative_fixtures_fail_with_their_code` CLI test.

## Changing the IR

1. Edit types in `crates/pc-ir` (keep `deny_unknown_fields`; keep TOML
   hand-writeable; no `human-role`, no `human_minutes`).
2. Regenerate both schemas (the patch embeds tasks, methods and
   capabilities): `cargo run -q -p pc-cli -- schema --out schema/plan.schema.json`
   and `cargo run -q -p pc-cli -- schema --patch --out schema/patch.schema.json`.
3. Round-trip property tests must stay green; if decimal precision is at
   stake, see the `finite_float` rationale in
   `crates/pc-ir/tests/roundtrip.rs`.

## Adding a pass (steps 2+)

Passes live in `pc-core` (or a new `pc-*` crate at a dependency boundary,
e.g. solvers stay in `pc-select`) and come in two kinds:

- Validation passes implement the `pc_core::Pass` trait and are registered
  in `check`, which runs them and reports diagnostics.
- Analysis passes, such as P6 select and P7 schedule-analyze, run only on
  a plan that passed `check` and are exposed as plain functions (P6 then P7
  run in `pc_core::compile`, called by `pc compile` and `pc explain`).

Document every pass's contract in `docs/passes.md`. Never copy code from
Task Master (Commons Clause) or `bv`/`br` (rider license).
