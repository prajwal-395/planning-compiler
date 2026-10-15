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
How measured totals render as observed ranges is owned by P5 in `passes.md`.

## Entities

- **Objective.** The root task plus hard constraints (`budget_usd`,
  `deadline`, `quality_floor`, and the `platforms` and `licenses` invoked
  capabilities must stay within), per-metric `weights`, and `acceptance`
  criteria. Each criterion is `manual` (a person signs off), `command` (a
  command exits zero) or `metric` (a `metric op value` comparison).
- **Task.** `id`, `title`, `state`, and a typed contract: `consumes` and
  `produces` lists of `{ artifact, fields }`. States: `open`, `decomposed`,
  `resolved`, `done`, `dropped`. The compiler reads `done` from imports and
  never sets it; `dropped` tasks are abandoned and skipped by contract and
  dataflow checks. `tasks_axi` carries opaque tasks-axi `backlog.md`
  passthrough (section, repo, kind, priority, holds, date gates, body) whose
  values no pass reads (P4 never shares a task carrying it), so a backlog
  round-trips byte-identical instead of being refused or dropped;
  `plan.tasks_axi_sections` preserves the file's section order including
  empty sections.
  `beads_jsonl` carries the original Beads issue record so fields without
  IR counterparts survive export; id, title and open/done state remain
  editable (renames reach every dependency naming the issue), `blocks`
  dependencies between issues in the file rebuild from non-rejected
  decompose order edges, and other states are refused on export. Both imported row types are distinct work under P4.
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
  `resources` it draws on. Optional `provider` and `source` record where
  the capability comes from; `pc capabilities` lists them per capability
  beside its P5 evidence coverage and flags capabilities with no live or
  staged measurement evidence as `unmeasured`. `platform_requirements` and
  `licensing_constraints` are checked against the objective's `platforms`
  and `licenses`. Each list holds alternatives (a capability runs on any
  listed platform and is usable under any listed licence): when the
  objective declares either list, every non-rejected invoke whose
  capability lists entries on that axis but none the objective declares is
  E016, and P3 never derives such a method. `pc context`
  includes all four fields for invoked capabilities. Omitted fields remain
  valid for existing plans.
- **Artifact.** A declared type: `name` plus its `fields`. Every contract
  entry must name a declared artifact and declared fields.
- **Resource.** A named capacity (e.g. `resolve` with capacity 1). A
  capability names the resources it holds while it runs, one unit of each;
  P6/P7 schedule holders against `floor` of the declared capacity, read as
  a count of concurrent instances whatever its `unit` (minimum one slot, so
  a budget-style total such as CI minutes is not a consumable pool;
  undeclared resources are unlimited), so exclusive holders serialize while
  other work overlaps them. This is planning-time contention only: the
  existing execution infrastructure stays responsible for running work.
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
- **Investigation.** An explicit value-of-information model for one task:
  an investigation `cost` with `cost_source` provenance, plus mutually
  exclusive `hypotheses`, each with provenance, optional probability bounds
  (`low`/`high` with `declared` or `measured` provenance, a `source`, and an
  optional observation count) and the expected utility of every available
  implementation under that hypothesis. Utilities and cost share one scale
  (higher is better). A missing probability means unknown, never a midpoint:
  such models carry no EVPI upper-bound interval. P2 check E017 requires
  finite sourced inputs,
  method alternatives belonging to the task, the same alternative set under
  every hypothesis covering every accepted method (at least one), and
  bounds that admit a
  distribution summing to 1; P8
  evaluates an EVPI upper bound (perfect revelation of the true hypothesis)
  against the prior-optimal implementation, since real observations can be
  noisy,
  reports any switch gain separately, and explains when the investigation
  outranks implementing the preferred approach (see `passes.md`).

## Decision and outcome events

`decisions.jsonl` stores the selected alternative, the planner's optional
`initial_decision` (recorded before the recommendation is shown), compiler
recommendation, provenance, constraints, supporting evidence, baseline, and
a predicted outcome. Each decision also carries `compiler_runs`: the IDs of
the `metrics.jsonl` compiler runs that informed it. Attribution is by that
explicit reference only, never by timestamp proximity: a run recorded near
the decision but not listed costs nothing for it. `outcomes.jsonl` stores separately measured results
linked to an existing decision, with confidence, an `evaluation_status`
(`confirmed`, `changed`, `corrected`, `investigated`, `rejected` or
`inconclusive`; anything else is refused) and an optional `usefulness`
(`useful`, `neutral` or `harmful`) assessed independently of the status. Both event types
carry an ID, RFC 3339 timestamp, plan revision, source, schema version, and
task/capability references. `pc decision <event.json>` and `pc outcome
<event.json>` append records; `pc decisions` and `pc outcomes` read them.
Prediction and acceptance never create an observed outcome. Schemas are
generated by `pc schema --decision` and `pc schema --outcome`.

`pc evaluate`, `pc metrics` and `pc calibration` judge each decision by its
latest outcome only (by RFC 3339 instant, ties going to the later log
entry), so re-evaluating a decision replaces its earlier outcome rather than
weighting it twice. `pc history` lists every decision, outcome, and
compiler-run event by instant. `pc evaluate` reports per-decision
`planning_cost` as the summed wall-clock duration of the explicitly
referenced runs (`measured`, in milliseconds); a decision with no
attributable runs reports `unknown` with the reason (no runs referenced, or
referenced IDs with no matching `metrics.jsonl` rows). `pc metrics`
summarizes the same attribution across decisions, counting each distinct
run once even when several decisions reference it. Unparsable metrics rows
are skipped, never fatal, and reported (`skipped_run_rows` in `pc metrics`,
and in the `pc evaluate` note when a referenced run is missing). Operational costs needing GitHub/CI stay out of
this compiler-run cost.

## Compiler runs

`metrics.jsonl` stores one automatic observation per `pc check`,
`pc compile`, `pc frontier`, and `pc context` run, beside the
decision/outcome logs: wall-clock duration, P1-P5 validation duration,
P6 selection-solver duration with optimality status and gap, P8
frontier-ranking duration (frontier runs only), rendered context size
(context runs only), resident memory, task/method/constraint counts,
alternatives considered, contract rejections, evidence coverage,
estimate uncertainty, the critical-path tasks with how many entered or
left it since the previous recorded run of the same plan file with the
same selection objective and time budget (so switching objective or
command alone never reports a change), and how many recorded decisions
gained evidence on their supporting tasks or capabilities after they
were made. Fields the operation cannot observe stay null: `check`
records no solver or critical-path fields. The write is best-effort and
never fails the command; `PC_METRICS=off` skips it. The schema is generated by
`pc schema --metrics`.

Deferred to follow-up PC-03b: operational measurements need GitHub/CI
data the compiler cannot observe (plan-preparation time,
decision-to-implementation latency, actual completion durations, rework
introduced or avoided, model-maintenance cost).

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

A proposed or derived invoke that would violate the same rule is a
hypothesis, not an error: `pc check` reports it as an informational contract
note (see P2 scope notes in `passes.md`).

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
