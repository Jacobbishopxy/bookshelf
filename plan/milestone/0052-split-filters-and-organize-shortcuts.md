# 0052 - Split Filters and Organize shortcuts

Goal: split the overlay state into explicit modes and add direct shortcuts for opening Filters vs Organize.

Constraints:

- Overlay state tracks the active mode.
- `/` opens Filters overlay directly.
- `l` opens Organize overlay directly.
- Filters is not shown as a tab inside Organize.

## Work

- [x] Add mode to overlay state
- [x] Bind `/` to Filters overlay
- [x] Bind `l` to Organize overlay
- [x] Hide Filters from Organize tabs
- [x] Update footer/help hints

## Test plan

No explicit test run recorded in this milestone.
