# Roadmap

Milestones are ordered; each ends with something you can run and use. Tick boxes as tasks land. Each milestone has a matching prompt in `docs/PROMPTS.md`.

Legend: `[ ]` todo · `[x]` done · `[~]` partial (add a note)

## M0: Foundations
Goal: open an unencrypted backup and browse its raw file tree from the CLI.

- [x] Workspace compiles: `ibex-core`, `ibex-cli`, `src-tauri`; CI-equivalent check script `scripts/check.sh` (fmt, clippy, test, npm build)
- [x] Error types (`thiserror`) and `util::time` (Apple epoch, s/ns heuristic) with tests
- [x] Parse `Info.plist`, `Manifest.plist`, `Status.plist` into `BackupInfo`
- [x] `util::nskeyedarchiver`: resolve the `file` blob into `FileMeta`
- [x] `Manifest`: open read-only, `find`, `list_dir`, `domains`, `fileID` computation
- [x] `Backup::open` for unencrypted backups; `open_file`, `materialize`
- [x] `workcopy`: db + wal + shm copy into cache, open read-only
- [x] Cache dir management (`~/.local/share/ibex/cache/<backup-id>/`)
- [x] `testutil` synthetic backup builder
- [x] CLI: `info`, `domains`, `ls <domain> [path]`, `cat <domain> <path> > out`, `sql <domain> <path> "<query>"` (schema/count only by default)
- [x] Integration test gated by `IBEX_TEST_BACKUP` — **run against a real backup on 2026-11-08 and passing** (10 tests). Needs `IBEX_TEST_PASSWORD` too for an encrypted backup.

**Done when:** `ibex-cli info $IBEX_TEST_BACKUP` and `ibex-cli ls HomeDomain Library/SMS` work on a real backup.

Notes:
- Verified end to end against a synthetic fixture only (`ibex-cli fixture --out DIR`, behind the `fixture` feature). 81 tests pass; `scripts/check.sh` is green.
- Three decisions that differ from the docs, recorded in `docs/ARCHITECTURE.md`:
  `domains()` is counts-only with a separate `domains_with_sizes()`; `workcopy` takes
  paths rather than a `Backup`; `Backup::open_with_cache` exists so tests and the app can
  redirect the cache.
- **Resolved 2026-11-08:** `MBFile`'s `LastModified`/`Birth` really are **Unix** seconds, as
  implemented. Real dates run 2023-04-11 to 2026-11-07 18:57:38, the newest matching the backup's
  own timestamp; an Apple-epoch reading would have put them past 2050. Also confirmed on real
  data: `fileID` = SHA-1 of `"<domain>-<relativePath>"` (437 rows recomputed).

## M1: App shell and Raw browser
Goal: open a backup in the GUI and browse every file.

- [x] Tauri `AppState`, `open_backup`, `close_backup`, `recent_backups`, `list_domains`, `list_dir`, `file_info`
- [x] `ts-rs` bindings generation wired up; `src/api/` wrappers + TanStack Query
- [x] `ibex://` protocol: `file/` with Range support and sniffed Content-Type
- [x] Layout: top bar, sidebar, list/detail panes; tokens.css; light/dark
- [x] Welcome screen: open folder dialog, recents, validation errors
- [x] Overview screen (device info, domain count, total size)
- [~] Raw view: domain list, path tree, file table, previews (image, text, plist pretty-print, hex, audio/video, PDF) — all previews implemented; **HEIC shows an explanatory placeholder** rather than an image, because WebKitGTK cannot decode it. Conversion is M5's thumbnail work.
- [x] "Save as…" for a single file (keeps original name and mtime)

**Done when:** you can open your backup, browse to any file, preview it and save it out.

Notes:
- Added to the IPC contract (recorded in ARCHITECTURE.md): `backup_stats`, `save_file_as`,
  `forget_recent`, `total_cache_size`. No documented signature changed. Overview's "total size"
  comes from a disk walk rather than summing per-domain sizes, which would mean decoding every
  metadata blob; per-domain sizes wait for M4's job system.
- `Manifest` now holds its SQLite connection in a `Mutex`: `rusqlite::Connection` is `Send` but not
  `Sync`, and the app shares one open backup across the command workers and the protocol thread.
- Verified against synthetic fixtures only — still no real backup on this machine. 160 tests pass,
  including 11 that drive the real IPC boundary through Tauri's mock runtime and a protocol
  status/header matrix (200/206/416/404/405).
- Not verified by me: the GUI's appearance and interaction. The React tree is proven to render
  (server-side render smoke test) and the app launches cleanly, but a human needs to click through.
- Checked against the real backup on 2026-11-07: opening it reports "this backup is encrypted;
  a password is required", which is the correct M1 behaviour, and the Welcome screen has a
  plain-words state for it. No cache directory and no recents entry were created by the failed
  open. All 72,037 entries in the 45 GB folder were byte-, mtime- and mode-identical afterwards
  (hard rule 1 verified at real scale). Everything past the password prompt needs M7.

## M2: SMS / iMessage
- [x] Decide: `imessage-database` crate vs own parser (record decision + license impact in DATA_SOURCES.md and README)
- [x] Contacts resolver from AddressBook
- [x] `sms::list_conversations`, `sms::list_messages` (cursor pagination), attachments resolved to manifest entries, tapbacks as reactions, replies, edited/unsent flags
- [x] CLI: `sms convs`, `sms msgs <id> --limit N` (counts/IDs by default)
- [x] Tauri commands + Messages view (conversation list, thread, inline attachments, lightbox, day separators)
- [~] Jump to date — not built. Replies jump to their parent, and threads open at the newest message, but there is no date picker; it is most useful together with M8's search, so it waits for that.
- [x] Fixture + unit tests; real-backup structure test

**Done when:** you can read any SMS/iMessage conversation with its pictures and attachments in the app.

Notes:
- **ibex is now GPL-3.0-or-later**, because `imessage-database` is. `LICENSE` added, `license`
  fields set on all three crates. **README.md is mode 444, so its "Licence: to be decided"
  section is still wrong** — the replacement text is in the M2 summary and needs pasting by hand.
- Verified end to end against the synthetic fixture: a message whose text exists *only* as an
  `attributedBody` typedstream blob round-trips to "Text that lives only in attributedBody", which
  is the whole reason for taking the crate on.
- The conversation list's message count excludes tapbacks, so it agrees with what the thread
  actually lists. A test asserts the two match for every conversation.
- **Verified against real data 2026-11-08** (iOS 27.2, 17,588 messages, 859 conversations): every
  column M2 needs is present; per-conversation counts reconcile exactly with the database
  (17,449 = 17,449); and 1,619 of 1,629 messages in the chats richest in `text`-NULL rows come
  back with text, so attributedBody decoding works on real data.
- **Bug found by real data and fixed:** `associated_message_type` 2 and 3 are ordinary messages,
  not tapbacks. ibex had excluded every non-zero code, dropping 16 real messages from their
  threads. Only 1000 and 2000–2007 / 3000–3007 are tapbacks. See docs/DATA_SOURCES.md.
- Group chats remain fixture-only: the real backup has 859 one-to-one chats and no groups.

## M3: WhatsApp
- [ ] Detect WhatsApp and WhatsApp Business containers
- [ ] Chats, group members, names (ContactsV2 + address book fallback)
- [ ] Messages with type mapping, media resolution (path candidates), replies, deleted markers, locations, vCards
- [ ] Voice notes: verify `.opus` playback in WebKitGTK; transcode to cached playable format if needed
- [ ] CLI `wa chats`, `wa msgs`
- [ ] WhatsApp view (reuse chat components, WhatsApp bubble colors)

**Done when:** WhatsApp chats, including groups, media and voice notes, read and play in the app.

## M4: Export engine
- [ ] Job system in src-tauri: start, progress events, cancel, finished report
- [ ] `export::html` (per conversation + top-level index, copied media, offline, no JS)
- [ ] `export::csv`, `export::json`
- [ ] `export::files` (original names, mtime, de-dup)
- [ ] Export dialog + job toasts + Exports history view
- [ ] `export-report.json`
- [ ] CLI `export sms|wa <conv-id|all> --format html --out DIR`

**Done when:** you can export one conversation, or all of them, to HTML with media and open it in a browser.

## M5: Photos
- [ ] `Photos.sqlite` parsing (ZASSET / ZGENERICASSET), albums join-table discovery, cloud-only detection
- [ ] DCIM fallback with EXIF dates
- [ ] Thumbnail service (image + HEIC via libheif-rs), bounded worker pool, `ibex://thumb/`
- [ ] Photos grid (virtualized, month headers), lightbox with metadata, multi-select, export originals (optional HEIC→JPEG)
- [ ] Live Photo pairing

**Done when:** you can scroll the whole camera roll smoothly and export a selection.

## M6: Voice Memos and Documents
- [ ] Voice Memos: probe locations, titles from CloudRecordings.db, list/play/export with titled file names
- [ ] Documents: Files app storage + per-app Documents roots with app names; Files view with previews
- [ ] Batch export for both

**Done when:** recordings play with their titles, and documents are browsable by app and exportable.

## M7: Encrypted backups
> **Done out of order, ahead of M2.** The only real backup available for development is
> encrypted (iOS 27.2), so until this landed nothing could be verified against real data.

- [x] Keybag TLV parsing, passcode key derivation, class key unwrap (RFC 3394)
- [x] `Manifest.db` decryption to cache; per-file decryption on demand
- [x] `zeroize` keys on close; never persist the password
- [x] Password modal + bad-password state; cache-sensitivity notice; clear cache
- [x] Tests: synthetic encrypted fixture (builder encrypts with known password)

**Done when:** an encrypted backup opens with its password and every previous feature works on it.

Notes:
- The keybag parser was written against the real backup's verified structure, and the
  known-answer tests (RFC 3394 §4.6, RFC 6070) pin the primitives so a refactor cannot
  silently change them.
- `testutil` gained the encrypt-and-wrap half of the scheme, so encrypted fixtures are produced
  by the same code that reads them. The round trip is checked at awkward lengths (0, 1, 15, 16,
  17 bytes) and across a 40 KB WAV.
- Timing on the real backup's ten million rounds: **0.79s release, 1.04s dev**. Getting there
  needed two profile fixes — all dependencies at `opt-level = 3`, and ibex-core itself at
  `opt-level = 1`, because `pbkdf2_hmac` is generic and monomorphizes into ibex-core rather than
  staying in pbkdf2. Without the second, unlocking took 11s in `tauri dev`.
- Verified against the real backup as far as is possible without its password: the real keybag
  parses, derivation runs at full cost, and a deliberately wrong password is correctly reported
  as `bad_password` rather than crashing or hanging.
- `ibex-cli` takes `--password` and reads it from `IBEX_PASSWORD` or a prompt, never from argv
  where it would reach shell history and `ps`.
- **Still open:** the MBFile epoch question, and M0's `[~]` integration test. Both now need only
  one run with the real password — `IBEX_TEST_PASSWORD` makes `real_backup.rs` assert, including
  a new test that fails if file dates land outside 2007–2030.

## M8: Search and polish
- [ ] FTS5 index job over SMS + WhatsApp; search palette (Ctrl+K) with jump-to-message
- [ ] Keyboard shortcuts, empty states, error toasts with details
- [ ] Performance pass on a large backup (open time, scroll, thumbnail throughput); document numbers
- [ ] Packaging: AppImage and .deb via `tauri build`; app icon

**Done when:** a release build installs on a fresh Linux machine and handles a large backup comfortably.

## Later
- Call history, Notes, Safari history, Calendar
- Contacts browser with vCard export
- Timeline view across sources
- PDF export of conversations
- macOS / Windows builds
