# ADR 0001: license deferred, no LICENSE file

Date: 2026-10-10

The scoping report recommended `MIT OR Apache-2.0`, public at step 3. The
captain chose "defer to later to decide" for both license and open timing.

## Decision

- The repo carries **no `LICENSE` file** and no `license` field in any
  `Cargo.toml` until the captain decides.
- The repo stays **private**.
- `cargo-deny` still polices *dependency* licenses (`deny.toml`): no
  AGPL/GPL/LGPL, no EPL (which would arrive via solver C dependencies),
  no unknown registries. The core stays permissively licensable so the
  future choice is unconstrained.

## Consequences

- No publishing to crates.io until licensed (explicitly out of scope).
- Contributors grant no explicit license yet; the private repo and the
  small team make this acceptable short-term. Revisit before going public.
