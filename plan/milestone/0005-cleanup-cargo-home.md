# 0005 - Cleanup temporary Cargo home artifacts

- [x] Remove `.cargo-home/` created during earlier builds
- [x] Remove temporary debug build folders (`tmp_bin/`, `tmp_out*/`)
- [x] Remove obsolete dev note that referenced `.cargo-home` (`plan/dev.md`)
- [x] Update `.gitignore` to prevent re-adding these artifacts
