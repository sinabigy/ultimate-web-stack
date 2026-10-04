# Lessons

_Verified, durable lessons only. Each lesson is a statement, its evidence and its scope. Do not record hypotheses or narratives here._

Format:

```
LESSON: <one-sentence rule>
EVIDENCE: <test, commit, or measurement that proved it>
APPLIES TO: <components>
```

LESSON: Keep property tests in canonical validation and give critical generators enough cases; randomized suites find bugs on some runs and not others.
EVIDENCE: `Slug::suggest` truncation-after-trim bug passed earlier `cargo test` runs and was caught later by ai-validate (proptest), commit "Fix slug truncation"; regression test added and case count raised to 2000.
APPLIES TO: backend/crates/domain, backend/crates/authz (differential test)

LESSON: Run `python3 tools/ai-check` (and clippy) as a gate, not a habit: two commits landed with failures before the pre-commit hook existed.
EVIDENCE: roadmap id schema failure (fixed in "Renumber roadmap milestones"); clippy failure in db tests (fixed with .githooks/pre-commit).
APPLIES TO: whole repository; `git config core.hooksPath .githooks` (./dev setup)

LESSON: Personal workspaces must not take clean slugs derived from the user's name.
EVIDENCE: tenancy test `last_owner_cannot_leave_or_be_demoted` collided with personal slug "solo"; personal slugs now always carry a suffix.
APPLIES TO: backend/crates/db/src/orgs.rs ensure_personal_org
