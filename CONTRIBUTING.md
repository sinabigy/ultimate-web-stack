# Contributing

Thanks for helping. This project has one unusual rule that shapes everything else:

> **Claims need evidence.** The architecture is frozen at 1.0.0. A change to it needs either
> a failing acceptance test (a bug), or new measurements that beat the evidence behind the
> current decision (an ADR's *reversal conditions*). Opinions and benchmarks from elsewhere are
> welcome as context, but they don't change defaults on their own.

Documentation, tests, tooling, examples, bug fixes and new evidence are always welcome and need
no proposal.

## Getting set up

Prerequisites: Rust (pinned in `backend/rust-toolchain.toml`), Node ≥ 22, Python ≥ 3.11, Docker
with compose (give the VM at least 6 GiB), and `oha` if you run benchmarks.

```sh
./dev setup        # toolchains, dependencies, git hooks
./dev up           # PostgreSQL + enabled modules, API, worker, SPA (mock identity provider)
./dev check        # every canonical validation command; must be green before a PR
```

`./dev check` takes about 6–10 minutes warm. Its non-obvious workflow rules (compile-time-checked
SQL, generated TypeScript types, where org navigation lives) are in
[`.ai/knowledge/CONVENTIONS.md`](.ai/knowledge/CONVENTIONS.md). Read that file before your first
change.

## Kinds of contribution

| you want to… | do this |
|---|---|
| report a bug | open a **Bug report** issue with the failing command and its output |
| report a security problem | **don't open an issue**: follow [SECURITY.md](SECURITY.md) |
| fix a bug | add a test that fails first, then fix it; mention the test in the PR |
| improve docs, examples, DX | open a PR directly |
| contribute benchmark results | open a **Benchmark result** issue (see below) |
| change a default or add a module | open an **Architecture proposal** issue first (see below) |
| ask a question | GitHub Discussions → Q&A (see [SUPPORT.md](SUPPORT.md)) |

Looking for a first task? Filter issues by **`good first issue`** or **`help wanted`**.

## Pull requests
1. Branch from `main`. Keep the PR to one concern.
2. Make `./dev check` pass locally. CI runs the same commands (`tools/ai-validate`), so there is
   no separate "CI-only" configuration to satisfy.
3. Tests must use the existing harnesses:
   - Rust: `backend/crates/*/tests/`, HTTP through `support::TestApp`;
   - frontend: Vitest (`frontend/tests/unit/`) and Playwright (`frontend/tests/e2e/`).
4. Security-relevant changes must keep the 10 invariants in
   [`.ai/knowledge/CONSTRAINTS.md`](.ai/knowledge/CONSTRAINTS.md). Authorization changes must
   pass `authorization_matrix.rs` under both RBAC and Cedar.
5. Fill in the PR template, including **what evidence proves this**.

AI coding agents are welcome contributors too. Start with [`AI_PROTOCOL.md`](AI_PROTOCOL.md). The
same rules apply: a human maintainer reviews every PR.

## Benchmark contributions
Results are only comparable on the same machine, so we collect them per machine:
1. Run `./dev benchmark` on a clean tree. It writes `benchmarks/results/<UTC>-<sha>.json`, with
   your machine fingerprint, git commit and dirty flag.
2. Open a **Benchmark result** issue. Attach the JSON and describe the hardware, the OS and where
   services ran (native, VM or cloud).
3. Results on a new machine never fail regression gates against our baseline; they add
   evidence. Repeated, interleaved runs that contradict a decision may reopen it through an
   architecture proposal.

Particularly wanted: dedicated Linux hardware for the inconclusive candidates (PGO,
mimalloc/jemalloc, thread-per-core Tokio).

## Architecture proposals
1. Open an **Architecture proposal** issue covering:
   - the problem;
   - the evidence you have or plan to gather;
   - the alternatives;
   - what would make the proposal wrong (its reversal conditions).
2. If maintainers agree it is worth measuring, the result becomes an ADR in
   `.ai/knowledge/DECISIONS/`, using the template `0000-template.md`. The ADR must include
   Evidence, Alternatives and Reversal conditions.
3. A new optional module must be switchable like the existing ones: a cargo feature,
   configuration, a compose profile, a CI service and a validation command. It must also be
   absent from generated projects that don't select it (`scripts/validate-generated` checks this).

## Licensing of contributions
Unless you explicitly state otherwise, any contribution you intentionally submit for inclusion
in this project is dual-licensed under the MIT and Apache-2.0 licenses, as defined in
[LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE), without any additional terms or
conditions.

By participating you agree to the [Code of Conduct](CODE_OF_CONDUCT.md).
