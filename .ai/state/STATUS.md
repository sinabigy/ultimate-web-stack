# Status — ultimate-web-stack-blueprint

_Last updated: 2026-10-05 (release 1.0.0). Keep this file short; it answers the eight questions below and nothing else._

| Question | Answer |
|---|---|
| What are we building? | A reusable, evidence-driven web blueprint: SolidJS + Rust/Axum + PostgreSQL, with optional Redis, NATS, ClickHouse and Pingora. `scripts/create-project` generates independent, secure products from it. Objective: `.ai/config/project.json`. |
| What works? | Release 1.0.0 (tag `v1.0.0`). Everything in `docs/FINAL_REPORT.md`: auth, RBAC/Cedar, tenancy, dashboards, jobs, realtime, the outbound engine, optional modules, deployment tiers, the generator. Evidence: `docs/FINAL_ACCEPTANCE.md`. |
| What is being worked on? | Nothing: all tasks complete (`.ai/state/CURRENT_TASK.json`). |
| What is blocked? | Nothing. `python3 tools/ai-task list --status blocked` |
| What comes next? | Only on new evidence or a failing acceptance test (architecture frozen at 1.0.0). Candidates: re-measure PGO, allocators and thread-per-core Tokio on dedicated Linux hardware; run on a live Kubernetes cluster; first GitHub CI run. |
| Major decisions? | `.ai/knowledge/DECISIONS/` (ADRs 0003–0011, each with evidence, alternatives and reversal conditions); one-page view in `docs/benchmarks/SUMMARY.md`. |
| What proves current behaviour? | `./dev check` (29 confirmed commands), `./dev test --system`, `--journey` and `--drill` against a running stack, `scripts/validate-generated` for generated projects, and `benchmarks/results/*.json`. |
| Known problems? | `.ai/knowledge/KNOWN_ISSUES.md` |
