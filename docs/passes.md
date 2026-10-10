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
| E014 | invalid evidence | subject exists (`capability:`/`method:`/`task:`), timestamp is RFC 3339, value is finite and non-negative (units are checked in P1) |
| E015 | invalid objective | weights name known metrics and are finite/non-negative; budget non-negative; floor in `[0,1]`; criteria carry their kind's fields |

Scope notes:

- Cycle detection uses `petgraph` topological sort only.
- Line numbers in diagnostics for plan-file entities are best-effort
  (first line mentioning the quoted id), not true source spans; TOML
  parsing does not preserve them. Records from `evidence.jsonl` are
  anchored to their exact file and line.
- One unit test per code lives in `crates/pc-core/tests/diagnostics.rs`;
  one CLI fixture per code lives in `tests/fixtures/invalid-E*.toml`.

## P3: match (step 4)

`pc check` proposes `derived` invoke methods for `open` tasks with no
committed (`accepted`/`derived`) method, using exact typed subset matching
only: the capability must produce everything the task promises, and
everything the capability consumes must come from the task's consumables or
plan external inputs. No embeddings, no negotiation.

Guards: tasks that promise no typed field are skipped (an empty promise is
covered by every capability, so proposing would spam vacuous
substitutions); proposals that collide with an existing method id or that
duplicate a live invoke of the same capability for the same task are
skipped. Each proposal carries `provenance = "pass:match"` and a `derived`
trace event. P3 runs only over clean plans: any P1-P2 diagnostic leaves the
document untouched. Selection (P6) still considers `accepted` methods only;
derived proposals await the step-7 accept loop.

## P4: share (step 4)

`pc check` merges structurally identical tasks by blake3 content hash over
the canonical form (title, lifecycle state, sorted consumes/produces, and
the committed method subtree: each accepted/derived method's kind,
capability, and the hashes of its subtasks and order edges), so shared work
is counted once downstream. The smallest id wins; every reference
(`method.task`, subtasks, order edges, `objective.root_task`, `task:`
evidence subjects) is rewritten and self-edges from a merge are dropped.
Methods that become identical on the surviving task are deduplicated,
keeping accepted over derived over proposed (`method:` evidence subjects
follow the survivor); methods untouched by a merge are never removed.
Subtask order is preserved; only methods that reference a duplicate are
touched. Every merge emits a `merged` trace event, every removed method a
`deduplicated` event, never silent.

Guards: the title is load-bearing (contract-only equality wrongly merges
distinct work, e.g. two clips' transcriptions), the state keeps finished or
abandoned work from merging into live work, the subtree keeps tasks with
different decompositions apart, merge groups are tried one at a time in
canonical-id order and a group is refused with a `kept` trace event when it
would cycle a decomposition's own order edges, or when compile's leaf graph
over every P6 selection (accepted methods only; `done`/`dropped` and
unresolved tasks are leafless and relay order, as in compile) has a cycle
through the canonical task's leaves, or any cycle at all if the canonical
task may be leafless; P4 runs only over clean plans. `pc compile` and
`pc explain` run over the shared document, so P6 sees the merged graph.

## P5: estimate (step 5)

`pc check` folds evidence into per-capability profiles and reports one
`measured` trace event per capability and metric with observations, plus
`stale` and `contradicts` events. Evidence never edits declared estimates:
each profile keeps the declared triple beside the measured triple
(min/median/max over attributed observations), and selection (P6) and
roll-ups run on the *effective* numbers (measured when present, declared
otherwise).

Attribution is by subject: `capability:<id>` measures directly,
`method:<id>` measures the capability its invoke method calls, and
`task:<id>` measures the one capability the task's accepted invoke methods
call. When a task's accepted invoke methods call more than one capability,
the record cannot say which alternative ran, so it counts as unattributed;
record such observations against `method:<id>` or `capability:<id>`
instead. `pc record` still appends an ambiguous `task:<id>` record but warns,
naming those subjects, and `pc compile` (text and `unattributed_tasks` in
JSON) lists every ignored `task:<id>` observation by task and metric, as
does `pc explain` for that task or any capability it could have measured.
P5 runs after P4, so evidence recorded against a merged-away task
already names the surviving task and counts toward the same capability.

Statistics are deterministic with no wall-clock input: values sort
ascending, the median is the middle value (mean of the two middles for even
counts), spread is min/max, staleness is older than 90 days before the
newest observation for the same capability and metric, and contradiction
means the measured median falls outside declared `[low, high]`. The v0
window is all records. `pc record <evidence.json> [path]` appends one
validated observation to the sibling `evidence.jsonl`, refusing anything
`pc check` would flag.

`pc compile` and `pc explain` show predicted versus measured: per-metric
lines carry the declared-only prediction beside the effective total with a
measured/total node count, `pc explain capability:<id>` renders the
provenance report (predicted triple vs measured triple, observation and
stale counts, contradiction flag), and `--json` carries the same rows:
`pc compile --json` adds the per-metric basis, and `pc explain --json` emits
an object with the decision `rows`, the capability's `provenance` rows
(empty for non-capability targets), and the matching `unattributed_tasks`.

## P6: select (step 3)

`compile` considers accepted methods reachable from the objective root.
Proposed, derived and rejected methods are outside selection, and `done` or
`dropped` tasks are leaves, matching P7's skip rule. The objective is
the weighted sum of each method's P5 effective PERT expected metric costs,
so recorded evidence moves selection. Hard limits are
applied to total `money_usd` when `budget_usd` is set. A quality floor is
enforced as an independent-success constraint: sum `-ln(1-risk)` must not
exceed `-ln(quality_floor)`. Invocations missing a required money or risk
estimate in either declared or measured form are ineligible under the corresponding hard constraint. `deadline`
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
low/likely/high makespan. Roll-ups and durations run on the P5 effective
estimates, with the declared-only prediction beside each total.
Unresolved tasks are listed with zero rolled work;
this pass never chooses among methods. Money sums across invoked work and
failure risk combines independent task risks as `1 - product(1 - risk)`.
`pc explain` lists each selected method's assumptions and each executable
task's slack and critical status.

## Deferred (steps 6-8)

| Pass | Ships in |
|---|---|
| P8 frontier (ranking) | step 6 |
| P9 impact/diff | step 6 |
| proposal loop, context slices | step 7 |
