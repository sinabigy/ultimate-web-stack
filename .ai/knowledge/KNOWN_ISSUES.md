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
- **Impact**: a red `rust-test` that passes on rerun. Not seen on GitHub-hosted Linux CI (no VM forwarder), across the release-candidate runs.
- **Next step**: if it recurs, try lima's SSH port forwarder or run the tests inside the VM to
  confirm the cause. Do not add blind retries to the test harness.

## Cosmetic UI gaps seen in browser acceptance (2026-10-05)
- The admin overview "Job queues" chart renders an empty card when no jobs are queued, instead of
  an empty-state message (`frontend/test-results/journey/acceptance-09-admin-overview.png`).
- On an organization "Not found" page, the organization switcher shows an empty selection instead
  of the user's current workspace.
- **Impact**: cosmetic only; authorization and data are correct. Not release blockers.

## Intermittent outbound-engine test timeout (2026-10-05)
- **Seen**: once, in a full `./dev check` on the release tree.
  `rate_limited_provider_is_respected_and_work_completes` (`backend/crates/networking/tests/engine.rs`)
  exceeded its 20 s bound; it normally takes about 5 s.
- **Not reproduced**: 14 further runs passed, including 6 under 3× concurrent load, plus about
  10 earlier full workspace runs.
- **Unknown**: the cause. The `ai-validate` failure excerpt shows the output tail, which was
  compiler lines, so the panic message was lost.
- **Mitigation**: the assertion now reports elapsed time, the 429 count, requests sent and the
  learned rate cap, so the next occurrence explains itself. The bound was not loosened: a
  4× slowdown would be a real controller problem worth seeing.

## Transient failures during v1.0.2 release validation (2026-10-06)
Two one-off failures in local validation of a generated SaaS project (`scripts/validate-generated`,
macOS host, colima 2 CPU / 4 GiB). **Neither is classified as an application bug.** Each passed on
every rerun, and a third full run was entirely clean before tagging.

1. **`rust-test` failed once** inside the generated project's `./dev check`.
   - Then 3/3 standalone reruns passed, and the next two full runs passed 24/24.
   - **Unknown:** which test failed. `ai-validate` keeps the output tail as stdout then stderr, so
     the tail held only compiler stderr.
   - **Candidates (unconfirmed):** the colima port-forwarder connect flake above, or the
     outbound-engine timing test above.
2. **System smoke failed once** with a name-resolution error (`[Errno 8] nodename nor servname
   provided`). The smoke test talks only to `localhost` and `127.0.0.1`, and 5/5 reruns passed.
   - **Unknown:** which request failed. The traceback didn't include the URL.

**Diagnostics added (no retries):**
- the `rust-test*` commands merge stderr (`2>&1`), so the tail ends with the failing test's name
  (mutation-checked with a deliberately failing test);
- `scripts/system_smoke.py` names the method and URL of any connection failure.

**If either recurs:**
1. Keep `var/check.log` and `var/validation-report.json` from the generated project.
2. Record the named test or URL, the time, and host load.
3. Check whether it coincides with the colima connect flake (`SSLRequest: 0x00`). A shared
   environmental cause is the leading hypothesis.
4. Fix only once it's reproducible.

## GCRA property test failed under load: fixed (2026-10-06)
- **Seen**: once, in a full `./dev check` during the hardening pass:
  `rate::tests::granted_slots_conform_to_rate_and_burst` reported 39 grants where 38.98 were
  allowed. The span was measured about 79 µs short.
- **Cause**: the test, not the limiter. The test read `Instant::now()` and the limiter read the
  clock again; a thread preempted between the two reads made a slot look early. The test's 50 µs
  allowance assumed no preemption, and `./dev check` runs many test binaries in parallel.
- **Evidence**: 8 parallel copies × 5 rounds under 6 busy loops: the old test failed 13 of 40
  runs, the fixed one 0 of 40.
- **Fix**: the limiter's private `reserve_within_at(now, …)` takes the clock reading (production
  passes `Instant::now()`, unchanged), so the tests compute slots exactly. The slack went from
  50 µs to 1 µs. A one-slot-early mutation of the limiter still fails the test.
