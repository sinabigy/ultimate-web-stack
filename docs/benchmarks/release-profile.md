# Release profile: LTO, codegen units, allocators, PGO

Reproduce with:
- `python3 benchmarks/release_profile.py` (macOS host; HTTP suite, median of 3 per point);
- `python3 benchmarks/release/pgo_linux.py` (Linux container).

Each variant is a separate build of `app-server`.

## LTO and codegen units
Source: `results/20261005T000315Z-41a60c9ff3b3-release-profile.json`. Each run is listed in it.

| variant | cold build | binary | plaintext c=64 | json c=64 | json c=256 | db |
|---|---:|---:|---:|---:|---:|---:|
| **thin LTO, cgu=1 (shipped)** | 130 s | 23.7 MB | 147k | 154k | 162k | 13.2k |
| no LTO, cgu=16 | 110 s | 32.2 MB | 147k | 152k | 159k | 13.1k |
| fat LTO, cgu=1 | 179 s | 20.4 MB | 149k | 155k | 164k | 13.1k |

Throughput differences between these profiles are within the measured run-to-run noise (≤10%,
[benchmarks/README.md](../../benchmarks/README.md#measured-noise)). What the profiles do change
is binary size and build time:
- thin LTO is 26% smaller than no LTO, for an 18% longer build;
- fat LTO saves another 14% of size, for 38% more build time than thin.

## Allocators

The first sequential runs showed mimalloc and jemalloc 26–36% *slower* on json, cached and DB
routes. A DB-bound route cannot lose 36% to an allocator, so I re-measured interleaved
(baseline, mimalloc, jemalloc, baseline: `results/20261005T0003…-abab-*.json`):

| run | plaintext | json c=64 | json c=256 | cached | db | peak RSS |
|---|---:|---:|---:|---:|---:|---:|
| baseline (1) | 151k | 155k | 165k | 23k ⚠ | 12.7k | 109 MB |
| mimalloc | 148k | 144k | 171k | 140k | 13.1k | 74 MB |
| jemalloc | 159k | 162k | 167k | 153k | 14.4k | 89 MB |
| baseline (2) | 137k | 146k | 157k | 128k | 13.3k | 30 MB |

- The two baseline runs differ by up to 10% from each other, and both allocators fall inside that
  band. The earlier slowdown was environmental drift, not the allocators.
- ⚠ The first baseline's cached route was low in all three samples (20k–38k req/s at 130% server
  CPU), while the same binary did 128k later. The cause is not identified; it is reported, not
  discarded.
- Peak RSS varies 30–109 MB for the *same* binary, so the allocator memory comparison is not
  conclusive either.

## PGO (Linux)

macOS: instrumented builds of the full dependency graph wrote profiles that `llvm-profdata`
rejected:
- "Runtime and instrumentation version mismatch" with LTO;
- "file header is corrupt" without LTO.

A minimal crate worked with the same toolchain (rustc 1.99, LLVM 23.1.1). PGO was therefore
measured on Linux, in `rust:1.99.0-bookworm` on 2 vCPUs, with PostgreSQL reached through the host
gateway: `results/20261005T003338Z-87b596d601f5-pgo-linux.json`.

The procedure:
1. Build instrumented, on the `release-nolto` profile.
2. Train for 10 s each on plaintext, json, db and cached.
3. Run `llvm-profdata merge`.
4. Build the optimised binary on `release` (thin LTO).
5. Measure A B A B at c=64.

| endpoint | baseline (2 rounds) | PGO (2 rounds) | mean change |
|---|---|---|---:|
| plaintext | 106k / 126k | 125k / 120k | +7% |
| json | 104k / 123k | 127k / 121k | +9% |
| db | 7.2k / 6.4k | 8.7k / 7.1k | +16% |
| cached | 107k / 106k | 107k / 100k | −3% |

The baseline's own round-to-round spread is up to 19%, so none of these differences is
established. PGO adds an instrumented build, a training run, and a 241 s optimised build.

## Decision

[ADR 0011](../../.ai/knowledge/DECISIONS/0011-release-profile-thin-lto-system-allocator.md):
- Keep thin LTO with codegen-units 1 and the system allocator.
- The allocator features (`alloc-mimalloc`, `alloc-jemalloc`) and the PGO pipeline stay
  available, as optional, measured options.
