# Known issues

_Open problems that a future worker must not rediscover the hard way. Each entry gives the symptom, a reproduction, the impact and any workaround. Delete an entry once it is fixed._

- **No runnable validation yet** (2026-10-04): the repository contains only protocol files. Confirmed validation commands are added as each component lands (T-0002 onward). Impact: `ai-check` warns "no confirmed validation commands".

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
