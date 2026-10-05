# Status — ultimate-web-stack-blueprint

_Last updated: 2026-10-05. Keep this file short; it answers the eight questions below and nothing else._

| Question | Answer |
|---|---|
| What are we building? | A reusable, evidence-driven web blueprint (SolidJS + Rust/Axum + PostgreSQL, optional Redis, NATS, ClickHouse, Pingora). `scripts/create-project` generates independent, secure products from it. Objective: `.ai/config/project.json`. |
| What works? | The full stack: auth (BFF/OIDC, sessions, RBAC/Cedar, organizations, admin, audit, API keys), jobs, realtime, the outbound engine, benchmarks with gates, observability, and four deployment tiers. `./dev check` passes. Generated projects are validated by `scripts/validate-generated`; results are in `docs/acceptance/generated-projects.md`. |
| What is being worked on? | `.ai/state/CURRENT_TASK.json` (M6/M7: generator acceptance, final report). |
| What is blocked? | Nothing. `python3 tools/ai-task list --status blocked` |
| What comes next? | `.ai/state/ROADMAP.json`: finish M6 Reuse and M7 Completion. After that, measure on dedicated Linux hardware the candidates marked inconclusive (PGO, allocators) in `docs/benchmarks/SUMMARY.md`. |
| Major decisions? | `.ai/knowledge/DECISIONS/` (0003–0011, each with evidence, alternatives and reversal conditions); one-page view in `docs/benchmarks/SUMMARY.md`. |
| What proves current behaviour? | `validation.commands` in `project.json` (`./dev check`), `./dev test --system` against a running stack, and `benchmarks/results/*.json`. |
| Known problems? | `.ai/knowledge/KNOWN_ISSUES.md` |
