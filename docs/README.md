# Documentation

| area | documents |
|---|---|
| **Start here** | [`../README.md`](../README.md) (quick start), [`../AI_PROTOCOL.md`](../AI_PROTOCOL.md) (for AI agents), `.ai/knowledge/ARCHITECTURE.md` (component map) |
| **Identity & access** | [authentication](authentication/README.md) · [authorization](authorization/README.md) · [multitenancy](multitenancy/README.md) · [admin](admin/README.md) |
| **Architecture** | [outbound API engine](architecture/outbound-engine.md) · [messaging (PostgreSQL queue, NATS/JetStream)](architecture/messaging.md) · [analytics (ClickHouse)](architecture/analytics.md) |
| **Operations** | [observability](operations/observability.md) · [failure modes](operations/failure-modes.md) · [deployment tiers](deployment/README.md) ([VPS](deployment/vps.md), [containers](deployment/containers.md), [Kubernetes](deployment/kubernetes.md)) |
| **Security** | [threat model](security/threat-model.md) |
| **Evidence** | [benchmark results](benchmarks/latest.md) · [Pingora gateway](benchmarks/pingora.md) · [runtime / io_uring](benchmarks/runtime.md) · [release profile](benchmarks/release-profile.md) · [`../benchmarks/README.md`](../benchmarks/README.md) (methodology, gates, measured noise) |
| **Decisions** | `.ai/knowledge/DECISIONS/` (ADRs with evidence, alternatives, consequences and reversal conditions) |
| **Modules** | [capability matrix](BLUEPRINT_CAPABILITY_MATRIX.md) (profiles and what they contain) · [enabling a module later](MODULES.md) |
| **Evidence summary** | [kept / rejected / inconclusive / optional](benchmarks/SUMMARY.md) · [generated-project acceptance](acceptance/generated-projects.md) |
| **Generating a project** | [`../scripts/create-project`](../scripts/create-project) (`--help`), [`../scripts/validate-generated`](../scripts/validate-generated) (blueprint only) |

Conventions:
- Every performance claim links to a result file in `benchmarks/results/`. Numbers come from runs on
  the machine named in that file.
- Every "we chose X" links to an ADR.
- Behaviour that a test proves names the test.
