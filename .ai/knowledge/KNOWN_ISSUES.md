# Known issues

_Open problems that a future worker must not rediscover the hard way. Each entry gives the symptom, a reproduction, the impact and any workaround. Delete an entry once it is fixed._

## Dev VM memory: release image builds can OOM-kill dev services (2026-10-04)
- **Seen**: ClickHouse was OOM-killed (exit 137, `OOMKilled=true`) while `infra/docker/smoke.sh`
  compiled the release image inside the 4 GiB colima VM.
- **Impact**: later ClickHouse tests failed with "connection refused" until the service was
  restarted.
- **Mitigation**:
  - Give the VM at least 6 GiB (`colima start --memory 6`), or stop optional services
    (`./dev down`) before release builds.
  - `./dev up` restarts exited services.

## Disk usage of target/ (2026-10-05)
- **Seen**: `backend/target` grew to 70 GB (debug `deps` 44 GB, `incremental` 19 GB) and the
  disk filled during development.
- **Mitigation**:
  - `[profile.dev] debug = "line-tables-only"`, with no debug info for dependencies.
  - Run `cargo clean` occasionally; generated projects have their own target/.

## Intermittent E2E failure under full-validation load (2026-10-05)
- **Seen**: one `e2e` failure during a full `./dev check`, right after the heavy Rust test and
  build steps.
- **Not reproduced**: the immediate rerun and three further consecutive runs passed (13/13).
- **Unknown**: the failing test, because its report was overwritten.
- **Mitigation**: CI retries E2E once (`retries: process.env.CI ? 1 : 0`).
- **Next step**: if it recurs locally, keep `frontend/playwright-report` and fix the specific race.

## Intermittent PostgreSQL connect failure in tests through colima (2026-10-05)
- **Seen**: one `sqlx::test` per run failed with `failed to connect to test database:
  unexpected response from SSLRequest: 0x00`. This happened in 2 of 3 full validations of
  generated projects (inside `./dev check`, `rust-test`), each time a different test.
- **Not reproduced**: seven standalone `cargo test --workspace` runs right afterwards all passed.
- **Likely cause**: the colima 0.10.3 host port forwarder. The failure drill showed the
  forwarder producing exactly this error (`SSLRequest: 0x00`) whenever it cannot reach the
  container: with PostgreSQL stopped, the host port still accepts the TCP connection. Under a burst
  of new connections (`sqlx::test` creates a database and a pool per test, in parallel), it
  occasionally fails the same way while PostgreSQL is up. Not reproduced on Linux.
- **Hardening that came out of it**: protocol errors now map to a retryable 503 (`app-db`
  `outages_are_unavailable_not_internal`), not a 500.
- **Impact**: a red `rust-test` that passes on rerun. CI on Linux (no VM forwarder) has not run yet, because nothing has been pushed.
- **Next step**: if it recurs, try lima's SSH port forwarder or run the tests inside the VM to
  confirm the cause. Do not add blind retries to the test harness.

## Cosmetic UI gaps seen in browser acceptance (2026-10-05)
- The admin overview "Job queues" chart renders an empty card when no jobs are queued, instead of
  an empty-state message (`frontend/test-results/journey/acceptance-09-admin-overview.png`).
- On an organization "Not found" page, the organization switcher shows an empty selection instead
  of the user's current workspace.
- **Impact**: cosmetic only; authorization and data are correct. Not release blockers.
