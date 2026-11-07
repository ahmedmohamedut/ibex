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
- [~] Integration test gated by `IBEX_TEST_BACKUP` — written (`crates/ibex-core/tests/real_backup.rs`), but never yet run against a real backup: `IBEX_TEST_BACKUP` was unset on the development machine, so it skipped. **Needs one run against a real backup to count as verified.**

**Done when:** `ibex-cli info $IBEX_TEST_BACKUP` and `ibex-cli ls HomeDomain Library/SMS` work on a real backup.

Notes:
- Verified end to end against a synthetic fixture only (`ibex-cli fixture --out DIR`, behind the `fixture` feature). 81 tests pass; `scripts/check.sh` is green.
- Three decisions that differ from the docs, recorded in `docs/ARCHITECTURE.md`:
  `domains()` is counts-only with a separate `domains_with_sizes()`; `workcopy` takes
  paths rather than a `Backup`; `Backup::open_with_cache` exists so tests and the app can
  redirect the cache.
- Unverified assumption: `MBFile`'s `LastModified`/`Birth` are read as **Unix** seconds,
  not Apple-epoch seconds. Wrong would mean mtimes off by 31 years. Confirm on a real backup.

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

## M2: SMS / iMessage
- [ ] Decide: `imessage-database` crate vs own parser (record decision + license impact in DATA_SOURCES.md and README)
- [ ] Contacts resolver from AddressBook
- [ ] `sms::list_conversations`, `sms::list_messages` (cursor pagination), attachments resolved to manifest entries, tapbacks as reactions, replies, edited/unsent flags
- [ ] CLI: `sms convs`, `sms msgs <id> --limit N` (counts/IDs by default)
- [ ] Tauri commands + Messages view (conversation list, thread, inline attachments, lightbox, day separators, jump to date)
- [ ] Fixture + unit tests; real-backup structure test

**Done when:** you can read any SMS/iMessage conversation with its pictures and attachments in the app.

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
- [ ] Keybag TLV parsing, passcode key derivation, class key unwrap (RFC 3394)
- [ ] Manifest.db decryption to cache; per-file decryption on demand
- [ ] `zeroize` keys on close; never persist the password
- [ ] Password modal + bad-password state; cache-sensitivity notice; clear cache
- [ ] Tests: synthetic encrypted fixture (builder encrypts with known password)

**Done when:** an encrypted backup opens with its password and every previous feature works on it.

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
