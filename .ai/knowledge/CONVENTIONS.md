# Conventions

_Only conventions that are **not obvious from the code or the formatter config**. Each entry is a one-line rule plus where it is enforced, if anywhere._

## Workflow
- **A concrete feature request before discovery is done**: the user's request *is* their
  objective statement. Record it (`objective` in `.ai/config/project.json`), complete T-0001 with
  that evidence, then create the feature task with `tools/ai-task new`. Do not leave discovery
  queued while building.
- **Adding an endpoint or resource**: follow `docs/authorization/README.md` → "Adding a permission
  and protecting a new endpoint". The runs feature (`routes/orgs.rs` runs section,
  `db/src/runs.rs`, `routes/org.tsx`) is the reference implementation. Enforced by: the
  authorization matrix and conformance tests.
- **SQL is checked at compile time (sqlx).** Queries already recorded in `backend/.sqlx/` build
  offline. A new or changed query needs the dev database:
  1. `./dev up --no-app`;
  2. `export DATABASE_URL=postgres://app:app-dev-only@localhost:55432/app` (development only, the
     same URL `project.json` uses);
  3. build;
  4. `./dev db prepare`, then commit `backend/.sqlx/`.

  `./dev` commands set this up themselves. Enforced by: `sqlx-offline-fresh`.
- **New migration**: `backend/migrations/<UTC timestamp>_<name>.sql`. It is applied by
  `./dev up` and `./dev db migrate`, and at server start when `database.migrate_on_start`.
- **API types**: after changing a DTO (`#[derive(TS)]`), run `./dev types` and commit
  `frontend/src/api/generated/`. Enforced by: `ts-bindings-fresh`.
- **Repository map**: run `python3 tools/ai-map` after commits that add or move files, otherwise
  `ai-check` warns that the map is stale.

## Backend
- **Handlers decide through the engine**: `require` for writes (audits denials), `require_read`
  for reads. Never `access.can()` or `access.require()` in a handler. Enforced by: review;
  `authorization_matrix` runs under RBAC and Cedar.
- **Tenant data**: repository functions take `&OrgAccess`, never an organisation id from the
  request.
- **Audit**: a state change and its `audit::insert` share one transaction. Denied writes are
  audited by `require`.
- **Errors**: return `ApiError` (problem+json). A non-member gets 404, never 403, so existence does
  not leak.
- **Permission keys**: read permissions end in `:read`. `deny()` derives "read = not audited" from
  that suffix (except `audit:read`), so keep the naming.
- **In-app notifications**:
  1. insert with `app_db::notifications::create(db, user_id, Some(org_id), kind, title, body, link)`
     (one row per recipient, inside the same transaction as the change);
  2. after commit, publish `RealtimeEvent::Notification { user_id, notification_id, title }` on
     `svc.events`.

  The reference is the run-finished notification in `backend/crates/workers/src/handlers.rs`.
  Never notify users outside the organization: take recipients from the organization's
  membership.

## Frontend
- **Org navigation is defined in three places**: the tab list in `src/routes/org.tsx`, the sidebar
  in `src/features/shell/AppShell.tsx`, and the command palette in
  `src/features/shell/CommandPalette.tsx`. A new org page goes in all three, plus its route in
  `src/app.tsx` (`tests/e2e/routes.spec.ts` checks every route renders).
- **Permission-gated UI**: hide controls by checking the `permissions` list the API returns. The
  server enforces every decision anyway.
- **Unit tests** (`frontend/tests/unit/`, Vitest + Solid testing library): to test a page, extract
  its presentational part, as a component taking data and permissions as props, and render that.
  Stub `fetch` as `client.test.ts` does when the API layer itself is under test.
  jsdom does not expose an open modal `<dialog>` to role queries: query its content by
  label or text.

## Running checks
- `./dev check` takes about 6–10 minutes warm, longer from a cold `target/`. It builds the release
  image (`release-smoke`) and runs the live systemd test, and it starts the services its commands
  need. Give it a timeout of at least 30 minutes, and the Docker VM at least 6 GiB
  (`KNOWN_ISSUES.md`). `tools/ai-validate` prints its summary at the end, not progressively.
- `./dev check` runs alongside a running `./dev up`, because E2E uses its own ports (18080, 59082,
  59091).
- Live proofs need a running `./dev up`:
  - `./dev test --system`: one journey through every component;
  - `./dev test --journey`: browser acceptance per role, with screenshots;
  - `./dev test --drill`: failure drill, which stops services briefly.
- Integration tests need the selected modules' services (`./dev up --no-app`).
