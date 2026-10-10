# Step 8 dogfood: pre-registered criteria and decision log

Step 8 of the scoping report (`firstmate/data/planning-compiler-scope/report.md`,
section 4.2) requires the compiler to prove it "changes or catches a real
decision, at a maintenance cost below what it saves". This file fixes the
rules of that proof **before any decision is taken with the compiler**, and
logs every entry under them.

Fixed 2026-10-10, before the first compiler-assisted decision. Later entries
may add log rows; they may not weaken sections 1-3 without a dated note
saying what changed and why.

## 1. What counts

A decision counts as **changed** only when the compiler's output led to a
different choice than the planner would otherwise have made. A decision
counts as **caught** only when the compiler's output flagged something the
planner had not noticed (a dropped contract field, a contradiction between
declared and measured numbers, a blocked alternative, a stale estimate).

Every counted entry must name the exact output line that did it (for
example `contract method:via-hybrid: would drop
transcript.segment_confidence (consumed by task:select-lines)`), the command
that produced it, and the decision taken differently because of it.
A recommendation the planner was already following does not count.
Presence of hand-written assumptions in `pc explain` does not count: the
retrospective already established those are fixture authorship, not
derivation.

## 2. Maintenance cost

Every edit to the dogfooded plan or its evidence log is a log row with
minutes spent. Minutes are wall-clock per edit session, timed while doing
the edit. The setup row below predates the discipline (its session was not
timed) and says so instead of inventing a number.

The comparison at write-up time is: summed maintenance minutes versus the
cost of the decisions changed or caught (estimated at write-up, with the
basis stated). If upkeep exceeds what it saved, step 8 says so plainly.

## 3. Stop rule

If two weeks pass with nothing changed or caught at a cost the captain
would notice, step 8 reports that plainly and stops expanding: the compiler
goes back to the triage queue rather than accumulating maintenance.

## 4. Measurement discipline

- Duration observations from the forge are recorded with typed basis
  `absolute`. They measure **PR open-to-merge pipeline time**, not total
  implementation effort; the source string on each record says so.
- Any future accuracy figure is recorded as
  `distance-from:<capability>`, never as absolute accuracy (PR 8, gap G4).
- One attributed family carries one basis; mixes are refused with E014,
  never resolved by majority.
- Out of scope for this dogfood: the live Ren cycle, any real Ren data,
  import/export adapters, gaps G2/G3/G5, and any compiler code change.
  A compiler defect found through the dogfood is logged as a finding for
  firstmate, not fixed here.

Binding constraints (captain, 2026-10-10): no LLM calls in the core,
text-file state, typed matching only, people left out, task tracking out
of scope, license deferred.

## 5. Baseline evidence (measured, 2026-10-10)

PR open-to-merge times from the forge for the finished steps, recorded on
`capability:crewmate@1` with basis `absolute` (see `examples/self-plan/`).
Values in minutes (3dp); timestamps are the merge (measurement completion).

| PR | Open (UTC) | Merged (UTC) | Minutes | Covers |
|---|---|---|---|---|
| https://github.com/prajwal-395/planning-compiler/pull/1 | 2026-10-10 04:55:09 | 2026-10-10 05:05:23 | 10.233 | steps 0-1 |
| https://github.com/prajwal-395/planning-compiler/pull/2 | 2026-10-10 05:29:59 | 2026-10-10 05:32:52 | 2.883 | step 2 |
| https://github.com/prajwal-395/planning-compiler/pull/3 | 2026-10-10 05:58:01 | 2026-10-10 06:01:54 | 3.883 | step 3 |
| https://github.com/prajwal-395/planning-compiler/pull/4 | 2026-10-10 07:05:47 | 2026-10-10 07:10:31 | 4.733 | step 4 |
| https://github.com/prajwal-395/planning-compiler/pull/5 | 2026-10-10 08:02:13 | 2026-10-10 08:06:10 | 3.950 | step 5 |
| https://github.com/prajwal-395/planning-compiler/pull/6 | 2026-10-10 08:41:25 | 2026-10-10 08:45:12 | 3.783 | step 6 |
| https://github.com/prajwal-395/planning-compiler/pull/7 | 2026-10-10 09:12:12 | 2026-10-10 09:17:11 | 4.983 | step 7 |
| https://github.com/prajwal-395/planning-compiler/pull/8 | 2026-10-10 14:46:25 | 2026-10-10 15:03:04 | 16.650 | gap fixes G1+G4 |

P5 folds these into `capability:crewmate@1 duration_minutes: predicted
2.000/5.000/15.000 vs measured 2.883/4.341/16.650 (n=8, [basis absolute])`.
The measured median (4.341) is recorded as a baseline only: no remaining
task estimates through `crewmate@1` yet (its one invoking method belongs to
the `done` retro task), and the values measure PR open-to-merge pipeline
time, not total implementation effort (section 4).

## 6. Log

### Entry 1 - 2026-10-10, setup + first compiler read (no decision taken)

Setup: brought `examples/self-plan/` to the actual state on main (steps
0-7 and the PR 8 gap fixes `done`, step 8 `decomposed` with retro `done`,
dogfood and live Ren cycle `open`, the report's 11 post-MVP items `open`
under `post-mvp`, real proposed alternatives where the report names them:
Pumpkin vs Aries for scheduling, SQLite vs Postgres for the store).
Maintenance: untimed setup session before log discipline started; all
future edits timed per section 2. Changed/caught: none (baseline read).

Commands run on the updated self-plan (`target/debug/pc` built from main):

`pc check examples/self-plan`:

```text
ok: 28 tasks, 12 methods, 6 capabilities, 1 artifacts, 10 evidence (examples/self-plan/plan.toml)
```

`pc compile examples/self-plan`:

```text
compiled: ship-compiler (examples/self-plan/plan.toml); makespan 0.000 minutes; parallel width 0
selection: optimal (2000 ms budget)
unresolved: dogfood-2wk, live-ren-cycle, post-mvp-01-tasks-axi, post-mvp-02-beads, post-mvp-03-taskmaster, post-mvp-04-skill-scan, post-mvp-05-mcp, post-mvp-06-proposer, post-mvp-07-pareto, post-mvp-08-scheduling, post-mvp-09-sqlite, post-mvp-10-rewrite, post-mvp-11-mining
```

Makespan 0.000 is the honest shape of the plan: every finished step is a
`done` leaf contributing zero forward work, and no remaining task has an
accepted method yet, so there is nothing executable to roll up. No metric
totals print for the same reason.

`pc frontier examples/self-plan` (17 items, all score 0.107, all critical):

```text
frontier: 17 items (examples/self-plan/plan.toml)
0.107 method:sched-via-aries [proposal] (critical): proposed by agent:crewmate for task:post-mvp-08-scheduling; awaiting accept or reject
0.107 method:sched-via-pumpkin [proposal] (critical): proposed by agent:crewmate for task:post-mvp-08-scheduling; awaiting accept or reject
0.107 method:store-via-postgres [proposal] (critical): proposed by agent:crewmate for task:post-mvp-09-sqlite; awaiting accept or reject
0.107 method:store-via-sqlite [proposal] (critical): proposed by agent:crewmate for task:post-mvp-09-sqlite; awaiting accept or reject
0.107 task:dogfood-2wk [open-task] (critical): no accepted method; on the selected path from the root (required by step-8)
0.107 task:live-ren-cycle [open-task] (critical): no accepted method; on the selected path from the root (required by step-8)
0.107 task:post-mvp-01-tasks-axi [open-task] (critical): no accepted method; on the selected path from the root (required by post-mvp)
0.107 task:post-mvp-02-beads [open-task] (critical): no accepted method; on the selected path from the root (required by post-mvp)
0.107 task:post-mvp-03-taskmaster [open-task] (critical): no accepted method; on the selected path from the root (required by post-mvp)
0.107 task:post-mvp-04-skill-scan [open-task] (critical): no accepted method; on the selected path from the root (required by post-mvp)
0.107 task:post-mvp-05-mcp [open-task] (critical): no accepted method; on the selected path from the root (required by post-mvp)
0.107 task:post-mvp-06-proposer [open-task] (critical): no accepted method; on the selected path from the root (required by post-mvp)
0.107 task:post-mvp-07-pareto [open-task] (critical): no accepted method; on the selected path from the root (required by post-mvp)
0.107 task:post-mvp-08-scheduling [open-task] (critical): no accepted method; on the selected path from the root (required by post-mvp)
0.107 task:post-mvp-09-sqlite [open-task] (critical): no accepted method; on the selected path from the root (required by post-mvp)
0.107 task:post-mvp-10-rewrite [open-task] (critical): no accepted method; on the selected path from the root (required by post-mvp)
0.107 task:post-mvp-11-mining [open-task] (critical): no accepted method; on the selected path from the root (required by post-mvp)
```

`pc explain capability:crewmate@1 examples/self-plan`:

```text
capability:crewmate@1: measured (P5 median 4.341 over 8 observations for `duration_minutes` (min 2.883, max 16.650) [basis absolute])
provenance crewmate@1 duration_minutes: predicted 2.000/5.000/15.000 vs measured 2.883/4.341/16.650 (n=8, sources: https://github.com/prajwal-395/planning-compiler/pull/1 open-to-merge; covers steps 0-1, https://github.com/prajwal-395/planning-compiler/pull/2 open-to-merge; covers step 2, https://github.com/prajwal-395/planning-compiler/pull/3 open-to-merge; covers step 3, https://github.com/prajwal-395/planning-compiler/pull/4 open-to-merge; covers step 4, https://github.com/prajwal-395/planning-compiler/pull/5 open-to-merge; covers step 5, https://github.com/prajwal-395/planning-compiler/pull/6 open-to-merge; covers step 6, https://github.com/prajwal-395/planning-compiler/pull/7 open-to-merge; covers step 7, https://github.com/prajwal-395/planning-compiler/pull/8 open-to-merge; covers gap fixes G1+G4) [basis absolute]
```

`pc compile --json` reports no unattributed, staged, or pending evidence:
all 10 records are live. No compiler defect surfaced in this read, so no
finding for firstmate. (`pc explain` on a proposed method refuses with
`error: no explanation for ... in reachable accepted plan`; that is the
documented accepted-plan scope of explain, not a defect.)

What it recommends doing next: resolve the four pending proposals first
(scheduling backend, store backend), then the in-progress step-8 work
(dogfood, live Ren cycle), then the post-MVP list.

Whether that differs from the report's post-MVP order: yes. The report
orders post-MVP by value with the tasks-axi import first (item 01). The
frontier instead surfaces the two post-MVP tasks that already carry
proposed alternatives (08 scheduling, 09 store) ahead of item 01, because
proposal entries sort with the open tasks on the tied score and method
subjects sort first. The 01-11 open tasks themselves tie in exactly the
report's order. No decision was taken on this divergence in this entry;
the next planning session decides whether to accept the compiler's
proposal-first order or the report's value order, and that decision - with
its output line - is entry 2 material.
