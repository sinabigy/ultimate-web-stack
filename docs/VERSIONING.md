# Versioning and releases

The blueprint uses **semantic versioning** on git tags (`vMAJOR.MINOR.PATCH`). A generated
project records its origin in two places:
- `.ai/config/project.json → architecture.origin.version` (for example `v1.0.0`);
- the first lines of its `README.md`.

| change | version bump |
|---|---|
| bug or security fix, docs, tests, tooling that doesn't change generated output semantics | PATCH |
| new optional module, new generator flag, new profile, additive API | MINOR |
| a changed default (stack, profile contents, config keys, wire formats), or anything that breaks `./dev` commands for existing generated projects | MAJOR |

## What a release must show
A tag is cut only when, on the exact tree being tagged:
- `./dev check` passes;
- `scripts/validate-generated` passes for the three reference profiles (core/basic,
  performance/b2b/cedar, distributed/b2b);
- `CHANGELOG.md` lists the changes, the compatibility (AI protocol version, toolchains,
  service versions) and any upgrade steps for generated projects.

The release workflow (`.github/workflows/release.yml`) publishes the changelog section as
GitHub release notes when a `v*` tag is pushed.

## Generated projects after a release
Generated projects are independent: they don't update automatically. Each release note lists the
files that changed, so you can apply fixes by hand, or regenerate and compare (`diff -r`). A
guided updater is on the [roadmap](../ROADMAP.md).

## Benchmark evidence across versions
Benchmark results are tied to the machine and commit they were measured on
(`benchmarks/results/*.json`). A release does not re-run every benchmark. It re-runs the invariant
gates (`./dev benchmark --smoke`), and a full re-measure happens when a change could plausibly move
a number.
