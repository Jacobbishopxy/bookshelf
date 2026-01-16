# 0053 - Separate Labels and Catalog shortcuts

Goal: split the overlay into explicit Filters/Labels/Catalog modes with direct shortcuts, and remove tab UI/hints that are no longer needed.

Constraints:

- Overlay modes are: Filters, Labels (assign), Catalog (manage).
- `/` opens Filters.
- `l` opens Labels (assign).
- `c` opens Catalog (manage).
- Remove tab UI and related hints where the new modes make them redundant.

## Work

- [x] Split overlay modes: Filters/Labels/Catalog
- [x] Bind `/` to Filters
- [x] Bind `l` to Labels (assign)
- [x] Bind `c` to Catalog (manage)
- [x] Remove tab UI and hints

## Test plan

No explicit test run recorded in this milestone.
