# Architecture

_One line per major component: name, responsibility and source path. Machine-readable state (profile, enabled modules) is `project.json → architecture`; reasoning is in `DECISIONS/`. Topic detail lives in `.ai/knowledge/<topic>/`._

| Component | Responsibility | Path |
|---|---|---|
| (planned) backend workspace | Rust modular monolith: crates per concern, binaries in `apps/` | `backend/` |
| (planned) frontend | SolidJS + TS + Vite SPA/dashboard | `frontend/` |
| (planned) dev CLI | one-command up/down/check/test/benchmark; delegates validation to `tools/ai-validate` | `dev` |
| (planned) benchmarks | own benchmark suite, results JSON, reports, regression gates | `benchmarks/` |
| (planned) infra | compose profiles, Dockerfiles, systemd, k8s | `infra/` |
| (planned) generator | `scripts/create-project` | `scripts/` |
