# Backlog

## In flight
- [ ] pc-import-axi-a1 - Sketch the tasks-axi adapter shape (repo: planning-compiler) (kind: ship) (priority: 1) (since 2026-10-12)
  Decided: narrow IR passthrough plus import/export verbs.
- [ ] pc-import-axi-a2 - Probe canonical rendering (parens) and plain https://example.com/spec links blocked-by: pc-import-axi-a1 (repo: planning-compiler) (kind: scout) (since 2026-10-12)
  First paragraph.

  Second paragraph (with parens).
## Queued
- [ ] pc-import-axi-b1 - Extend the IR with tasks_axi passthrough blocked-by: pc-import-axi-a2 (repo: planning-compiler) (kind: ship) (priority: 2) (since 2026-10-12) (hold: waiting on IR review)
- [ ] pc-import-axi-b2 - Parse backlog.md into the IR blocked-by: pc-import-axi-b1 (repo: planning-compiler) (kind: ship) (since 2026-10-12) (hold: waiting on captain) (hold-kind: captain)
  Strict parser, loud errors, never silent drops.
- [ ] pc-import-axi-b3 - Render the IR back to backlog.md blocked-by: pc-import-axi-b1 blocked-by: pc-import-axi-b2 (repo: planning-compiler) (kind: scout) (since 2026-10-12) (hold: waiting on captain) (hold-kind: captain) (hold-until: 2026-12-01)
- [ ] pc-import-axi-b4 - Bare task with no repo or kind (since 2026-10-12)
- [ ] pc-import-axi-b5 - Wire pc import-tasks-axi and export-tasks-axi blocked-by: pc-import-axi-b3 (repo: planning-compiler) (kind: docs) (priority: 0) (since 2026-10-12)
## Done
- [x] pc-import-axi-c1 - Scout the master plan section 4 (repo: planning-compiler) (kind: ship) (done 2026-10-11)
  Read-only pass, no commits.
- [x] pc-import-axi-c2 - Verify PR-linked done label https://github.com/prajwal-395/planning-compiler/pull/1 blocked-by: pc-import-axi-c1 (repo: planning-compiler) (kind: ship) (priority: 1) (merged 2026-10-12)
  Closed with a PR link, so the label reads merged.
- [x] pc-import-axi-c3 - Record report path data/pc-tasks-axi/report.md blocked-by: pc-import-axi-c2 (repo: planning-compiler) (kind: docs) (reported 2026-10-12)
