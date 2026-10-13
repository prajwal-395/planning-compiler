# Explicit state reconciliation

`pc sync` applies source snapshots to explicitly mapped Planning IR tasks. It
does not create or match tasks by title. Start by adding `sync-map.json` beside
the plan, containing a JSON array such as:

```json
[
  {"source":"tasks-axi","external_id":"PC-05","task_id":"pc-05-sync"},
  {"source":"beads","external_id":"bd-a12","task_id":"pc-05-sync"},
  {"source":"github","external_id":"I_kwDO...","task_id":"pc-05-sync"}
]
```

`sync-map.json` is the only source of identity mappings and is re-read on
every run, so added, corrected, or removed mappings take effect immediately.
Source records without a mapping are counted as `unmapped` and ignored; they
are not conflicts.

Run `pc sync --source tasks-axi --input backlog.md [plan-dir]` or
`pc sync --source beads --input issues.jsonl [plan-dir]`. GitHub issues and
pull requests are read from the current repository with `gh`; a closed issue
or merged PR never marks a task done by itself, because no verification
evidence path exists yet, so it stays a conflict unless the task is already
done locally. Without `--input`, task-system snapshots are read from
`sources/tasks-axi.backlog.md` and `sources/beads.jsonl` beside the plan and
GitHub issues are fetched through `gh`; `--input` cannot be combined with
`--all`.
`--all` reconciles tasks-axi, then Beads, then GitHub, so a GitHub closure is
judged against task-system changes applied in the same run. It skips, and
reports with the reason, every source that has no mapping
in `sync-map.json` or no snapshot file, and writes nothing for it; a mapped
source that fails to load or parse still fails the run.
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

Follow-up work, outside this change: ingest CI verification results,
runtimes, and failures; record verification evidence so mapped GitHub
closures can complete tasks; add the Planning Compiler source (objectives,
alternatives, decisions, evidence); and apply externally managed priority and
dependency changes through explicit source-to-plan mappings.
