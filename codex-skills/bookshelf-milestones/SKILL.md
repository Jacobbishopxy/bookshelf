---
name: bookshelf-milestones
description: Create, rewrite, or review Bookshelf milestone files under `plan/milestone/` so they follow the repo's standard milestone structure (title, Goal, Constraints, Work checkboxes, Test plan). Use when asked to add a new milestone, renumber milestones, or standardize existing milestone markdown files.
---

Follow the repo standard for milestones in `plan/milestone/README.md`.

## Create a new milestone

1) Pick the next available milestone number `NNNN` and filename `plan/milestone/NNNN-short-slug.md`.
2) Use this structure (keep it tight and concrete):

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

3) During implementation, keep tasks `[ ]` until done; flip to `[x]` only when completed.
4) When finished, ensure all tasks are `[x]` and the test plan reflects what was actually run.

## Rewrite an existing milestone to the standard structure

1) Preserve the original intent and checklist items; do not silently add/remove scope.
2) Convert loose bullets into:
   - `Goal:` (1 sentence)
   - `Constraints:` (only true constraints, not work items)
   - `## Work` (checkbox list)
   - `## Test plan` (checkbox list, or `No explicit test run recorded in this milestone.` if unknown)
3) Keep any file/module references inline (e.g. `crates/ui`) to make the checklist actionable.

