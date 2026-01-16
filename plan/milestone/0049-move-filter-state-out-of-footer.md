# 0049 - Move filter state out of footer

Goal: move active filter state out of the footer and into the overlay header so the footer can stay focused on key hints.

Constraints:

- Filter state is always visible in the header when the overlay is open.
- Footer stays concise (keys/hints, not state).

## Work

- [x] Move filter state out of footer
- [x] Show active filter state in header

## Test plan

- [x] Keep `cargo fmt` clean
