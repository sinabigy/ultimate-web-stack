# Conventions

_Only conventions that are **not obvious from the code or the formatter config**. Each entry is a one-line rule plus where it is enforced, if anywhere._

## Workflow
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

## Frontend
- **Permission-gated UI**: hide controls by checking the `permissions` list the API returns. The
  server enforces every decision anyway.
- **Unit tests** (`frontend/tests/unit/`, Vitest + Solid testing library): to test a page, extract
  its presentational part, as a component taking data and permissions as props, and render that.
  Stub `fetch` as `client.test.ts` does when the API layer itself is under test.

## Running checks
- `./dev check` runs alongside a running `./dev up`, because E2E uses its own ports (18080, 59082,
  59091).
- Integration tests need the selected modules' services (`./dev up --no-app`).
