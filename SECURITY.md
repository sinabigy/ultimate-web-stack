# Security policy

## Supported versions

| version | supported |
|---|---|
| 1.0.x | ✅ security fixes |
| < 1.0 | ❌ |

Generated projects are independent repositories. A fix lands in the blueprint and is announced
in a release, but it does not reach existing generated projects automatically. Each release note
says which files to update.

## Reporting a vulnerability

**Please do not open a public issue.** Report privately through **GitHub private vulnerability
reporting**: the repository's *Security* tab → *Report a vulnerability*, or
<https://github.com/sinabigy/ultimate-web-stack/security/advisories/new>.

Include:
- the affected component and version or commit;
- reproduction steps or a proof of concept;
- the impact you expect;
- whether it affects generated projects, the blueprint tooling, or both.

What to expect (best effort; this is a community project):
- acknowledgement within 5 business days;
- an assessment and a plan within 14 days;
- a coordinated disclosure date, with credit if you want it.

## Scope
**In scope:**
- authentication (BFF/OIDC, sessions, CSRF);
- authorization (RBAC/Cedar, tenant isolation, admin trust levels);
- audit integrity;
- API keys and service tokens;
- default configuration and deployment artefacts (container image, systemd units, Kubernetes
  manifests);
- the generator, if it produces an insecure project.

**Out of scope:**
- the mock OIDC provider and simulated provider: development and test tools, documented as
  never-deploy;
- findings that need a misconfiguration the docs explicitly warn against, such as production
  without `cookie_secure`;
- vulnerabilities in third-party dependencies, unless the blueprint's use of them makes them
  exploitable. Please report those upstream; `cargo audit`, `cargo deny` and `npm audit` run in
  every `./dev check`.

## Security design
The design invariants are in [`.ai/knowledge/CONSTRAINTS.md`](.ai/knowledge/CONSTRAINTS.md); the
threat model is in [`docs/security/threat-model.md`](docs/security/threat-model.md). Highlights:
- no tokens in the browser (HttpOnly `__Host-` session cookie);
- default-deny authorization through one engine path;
- non-members get 404;
- an append-only audit trail;
- system administration as a separate, MFA-gated trust level.

How these are tested: [`docs/FINAL_ACCEPTANCE.md`](docs/FINAL_ACCEPTANCE.md).
