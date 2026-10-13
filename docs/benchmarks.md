# Benchmark suite (PC-16)

Representative compiler benchmarks over generated plans. No large fixtures
are committed: every scenario builds its plan deterministically in memory
(`crates/pc-bench`), so baselines stay comparable across runs.

## Scenarios

| Name | Leaves | Alternatives/leaf | Shape | Default |
|---|---|---|---|---|
| `flat-10` | 10 | 3 | flat: root over leaves, no order edges | yes |
| `flat-100` | 100 | 3 | flat | yes |
| `shared-100` | 100 | 3 | shared DAG: root over mid tasks sharing leaves | yes |
| `dense-100` | 100 | 3 | dense deps: chain plus skip-one order edges | yes |
| `mesh-100` | 100 | 3 | denser-than-linear deps: each subtask precedes the next `floor(sqrt(n))` | yes |
| `alts-100` | 100 | 16 | flat, many alternatives per leaf | yes |
| `flat-1000` | 1000 | 3 | flat | `--all` |
| `shared-1000` | 1000 | 3 | shared DAG | `--all` |
| `dense-1000` | 1000 | 3 | dense deps | `--all` |
| `mesh-1000` | 1000 | 3 | denser-than-linear deps | `--all` |
| `alts-1000` | 1000 | 16 | flat, many alternatives | `--all` |
| `flat-10000` | 10000 | 3 | flat | `--include-large` (opt-in) |

Capability pool is 8, widened to the alternatives count when a scenario
asks for more (16 for `alts-*`), so every alternative of a leaf is a
distinct choice and capability count stays flat while task count scales.
Contracts are empty, so E008/E009 are vacuous and every cost is compiler
work, not diagnostic handling.

## What each column measures

| Column | Captain's cost |
|---|---|
| `parse_ms` | parsing (`pc_ir::from_toml` over generated TOML) |
| `valid_ms` | validation (P1-P5 `check`) |
| `sel_ms` | P6 selection solver time (inside compile) |
| `sched_ms` | compile time outside P6 (P7 CPM plus roll-ups) |
| `front_ms` | P8 frontier ranking |
| `incr_ms` | incremental update: append one evidence record, re-check, re-compile |
| `selection` | optimality status plus solver gap when reported |
| `rss` | peak RSS (`getrusage` max RSS) of the subprocess that ran the scenario |

The P6 solver budget passes straight through
(`--selection-time-budget-ms`, default 2000 ms): the suite preserves it, it
never raises it to make a scenario pass.

## How to run

```sh
pc bench                 # small suite only (10/100): safe on shared machines
pc bench --all           # plus the 1,000-task scenarios
pc bench --include-large # plus the opt-in 10,000-task scenario
pc bench --scenario flat-100 --json
pc bench --list
```

`pc bench` runs in memory and writes nothing (no plan reads, no
`metrics.jsonl` appends). Each scenario runs in its own `pc` subprocess, so
its peak RSS covers only that scenario (parse through the incremental
re-run) and rows never accumulate earlier scenarios' memory. CI has no
bench job and tests that run a scenario are `#[ignore]`d (opt in with
`cargo test -- --ignored`): the captain forbids synthetic load on the
shared machine.

No caching or incremental recomputation lives behind these numbers (that is
PC-17): the suite only measures.

## Baseline (2026-10-13)

Machine: MacBook Pro arm64, 24 GB RAM, shared with parallel workers.
Build: debug (`cargo build -p pc-cli`), solver budget 2000 ms.
Command: `pc bench` (small suite), run once per the PC-16 instruction.

| Scenario | Tasks | Methods | parse_ms | valid_ms | sel_ms | sched_ms | front_ms | incr_ms | Selection | RSS |
|---|---|---|---|---|---|---|---|---|---|---|
| flat-10 | 11 | 31 | 2 | 4 | 0 | 1 | 0 | 6 | optimal, no gap | 14.9 MiB |
| flat-100 | 101 | 301 | 16 | 229 | 23 | 5 | 1 | 264 | optimal, no gap | 21.1 MiB |
| shared-100 | 111 | 311 | 18 | 238 | 37 | 5 | 1 | 296 | optimal, no gap | 21.5 MiB |
| dense-100 | 101 | 301 | 18 | 231 | 23 | 4 | 2 | 265 | optimal, no gap | 21.8 MiB |
| mesh-100 | 101 | 301 | 30 | 245 | 23 | 5 | 5 | 271 | optimal, no gap | 23.9 MiB |
| alts-100 | 101 | 1601 | 73 | 5085 | 36 | 22 | 9 | 5679 | optimal, no gap | 38.8 MiB |

Read: at 100 tasks P1-P5 validation dominates (~10x selection), and it
scales with method count far faster than linearly: `alts-100` (16
alternatives, 5x the methods of `flat-100`) spends ~20x longer in
validation, the first measured PC-17 candidate;
selection stays `optimal` inside the 2000 ms budget on every small
scenario; the incremental update costs roughly one more validation plus one
more compile. The 1,000 and 10,000-task scenarios are unmeasured here by
design (shared-machine load rule); measure them on dedicated hardware with
`--all` / `--include-large` before any PC-17 optimisation claims. (One
data point for why `flat-10000` is opt-in: a debug build on this shared
machine did not finish it inside 10 minutes; use a release build.)
