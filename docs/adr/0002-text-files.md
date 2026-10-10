# ADR 0002: text files in the repo, no database

Date: 2026-10-10

The report recommended text files first, SQLite only on measured need. The
captain chose "text files in the repo".

## Decision

- Plans live as **TOML or JSON** in the project repo; evidence appends to
  a sibling **JSONL** log. Every run compiles in memory.
- No SQLite, no Postgres, no central planning home in v0.
- Git history plus the JSONL log is the change log and the provenance for
  "the decision evolved and why".

## Consequences

- Plans diff and review in PRs; agents edit them with ordinary file tools.
- Full recompute is cheap at these sizes (the 1,000-task ILP spike was the
  only slow part, and selection arrives in step 3 with a time budget).
- A measured need (query latency, concurrent writers) reopens this ADR.
