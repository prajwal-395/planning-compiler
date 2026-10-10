# ADR 0003: no human executors in v0

Date: 2026-10-10

The report recommended modelling roles (`captain-review`) without named
people. The captain chose "leave people out for now".

## Decision

- IR v0 has **no `human-role` capability kind** and **no `human_minutes`
  metric**. The remaining kinds are `agent`, `tool`, `workflow`, `skill`
  and `service`; the remaining metrics are `duration_minutes`, `money_usd`
  and `failure_risk`.
- Weights or estimates naming removed metrics fail validation (E015/E010)
  instead of silently passing, so reintroducing people later is loud.

## Consequences

- The "minimize human input" objective cannot be expressed yet; it returns
  with human modelling after the compiler proves useful without it.
- Named-person performance profiles stay out for consent and privacy
  reasons, independent of this ADR.
