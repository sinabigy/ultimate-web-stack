# Runtime experiment: Tokio vs Monoio, epoll vs io_uring (Linux)

Sources: `benchmarks/results/20261004T141911Z-5d7a9e6f204e-runtime.json` and
`benchmarks/results/20261004T142225Z-5d7a9e6f204e-runtime.json` (two consecutive runs).
Reproduce with `python3 benchmarks/runtime/run.py`. The code is in `backend/experiments/runtime`.

**Setup**
- Linux 6.8.0 (aarch64) in a Docker container on the colima VM, with 2 vCPUs on an Apple M5.
- seccomp unconfined, because Docker's default profile blocks io_uring; `io_uring_disabled=0`.
- rustc 1.99.0, release builds (thin LTO, codegen-units 1).
- oha 1.16.0 in the same container, over loopback, with keep-alive: 8 s per point after a 2 s
  warm-up.

**Variants.** Every one serves the same 13-byte response with identical request-parsing code, so
what differs is the runtime and the I/O driver:

| variant | runtime | I/O interface | threading |
|---|---|---|---|
| `tokio_mt` | Tokio multi-thread | epoll | work-stealing, one listener |
| `tokio_tpc` | Tokio current-thread × N | epoll | thread-per-core, `SO_REUSEPORT` |
| `monoio_epoll` | Monoio (legacy driver) | epoll | thread-per-core, `SO_REUSEPORT` |
| `monoio_uring` | Monoio (io_uring driver) | io_uring | thread-per-core, `SO_REUSEPORT` |
| `axum_ref` | axum on Tokio multi-thread | epoll | the blueprint's real stack (routing, middleware-free) |

**Results** (requests/s; the two runs are shown as a range):

| variant | 1 thread, c=64 | 1 thread, c=256 | 2 threads, c=64 | 2 threads, c=256 |
|---|---:|---:|---:|---:|
| `tokio_tpc` | 294k–295k | 302k–304k | 317k–318k | 342k–352k |
| `tokio_mt` | 283k–288k | 285k–297k | 308k | 328k–330k |
| `monoio_epoll` | 274k–276k | 278k | 309k–312k | 332k–336k |
| `monoio_uring` | 253k–256k | 276k–279k | 235k–238k | 300k–301k |
| `axum_ref` | 226k–228k | 266k–270k | 267k–268k | 277k–280k |

p99 latency is between 0.8 and 3.4 ms for every variant; no variant stands out.

## Findings

1. **io_uring did not beat epoll here.** On identical code and runtime, Monoio's io_uring driver
   was 0–24% *slower* than its own epoll driver, and slower than Tokio at every point.
2. **The threading model matters more than the syscall interface.** Thread-per-core Tokio
   (`SO_REUSEPORT`, no work stealing) was the fastest at every point, 3–7% above Tokio's
   work-stealing runtime.
3. **The framework costs more than either.** axum is 15–20% below the raw responders, which is
   the price of routing, `http` types and hyper's connection state machine. That overhead buys
   correctness and features that a raw loop lacks.

## Caveats (why this is not a universal verdict)

- The load generator shares the container's 2 vCPUs with the server, and traffic is loopback-only.
  io_uring's documented advantages show with many connections per core, larger reads and writes,
  file I/O, multishot accept/recv with provided buffers, or SQPOLL. Monoio's default TCP path uses
  none of these.
- These results come from a virtualised kernel. Bare-metal Linux with dedicated cores and real NICs
  may differ. Re-run `benchmarks/runtime/run.py` on the target hosts before deciding otherwise.

Decision: [ADR 0010](../../.ai/knowledge/DECISIONS/0010-stay-on-tokio-no-io-uring-runtime.md).
