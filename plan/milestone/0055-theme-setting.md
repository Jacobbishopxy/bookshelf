# 0055 - Theme setting

Goal: Allow users to choose a UI theme and persist it.

Constraints:

- Reuse existing settings and persistence patterns.
- Keep theme selection in the settings panel UI.

## Work

- [x] Add theme to core settings and persistence (`crates/core`, `crates/storage`)
- [x] Expose theme selection in settings panel and cycle handling (`crates/ui`)
- [x] Apply theme accent color to UI highlights (`crates/ui`)

## Test plan

- [x] No explicit test run recorded in this milestone.
