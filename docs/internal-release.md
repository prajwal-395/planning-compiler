# Internal release: 0.1.0

This is an internal, source-based release of the Planning Compiler. The
repository is private and unlicensed; install from a trusted checkout and do
not publish these crates to crates.io. The release tag is
`pc-internal-v0.1.0`.

## Install from a checkout

Install Rust 1.89 or newer, clone the repository, then build and install both
binaries from the repository root:

```sh
git checkout pc-internal-v0.1.0
cargo install --path crates/pc-cli
cargo install --path crates/pc-mcp
pc --version
command -v pc-mcp
```

Cargo puts both executables in `$CARGO_HOME/bin` (normally `~/.cargo/bin`).
Add that directory to `PATH` if `pc` or `pc-mcp` is not found. To select a
different install directory, add `--root /path/to/install` to both
`cargo install` commands and put `/path/to/install/bin` on `PATH`.

The `pc` executable is the command-line compiler. `pc-mcp` starts an MCP
stdio server; configure an MCP host to launch the installed executable. For
example, a host configuration can use:

```json
{
  "mcpServers": {
    "planning-compiler": {
      "command": "pc-mcp",
      "env": {
        "PC_MCP_ALLOWED_ROOTS": "/absolute/path/to/your/plans"
      }
    }
  }
}
```

The allowed-roots setting is needed only for write tools. Read-only tools
work without it. Keep it limited to plan directories the host should be
allowed to change. The host must request human approval for `accept` and
`reject`; `accept` also requires `approved=true`. Every write supplies the
current `expected_revision`, available through `revision`, to reject stale
updates.

The server exposes read tools `check`, `compile`, `frontier`, `context`,
`impact`, `diff`, and `revision`, and write tools `propose`, `accept`,
`reject`, and `record`. For example, call `revision` with the plan path, then
call `propose` with the patch, path, and returned revision. The tool
descriptions provide each input shape. The stdio server is intended to be
started and managed by the MCP host, not run as a standalone interactive
command.

## Run the integrated examples

From the repository root, run:

```sh
sh examples/internal-release/run.sh
```

The example runner requires Python 3 as well as Rust 1.89 or newer. The
script builds the local binaries and runs from a temporary directory. It uses
committed fixtures only and removes its temporary files on exit. Its
commands demonstrate:

- decision and observed outcome recording, then `evaluate`;
- task-system `sync` in dry-run mode;
- makespan selection under resource caps;
- execution and discovery frontiers, including value-of-information output;
- capability inventory and manifest-backed `discover`;
- benchmark-suite discovery with `pc bench --list` (scenario execution is
  documented in [benchmarks](benchmarks.md));
- MCP initialization, tool listing, and a read-only `revision` tool call.

`command -v pc-mcp` confirms the installed server is on `PATH`; the runner's
MCP smoke client verifies that it starts, lists tools, and handles a tool
call. Do not run `pc-mcp --help`: the executable starts its stdio server and
waits for JSON-RPC input from an MCP host.

For manual exploration after installation, try these from the repository
root:

```sh
pc compile --objective makespan examples/resource-contention
pc frontier examples/resource-contention
pc capabilities examples/ren-native
pc frontier examples/ren-native
pc bench --list
```

`examples/ren-native` is a documented real planning cycle. Its decisions
record the initial intent, compiler recommendation, and final decision; it
does not claim an observed execution outcome. The release runner creates a
separate illustrative outcome event so it does not alter that record.
The runner also adds a synthetic investigation model with assumed
probabilities and utility values to demonstrate VOI output; those values
are not measurements or Ren evidence.

## Release notes

This internal release brings together decision and outcome records with
evaluation, explicit task-state reconciliation, resource-constrained
makespan analysis, execution and discovery frontiers with VOI, capability
inventory and discovery, generated-plan benchmarking, and read/write MCP
tools. The release includes the PC-16 benchmark suite, documented in
[benchmarks](benchmarks.md). The guide and runner make the
installation route and these workflows reproducible from a clean checkout.

Milestone A evidence in this checkout:

- **Holds:** agents can interact through the CLI and MCP tools; decisions
  and outcomes have separate structured records and an evaluation command;
  compiler runs record planning metrics; automated crate tests cover the
  compiler workflows. The Ren planning example records a real decision
  sequence and is explicit that its outcome remains unobserved.
- **Does not yet hold:** project state is not yet reconciled with CI results;
  PC-05b CI ingestion is pending. The repository contains examples and
  automated tests, but the claim that the compiler is used without separate
  manual test campaigns has not been established by measured ongoing use.
  Thus the complete Milestone A is not yet met.

PC-09 Pareto evaluation is awaiting a captain decision, and PC-05b CI
ingestion is still landing. They are pending and are not documented here as
available behavior. PC-17 measured performance optimizations remain pending;
the benchmark suite establishes measurements but does not itself optimize
the Rust implementation. No public package or release is created by this
internal tag.
