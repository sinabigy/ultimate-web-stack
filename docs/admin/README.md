# Administration

There are two separate admin surfaces, matching the two trust levels (invariant 9):

| surface | who | routes (UI / API) | scope |
|---|---|---|---|
| **Organisation admin** | `admin` / `owner` of that organisation | `/org/:slug/settings`, `/members`, `/teams`, `/roles`, `/api-keys`, `/audit`, `/billing` · `/api/v1/orgs/:slug/*` | one tenant |
| **System admin** | `system_admin` (all) / `system_auditor` (read-only) | `/admin/*` · `/api/v1/admin/*` (only when `admin.enabled`) | the platform |

## System admin

- **Users**: list, search and view. Suspend or reactivate; suspension ends all of the user's
  sessions immediately. Change system roles.
- **Organisations**: list and view, read-only. This is not organisation data access: admins
  cannot browse a tenant's runs or members through organisation APIs. Suspending or deleting
  organisations from the system console is not implemented; owners delete their own
  organisation.
- **Audit**: the global trail, filterable by action, actor and organisation.
- **Jobs**: queue statistics, the dead-letter list, retry.
- **Providers**: live outbound-engine health per provider: concurrency limit, learned rate cap,
  circuit state, error rates and latency.
- **System**: health checks (database, IdP, cache), version and configuration summary. Secrets
  are never shown.

Guards are independent of the UI (`require_system` in `backend/crates/api/src/auth/extract.rs`):
1. The caller must be a user session. API keys and service accounts are never system principals.
2. The user's system role must hold the system permission.
3. MFA is checked from the session's `amr` when `auth.require_mfa_for_system_admin` is set (the
   default).
4. Mutating system actions (`*:manage`) require authentication within
   `auth.reauth_window_minutes`; otherwise the API answers `reauth_required` and the UI starts a
   step-up login.
5. Every denial is audited as `admin.access_denied`, and every change as `admin.*`.

## Bootstrapping the first system admin

Two options:
- **`auth.bootstrap_system_admins = ["ops@example.com"]`**: granted at first login, and only if the
  IdP reports the email as *verified*. A test covers this.
- **`auth.system_roles_from_idp = true`**: the system role is taken from the IdP's role claim at
  every login. `./dev up` with the mock IdP turns this on (development only), so entering
  `system_admin` or `system_auditor` in the mock form's "IdP roles" field, with method
  "Password + TOTP", opens the console. With ZITADEL this is the project role `system_admin` or `system_auditor` on the
  user; `scripts/zitadel_bootstrap.py` grants it to the dev user `alice@example.com`.

## Tests

`backend/crates/api/tests/admin.rs`:
- `ordinary_users_and_org_owners_are_denied`;
- `system_admin_requires_mfa_when_configured`;
- `auditor_reads_but_cannot_manage`;
- `admin_actions_take_effect_and_are_audited`;
- `system_roles_from_identity_provider`;
- `bootstrap_admin_requires_verified_email`.

`frontend/tests/e2e/routes.spec.ts` sweeps every admin route for access, and a11y checks cover
the admin pages.
