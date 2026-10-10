# Pass contracts

Every pass is deterministic: same inputs, byte-identical output. Every
check emits a trace event; `explain` renders selected-plan decisions.
`compile` runs P6 over accepted alternatives before P7. It also rejects order
edges that, through shared subtasks between selected methods, form a
precedence cycle.

## P1: load/resolve (`ResolvePass`)

Parse (in `pc-store`), then:

- reject `ir_version != 0` (**E000**);
- index ids per namespace, reporting duplicates (**E001**);
- canonicalize evidence units: the metric's canonical unit is accepted
  case-insensitively (`USD` -> `usd`); any other unit for a known metric,
  including an empty one, is **E014**.

## P2: validate (`ValidatePass`)

| Code | Name | Rule |
|---|---|---|
| E002 | undefined task | `method.task`, `subtasks`, order-edge endpoints and `objective.root_task` name declared tasks (non-rejected methods only) |
| E003 | undefined capability | non-rejected `invoke` methods name declared capabilities |
| E004 | unknown artifact | every contract entry names a declared artifact |
| E005 | unknown field | every contracted field is declared on its artifact |
| E006 | hierarchy cycle | `accepted`/`derived` decompose edges are acyclic (Erol et al. Thm 2: acyclic hierarchies keep plan existence decidable) |
| E007 | order cycle | order edges inside each non-rejected decompose method are acyclic |
| E008 | contract violation | committed `invoke`: capability produces cover the task's promises; capability consumables come from the task or external inputs |
| E009 | unsatisfied input | each consumed field is produced by another live task or a plan external input |
| E010 | unknown metric | estimates and evidence name known metrics (objective weights are E015) |
| E011 | invalid estimate | finite, `low <= likely <= high`, non-negative, risk in `[0,1]`, `measured` cites observations |
| E012 | undefined resource | capability resources name declared resources |
| E013 | method shape | `invoke` names a capability and no subtasks/edges; `decompose` names no capability and at least one subtask; order endpoints are subtasks of the same method |
| E014 | invalid evidence | subject exists (`capability:`/`method:`/`task:`), timestamp is RFC 3339, value is finite (units are checked in P1) |
| E015 | invalid objective | weights name known metrics and are finite/non-negative; budget non-negative; floor in `[0,1]`; criteria carry their kind's fields |

Scope notes:

- Cycle detection uses `petgraph` topological sort only.
- Line numbers in diagnostics for plan-file entities are best-effort
  (first line mentioning the quoted id), not true source spans; TOML
  parsing does not preserve them. Records from `evidence.jsonl` are
  anchored to their exact file and line.
- One unit test per code lives in `crates/pc-core/tests/diagnostics.rs`;
  one CLI fixture per code lives in `tests/fixtures/invalid-E*.toml`.

## P6: select (step 3)

`compile` considers accepted methods reachable from the objective root.
Proposed, derived and rejected methods are outside selection, and `done` or
`dropped` tasks are leaves, matching P7's skip rule. The objective is
the weighted sum of each method's PERT expected metric costs. Hard limits are
applied to total `money_usd` when `budget_usd` is set. A quality floor is
enforced as an independent-success constraint: sum `-ln(1-risk)` must not
exceed `-ln(quality_floor)`. Invocations missing a required money or risk
estimate are ineligible under the corresponding hard constraint. `deadline`
remains a post-analysis constraint.

The pure-Rust `pc-select` crate builds a 0/1 ILP with a needed-task variable
and a chosen-method variable per alternative. A chosen method activates its
children. Each needed task chooses exactly one method, and shared descendants
are charged once. The default time budget is 2,000 ms; `pc compile` and
`pc explain` accept `--selection-time-budget-ms`. The ILP reports `optimal`
when proof completes and `best-found (gap x%)` when it finds an incumbent
before its time limit. If the time limit ends with no incumbent, or with an
incumbent worse than greedy, a feasible greedy result is reported as
`greedy-only`. A proven-infeasible selection is a compile error, and so is a
time limit with neither an incumbent nor a feasible greedy result. A task is
needed only when the root or a chosen parent method requires it. Greedy
selection uses recursive tree cost and stable method-id tie breaking.
Unchosen alternatives of needed tasks are traced as `not selected`, naming the
chosen method or the missing estimate that made them ineligible.

## P7: schedule-analyze (step 2+)

`pc compile` traverses from the objective root through selected accepted methods.
It preserves three-point estimates, uses PERT expected durations for CPM,
reports earliest/latest times, slack, critical tasks, parallel width (tasks
with zero expected duration are not counted), and the
low/likely/high makespan. Unresolved tasks are listed with zero rolled work;
this pass never chooses among methods. Money sums across invoked work and
failure risk combines independent task risks as `1 - product(1 - risk)`.
`pc explain` lists each selected method's assumptions and each executable
task's slack and critical status.

## Deferred (steps 4-8)

| Pass | Ships in |
|---|---|
| P3 match (derived invoke proposals) | step 4 |
| P4 share (content-hash dedup) | step 4 |
| P5 estimate (evidence folding) | step 5 |
| P8 frontier (ranking) | step 6 |
| P9 impact/diff | step 6 |
| proposal loop, context slices, record | steps 5-7 |
