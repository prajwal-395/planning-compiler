# Security Policy

## Reporting a Vulnerability

**Do not open a public issue for a security vulnerability.**
Report it privately through
[GitHub security advisories](https://github.com/prajwal-395/planning-compiler/security/advisories/new)
("Report a vulnerability" on the repository's Security tab) so it can be
triaged before any public disclosure.

Include, where you can:

* the affected version or commit, and how to reproduce the issue;
* the impact you see (what an attacker could do with it);
* any suggested mitigation.

## What to Expect

* Acknowledgement of your report within 7 days.
* A triage decision (accepted / not-a-vulnerability / needs-more-info).
* For accepted reports: a fix and a coordinated disclosure timeline agreed
  with you before anything is published. Credit is given on request.

## Scope

This policy covers the code, build, and release artifacts in this repository
(the `pc` and `pc-mcp` binaries and the `pc-*` library crates). Dependency
vulnerabilities are tracked separately via `cargo-deny` (`deny.toml`) and the
RustSec advisory database in CI.

## Past Advisories

Published advisories, if any, are listed on the repository's
[security advisories page](https://github.com/prajwal-395/planning-compiler/security/advisories).
