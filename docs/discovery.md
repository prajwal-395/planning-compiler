# Capability discovery (PC-13)

Structured discovery lives outside the deterministic core. Deterministic
adapters scan external sources and emit `pc propose` patches; the compiler
validates them. A candidate never becomes accepted because an adapter
found it: every emitted method is `proposed`, and `pc accept` stays the
only way in.

## Interface

`crates/pc-discovery` owns the adapter contract:

- `discover_from_manifest` maps `[dependencies]` entries to `tool`
  capabilities (`name@version`, `source = manifest:<path>#<name>`).
- `discover_from_repo` maps top-level non-hidden files to `tool`
  capabilities (`stem@0`, `source = repo:<dir>#<file>`).
- `with_contract` attaches the operator- or agent-asserted typed contract.
  Adapters report existence, not contracts: a manifest entry says an
  implementation exists, never what artifacts it produces.
- `check_compatibility` evaluates that hypothesis with the compiler's own
  typed-subset matching (`contract_note_for`) plus the E016
  platform/licence overlap, so discovery and `pc check` never disagree.
- `bottleneck_task` picks the investigation target: unmeasured critical
  cost first, then the top execution entry. `None` means fully planned.
- `candidates_to_patch` builds the patch: a capability the plan already
  models is reused (the patch adds only a method for the target task,
  whose assumption says the modeled record governs; an asserted contract
  that diverges from that record is skipped with both contracts named),
  candidates the task already invokes, E016 violations and anything
  `validate_patch` refuses are skipped with reasons (each would refuse
  the whole patch), contract mismatches stay proposed as blocked
  alternatives carrying the frontier's reason.

`pc discover [--manifest F] [--repo D] [--task T] [--produces a[f,..]]
[--consumes a[f,..]] [--out patch.json] [--json] [plan]` runs the adapters
over a clean plan, defaults `--task` to the bottleneck, and emits the
patch. Without `--out` the patch goes to stdout and the human summary to
stderr, so `pc discover ... > patch.json` stays a clean redirect.

## Evidence

Adapters emit no estimates and no evidence: versions and file names are
existence facts, not measurements. Measurement evidence arrives through
`pc record` (benchmark runs, profiling) and P5 stages it with the proposal
until the method is accepted; the frontier then ranks the proposal with
lowered uncertainty. The `tests/fixtures/discovery` demo walks the full
loop: bottleneck, manifest alternative, compatibility check, staged
benchmark evidence, evidence-backed proposal, incumbent still selected.

## Follow-up sources (not implemented here)

GitHub research, documentation research, and benchmark ingestion plug in
through the same proposal interface: a research agent asserts the
candidate's contract (and records measurements with `pc record`), the
adapter formats the patch, the compiler validates. Per-candidate
contracts, `[workspace.dependencies]` manifests, and an MCP `discover`
tool belong to that follow-up, not to this change.
