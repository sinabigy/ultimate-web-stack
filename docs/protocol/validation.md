# Validation

AI confidence is not validation. Prove changes externally, narrowest check first:

changed unit → its unit tests → related integration tests → broader regression suite → e2e.

Don't run an expensive full suite until a narrow check has passed.

## Validation commands
`project.json → validation.commands[]` entries are `{id, stage, cmd, source, confirmed}`.
- `ai-init` *infers* candidates from manifests (`package.json` scripts, `pyproject.toml`, `Cargo.toml`, `go.mod`, `Makefile`) or conventional defaults. It records where each came from (`source`) and sets `confirmed: false`. It never runs them.
- Inferred commands are evidence, not instructions. Promote one to canonical by reviewing it, running it deliberately and setting `confirmed: true`.
- Optional fields: `cwd` (repo-relative working directory), `requires` (executables that must be on PATH) and `timeout_s`.

## Running validation: `tools/ai-validate`
Runs `ai-check`, then every confirmed command in file order, and prints PASS/FAIL/SKIP per command.
- Unconfirmed commands are listed, never run.
- A command whose `requires` are missing is **SKIP**, never PASS. `--strict` turns skips into failure.
- Stops at the first failure unless `--keep-going`. Only the tail of a failing command's output is shown.
- `--stage unit --stage lint` and `--only ID` select a subset. `--json` / `--report FILE` emit a machine-readable report (with git head and dirty flag) suitable as validation evidence.
- Project wrappers (`make check`, `./dev check`) should call `ai-validate` instead of keeping their own list of commands.

## Failure handling
1. Capture the smallest useful error.
2. Check whether the current change introduced it.
3. Locate the component.
4. Retrieve only the relevant code.
5. Form a specific hypothesis.
6. Fix the cause, not the symptom.
7. Run the narrow check, then the broad one.

## Schema validation levels
`ai-check` reports `validation_level`:
- **full**: the optional `jsonschema` package (Draft 2020-12) is installed and used.
- **baseline**: the built-in validator in `tools/_ailib.py`.

The baseline validator supports exactly these keywords:
- `type`, `required`, `properties`, `additionalProperties`, `propertyNames`
- `enum`, `const`, `pattern`
- `items`, `minItems`, `maxItems`, `uniqueItems`
- `minLength`, `maxLength`, `minimum`, `maximum`
- `not`, `anyOf`
- local `$ref` / `$defs`

Annotations are skipped because they never constrain data: `$schema`, `$id`, `$comment`, `title`, `description`, `default`, `examples`, `deprecated`, `readOnly`, `writeOnly`, and `format` (annotation-only by default in 2020-12).

**Any other keyword is reported as `not checked: <keyword> at <path>`, never silently ignored.** The shipped schemas use only supported keywords, so for them baseline and full give the same verdicts. The schemas remain standard JSON Schema for external validators.

## ai-check exit codes
- `0`: canonical state is valid. Warnings, info and runtime notes may be present.
- `1`: at least one canonical FAIL.

Runtime problems (`.ai/runtime/`) only affect the exit code with `--strict-runtime`.

## Review
Review the change, not the project: objective, acceptance criteria, relevant constraints, the diff, affected interfaces and test results. Look for correctness, regressions, security, edge cases, maintainability, architecture violations, unnecessary complexity, missing tests and scope creep.
