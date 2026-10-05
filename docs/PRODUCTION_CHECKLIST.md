# Production checklist

Work through this before your generated product takes real traffic. Each item names the setting
or the command that proves it. The server enforces many of them itself: with
`APP__ENVIRONMENT=production` it **refuses to start** with insecure cookies, non-HTTPS origins,
missing secrets or bench endpoints enabled.

## Configuration and secrets
- [ ] `APP__ENVIRONMENT=production`, and `app-server check-config --online` passes. This
      validates configuration plus IdP discovery, issuer and PKCE.
- [ ] Generate `APP__AUTH__TOKEN_ENCRYPTION_KEY` and `APP__AUTH__API_KEY_PEPPER` (32 random
      bytes, base64). Store them in a secret store or a `0640 root:app` environment file, never in
      git or an image. Rotating the pepper invalidates API keys.
- [ ] The database URL uses a dedicated role with a strong password; TLS to PostgreSQL if it runs
      on another host.
- [ ] `.env`, `.env.zitadel` and other local files are not deployed. Only `.env.example` lives in
      the repository.

## Identity
- [ ] A real OIDC provider is configured (`APP__AUTH__ISSUER_URL`, client, redirect and
      post-logout URLs over HTTPS). The mock IdP is development-only: never deploy it.
- [ ] Choose how system administrators are appointed. Either
      `auth.bootstrap_system_admins = ["you@company"]`, granted at first verified login, or
      `auth.system_roles_from_idp = true` with roles managed in the IdP. Keep
      `auth.require_mfa_for_system_admin = true`.
- [ ] Email verification, password policy, MFA and recovery are configured **in the IdP**, which
      owns them ([authentication](authentication/README.md)).

## Network
- [ ] TLS terminates in front of the API (Caddy, nginx, a load balancer or an ingress).
      `APP__HTTP__TRUST_FORWARDED_FOR=true` only if that proxy overwrites `X-Forwarded-For`.
- [ ] The ops port (`APP__HTTP__OPS_PORT`, 9090) is reachable only from monitoring. It serves
      `/metrics` and detailed `/readyz`. The public `/readyz` returns status only, and the public
      `/metrics` is a 404.
- [ ] Rate limits fit your traffic (`rate_limit.per_client_rps`, `burst`). With several API
      instances, use the Redis backend so limits are shared.

## Data and operations
- [ ] PostgreSQL backups and a tested restore. The database holds sessions, jobs, events and the
      audit trail; the audit trail is append-only.
- [ ] Migrations run on API start (`APP__DATABASE__MIGRATE_ON_START=true`), behind an advisory
      lock, so concurrent replicas are safe.
- [ ] Shutdown grace is at least 45 s, so in-flight requests and jobs drain (systemd
      `TimeoutStopSec`, Kubernetes `terminationGracePeriodSeconds`).
- [ ] Alerts on readiness (`degraded` means an optional dependency is down), on 5xx rate, on the
      job dead-letter count, and on provider breaker state (`app_provider_*` metrics).
- [ ] Optional modules: if a module is selected, its service is provisioned and monitored. An
      unreachable NATS falls back to PostgreSQL and shows as degraded; a ClickHouse outage drops
      analytics events by design ([failure modes](operations/failure-modes.md)).

## Verify before launch
- [ ] `./dev check` is green on the exact commit you deploy, including `release-smoke` and
      `systemd-live`.
- [ ] `./dev test --drill` against a staging stack: outages produce 503 or `degraded`, never 500
      or hangs.
- [ ] Follow your tier's guide: [VPS](deployment/vps.md), [containers](deployment/containers.md)
      or [Kubernetes](deployment/kubernetes.md). The Kubernetes manifests are statically
      validated only, so test them on your cluster before relying on them.
- [ ] Read [`.ai/knowledge/KNOWN_ISSUES.md`](../.ai/knowledge/KNOWN_ISSUES.md) and
      [FINAL_ACCEPTANCE.md → limitations](FINAL_ACCEPTANCE.md#known-limitations).
