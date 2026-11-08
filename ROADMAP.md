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
  metadata blob; per-domain sizes are a progress-reporting walk, available in the CLI
  (`ibex-cli info`) and deferred in the UI to M8's polish pass rather than M4 as first
  planned — M4's job system exists, but nothing in the UI asks for the numbers yet.
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
- [x] Detect WhatsApp and WhatsApp Business containers
- [x] Chats, group members, names (ContactsV2 + address book fallback)
- [x] Messages with type mapping, media resolution (path candidates), replies, deleted markers, locations, vCards
- [x] Voice notes: verified `.opus` is Ogg/Opus and WebKitGTK has `oggdemux`+`opusdec`, so **no transcode is needed**
- [x] CLI `wa chats`, `wa msgs`
- [x] WhatsApp view (reuses the M2 chat components, WhatsApp bubble colors)

**Done when:** WhatsApp chats, including groups, media and voice notes, read and play in the app.

Notes:
- Planned against the **real schema** rather than the docs, and the inspection paid for itself by
  catching two classification bugs before they shipped (both in docs/DATA_SOURCES.md):
  `ZMESSAGETYPE` cannot decide the message kind (27,813 "text" messages carry a link-preview
  media item), and `ZGROUPEVENTTYPE` 2 covers 35,467 messages including 27,462 with ordinary
  text — treating it as a group-event flag made 62% of a sampled thread render as system notices.
- Hidden sessions (123, holding 283 messages) are **listed**, not filtered: dropping them would
  lose real conversations. Only `ZSESSIONTYPE` separates conversations (1,107) from status and
  channel feeds (85, behind a toggle).
- Media resolution is one deterministic `Message/` prefix: 97.5% of rows on the real backup,
  100% of the 198 attachments in the sampled threads.
- `util::paths::redact_jid` was added because a WhatsApp path embeds a phone number, so logging
  one breaks hard rule 3 even though the rule permits backup paths.
- Reactions are deliberately not read: undocumented protobuf in `ZWAMESSAGEINFO`.
- Group chats finally get real coverage here — 18 groups, 2,278 members — which M2 could not get
  since all 859 of the backup's SMS chats are one-to-one.
- **Not verified by me:** that a voice note actually plays. The container and codecs check out,
  but confirming it needs ears.

## M4: Export engine
- [x] Job system in src-tauri: start, progress events, cancel, finished report
- [x] `export::html` (per conversation + top-level index, copied media, offline, no JS)
- [x] `export::csv`, `export::json`
- [x] `export::files` (original names, mtime, de-dup)
- [x] Export dialog + job toasts + Exports history view
- [x] `export-report.json`
- [x] CLI `export sms|wa <conv-id|all> --format html --out DIR`

**Done when:** you can export one conversation, or all of them, to HTML with media and open it in a browser.

Notes:
- One `ChatSource` enum over `SmsDb` and `WhatsAppDb` means the four formats are written once and
  serve SMS, iMessage and WhatsApp alike. `messages_chronological` reuses the keyset cursor, so a
  66,407-message chat streams a page at a time rather than being held whole.
- `export/naming.rs` is the deliberately paranoid part: reserved device names, control characters,
  trailing dots, 200-byte caps on UTF-8 boundaries, case-insensitive de-duplication, and a test
  that tries to escape the output folder with `../../etc/passwd` as a conversation name.
- Two output-safety decisions, both counted rather than silent: HTML content passes through one
  `escape_html` and the export carries **no JavaScript** (so it opens from `file://` forever), and
  CSV cells starting `=`, `+`, `-` or `@` are prefixed with `'` and reported as
  `formulasNeutralised`.
- `export-report.json` reports `attachmentsMissing` and `skipped`, because an export that quietly
  dropped twelve photos would look exactly like a complete one.
- Progress is throttled to ~10/s inside the job, not in the UI, while the registry is updated on
  every callback — the bar stays smooth without the IPC channel becoming the bottleneck.
- **Third real-data bug caught here:** `ZWACHATSESSION.ZMESSAGECOUNTER` disagrees with the rows
  actually present for **all 1,107** conversations, so the chat list said 4,493 where the export
  wrote 4,490. Chat counts now come from a grouped join on `ZWAMESSAGE`
  (docs/DATA_SOURCES.md).
- Verified on real data: one SMS conversation to HTML (6,507 messages, 1.6 MiB, every index link
  resolving, no `<script` in the output) and one WhatsApp chat to CSV (4,490 messages, 1
  attachment referenced but absent). Backup mtimes unchanged afterwards; the exports were deleted.
- **Not verified by me:** that an exported HTML conversation *reads* like the app's thread. The
  structure is asserted; the resemblance needs eyes.

## M5: Photos
- [x] `Photos.sqlite` parsing (ZASSET / ZGENERICASSET), albums join-table discovery, cloud-only detection
- [x] DCIM fallback with EXIF dates
- [x] HEIC decode service for the lightbox (`libheif-rs`), bounded worker pool, `ibex://thumb/`
- [~] Photos grid (virtualized, month headers), lightbox with metadata, multi-select (shift/ctrl-click
      and "Export N items"), export via the existing `ExportDialog` — **not done:** rubber-band
      drag-select, an "export originals instead of edits" toggle, and an optional HEIC→JPEG
      conversion on export (it writes the file as-is)
- [x] Live Photo pairing, including playback on press-and-hold in the lightbox

**Done when:** you can scroll the whole camera roll smoothly and export a selection.

Notes:
- **The grid needs no thumbnail service at all.** Apple already leaves a pre-rendered 360 px
  JPEG in the backup for every asset (`Thumbnails/V2/.../5005.JPG`) — 7,909 of them for 7,865
  real assets, not one short. `thumbs::render` (libheif + a bounded semaphore) exists only for
  the lightbox, where over half a real library is HEIC and WebKitGTK cannot decode any of it.
  This deleted most of what "thumbnail service, bounded worker pool" implied when the milestone
  was planned.
- **Two concurrency bugs, not specific to Photos, found by this milestone's access pattern**
  (four files resolved per item instead of one) and fixed at the source rather than in a test
  workaround: `Backup::has_data` was decrypting whole files just to check they existed (28 GB /
  41s for one page of a real library, now 2.7s with no decryption); every "write a temp file,
  then rename" in the crate shared one fixed temp name across callers, which a cold parallel
  test run against one real backup corrupted outright. See the `fix(core)` commit.
- **Third real-data classification bug, same shape as `ZMESSAGECOUNTER` and `ZMESSAGETYPE`:**
  `ZUNIFORMTYPEIDENTIFIER` claims JPEG for 578 real assets that are PNG (mostly screenshots), and
  `ZHIDDEN` was `0` for all 7,865 real rows — `ZVISIBILITYSTATE` is where "hidden" actually lives.
  Neither is used; the media type comes from the file, hidden state checks both columns.
- **Album discovery needed Core Data's entity map, not a `sqlite_master` scan**: the real library
  has eleven `Z_%ASSETS` tables, and the one real join table has a `Z_FOK_` ordering column beside
  its real foreign key — joining on that under-reported 3 albums with members where the real
  count is 7 holding 651 assets. docs/DATA_SOURCES.md has the full story.
- Real HEIC files sniff as `image/heif`, not `image/heic` — code branching on the type has to
  accept both, which is also why the module is named generically (`thumbs`, not `heic`).
- Verified on real data: 7,162 of 7,865 assets in the default view (656 trashed, 47 hidden),
  **100% of originals and derivatives resolved**, every Live Photo found its companion, 1,065 of
  1,074 edits found their render. A sample of real HEIC assets decoded to correctly-sized JPEGs.
  The backup folder's newest mtime was unchanged after all of this session's testing.
- **Not verified by me:** that the grid scrolls smoothly and the lightbox looks right on a real
  7,000+ asset library — structure and counts are asserted, not feel. HEIC pass-through in the
  lightbox vs the WebKitGTK-can't-decode-it baseline also needs eyes, not just a byte comparison.

## M6: Voice Memos and Documents
- [x] Voice Memos: probe locations, titles from CloudRecordings.db, list/play/export with titled file names
- [x] Documents: Files app storage + per-app Documents roots with app names; Files view with previews
- [x] Batch export for both

**Done when:** recordings play with their titles, and documents are browsable by app and exportable.

Notes:
- **The real backup's `CloudRecordings.db` is genuinely corrupted** — confirmed independently
  with plain `sqlite3` tooling, no `-wal`/`-shm` sidecar to explain it — so the documented
  fallback (list `.m4a`/`.qta` directly, no titles) is what this real backup's Voice Memos view
  actually runs on: **81 recordings**, zero duplicates, confirmed against real data. The database
  schema itself is therefore sourced from community documentation and a synthetic fixture only,
  not a readable real database — the reverse of M5's DCIM fallback, which stayed fixture-only
  because the real `Photos.sqlite` happened to parse fine.
- `.qta` (29 of the 81 real recordings) is a QuickTime container; `.composition` folders hold no
  audio and `-trackN.waveform` files are UI data only — both are skipped, not listed as
  recordings. **Found after this milestone shipped:** `.qta` sniffs as `video/quicktime`, and the
  player used a plain `<audio>` element for every recording regardless — WebKitGTK's `<audio>`
  refuses to play a `video/*`-typed resource at all, so every `.qta` recording silently failed to
  play, the newest recordings first since those sort to the top of the list. Fixed by picking
  `<video>` when the sniffed preview kind is video, exactly as the Raw browser's own
  `PreviewPane.tsx` already does.
- **Documents needed no new browsing code at all.** `Backup::list_dir` already works for any
  domain; the only gap was naming the starting points. Found in the process:
  `docs/DATA_SOURCES.md`'s own note on where app display names live was wrong — the real source
  is a nested binary plist inside `Info.plist`, not a flat name field — corrected there, with the
  fix giving readable labels for all 92 of the real backup's sources and zero raw bundle ids.
- Both exporters reuse `ConversationReport`/`ExportTotals` rather than growing parallel fields,
  the same choice M5 made for Photos; `Selection` gained one more additive field
  (`document_root`) alongside M5's `photo_filter`.
- Verified on real data: 81 recordings via the fallback; 92 Files sources, matching a direct
  count against the manifest exactly; a real export of 3 recordings and 5 On-My-iPhone documents,
  byte-compared against the originals; the backup folder's mtimes unchanged afterward.
- **Not verified by me:** that recordings actually *sound right* when played — the audio/video
  element choice is confirmed correct structurally (an SSR check and the sniffed-type logic both
  check out), but hearing it is still the user's to confirm, the same caveat as every audio/video
  claim so far. Titles are unverified for a different reason: the one real backup available has
  no titles to show at all, since its database is unreadable, so that half of this milestone's
  own done-when is proven only against the fixture.

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
- [ ] Per-domain sizes in Overview as a background job (deferred from M1)
- [ ] Performance pass on a large backup (open time, scroll, thumbnail throughput); document numbers
- [~] Packaging: **.deb and .rpm build and verify** (`scripts/package.sh`); AppImage and a real
      app icon still to do

**Done when:** a release build installs on a fresh Linux machine and handles a large backup comfortably.

Notes:
- `scripts/package.sh` builds both packages. 4.1 MiB each, a 9.4 MiB binary; `Depends` /
  `Requires` resolve to `libwebkit2gtk-4.1-0` and `libgtk-3-0` only, since SQLite is statically
  bundled. Both ship the GPL text (`/usr/share/doc/ibex/copyright`,
  `/usr/share/licenses/ibex/LICENSE`), and the two binaries differ by exactly three bytes —
  Tauri's `DEB`/`RPM` bundle-type marker.
- The script forces `umask 022` because the rpm bundler copies file modes from disk while the deb
  bundler normalizes them: with a 002 umask the rpm alone shipped a group-writable
  `/usr/bin/ibex`.
- **Still the Tauri placeholder icon**, so the packages install with a generic app image. Vendor
  and URL are empty in the rpm too — `publisher` only reaches the deb's `Maintainer`, and
  `RpmConfig` has no vendor field.
- **Not verified by me:** that either package installs and launches. They were unpacked and every
  file checksummed, but `dpkg -i` needs root and starting the app needs a display.

## M9: Contacts, Call History and Notes

I probed all three against the real backup before writing this; counts and schema below are from
that backup, not just the community-documented shape.

- [ ] Contacts: `HomeDomain` / `Library/AddressBook/AddressBook.sqlitedb` — `ABPerson` (name,
      organization) joined to `ABMultiValue` (`property` 3 = phone, 4 = email) by `record_id`.
      774 people on the real backup. Reuse `extractors::contacts::ContactResolver`'s existing
      phone/email normalization (already built for SMS/WhatsApp name resolution) rather than a
      second implementation; this milestone is making that data *browsable and exportable* for
      the first time, not re-deriving it. Contact photos from `AddressBookImages.sqlitedb`
      (present on the real backup) if the join is simple enough once inspected properly.
      vCard (`.vcf`) export, per contact and in bulk.
- [ ] Call History: `HomeDomain` / `Library/CallHistoryDB/CallHistory.storedata` —
      `ZCALLRECORD` (`ZDATE`, `ZDURATION`, `ZADDRESS`, `ZORIGINATED` 0/1 for in/outgoing,
      `ZANSWERED`). **4,044 real records.** `ZSERVICE_PROVIDER` cleanly separates
      `com.apple.Telephony` (3,975) from `com.apple.FaceTime` (69) — don't assume every row is a
      cellular call. `ZCALLTYPE` has at least three real values (1/8/16); nail down what each
      means (audio vs. video vs. audio-only FaceTime is the likely split) before relying on it.
      Resolve `ZADDRESS` against Contacts the same way SMS/WhatsApp resolve a number today.
- [ ] Notes: `AppDomainGroup-group.com.apple.notes` / `NoteStore.sqlite` — **substantially
      harder than anything built so far.** `ZICCLOUDSYNCINGOBJECT` is one polymorphic table
      backing notes, folders, attachments and accounts together (190+ columns on the real
      backup); the actual text lives in `ZICNOTEDATA.ZDATA`, a **gzip-compressed, undocumented
      Apple protobuf blob** — not a format any existing dependency in this project reads. 281
      notes on the real backup. Needs: identifying which `ZICCLOUDSYNCINGOBJECT` rows are notes
      vs. folders vs. attachments (via `Z_ENT`/`ZTYPE`-style discrimination, the same approach
      M5 used for Photos' album entity lookup), gunzipping `ZDATA`, and reverse-engineering (or
      finding and verifying) the protobuf schema well enough to pull plain text and checklist
      state out safely — getting this wrong silently could reorder or corrupt shown text, not
      just drop it, so validate against the real backup's rendered-by-Notes content before
      trusting it. A pure Rust `flate2` (already a transitive possibility to check) plus a
      hand-rolled minimal protobuf reader, or `prost`, would both need discussion with the
      project's owner before adding (ibex-core dependency rule). Attachments (images, scanned
      documents, drawings) are a separate, later pass — text first.

**Done when:** contacts are browsable and export to vCard; call history is browsable, resolves
contact names the way Messages does, and separates FaceTime from cellular; notes show readable
text (attachments can wait).

## Later
- Safari history, Calendar
- Timeline view across sources
- PDF export of conversations
- macOS / Windows builds
