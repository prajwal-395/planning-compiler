## Notes

- Proposal-loop commands (`pc propose`/`accept`/`reject`, in `crates/pc-core/src/propose.rs`) edit the pristine plan and only write when a probe copy of the merged plan passes `check`; pass mutations (P3 derived, P4 merges, P1 unit canonicalization) are never written back.
- Evidence may name proposed or not-yet-existing subjects: P5 holds it as staged/pending, never effective until an invoke method is accepted, and records carry a typed `basis` (`absolute`, `distance-from:<capability>`); see P5 in `docs/passes.md`.
- `pc context` fits sections (methods, capabilities, parents, children, evidence, frontier) while the rendering fits `chars/4` tokens under `--budget` (default 2000); text and `--json` always agree.
- `pc explain method:<id>` is the joined substitution view: the method's assumptions beside its capability's provenance (live or staged) and its typed contract exposure; see P9 in `docs/passes.md`.
- `pc compile` prints an open-work line (and `open_work` in `--json`) from the P8 frontier's own per-task estimates, so a zero makespan beside open work reads as remaining work; the makespan value and meaning are unchanged.
- Resource caps (`[[resource]]` capacity + capability `resources`) constrain P6/P7 makespans via serial schedule-generation in `pc-select` (exact for small graphs); binding resources get a `resource-limited` trace row. See P6/P7 in `docs/passes.md`.
- Schemas are generated: `pc schema --out schema/plan.schema.json`, `pc schema --patch --out schema/patch.schema.json`; CI fails when stale.

## Maintaining this file

Keep this file for knowledge useful to almost every future agent session in this project.
Do not repeat what the codebase already shows; point to the authoritative file or command instead.
Prefer rewriting or pruning existing entries over appending new ones.
When updating this file, preserve this bar for all agents and keep entries concise.
