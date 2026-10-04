# Task lifecycle

## Packets
Tasks live at `.ai/tasks/<status>/T-NNNN-slug.json` (schema: `.ai/schemas/task.schema.json`). The fields are:
- `objective`
- `acceptance_criteria` (≥ 1)
- `validation` (≥ 1)
- `non_goals`, `dependencies`, `components`, `likely_files`, `constraints`
- `risk`, `task_class` (→ routing), `required_capabilities`, `context_class`
- `status`, `history`

Required text fields reject placeholders such as `TODO`, `TBD` or `<...>`. A task that can't state real acceptance criteria isn't ready. Clarify it or decompose it first.

## Creating
```
python3 tools/ai-task new "Add CSV export" \
  --objective "Users can export the report table as CSV" \
  --accept "GET /report.csv returns RFC 4180 CSV with a header row" \
  --validate "pytest tests/test_report_export.py" \
  --class feature --risk medium --file src/report.py
python3 tools/ai-task new --from-template project-discovery
```
Reusable task definitions live in `.ai/templates/*.task.json`. `ai-check` verifies that each one instantiates into a valid task.

## State machine
```
queue ──start──▶ active ──complete──▶ completed
  │                │  ▲
  └──block──▶ blocked ┘ (unblock --to queue|active)
       active ──block──▶ blocked
```
- Any other transition is refused unless you pass `--force`. A forced transition is recorded in `history` with `forced: true`.
- A task can only start once its dependencies are completed (or `--force`).
- Only one task can be active at a time, unless `policies.allow_parallel_active` is set.

## CURRENT_TASK
`.ai/state/CURRENT_TASK.json` is a pointer: `task_id`, `path`, `next_step`. `ai-check` fails when it points at a missing task, a task that isn't active, or a wrong path.
- `start` sets the pointer.
- `block` and `complete` clear it.
- `complete --next` activates the next eligible task by a documented rule: ROADMAP order first, then lowest ID, considering only queued tasks whose dependencies are completed. No priority is invented.

## Completing
```
python3 tools/ai-task complete T-0003 --summary "CSV export added" \
  --evidence "pytest tests/test_report_export.py: 4 passed"
```
This writes `.ai/context/summaries/T-0003.result.json`. Enrich it with changes, decisions, unresolved issues and lessons, then update `STATUS.md` and run `ai-check`.
