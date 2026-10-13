# Explicit state reconciliation

`pc sync` applies source snapshots to explicitly mapped Planning IR tasks. It
does not create or match tasks by title. Start by adding `sync-map.json` beside
the plan, containing a JSON array such as:

```json
[
  {"source":"tasks-axi","external_id":"PC-05","task_id":"pc-05-sync"},
  {"source":"beads","external_id":"bd-a12","task_id":"pc-05-sync"},
  {"source":"github","external_id":"I_kwDO...","task_id":"pc-05-sync"},
  {"source":"ci","external_id":"12345678","task_id":"pc-05-sync"},
  {"source":"planning-compiler","external_id":"task:pc-05-sync","task_id":"pc-05-sync"}
]
```

`sync-map.json` is the only source of identity mappings and is re-read on
every run, so added, corrected, or removed mappings take effect immediately.
Source records without a mapping are counted as `unmapped` and ignored; they
are not conflicts.

Run `pc sync --source tasks-axi --input backlog.md [plan-dir]` or
`pc sync --source beads --input issues.jsonl [plan-dir]`. Without `--input`,
task-system snapshots are read from `sources/tasks-axi.backlog.md` and
`sources/beads.jsonl` beside the plan; `--input` cannot be combined with
`--all`.
`--all` reconciles tasks-axi, then Beads, then CI, then GitHub, then the
Planning Compiler's own records, so a GitHub closure is judged against
task-system changes and CI verification applied in the same run, and every
task change in the run invalidates the mapped planning analyses at once. It skips,
and reports with the reason, tasks-axi and Beads without a mapping in
`sync-map.json` or a snapshot file, and GitHub and CI without a mapping
(both fetch live through `gh` when no snapshot file exists), and writes
nothing for a skipped source. The Planning Compiler source reads the plan
itself, so it is never skipped. A mapped source that fails to load or parse
still fails the run.
`--dry-run` runs the same reconciliation in memory, so it reports exactly
the changes, conflicts, and exit status of a real run, without writing either
file. Omit the optional plan path to use the normal `.plan`/current-directory
lookup. The command exits 1 when any conflict is reported.

Only the lifecycle is reconciled: open, decomposed, and resolved tasks all
compare as not done. Marking a task done writes `done`; reopening writes
`decomposed` when the task has an accepted decomposition and `open`
otherwise. Every other task field, including the tasks-axi and Beads
passthrough, is left untouched.

`sync-state.json` stores the last observed revision and lifecycle state of
each mapped record, plus a `provenance` entry (source, external ID,
revision, task, previous and new state) for every applied change. A first
observation that disagrees with local task state is a conflict, so it is
left for an explicit resolution. Later source changes apply only while the
local state still matches the previous observation; a local change made after
the observed revision is a conflict. Conflicting records are not observed and
are retried on the next run. A newer revision whose lifecycle agrees with the
plan is recorded as observed, so reruns are idempotent.

## GitHub closures need verification

A closed issue or merged PR never marks a task done by itself, because a
merge alone does not prove the acceptance criteria hold. It stays a conflict
unless the task is already done locally or CI verification has been observed
for the same mapped task (see below). As with every source, the first
observation only records: sync once while the PR is open, then again after
the merge and the passing check, and the second run derives the completion
from the explicit mapping plus the verification evidence.

## CI verification results, runtimes, and failures

`pc sync --source ci --input checks.json [plan-dir]` ingests CI check
results. Mappings are explicit check IDs to task IDs:
`{"source":"ci","external_id":"12345678","task_id":"pc-05-sync"}`. Each
snapshot row carries a stable check `id`, a revision timestamp (RFC 3339,
which doubles as the evidence timestamp), a GitHub `conclusion`, and
optionally `duration_seconds`:

```json
[{"id": "12345678", "revision": "2026-10-13T12:00:00Z", "conclusion": "success",
  "duration_seconds": 120}]
```

Rows also accept the `gh run list --json` keys `databaseId` for `id` and
`updatedAt` for `revision`. Without `--input`, the snapshot defaults to
`sources/ci.json` beside the plan, falling back to
`gh run list --json databaseId,conclusion,updatedAt` when no
snapshot file exists. A missing or empty conclusion is pending, never a
failure; `success` passes; `failure`, `cancelled`, `timed_out`,
`action_required`, `stale`, and `startup_failure` fail; `skipped` and
`neutral` checks are ignored, neither verifying nor blocking their task; any
other conclusion fails the run rather than being silently ignored.

CI never edits task state. A successful check appends verification evidence
to the sibling `evidence.jsonl` (runtime as `duration_minutes` when reported,
plus the pass as `failure_risk` 0) and observes the check as passed, so a
later GitHub closure for the same mapped task completes it. A failed check
appends failure evidence (`failure_risk` 1) the same way; when the local task
is already done, the failure is instead a conflict, reported on every run,
that preserves the local completion while the check observes as failed, so it
no longer verifies the task. Pending checks observe without recording
evidence. Replaying the same check revision records nothing twice. A check counts as verification for
its mapped task only once observed, and only while every observed mapped
check for that task passed; checks absent from the snapshot do not block.

Sync refuses to append evidence `pc check` would refuse, so a malformed CI
revision fails the run instead of poisoning `evidence.jsonl`.

Mapped pull requests retain their GitHub creation and completion timestamps in
`sync-state.json`. `pc evaluate` joins these records to a decision only through
the decision's task IDs and the corresponding `sync-map.json` entries. With
one matching mapped pull request, it reports decision-to-PR-creation latency
and PR creation-to-merge duration. Missing dates, absent mappings, or
multiple matching PRs leave the value unrecorded with a reason. These dates do
not measure plan-preparation wall time, rework, or planning-model maintenance;
compiler-run durations remain a separate measured tool-runtime cost.

## Planning-compiler drift checks

`pc sync --source planning-compiler [plan-dir]` re-checks the plan's own
objectives, alternatives, decisions, and evidence for drift. Mappings name
the entity and the task whose planning analyses depend on it:
`task:<id>`, `method:<id>`, `decision:<event id>`, `evidence:<subject>`
(covering every record for that subject), or `objective:<plan id>`.

Each mapped entity is read live from `plan.toml` and the `.plan` event logs
and hashed into a content revision. The first run observes it as unchanged;
an entity whose revision moved since the last observation is reported as
changed, meaning the mapped task's planning analyses are invalidated, and
the new revision is observed. A mapped entity that does not exist, whether
removed or never recorded, is a conflict. The plan is never written by this source and it records no
provenance.

A snapshot file is an optional override: with `--input snapshot.json`, or
`sources/planning-compiler.json` beside the plan, only its rows are checked,
and each must still match its expected revision:

```json
[{"id": "task:pc-05-sync", "revision": "<blake3 hex>"},
 {"id": "objective:self-plan", "revision": "<blake3 hex>"}]
```

Revisions come from `pc_sync::planner_revision`. A row whose entity was
re-hashed or removed is a conflict naming the stale record, and rows without
a mapping are counted as unmapped.

Follow-up work, outside this change: apply externally managed priority and
dependency changes through explicit source-to-plan mappings.
