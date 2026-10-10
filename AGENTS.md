## Notes

- Proposal-loop commands (`pc propose`/`accept`/`reject`, in `crates/pc-core/src/propose.rs`) edit the pristine plan and only write when a probe copy of the merged plan passes `check`; pass mutations (P3 derived, P4 merges, P1 unit canonicalization) are never written back.
- `pc context` fits sections (methods, capabilities, parents, children, evidence, frontier) while the rendering fits `chars/4` tokens under `--budget` (default 2000); text and `--json` always agree.
- Schemas are generated: `pc schema --out schema/plan.schema.json`, `pc schema --patch --out schema/patch.schema.json`; CI fails when stale.

## Maintaining this file

Keep this file for knowledge useful to almost every future agent session in this project.
Do not repeat what the codebase already shows; point to the authoritative file or command instead.
Prefer rewriting or pruning existing entries over appending new ones.
When updating this file, preserve this bar for all agents and keep entries concise.
