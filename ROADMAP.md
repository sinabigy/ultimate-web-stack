# Roadmap

The architecture is frozen at 1.0.0, so the roadmap is mostly **turning "not yet proven" into
proven**. Items that would change a default need evidence first (see
[CONTRIBUTING.md](CONTRIBUTING.md)).

## Now: 1.0.x, close the evidence gaps
- [x] **First run on GitHub-hosted CI.** All jobs green, including the live systemd test (private
      release-candidate phase).
- [ ] **Live Kubernetes test.** Deploy the base manifests to a real cluster (kind or k3d in CI,
      then a cloud cluster) and cover probes, HPA, PDB, NetworkPolicy and rolling restart.
      Today they are statically validated only.
- [ ] **Benchmarks on dedicated Linux hardware.** Re-measure the inconclusive candidates (PGO,
      mimalloc/jemalloc, thread-per-core Tokio) with interleaved runs on a quiet host.
- [ ] **Live OIDC beyond self-hosted ZITADEL.** Verify ZITADEL Cloud and one generic provider
      end to end.
- [ ] **Polish:** an empty-state "Job queues" chart, and the organization switcher on not-found
      pages.

## Next: 1.x, additive, no default changes without evidence
- [ ] **v1.1: MCP control layer.** Operate generated projects from any MCP client through typed,
      permission-scoped tools that wrap the existing protocol tools (proposal:
      [docs/v1.1-mcp-proposal.md](docs/v1.1-mcp-proposal.md); awaiting approval).
- [ ] `create-project --update`: a guided way to bring blueprint fixes into existing generated
      projects. Today, release notes list the files to update.
- [ ] More example features as documented walkthroughs (the clean-room features: projects,
      announcements).
- [ ] A recorded demo and an asciinema cast of the golden path (`scripts/demo.sh`).
- [ ] Benchmark results from community hardware, published per machine.
- [ ] Multi-region and read-replica guidance for PostgreSQL, with measurements.

## Later: needs a proposal and evidence
- Alternative frontends or SSR. Rejected for 1.0: no Node.js in production without a
  requirement.
- Additional identity providers as first-class profiles.
- Hosted services around the open-source core: managed deployments, updates and support. The
  open-source edition stays complete.

## Not planned
- Making Kubernetes the default deployment.
- Adopting a technology because it is popular. Every default needs its measurement.

Have a proposal? Open an **Architecture proposal** issue. Want to help with an item above? Look
for the matching `help wanted` issue.
