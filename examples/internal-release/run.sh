#!/bin/sh
set -eu

repo=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
work=$(mktemp -d "${TMPDIR:-/tmp}/pc-internal-release.XXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM

export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
export CARGO_BUILD_JOBS=2
cargo build --manifest-path "$repo/Cargo.toml" -p pc-cli -p pc-mcp --bins
pc="$repo/target/debug/pc"
pc_mcp="$repo/target/debug/pc-mcp"

mkdir -p "$work/plan"
cp "$repo/examples/resource-contention/plan.toml" "$work/plan/plan.toml"
cp "$repo/examples/internal-release/sync-map.json" "$work/plan/sync-map.json"
cat "$repo/examples/internal-release/investigation.toml" >> "$work/plan/plan.toml"

echo '== Record and evaluate a decision/outcome =='
"$pc" decision "$repo/examples/internal-release/decision.json" "$work/plan"
"$pc" outcome "$repo/examples/internal-release/outcome.json" "$work/plan"
"$pc" evaluate --decision internal-release-demo-2026-10-13 "$work/plan"

echo '== Reconcile a mapped tasks-axi snapshot =='
"$pc" sync --source tasks-axi --input "$repo/examples/internal-release/tasks-axi.backlog.md" --dry-run "$work/plan"

echo '== Select by makespan with resource constraints =='
"$pc" compile --objective makespan "$work/plan"

echo '== Execution and discovery frontiers (VOI) =='
"$pc" frontier "$work/plan"

echo '== Capability inventory =='
"$pc" capabilities "$work/plan"

echo '== List available benchmark scenarios =='
"$pc" bench --list

echo '== Discover a manifest candidate and emit a proposal =='
"$pc" discover --manifest "$repo/tests/fixtures/discovery/Cargo.toml" \
  --task transcribe-footage \
  --produces 'transcript[words,timings,segment_confidence]' \
  --out "$work/discovery-patch.json" \
  "$repo/tests/fixtures/discovery/plan.toml"
test -s "$work/discovery-patch.json"

echo '== List and call MCP tools =='
python3 "$repo/examples/internal-release/mcp-smoke.py" "$pc_mcp" "$work/plan"
