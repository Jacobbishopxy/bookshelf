# 0054 - Catalog CRUD only

Goal: keep label assignment lightweight in the Labels overlay and move catalog CRUD to the Catalog overlay.

Constraints:

- Labels overlay is for assigning existing collections/tags (no create).
- Catalog overlay is for create/rename/delete.
- Tab switches between Collections and Tags within Catalog.

## Work

- [x] Remove `n` create from Labels
- [x] Keep `n/r/d` CRUD in Catalog
- [x] Tab switches Collections/Tags in Catalog

## Test plan

No explicit test run recorded in this milestone.
