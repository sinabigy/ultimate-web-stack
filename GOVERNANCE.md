# Governance

## Model
The project is currently run by its founding maintainer, who has final say. As regular
contributors appear, maintainership will be extended to them (see below), and this document will
be updated.

## How decisions are made
- **Evidence over opinion.** Defaults change only with a failing acceptance test (a bug) or new
  measurements that meet an ADR's reversal conditions. Decisions are recorded as ADRs in
  `.ai/knowledge/DECISIONS/`, with evidence, alternatives considered and reversal conditions.
- **Small changes** (docs, tests, fixes, tooling) are merged by any maintainer after review and a
  green `./dev check` in CI.
- **Architecture changes** go through an *Architecture proposal* issue and, if accepted, an ADR,
  reviewed by at least one maintainer who did not write it.
- **Security fixes** may be developed privately and released before public discussion (see
  [SECURITY.md](SECURITY.md)).

## Becoming a maintainer
Contributors with a sustained record of good reviews and merged work may be invited. Maintainers
can merge, triage and cut releases. Inactive maintainers may step down to emeritus status at any
time.

## Releases and versioning
See [docs/VERSIONING.md](docs/VERSIONING.md).
