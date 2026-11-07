# Prompts

Paste these into your coding agent (Claude Code, Codex, Cursor…) in order. Start a **fresh session per milestone** so context stays clean; the docs carry the state between sessions.

Tips:
- If the agent drifts, point it back: "Re-read AGENTS.md hard rules and ROADMAP.md M_N, then continue."
- Review each plan before approving. The most expensive mistakes are wrong module boundaries and IPC shapes.
- After each milestone, run the app yourself against your real backup before moving on.

---

## Kickoff prompt (run once, after SETUP.md)

```
You're starting work on ibex, an iPhone backup explorer (Rust + Tauri 2 + React/TS).

1. Read AGENTS.md, ROADMAP.md, docs/ARCHITECTURE.md, docs/DATA_SOURCES.md and docs/UI_DESIGN.md.
2. Inspect the scaffolded repo (workspace Cargo.toml, crates/, src-tauri/, src/, package.json).
3. Report back, without writing code yet:
   - anything in the docs that is inconsistent, ambiguous or that you think is wrong
   - anything in the scaffold that doesn't match the docs (versions, edition, layout)
   - the crates and npm packages you plan to use for M0 and M1, with versions you verified on docs.rs / npm
   - a proposed `scripts/check.sh` that runs fmt, clippy -D warnings, workspace tests and npm build
4. Wait for my answers before starting M0.
```

---

## M0: Foundations

```
Work on ROADMAP.md milestone M0 (Foundations).

Start in plan mode. Plan the ibex-core module layout (backup/, workcopy/, util/, testutil/, error.rs), the public API of Backup and Manifest as described in docs/ARCHITECTURE.md, and the CLI subcommands. Include the test plan: which unit tests use the synthetic fixture and what the IBEX_TEST_BACKUP integration test asserts (structure and counts only).

After I approve, implement task by task in roadmap order, committing after each with a conventional commit message and ticking ROADMAP.md.

Specific requirements:
- util::time::from_apple must handle seconds (float/int) and nanoseconds; unit test both, plus 0/NULL.
- The NSKeyedArchiver decoder must resolve UID references generically, not by hard-coded indices.
- workcopy must copy -wal and -shm when present and work when they're absent.
- The CLI `sql` subcommand prints schema and counts by default; it must refuse non-SELECT/PRAGMA statements.

When done, run scripts/check.sh, then run `ibex-cli info` and `ibex-cli domains` against $IBEX_TEST_BACKUP and show me only the structural output (device model, iOS version, domain names and counts). Summarize what was built and anything uncertain.
```

---

## M1: App shell and Raw browser

```
Work on ROADMAP.md milestone M1 (App shell and Raw browser).

Start in plan mode. The plan must cover:
- AppState and the M1 commands from the IPC table in docs/ARCHITECTURE.md (exact Rust signatures and DTOs)
- ts-rs setup: where bindings are written (src/bindings/) and how they're regenerated
- the ibex:// protocol handler with HTTP Range support (206 responses, Accept-Ranges, Content-Range) and MIME sniffing
- frontend structure: src/api, src/views, src/components, src/styles/tokens.css, the Zustand store
- which npm packages you'll add (TanStack Query, TanStack Virtual, Zustand, lucide-react, anything else) with one-line reasons

After approval, implement in roadmap order. Follow docs/UI_DESIGN.md for layout, the Welcome screen, Overview and the Raw view. Previews: image, text, plist (pretty-printed; binary plists converted), hex, audio/video, PDF.

Verify: scripts/check.sh green; `npm run tauri dev` opens; you can open $IBEX_TEST_BACKUP, browse HomeDomain, preview a plist, play an audio file with seeking, and "Save as…" a file keeping its mtime. Tell me what I should click through to verify it myself.
```

---

## M2: SMS / iMessage

```
Work on ROADMAP.md milestone M2 (SMS / iMessage).

First, before planning: evaluate the `imessage-database` crate (current version on crates.io/docs.rs) for reading sms.db from an iOS backup copy and decoding attributedBody. Report: what it gives us, what we'd still write, how it fits our working-copy approach, and the GPL-3.0 implication. Recommend use or don't use and wait for my decision. Record the decision in docs/DATA_SOURCES.md and README.md.

Then plan (plan mode):
- contacts resolver (AddressBook.sqlitedb, phone normalization)
- sms extractor API and the shared Message/ConversationSummary types in ARCHITECTURE.md
- cursor pagination design (by message date + ROWID), newest-first loading for the thread view
- tapbacks as reactions, inline replies, edited/unsent flags, attachment resolution to MediaDomain
- fixture additions and tests
- Messages view components (ChatThread, MessageBubble, AttachmentView, lightbox), reverse infinite scroll with stable scroll position, day separators, jump to date

Verify schema assumptions against $IBEX_TEST_BACKUP with structure-only queries (PRAGMA table_info, counts, typeof, distinct enum values). Never print message text, names or numbers.

Implement after approval; CLI first, then UI. Done when scripts/check.sh is green and the CLI shows conversation/message counts for my backup.
```

---

## M3: WhatsApp

```
Work on ROADMAP.md milestone M3 (WhatsApp).

First inspect $IBEX_TEST_BACKUP (structure only): which WhatsApp container(s) exist, the tables and columns in ChatStorage.sqlite, distinct ZMESSAGETYPE values with counts, ZSESSIONTYPE values, and a sample of ZMEDIALOCALPATH *shapes* (replace any name/number segments with placeholders before showing me). Update docs/DATA_SOURCES.md with what differs from the doc.

Then plan (plan mode): extractor API reusing the shared message types, type mapping, media path resolution with candidates, group member names, contacts fallback order, and the voice-note playback plan. Check whether WebKitGTK plays .opus in an <audio> element via our ibex:// protocol; if not, propose a transcode-and-cache approach (prefer pure Rust; an optional system ffmpeg is acceptable as fallback) and wait for my choice.

Implement after approval, CLI first, reuse the chat UI components with the WhatsApp bubble tokens. Done when scripts/check.sh is green and WhatsApp chats, media and voice notes work in the app.
```

---

## M4: Export engine

```
Work on ROADMAP.md milestone M4 (Export engine).

Plan (plan mode):
- job system in src-tauri (registry, ids, progress throttling ~10/s, cancellation token passed into ibex-core, finished/failed events)
- Exporter trait and the four exporters described in docs/ARCHITECTURE.md
- HTML export: per-conversation folder, top-level index, media copied into media/ with relative links, CSS inlined from the same tokens as the app, readable without JavaScript, safe HTML escaping of all message content
- file naming and sanitization rules (Linux-safe and Windows-safe names, de-duplication), mtime preservation
- export-report.json schema
- Export dialog, job toasts, Exports history view per docs/UI_DESIGN.md

Tests: fixture-based golden tests for CSV/JSON structure; HTML test asserts structure (files exist, links resolve, escaping works with a malicious fixture message like "<script>").

Implement after approval. Verify by exporting one SMS and one WhatsApp conversation from $IBEX_TEST_BACKUP into /tmp and tell me the report counts (no content).
```

---

## M5: Photos

```
Work on ROADMAP.md milestone M5 (Photos).

Inspect $IBEX_TEST_BACKUP structure first: Photos.sqlite table names (ZASSET vs ZGENERICASSET), the albums join table name, count of assets vs count of DCIM files actually in the backup (cloud-only estimate), file extension distribution. Update docs/DATA_SOURCES.md.

Plan (plan mode): photos extractor with DCIM fallback, cloud-only detection, Live Photo pairing, thumbnail service (libheif-rs for HEIC, image crate otherwise, EXIF orientation applied, bounded pool, disk cache), ibex://thumb/ route, virtualized month-grouped grid, lightbox with metadata, multi-select export with optional HEIC→JPEG conversion.

Implement after approval. Report: time to first grid paint and thumbnails/second on my backup (cold and warm cache).
```

---

## M6: Voice Memos and Documents

```
Work on ROADMAP.md milestone M6 (Voice Memos and Documents).

Probe $IBEX_TEST_BACKUP for every Voice Memos location listed in docs/DATA_SOURCES.md and report which exist, plus the CloudRecordings.db columns. Probe the Files app container and count AppDomain-* domains that have non-empty Documents/ folders. Update the doc.

Plan, then implement after approval: voice extractor (titles, dates, durations, titled export names), documents sources with app display names from Info.plist, the Voice Memos and Files views, batch export via the M4 job system.
```

---

## M7: Encrypted backups

```
Work on ROADMAP.md milestone M7 (Encrypted backups).

Plan (plan mode) the crypto module following the scheme in docs/DATA_SOURCES.md: keybag TLV parser, key derivation, RFC 3394 unwrap, Manifest.db and per-file decryption, where decrypted data goes in the cache, zeroization, and how Backup::open changes. List the RustCrypto crates and versions.

Testing is critical: extend testutil so the fixture builder can produce an *encrypted* synthetic backup with a known password (implement the wrap/encrypt side for tests only). Tests: correct password opens, wrong password returns CryptoError::BadPassword, decrypted file bytes match originals, keys are zeroized on drop.

UI: password modal, bad-password state, cache sensitivity notice, clear cache.

Implement after approval. If I have an encrypted backup I'll test it manually; tell me exactly what to try.
```

---

## M8: Search and polish

```
Work on ROADMAP.md milestone M8 (Search and polish).

Plan, then implement after approval:
- FTS5 index job over SMS + WhatsApp into cache search.db (incremental rebuild if the backup changed), search command with snippets, Ctrl+K palette, jump to message with highlight
- keyboard shortcuts, empty states and error toasts per docs/UI_DESIGN.md
- performance pass: measure open time, conversation list load, thread scroll, thumbnail throughput on $IBEX_TEST_BACKUP; fix the worst offender; write the numbers into ROADMAP.md
- packaging: app icon, `tauri build` producing AppImage and .deb; verify the .deb installs and launches

Finish with a release checklist and anything you'd fix before calling it 1.0.
```

---

## Utility prompts

**Bug report**
```
Bug: <what happened> when <steps>. Expected: <…>.
Reproduce it (CLI first if it's a core issue), write a failing test using the synthetic fixture, fix it, and confirm scripts/check.sh is green. Don't print personal data while investigating.
```

**Schema mismatch on my backup**
```
<Feature> fails on my backup. Investigate with structure-only queries against $IBEX_TEST_BACKUP (PRAGMA table_info, typeof, counts, distinct enum values). Make the extractor tolerant of what you find, add a fixture variant reproducing the schema difference, and log the finding in docs/DATA_SOURCES.md under "Learned facts log".
```

**Docs sync check**
```
Compare the code with docs/ARCHITECTURE.md (IPC table, module layout, cache layout) and ROADMAP.md checkboxes. List every mismatch, then fix the docs or ask me where the code looks wrong.
```

**Review before moving on**
```
Review milestone M_N as a senior Rust/Tauri reviewer: correctness, AGENTS.md hard-rule violations (writes to the backup, content in logs, unbounded IPC lists, unwraps), error handling, test gaps. Report findings ranked by severity; fix only after I confirm.
```
