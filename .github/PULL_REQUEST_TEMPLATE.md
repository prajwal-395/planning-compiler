## What

<!-- One paragraph: what does this PR change and why? Link issues with `Fixes #N` where applicable. -->

## How

<!-- How is it implemented? Note any new diagnostics (E###), IR changes, or schema regenerations. -->

## Verification

<!-- Commands run and their results, e.g. `cargo test --workspace`, `cargo clippy`, `cargo deny check`. -->

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `cargo deny --workspace check licenses advisories bans sources`
- [ ] Schemas regenerated if the IR changed (`pc schema` / `pc schema --patch`; CI checks staleness)

## Notes for reviewers

<!-- Anything reviewers should pay attention to, and anything deliberately left out of scope. -->
