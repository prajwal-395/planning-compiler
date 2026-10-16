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

## PC-17 release measurements (2026-10-14)

Machine: MacBook Pro arm64, 24 GB RAM, same shared machine as the debug
baseline. Build: `cargo build --release -p pc-cli`, solver budget 2000 ms.
The `--all` tables were each one sequential run; `flat-10000` was measured
separately with `--scenario flat-10000 --include-large`. Millisecond values
are individual observations and can vary between runs.

Before optimization, `Time Profiler` on the in-process `alts-100` benchmark
and temporary per-check timing showed P2 taking about 600 ms per validation.
`check_contracts` took 280-332 ms and `check_method_shapes` took 288-298 ms:
both searched the entire TOML source for every method's line even when the
plan had no diagnostics. Those lookups now run only when a corresponding
diagnostic is emitted.

| Scenario | Tasks | Methods | parse_ms | valid_ms | sel_ms | sched_ms | front_ms | incr_ms | Selection | RSS |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---|---:|
| flat-10 | 11 | 31 | 0 | 0 | 0 | 0 | 0 | 0 | optimal | 10.0 MiB |
| flat-100 | 101 | 301 | 1 | 18 | 1 | 1 | 0 | 20 | optimal | 15.8 MiB |
| shared-100 | 111 | 311 | 1 | 19 | 2 | 0 | 0 | 22 | optimal | 16.3 MiB |
| dense-100 | 101 | 301 | 1 | 19 | 1 | 1 | 0 | 21 | optimal | 16.5 MiB |
| mesh-100 | 101 | 301 | 1 | 19 | 1 | 1 | 0 | 22 | optimal | 17.0 MiB |
| alts-100 | 101 | 1601 | 5 | 419 | 2 | 4 | 1 | 460 | optimal | 34.3 MiB |
| flat-1000 | 1001 | 3001 | 11 | 1951 | 73 | 48 | 12 | 2247 | optimal | 47.6 MiB |
| shared-1000 | 1101 | 3101 | 12 | 2069 | 123 | 47 | 13 | 2419 | optimal | 71.3 MiB |
| dense-1000 | 1001 | 3001 | 12 | 1881 | 83 | 28 | 13 | 2006 | optimal | 52.6 MiB |
| mesh-1000 | 1001 | 3001 | 49 | 1912 | 115 | 37 | 18 | 1987 | optimal | 163.1 MiB |
| alts-1000 | 1001 | 16001 | 58 | 42764 | 111 | 442 | 85 | 48619 | optimal | 240.9 MiB |
| flat-10000 | 10001 | 30001 | 169 | 328549 | 2356 | 6874 | 1619 | 258864 | greedy-only | 447.5 MiB |

After optimization, on the same machine and with the same release command:

| Scenario | Tasks | Methods | parse_ms | valid_ms | sel_ms | sched_ms | front_ms | incr_ms | Selection | RSS |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---|---:|
| flat-10 | 11 | 31 | 0 | 0 | 0 | 0 | 0 | 0 | optimal | 10.1 MiB |
| flat-100 | 101 | 301 | 1 | 1 | 1 | 1 | 0 | 3 | optimal | 16.1 MiB |
| shared-100 | 111 | 311 | 1 | 2 | 2 | 1 | 0 | 4 | optimal | 16.5 MiB |
| dense-100 | 101 | 301 | 1 | 2 | 1 | 1 | 0 | 3 | optimal | 16.7 MiB |
| mesh-100 | 101 | 301 | 2 | 2 | 1 | 1 | 0 | 5 | optimal | 18.9 MiB |
| alts-100 | 101 | 1601 | 5 | 5 | 2 | 5 | 1 | 12 | optimal | 34.1 MiB |
| flat-1000 | 1001 | 3001 | 12 | 54 | 81 | 48 | 12 | 185 | optimal | 65.5 MiB |
| shared-1000 | 1101 | 3101 | 14 | 70 | 124 | 49 | 12 | 240 | optimal | 72.2 MiB |
| dense-1000 | 1001 | 3001 | 13 | 56 | 77 | 29 | 12 | 177 | optimal | 70.8 MiB |
| mesh-1000 | 1001 | 3001 | 42 | 92 | 122 | 37 | 19 | 252 | optimal | 164.0 MiB |
| alts-1000 | 1001 | 16001 | 57 | 123 | 91 | 316 | 63 | 513 | optimal | 247.5 MiB |
| flat-10000 | 10001 | 30001 | 125 | 4527 | 2346 | 4288 | 1044 | 11113 | greedy-only | 561.3 MiB |

`alts-100` validation fell from 419 ms to 5 ms (about 84x); `alts-1000`
fell from 42,764 ms to 123 ms (about 348x). `flat-10000` completed before
and after: its validation fell from 328,549 ms to 4,527 ms, and its full
scenario from roughly 10 minutes to roughly 23 seconds. Selection outcomes
and benchmark-generated plans are unchanged.

## P0 decision-quality and overhead smoke (2026-10-16)

One local debug run of `cargo run -q -p pc-cli -- bench` on the default
six-scenario suite completed with `optimal` selection in all six scenarios
(6/6). For a simple planning-overhead measure, sum `parse_ms`, `valid_ms`,
`sel_ms`, `sched_ms`, and `front_ms` from that run; this excludes the
additional incremental re-run and peak memory. The sums were 5, 52, 65, 56,
77, and 153 ms for `flat-10`, `flat-100`, `shared-100`, `dense-100`,
`mesh-100`, and `alts-100`, respectively (68 ms mean). This is a one-run
smoke measurement, not an estimate of quality on live work or a stable
latency benchmark. The scenarios are generated in memory, and `optimal`
means the solver proved the modeled selection optimal under each scenario's
declared utilities; it does not establish that those utilities represent a
real project's preferences.
