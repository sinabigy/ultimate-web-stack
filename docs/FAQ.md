# FAQ

**Is this a framework?**
No. It is a generator plus a reference architecture. `scripts/create-project` copies a working
application into a new repository, and from then on it is your code: no runtime dependency, no
lock-in, no hidden package.

**Why Rust and SolidJS?**
Both were chosen deliberately and then measured where it mattered. Tokio + axum was the fastest
runtime variant we measured, and axum's overhead was larger than any runtime difference
([ADR 0010](../.ai/knowledge/DECISIONS/0010-stay-on-tokio-no-io-uring-runtime.md)). SolidJS gives
a small, fast SPA without a Node.js server in production. If your team doesn't want Rust, this
is not your stack, and that's fine.

**Do I need Redis, NATS, ClickHouse or Kubernetes?**
Not to start. The core runs on PostgreSQL alone: sessions, jobs, realtime fan-out and audit.
Each optional module comes with a measured *when it wins* and *when it doesn't*
([benchmarks/SUMMARY.md](benchmarks/SUMMARY.md)). Add one with a profile at generation time, or
later with [MODULES.md](MODULES.md).

**Are the benchmark numbers valid on my hardware?**
No, and we say so. They were measured on one Apple M5, with services in a 2-CPU / 4 GiB VM
(median of 3; noise measured at ≤ 7.6% between full runs). Use them as *relative* evidence for
the decisions, then run `./dev benchmark` on your own hardware. Results from other machines are
welcome ([CONTRIBUTING.md](../CONTRIBUTING.md#benchmark-contributions)).

**What does "AI-ready" actually mean?**
Every repository, including generated ones, carries a model-independent working protocol
(`AI_PROTOCOL.md`, `.ai/` state, tasks, knowledge and ADRs) plus written conventions. We tested
it: two coding agents were given *only* a generated repository and a product request. Each built
a complete, secure, organization-scoped feature and passed the full validation without looking
anywhere else ([FINAL_REPORT.md](FINAL_REPORT.md#clean-room-findings)).

**Which identity providers work?**
- **Development and tests:** an in-repo mock OIDC provider.
- **Verified live:** self-hosted ZITADEL.
- **Configuration only, not verified live:** any standards-compliant OIDC provider with PKCE
  (ZITADEL Cloud, Auth0, Keycloak and so on). `app-server check-config --online` validates
  discovery.

**Is it production-ready?**
- **Tested:** the release image and systemd deployments were tested live, including restart,
  graceful shutdown and crash recovery.
- **Failure drill:** database, NATS and ClickHouse outages, and provider faults, were exercised
  against a running stack.
- **Not proven:** Kubernetes (statically validated only), and GitHub CI, which has not run yet.

The full list is in [FINAL_ACCEPTANCE.md](FINAL_ACCEPTANCE.md#known-limitations). Read it before
you decide.

**Can I use it commercially? What license does my generated project have?**
The blueprint is dual-licensed MIT OR Apache-2.0. Your generated application is yours to license
as you like. Keep the blueprint's license notices that come with it (in `third_party_licenses/`),
as both licenses require.

**How do I get fixes into a project I already generated?**
Release notes list the changed files; apply them, or regenerate and diff. A guided updater is on
the [roadmap](../ROADMAP.md).

**Can I add GraphQL, SSR, MongoDB, …?**
In your generated project, yes: it's your code. In the blueprint's defaults, only with evidence
(see [CONTRIBUTING.md](../CONTRIBUTING.md#architecture-proposals)).
