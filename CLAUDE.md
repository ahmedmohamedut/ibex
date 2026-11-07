# CLAUDE.md

@AGENTS.md

## Claude Code specifics

- Start each roadmap milestone in **plan mode**. Present the plan, wait for approval, then implement.
- Use the task list to track roadmap items for the current milestone.
- Before finishing any task, run the full check sequence from AGENTS.md (fmt, clippy, test, frontend build) and fix what fails. Don't report success on a red build.
- For exploration across many files, prefer a read-only search over reading everything into context.
- When a crate API is uncertain, check docs.rs for the exact version in `Cargo.lock` rather than guessing from memory.
- Ask before: adding a new crate to ibex-core, changing an IPC command signature, changing the cache layout, or anything that touches the backup folder path handling.
