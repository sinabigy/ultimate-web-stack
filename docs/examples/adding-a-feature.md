# Walkthrough: adding an organization-scoped feature

This is the path both clean-room agents took to build secure features (Projects; Announcements
with notifications). It uses a hypothetical **Notes** feature: members read the notes, admins
write them. The reference implementation to copy from is **runs**:
- `backend/crates/api/src/routes/orgs.rs`, runs section;
- `backend/crates/db/src/runs.rs`;
- `frontend/src/routes/org.tsx`.

The snippets are sketches: names in `«»` are yours, everything else is the project's real API.

## 0. Start the task
```sh
python3 tools/ai-check
python3 tools/ai-task new "Organization notes" --objective "…" --accept "…" --validate "./dev check" --class feature
python3 tools/ai-task start T-000N
./dev up --no-app                      # database for compile-time-checked SQL
export DATABASE_URL=postgres://app:app-dev-only@localhost:55432/app   # development only
```

## 1. Permissions
In `backend/crates/authz/src/permission.rs`, declare the permissions. Read permissions must end in
`:read`:
```rust
NotesRead => "notes:read", "View notes";
NotesManage => "notes:manage", "Create, edit and archive notes";
```
In `backend/crates/authz/src/rbac.rs` → `builtin_permissions`, grant them: `NotesRead` to viewer
and up, `NotesManage` to admin. Then pin the decisions with rows in `role_permission_matrix`
(`backend/crates/authz/tests/conformance.rs`). Cedar needs no change, because `base.cedar` allows
any permission the role holds.

## 2. Database
Add `backend/migrations/<UTC timestamp>_notes.sql` with an `organization_id uuid not null
references organizations(id)` column and an index that starts with `organization_id`. Then add
`backend/crates/db/src/notes.rs`. Every function takes the `&OrgAccess` proof, never an org id:
```rust
pub async fn list(db: impl PgExecutor<'_>, access: &OrgAccess) -> DbResult<Vec<«Note»>> {
    sqlx::query_as!(«Note», "SELECT … FROM notes WHERE organization_id = $1 …", access.org_id())
        .fetch_all(db).await.map_err(Into::into)
}
```
Run `./dev db migrate` after writing the migration, and `./dev db prepare` once the queries
compile. Commit `backend/.sqlx/`.

## 3. API: authorize, write, audit in one transaction
```rust
pub async fn create_note(State(state): State<AppState>, o: Org, meta: ReqMeta, Json(b): Json<«NewNote»>)
    -> Result<(StatusCode, Json<«Note»>), ApiError> {
    require(&state, &o, &meta, P::NotesManage, &Resource::Organization).await?; // denial → 403 + authz.denied audit
    let svc = state.svc()?;
    let mut tx = svc.db.begin().await.api()?;
    let note = notes::create(&mut *tx, &o.access, &b).await.api()?;
    app_db::audit::insert(&mut *tx, &ev(&state, &o, &meta, "note.created")).await.api()?;
    tx.commit().await.api()?;
    Ok((StatusCode::CREATED, Json(note)))
}

pub async fn list_notes(State(state): State<AppState>, o: Org) -> Result<Json<…>, ApiError> {
    require_read(&state, &o, P::NotesRead, &Resource::Organization)?;  // reads use the engine too
    …
}
```
Register the routes in `backend/crates/api/src/routes/mod.rs` under `/api/v1/orgs/{slug}/notes`.
A non-member never reaches your handler: the `Org` extractor answers 404. To notify members,
see *In-app notifications* in `.ai/knowledge/CONVENTIONS.md`. For DTOs with `#[derive(TS)]`, run
`./dev types`.

## 4. Tests through HTTP
In `backend/crates/api/tests/notes.rs`, using `support::TestApp`:
```rust
let app = TestApp::new(pool).await;
let owner = app.login("owner@notes.example").await;
let org = app.create_org(&owner, "Notes A", "notes-a").await;
let member = app.login("member@notes.example").await;
app.add_member(&owner, &org, &member, "member").await;
// admin writes 201; member reads 200 but writes 403 and the denial is audited;
// another org's user gets 404 by slug and by id; anonymous gets 401.
```
Cover every boundary listed in the comment. For engine-specific behaviour, set
`c.authorization.engine` in `TestApp::with_config`.

## 5. Frontend
- Add calls to `frontend/src/api/endpoints.ts`.
- Add a page to `frontend/src/routes/org.tsx`, gating controls on the `permissions` the overview
  returns.
- Add the route in `frontend/src/app.tsx`.
- Add nav entries in `features/shell/AppShell.tsx` and `features/shell/CommandPalette.tsx`.
- Add a unit test of the presentational part (`frontend/tests/unit/`) and, ideally, an E2E spec.

## 6. Prove and record
```sh
./dev check                     # every canonical command; about 6–10 min warm
./dev up                        # then, in another terminal:
./dev test --system             # existing journey still passes
python3 tools/ai-task complete T-000N --summary "…" --evidence "./dev check: OK — N pass" --evidence "…"
python3 tools/ai-check
```
Update `.ai/knowledge/` if you established a new convention, and the permission table in
`docs/authorization/README.md`.
