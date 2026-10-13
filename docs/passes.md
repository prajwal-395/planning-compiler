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
| E014 | invalid evidence | subject is `capability:`, `method:` or `task:` with a non-empty id, timestamp is RFC 3339, value is finite and non-negative, basis is `absolute` or `distance-from:<capability>` when recorded, one attributed capability and metric (or pending subject) carries one basis (units are checked in P1) |
| E015 | invalid objective | weights name known metrics and are finite/non-negative; budget non-negative; floor in `[0,1]`; criteria carry their kind's fields |

Scope notes:

- A well-formed subject whose id names nothing in the plan is not an error. The record is
  held as pending evidence until the subject is proposed (see P5), so
  evidence can be staged together with the hypothesis it supports instead
  of failing every command.
- A proposed or derived invoke that would fail E008 if accepted is not an
  error either. `pc check` reports one informational `contract_notes` entry
  per violation - the same dropped fields and live consumers the frontier
  ranks as a blocked alternative and `pc impact` renders as a contract note
  (see P8, P9); notes never change `ok` or the exit code. Text output prints
  `contract method:<id>: would drop <fields> (consumed by task:<id>)` to
  stdout; `--json` carries the same rows beside the diagnostics.
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
abandoned work from merging into live work, a task carrying `tasks_axi`
or `beads_jsonl` passthrough is one imported row and never shares (its id joins the form), the
subtree keeps tasks with
different decompositions apart, merge groups are tried one at a time in
canonical-id order and a group is refused with a `kept` trace event when it
would cycle a decomposition's own order edges, or when compile's leaf graph
over every P6 selection (accepted methods only; `done`/`dropped` and
unresolved tasks are leafless and relay order, as in compile) has a cycle
through the canonical task's leaves, or any cycle at all if the canonical
task may be leafless; P4 runs only over clean plans. `pc compile` and
`pc explain` run over the shared document, so P6 sees the merged graph.

## P5: estimate (step 5)

`pc check` folds evidence into per-capability, per-basis profiles and
reports one `measured` trace event per capability, metric and basis with
observations, plus `stale` and `contradicts` events. Evidence never edits declared estimates:
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

Evidence staged with a proposal never moves the accepted plan's numbers.
A record is live only when its capability is invoked by at least one
accepted method, whichever subject (`capability:`, `method:` or `task:`)
named it; records for a capability only proposed, derived or rejected
methods invoke are held as staged evidence (`staged` trace events and
`staged` report entries naming the proposals currently invoking that
capability). Accepting any method that invokes the capability makes its
staged records live with no record movement; rejecting keeps them staged. A subject that names nothing in the plan stages
pending evidence (`pending` trace events and report entries) until the
subject is proposed. `pc record` accepts both shapes with a warning instead
of refusing them, and `pc compile` lists staged and pending holdings beside
the unattributed ones. `pc explain` for a proposed capability or method and
`pc context` for a task invoking it render the staged holdings, labelled
with the proposal they are staged for and not counted until accepted.

Observations carry an optional typed basis (`absolute`, or
`distance-from:<capability>`): what the value is measured against. Each
attributed family (capability and metric after placement, live or staged,
or the pending subject) carries one basis: P2 and `pc record` refuse a mix
with E014 naming every basis and the subjects that recorded it (a missing
basis counts as `none`), so bases are never pooled or resolved by
majority. Only the basis shape
is validated: the `distance-from` target may name a retired capability,
because evidence outlives the incumbents it was measured against. Evidence
with no basis field behaves exactly as before (one profile per capability
and metric).

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
measured/total node count, plus the sorted unique typed bases behind the
measured nodes (`; basis ...`, omitted for declared-only totals).
Measured totals also append the live observation count behind the
measured nodes (`; <n> observations`), and when every node is measured the
triple is labelled `observed range low ..., likely ..., high ...`, so a wide
machine-contention spread reads as a range instead of letting the median
read as a point value. Partly measured bounds mix in declared guesses and
carry no label; declared-only totals are unchanged.
`pc explain capability:<id>` renders the
provenance report (predicted triple vs measured triple, observation and
stale counts, contradiction flag, and the row's basis:
`[basis none]` when the evidence predates basis typing), and `--json` carries the same rows:
`pc compile --json` adds the per-metric basis (including `bases` and the
live observation count `measured_observations`), and
`pc explain --json` emits
an object with the decision `rows`, the capability's `provenance` rows
(empty for non-capability targets; `basis` is the `declared`/`measured(n)`
label and `measurement_basis` the typed basis), the matching
`unattributed_tasks`, and the matching `staged_evidence`.

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

`pc compile --objective makespan` (default `effort`) instead minimizes the
dependency-aware completion time of the selection, breaking ties by lower
weighted effort, under the same hard limits and with unlimited parallelism.
It exhaustively searches method assignments over activated tasks within the
time budget, scoring each with P7's leaf-based CPM (order edges included),
so the makespan P6 minimizes is the one P7 reports. Status labels match the
effort ILP: `optimal` when the search completes, `best-found` for a
time-limited incumbent, and `greedy-only` when the effort-greedy fallback is
faster or the only feasible result. `selection_objective` and
`selection_explanation` appear in `--json`; under `makespan` the text output
adds an `objective:` line whose explanation compares the result with the
lowest-effort selection found, stating how much extra effort buys how many
fewer minutes.

## P7: schedule-analyze (step 2+)

`pc compile` traverses from the objective root through selected accepted methods.
It preserves three-point estimates, uses PERT expected durations for CPM,
reports earliest/latest times, slack, critical tasks, parallel width (tasks
with zero expected duration are not counted), and the
low/likely/high makespan. The text headline reads
`compiled: <root> (<path>); makespan low <L>, likely <M>, high <H>,
expected <E> (optimistic) minutes; parallel width <W>`: the low, likely
and high spans are the CPM makespan over each executable task's low,
likely and high durations, and the expected span is the CPM makespan over
each task's PERT expected duration. The expected span is labelled
`(optimistic)` because CPM over expectations ignores PERT merge bias (the
exact duration distribution is #P-complete); it equals `makespan_minutes`
in `--json`, while `makespan_range` carries the full
`{low, likely, high, expected}` triple in both. `makespan_range` matches
`totals["duration_minutes"]` whenever executable work exists (both are the
same CPM range; the totals line quotes it with its predicted-versus-measured
basis), and is the zero range when no executable work exists. The `plan /
analyzed` trace row records the same range as
`P7 CPM makespan low <L>, likely <M>, high <H>, expected <E> (optimistic)
minutes; parallel width <W>`. Roll-ups and durations run on the P5 effective
estimates, with the declared-only prediction beside each total.
Unresolved tasks are listed with zero rolled work;
this pass never chooses among methods. `pc compile` also prints an
open-work line (and `open_work` in `--json`): the P8 frontier's summed
weighted cost over the estimated open tasks beside the count of open
tasks with no estimate, so a zero makespan beside open work reads as
remaining work, not done. The line never changes the makespan or its
meaning, and the frontier ranking itself is untouched.
Money sums across invoked work and
failure risk combines independent task risks as `1 - product(1 - risk)`.
`pc explain` lists each selected method's assumptions and each executable
task's slack and critical status. `pc explain method:<id>` joins that
method's assumptions (from the IR, so proposed methods show them too) with
its invoked capability's provenance rows and its contract exposure, so one
view carries assumptions, measurement basis and dropped-field consumers.
`pc explain task:<id>` joins that task's selection, its slack/critical
timing and the frontier entries touching it (the same entries `pc context`
assembles for the task), so one view carries selection, schedule position
and remaining planning work. Text states each fact once (the trace rows carry
selection and timing, followed by `frontier` lines); `--json` adds
`selection`, `timing` and `frontier` keys for task targets only.

## P8: frontier (step 6)

`pc frontier` ranks unresolved work over a checked, compiled plan
so people and agents can "see the frontier" (report C10). It never edits the
plan: kinds are `open-task` (reachable work with no accepted method),
`proposal` (a proposed method awaiting accept or reject), `blocked-alternative`
(a proposed or derived invoke that fails typed contract matching, so
accepting it would be E008) and `unmeasured-critical` (a critical leaf
whose duration rests on declared guesses).

Ranked and unestimated entries score
`share * uncertainty * criticality * readiness`. Estimated entries carry a weighted expected cost (`objective.weights`
times the P5 effective expected numbers; proposals use their own
capability, open tasks the largest of their contract-clean pending
invokes, unmeasured-critical leaves their selected capability) and their
`share` is that cost over the total weighted cost of open acceptable work:
per task, the largest cost among its contract-clean pending invokes
(alternatives, never summed) and its guessed critical capability.
They rank in `items`. Entries without a weighted estimate go to the
separate `unestimated` list, scored on the structural share
`(1 + transitive dependents) / tasks` and never ranked against `items`;
the text prints them after an `unestimated (...)` label and context marks
them `(unestimated)`. `uncertainty` is `1.0`, lowered to `0.5` when live
measured or staged evidence on a weighted metric (`duration_minutes` when
weights are empty) already backs the capability; `criticality` is `1.0`
on the selected path from the objective root and `0.5` otherwise (with
nothing executable yet every reachable task sits on that path, so
criticality ties and the other factors separate); `readiness` is `1.0`
when nothing live blocks the entry and `0.5` when it is blocked - an
accepted decompose orders a live predecessor before it, or a parent
through an accepted decompose is itself blocked (proposed or derived
decompositions never order work) - so blocked planning sinks below
parallel-ready work. Both lists sort by score descending, then kind
(proposal, open-task, unmeasured-critical), then subject ascending.
Pending invokes that fail typed contract matching cannot be accepted
(E008): they never feed the open total, never appear as ranked proposals,
and are listed once each in the separate `blocked` list by subject, as
`blocked-alternative` entries with no `share` or `score`, after a
`blocked (...)` label in the text. Items in one list sharing score and kind are genuinely
tied: they carry `tied: true`, the text marks them `(tied)` and
`(blocked)` beside `(critical)`, and their reasons say the order is
subject order, not priority. Only tasks reachable from the root through
non-rejected methods, and not done or dropped, are ranked;
the walk never descends below a done or dropped task, matching compile, and
this holds for open tasks, proposals and blocked alternatives alike.
`--json` emits the ranked report. A blocked entry names the dropped fields and the live tasks consuming them.
The same violation is also a `pc check` contract note (see P2 scope notes).

PC-10 adds a separate `discovery` list to the same report. It contains
actionable investigations for tasks with proposed invocation alternatives or
an unbacked selected capability. It ranks known decision importance first
(critical path, then evidence uncertainty, alternatives, downstream decisions)
with subject as a deterministic tie-breaker. Uncertainty uses the execution
frontier's scale: the measured factor only when the selected capability is
backed by evidence on an objective-weighted metric, the uncertain factor when
it is unbacked or no capability is selected. Investigation cost and the
likelihood of materially changing the plan serialize as `null`: the current
IR does not model them, and PC-11 owns value-of-information evaluation. The
text output labels these separately as the discovery frontier; the existing
`items`, `unestimated`, and `blocked` groups are execution work.

## P9: impact/diff (step 6)

`pc impact <change.json> [path]` previews a hypothetical change without
editing any files. The change document takes two optional keys
(`accept_method`, `add_evidence`; unknown fields are refused) and applies them to an in-memory copy, which is then
re-checked and, when clean, recompiled with P5-P7 under
`--selection-time-budget-ms`. The report carries the applied operations,
the affected tasks (seed tasks plus their transitive ancestors and
descendants through non-rejected decompose edges, one dataflow hop to
producers and consumers of their fields, ordered by graph distance from
the seeds nearest first with task-id tie order), the fresh diagnostics, contract notes naming dropped fields and
their consumers, and the selection, totals and makespan movements when both
sides compile. Totals movements label `accuracy_distance_from_incumbent` as
a distance from the incumbent, never absolute accuracy. A change that introduces diagnostics still exits 0: the
diagnostics ARE the finding. Unknown methods and committing an already
accepted or rejected method are usage errors (exit 1), as is a clean plan
that then fails P6/P7.

`pc diff <a> <b>` compares two plan files or directories: tasks,
capabilities and methods added, removed or changed (methods split into
pure status flips and deeper content changes), each side's makespan when
that side compiles, selection flips, makespan and expected-value movements
per metric when both sides compile (measured movements name the typed bases
behind each side: `[basis: X -> Y]`, with `-` for a declared-only side),
and diagnostics added or removed (compared by code, subject and message so
the same mistake in two files counts once), with the same distance label.
Sides that fail check still
diff structurally; only an unreadable plan or a P6/P7 failure exits 1.

`pc explain` joins the same contract exposure into the method view:
`pc explain method:<id>` renders the method's status first, then its
assumptions beside its invoked capability's provenance rows (predicted vs measured with typed
bases and sources, plus staged holdings while proposed) and the typed
contract note naming dropped fields and their consumers - the same line
`pc impact` previews and `pc frontier` ranks as a blocked alternative, so
verifying a substitution takes one command. A rejected method is refused,
since every pass ignores it. Measured rows name
their sorted unique evidence sources beside the observation count, so a
measurement basis such as "distance from the incumbent" is visible wherever
the numbers are. `--json` adds the matching `contract_notes` array beside
`rows`, `provenance`, `unattributed_tasks` and `staged_evidence`.

## Proposal loop (step 7)

External agents drive recursive decomposition through `pc propose`,
`pc accept`/`pc reject` and `pc context` while the core stays model-free.
The patch format (`add_task`, `add_method`, `add_capability`; unknown fields
refused) is published at `schema/patch.schema.json`, generated by
`pc schema --patch`; CI fails when it is stale.

- `pc propose <patch.json> [path]` stages new tasks, methods and
  capabilities. Validation: the patch is non-empty; no id repeats inside the
  patch or collides with the plan; method ids avoid the P3 `match-`
  namespace; new tasks are never `done` (the task-tracking boundary: the
  compiler reads `done`, never sets it); new methods are `proposed` with
  provenance; every task, capability, artifact, field, metric and resource
  reference resolves against the plan plus the patch; method shapes (E013)
  and estimate triples (E011) hold. Contract violations (E008) are
  deliberately allowed: a proposed invoke that drops a field is a hypothesis
  the frontier ranks as a blocked alternative and `pc check` reports as a
  contract note (see P2 scope notes), not a patch error. The merged
  plan must otherwise check clean or nothing is written.
- `pc accept <method>` / `pc reject <method>` flip a proposed or derived
  method to accepted/rejected. In-memory P3 derived proposals materialize
  into the file with the decided status, so the matcher does not propose them
  again. Accepting a decision that would check dirty (for example a
  substitution that drops a contracted output) is refused with the fresh
  diagnostics; preview such decisions with `pc impact`.
- `pc context <task> [path] [--budget tokens]` renders the slice an agent
  needs to propose the next decomposition: the task with its contract and
  plan position, its non-rejected methods, the capabilities they invoke,
  parent and child neighbours, the measured-evidence summary and the
  evidence staged with proposals for those capabilities, and the frontier
  entries touching the task. Sections fill in that order while the rendered
  text fits `chars/4` tokens under the budget (default 2000); `truncated`
  records the drop. `--json` emits the fitted struct.

File-edit rule: the decision commands edit the pristine plan and only write
when a probe copy of the merged plan passes `check`. Pass mutations (P3
proposals, P4 merges, P1 unit canonicalization) live only in the probe and
are never written back.

## Deferred (step 8)

Step 8 is proof work (the Ren retrospective, dogfooding, one live planning
cycle), not a pass, so no passes remain deferred.
