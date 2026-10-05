# Why this exists

Every serious web product rebuilds the same foundation:
- login that doesn't leak tokens;
- organizations and roles that don't leak data between tenants;
- an admin area, an audit trail, background jobs;
- calls to rate-limited APIs that don't melt down;
- observability, and a deployment story.

That is weeks of work before the first real feature, and it is where most security mistakes live.

Starter templates promise to skip it. In practice they pick technologies by popularity, test
the happy path, and leave you to discover the failure modes in production. This project tried
a different method.

## The method: try to disprove the stack
1. **Measure every choice.** Each technology had to earn its place in a benchmark on the same
   harness, with recorded noise and regression gates. Candidates that did not win were rejected,
   and that was written down: Monoio/io_uring, an extra proxy hop, a Redis session cache, fat LTO
   by default. Candidates that only win for some workloads became optional profiles, each with
   its measured "when".
2. **Attack the security model.**
   - An authorization matrix runs over HTTP under two policy engines.
   - Browser tests run per role.
   - A scan checks that every handler goes through one authorization path.
   - Each bug found became a regression test that was proven to fail without the fix.
3. **Break it on purpose.** A live drill stops PostgreSQL, NATS and ClickHouse, injects provider
   429s, 5xx errors and timeouts, forges sessions and drops CSRF tokens. Deployments are tested
   live: the release image, and systemd units in a real systemd.
4. **Hand it to a stranger.** Two AI coding agents were given *only* a generated repository and a
   feature request. They built secure, tenant-isolated features from the repository's own docs.
   Every gap they hit was fixed in the blueprint.

The result is not a list of fashionable tools. It's a foundation where every default has a
reason you can read (an ADR with evidence and reversal conditions), and every claim has a test
or a result file behind it.

## Who it's for
- **Teams starting a B2B or SaaS product** who want auth, tenancy, admin and audit done
  correctly on day one.
- **Engineers who want to see the evidence** before trusting a stack.
- **People building with AI coding agents** who need a codebase an agent can extend safely,
  without a human re-explaining the architecture every session.

## Who it's not for
- If you want a no-code or low-code builder, or a JavaScript-only server, this isn't for you.
- If you need something already proven on Kubernetes at scale: those manifests are statically
  validated only (see [FINAL_ACCEPTANCE.md](FINAL_ACCEPTANCE.md)).
