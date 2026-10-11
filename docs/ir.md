# Work IR v0

The typed data model for plans. One vocabulary maps onto the three research
traditions behind this project so contributors from each can orient:

| IR concept | HTN term | AND/OR graph |
|---|---|---|
| **Task** (a need: what must be true or produced) | task | OR node |
| **Method** (one way to achieve a task) | method | AND node |
| **Plan** (one method chosen per needed task) | solution tree | solution graph |

P6 chooses one accepted method per needed task, counting a shared descendant
once. Proposed, derived and rejected methods are not selectable in this step.

`pc compile` selects among reachable accepted methods before analyzing the
chosen alternatives. Proposed, derived, and rejected methods are excluded. Invocation
estimates roll up by metric: duration uses the longest precedence path, money
adds across invoked tasks, and failure risk combines as `1 - product(1 - risk)`.
Each reported range retains low, likely, and high values; capability durations
also use PERT's `(low + 4*likely + high) / 6` expected value for CPM. Tasks
without an accepted method remain visible as unresolved.

## Entities

- **Objective.** The root task plus hard constraints (`budget_usd`,
  `deadline`, `quality_floor`), per-metric `weights`, and `acceptance`
  criteria. Each criterion is `manual` (a person signs off), `command` (a
  command exits zero) or `metric` (a `metric op value` comparison).
- **Task.** `id`, `title`, `state`, and a typed contract: `consumes` and
  `produces` lists of `{ artifact, fields }`. States: `open`, `decomposed`,
  `resolved`, `done`, `dropped`. The compiler reads `done` from imports and
  never sets it; `dropped` tasks are abandoned and skipped by contract and
  dataflow checks.
- **Method.** `kind` is `decompose { subtasks, order_edges }` or
  `invoke { capability }`. `status` is `accepted`, `proposed`, `derived` or
  `rejected`. `accepted`/`derived` methods are checked, and only `accepted`
  methods are compiled;
  `proposed` methods are hypotheses for the step-7 loop and `rejected`
  methods are ignored by every pass. `provenance` is `human:<name>`,
  `agent:<name>`, `llm:<model>` or `pass:<name>`; `assumptions` are listed
  for `explain` (step 2) to surface. `pc explain method:<id>` renders the
  assumptions beside the invoked capability's P5 provenance (live rows, or
  staged holdings while proposed) and the typed contract exposure: dropped
  fields with the live tasks consuming them. Agents stage methods as `proposed` with
  `pc propose` and decide them with `pc accept`/`pc reject`; the P3 matcher
  stages `derived` proposals the same commands decide.
- **Capability.** `kind` is `agent`, `tool`, `workflow`, `skill` or
  `service`, plus a `version`. There is deliberately **no `human-role`
  kind** (see `adr/0003-no-human-executors.md`). Each capability carries a
  typed `consumes`/`produces` contract, per-metric three-point `estimates`
  (`low`/`likely`/`high` with `declared` or `measured` provenance), and the
  `resources` it draws on.
- **Artifact.** A declared type: `name` plus its `fields`. Every contract
  entry must name a declared artifact and declared fields.
- **Resource.** A named capacity (e.g. `ci-minutes`). v0 only validates
  references; reporting usage against capacity arrives with scheduling.
- **Evidence.** An append-only observation: `subject`
  (`capability:<id>`, `method:<id>` or `task:<id>`), `metric`, `value`,
  `unit`, RFC 3339 `time`, `source`, an optional typed `basis`
  (`absolute`, or `distance-from:<capability>` saying what the value is
  measured against), and an optional `artifact_hash`. Evidence
  never edits declared estimates: P5 derives measured triples from it
  (see `passes.md`) and selection and roll-ups run on the effective
  numbers. A well-formed subject that names nothing in the plan is held as pending,
  and evidence for a proposal no accepted method invokes is staged with
  it: neither moves the accepted plan's numbers (see `passes.md` P5).
  P1
  accepts only the metric's canonical unit (case-insensitive, never
  empty) and canonicalizes its case; P2 checks subject shape, timestamps,
  values and bases, and refuses one attributed capability and metric
  recorded under two bases. `pc record` appends one validated observation to the sibling
  `evidence.jsonl`.

## Metrics

v0 knows four metrics, each with a canonical unit:

| metric | unit | range |
|---|---|---|
| `duration_minutes` | `minutes` | `>= 0` |
| `money_usd` | `usd` | `>= 0` |
| `failure_risk` | `ratio` | `[0, 1]` |
| `accuracy_distance_from_incumbent` | `ratio` | `[0, 1]` |

`accuracy_distance_from_incumbent` measures how far a capability's output
lands from the incumbent's, never absolute accuracy. It is kept apart from
`failure_risk` in profiles and totals, and `pc compile`, `pc impact` and
`pc diff` label it as a distance from the incumbent.

The typed `basis` says what a value is measured against, for any metric:
`absolute`, or `distance-from:<capability>`. One attributed family carries
exactly one basis: the capability and metric a record measures after P5
placement (so `capability:<id>`, `method:<id>` and `task:<id>` records for
the same capability are one family, live or staged), or the subject itself
while it is pending. Any mix is an E014 error in P2 and in `pc record`,
naming every basis with the subjects that recorded it; record the other
basis under a distinct metric instead. A missing basis counts as its own
basis (`none`), so basis-less legacy records pool only with other
basis-less records, and a later absolute-accuracy figure can never
silently pool with distances. Mixes are refused, never resolved: no
majority pick, and no family is dropped from selection or costed at zero.

There is no `human_minutes`: people are out of scope for v0. Objective
weights may only name known metrics and must be finite and non-negative.

## Contracts

Contracts are **typed consumes/produces matching only**. For a committed
`invoke` method of task T via capability C:

1. C must produce every field T promises downstream (the Ren Voz case:
   dropping `transcript.segment_confidence` is error E008).
2. Everything C consumes must come from T's own consumables or plan
   `external_inputs`.

Dataflow (E009): every field a live task consumes must be produced by
*another* live task or declared in `plan.external_inputs`. Self-production
alone does not satisfy a consumption. The check is global in v0, not
ordering-aware. The same typed violation is what `pc explain method:<id>`
renders as its contract exposure (see `passes.md` P9).

## Formats

- **TOML** (`plan.toml`) is canonical for files people write by hand.
- **JSON** (`plan.json`, plus `--json` output) is canonical for machine
  interchange. The schema is published at `schema/plan.schema.json` and
  generated by `pc schema`; CI fails when it is stale.
- **No YAML**: implicit typing is a reliability hazard for agent-written
  files. No custom DSL in v0.
- **Evidence log**: a sibling `evidence.jsonl`, one JSON object per line,
  folded into the plan at load time.

## Determinism

The same inputs give byte-identical reports whatever the input order:
indexes are sorted, diagnostics sort by `(code, subject, file, line, message)`,
and `reports_are_byte_identical_regardless_of_input_order` pins this.
