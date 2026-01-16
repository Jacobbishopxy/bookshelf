# Milestones

Each set of code changes should be recorded as a new file in this folder.

Rules:

- One file per change-set (small/medium PR-sized chunk).
- Use checkboxes: `[ ]` for not done, `[x]` for done.
- Keep entries concrete (files touched, behavior added, tests run).
- When a change-set is complete, all items in its file should be `[x]`.
- Every milestone must follow the standard structure:
  - Title header: `# NNNN - <short name>`
  - `Goal:` paragraph
  - `Constraints:` section (bulleted)
  - `## Work` section (checkbox list)
  - `## Test plan` section (checkbox list; if unknown, write `No explicit test run recorded in this milestone.`)

Template:

```md
# NNNN - Short name

Goal: one sentence describing the user-visible outcome.

Constraints:

- Constraint 1
- Constraint 2

## Work

- [ ] Concrete task 1 (optional: `crates/...`)
- [ ] Concrete task 2

## Test plan

- [ ] `cargo test --workspace --offline`
- [ ] `cargo fmt --check`
```
