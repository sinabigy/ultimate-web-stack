# 0011: Release profile: thin LTO, codegen-units 1, system allocator; PGO and allocators optional

- Status: accepted
- Date: 2026-10-05

## Context
The brief asks for measured release optimisation: LTO, PGO and allocators. Each option costs
build time, tooling or operational complexity, and should be kept only if it pays off.

## Evidence
See [docs/benchmarks/release-profile.md](../../../docs/benchmarks/release-profile.md).

- **LTO and codegen units**: throughput is within noise across no LTO, thin and fat. Binary size:
  - no LTO: 32.2 MB, 110 s build;
  - thin LTO: 23.7 MB, 130 s;
  - fat LTO: 20.4 MB, 179 s.
- **Allocators**: interleaved runs put mimalloc and jemalloc inside the baseline's own ±10% band.
  An earlier sequential run that looked 26–36% slower was environmental drift.
- **PGO** (Linux, A B A B): +7% plaintext, +9% json, +16% db and −3% cached. All of these are
  within the baseline's 19% round-to-round spread. PGO adds an instrumented build, training, and
  a 241 s optimised build.
  - On macOS the full dependency graph's raw profiles were unreadable by `llvm-profdata`, with
    LTO and without; a minimal crate worked.

## Decision
- `[profile.release]` stays as it is: thin LTO, codegen-units 1, line-table debug info, stripped
  debuginfo. That is the size-to-build-time sweet spot, with no throughput loss.
- **The system allocator** stays the default. `alloc-mimalloc` and `alloc-jemalloc` remain cargo
  features of `app-server`; the Dockerfile takes `--build-arg CARGO_FEATURES`.
- **PGO** is not part of the release pipeline. `benchmarks/release/pgo_linux.py` reproduces it on
  Linux for teams with a stable production workload to train on.
- `release-nolto` and `release-fatlto` profiles remain for experiments (fast local release builds;
  size-critical images).

## Alternatives considered
- **Fat LTO by default**: 14% smaller than thin, for 38% more build time and no measured speed
  gain. Not worth it for CI time.
- **mimalloc or jemalloc by default**: no measured gain, plus a C dependency.
- **PGO in CI**: its gains are not distinguishable from noise here, and it needs a representative,
  maintained training workload.

## Reversal conditions
- On dedicated benchmark hardware, where noise is below 3%: adopt PGO or an allocator if it shows
  a gain of at least 5% at equal p99 on the production request mix.
- Adopt fat LTO if image size becomes a constraint (edge or embedded targets).
