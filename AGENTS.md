# AGENTS.md

Instructions for AI coding agents working on **ibex**, a Rust + Tauri desktop app that browses and exports data from iPhone backups.

## Read first

Before starting any task, read:

1. `ROADMAP.md`: what milestone we're on and what's done
2. `docs/ARCHITECTURE.md`: layers, module boundaries, IPC contract
3. `docs/DATA_SOURCES.md`: backup layout and database schemas (when touching extractors)
4. `docs/UI_DESIGN.md`: screens and components (when touching the UI)

If the code and the docs disagree, stop and ask which is right. Don't silently pick one.

## Repo map

| Path | What it is |
|---|---|
| `crates/ibex-core/` | All backup logic: manifest, resolver, crypto, extractors, exporters. **No Tauri dependency.** |
| `crates/ibex-cli/` | Thin CLI over ibex-core for debugging (`info`, `ls`, `cat`, `sms`, `whatsapp`, ...) |
| `src-tauri/` | Tauri app: `#[tauri::command]` wrappers, media URI protocol, background jobs, app state |
| `src/` | React + TypeScript UI |
| `src/bindings/` | TypeScript types generated from Rust DTOs by `ts-rs`. **Never edit by hand.** |
| `docs/` | Design docs and prompts |

## Commands

```bash
# Rust
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p ibex-cli -- <subcommand> <backup-path>

# Frontend
npm run build              # type-check + build
npm run lint               # if configured

# App
npm run tauri dev
```

A task is **not done** until `cargo fmt`, `cargo clippy -D warnings`, `cargo test --workspace` and `npm run build` all pass.

## Hard rules

1. **The backup is read-only.** Never open a file in the backup folder for writing. Open SQLite databases only through the working-copy mechanism (copy `db`, `db-wal`, `db-shm` into the cache, then open read-only). See ARCHITECTURE.md.
2. **No real personal data in the repo.** No backup files, no exported content, no message text in tests, fixtures, logs, or commit messages. Tests use the synthetic fixture builder.
3. **Never log message content, contact names, phone numbers or file contents.** Log counts, IDs, paths within the backup, and timings only.
4. **No network access.** Don't add crates or npm packages that phone home. No telemetry, no CDNs. Fonts and icons are bundled.
5. **ibex-core stays UI-agnostic.** It must not depend on `tauri`. Everything must be usable from `ibex-cli`.
6. **Schemas vary across iOS and app versions.** Before selecting a column, check it exists (`PRAGMA table_info`). Missing optional columns degrade gracefully; they don't crash.
7. **Dates go through one helper.** Apple timestamps use the 2001-01-01 epoch, sometimes in seconds, sometimes in nanoseconds. Use `ibex_core::util::time` only. Never inline the conversion.
8. **Generated files are generated.** Change the Rust DTO and regenerate; don't edit `src/bindings/`.

## Coding conventions

### Rust
- Edition 2024. `thiserror` for error types in ibex-core, `anyhow` only in ibex-cli.
- No `unwrap()` / `expect()` outside tests, except on invariants with a comment explaining why they hold.
- `rusqlite` with the `bundled` feature. All SQL lives in the extractor that owns that database.
- Blocking work (SQLite, file I/O, image decoding) runs in `tauri::async_runtime::spawn_blocking` from commands.
- DTOs crossing IPC: `#[derive(Serialize, Deserialize, TS)]`, `#[serde(rename_all = "camelCase")]`, `#[ts(export)]`.
- Paginate anything that can be large (messages, files, photos). Never return an unbounded list over IPC.
- Before using a crate API, check its current docs on docs.rs for the version in `Cargo.lock`. APIs change between versions.

### Frontend
- React + TypeScript strict mode. Function components and hooks.
- Data fetching via TanStack Query wrapping `invoke()`. One wrapper per command in `src/api/`.
- Long lists use `@tanstack/react-virtual`.
- Media (images, audio, video) loads through the `ibex://` URI protocol, never by passing bytes through `invoke`.
- Styling: plain CSS with CSS custom properties for theme tokens (see UI_DESIGN.md). Light and dark themes.

### Dependencies
Adding a dependency is allowed, but say so in your summary with a one-line reason. Prefer well-maintained, widely used crates and packages.

## Workflow

1. **Plan before coding** for anything bigger than a small fix. List the files you'll touch and the approach, then implement.
2. **Work in small steps.** One roadmap task at a time, committed separately with a conventional commit message (`feat(sms): ...`, `fix(core): ...`).
3. **Test as you go.** Every extractor gets unit tests against the synthetic fixture. Integration tests against a real backup are gated by the `IBEX_TEST_BACKUP` environment variable and are skipped when it's unset; they assert counts and structure, never content.
4. **Verify in the CLI first, then the UI.** A new extractor must work through `ibex-cli` before it's wired into Tauri.
5. **Update docs.** Tick the boxes in `ROADMAP.md`. If you learned something about a schema, add it to `docs/DATA_SOURCES.md`. If you changed an IPC command, update the contract in `docs/ARCHITECTURE.md`.
6. **Finish with a summary:** what changed, what you verified and how, anything left undone or uncertain.

## When working against a real backup

Inspect **structure, not content**: `.schema`, `PRAGMA table_info`, `SELECT COUNT(*)`, `SELECT typeof(col)`, distinct values of enum-like columns. Don't print message text, names or numbers into the conversation.
