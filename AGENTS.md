# Repository Guidelines

## Project Structure & Module Organization

- `Cargo.toml`: Rust workspace (default member: `crates/app`).
- `crates/app`: binary entrypoint; wires config + storage + UI loop.
- `crates/core`: domain types (`Book`, `Settings`, `PreviewMode`, `ScanScope`).
- `crates/application`: orchestration/state (`AppContext`).
- `crates/storage`: SQLite persistence (`Storage`, schema/migrations).
- `crates/engine`: PDF parsing/text extraction and preview rendering.
- `crates/ui`: terminal UI (ratatui/crossterm) and panels.
- `crates/test`: shared fixtures/helpers for unit tests.
- `plan/`: design notes/milestones (not runtime code).
- `tmp/`: local debug output (git-ignored).

## Prerequisites

- Developers: install Pdfium first (`bash scripts/pdfium/fetch_and_probe.sh`) — `crates/engine` requires a local Pdfium shared library at build time.
- Users: install Kitty first for Reader image mode (use `bash scripts/kitty/install.sh` and ensure `~/.local/kitty.app/bin` is on `PATH`).

## Build, Test, and Development Commands

- `cargo run`: run the app (workspace default).
- `cargo build`: compile all crates.
- `cargo test --workspace`: run unit tests across the workspace.
- `cargo fmt`: format code (rustfmt defaults).
- `cargo clippy --workspace --all-targets`: lint (prefer fixing warnings before PRs).

## Coding Style & Naming Conventions

- Use a recent stable Rust toolchain that supports edition 2024.
- Follow standard Rust naming: `snake_case` modules/files, `PascalCase` types, `SCREAMING_SNAKE_CASE` consts.
- Keep changes `cargo fmt` clean; avoid reformat-only diffs mixed with behavior changes.
- Prefer adding error context at boundaries (e.g., `anyhow::Context` in `crates/app`/UI/storage paths).

## Testing Guidelines

- Tests live next to code in `#[cfg(test)] mod tests` within each crate’s `src/`.
- Use helpers from `crates/test` when you need shared fixtures.
- Add focused unit tests for new logic; run `cargo test --workspace` before opening a PR.

## Commit & Pull Request Guidelines

- Commit subjects follow existing history: emoji + imperative summary (e.g., `✨ Add …`, `✨ Implement …`).
- PRs should include: what changed, how to verify (commands/steps), and screenshots or short clips for UI changes.

## Security & Configuration Tips

- Local state is stored under the OS config dir via `directories::ProjectDirs` (e.g., `~/.config/bookshelf/bookshelf.db` on Linux).
- Don’t commit local DBs or debug dumps; keep work in `target/`/`tmp*`/`.cargo-home/` untracked as intended by `.gitignore`.
