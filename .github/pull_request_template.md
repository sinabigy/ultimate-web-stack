## What and why
<!-- One concern per PR. Link the issue/proposal. -->

## Evidence
<!-- What proves this works? Test names, commands and their output, benchmark result files. For a bug fix: the test that failed before the fix. -->

## Checklist
- [ ] `./dev check` passes locally (paste the `ai-validate:` summary line)
- [ ] Tests added or updated (Rust through `support::TestApp` for HTTP behaviour; Vitest/Playwright for UI)
- [ ] Security invariants kept (`.ai/knowledge/CONSTRAINTS.md`); authorization changes pass `authorization_matrix` under RBAC and Cedar
- [ ] Docs updated (`docs/`, `.ai/knowledge/CONVENTIONS.md` if a new convention, `CHANGELOG.md` under *Unreleased*)
- [ ] Changes a default? → linked accepted architecture proposal / ADR
