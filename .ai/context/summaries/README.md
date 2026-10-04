# Summaries

`tools/ai-task complete` writes one result/handoff packet per finished task here (`T-NNNN.result.json`, schema: `.ai/schemas/result.schema.json`).

A packet is the compacted form of a task. It keeps:
- what changed and why
- decisions made
- validation evidence
- unresolved issues and next steps
- verified lessons

It drops investigation chatter, failed hypotheses that teach nothing, and raw logs.

Packets are history. They are **not** loaded by default (see `.ai/config/context.json`). Promote durable lessons to `.ai/knowledge/LESSONS.md` or an ADR.
