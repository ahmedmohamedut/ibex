# Architecture

## Overview

```
┌────────────────────────────── src/ (React + TS) ───────────────────────────────┐
│  Views: Overview · Messages · WhatsApp · Photos · Voice · Files · Raw · Exports  │
│  api/ (invoke wrappers + TanStack Query)   media via ibex:// URLs               │
└───────────────────────────────┬──────────────────────────────┬──────────────────┘
                     invoke() / events                 ibex://localhost/...
┌───────────────────────────────┴──────────── src-tauri/ ──────┴──────────────────┐
│  commands.rs   (thin wrappers, spawn_blocking)                                  │
│  protocol.rs   (ibex:// URI scheme: streams backup files, Range support)        │
│  jobs.rs       (exports, indexing: progress events, cancellation)              │
│  state.rs      (AppState: open backups, caches)                                 │
└───────────────────────────────┬─────────────────────────────────────────────────┘
                                │ plain Rust calls
┌───────────────────────────────┴──────────── crates/ibex-core/ ──────────────────┐
│  backup/     open, Info/Status/Manifest.plist, Manifest.db, resolver, crypto     │
│  workcopy/   SQLite working copies (db + wal + shm) in the cache                 │
│  extractors/ message (shared types) · contacts · sms · whatsapp · photos · ...   │
│  export/     html · csv · json · files                                           │
│  thumbs/     thumbnail generation + cache (HEIC, JPEG, PNG, video poster later)  │
│  search/     FTS5 index over messages                                            │
│  crypto/     keybag, key derivation, RFC 3394 unwrap, AES-256-CBC decryption      │
│  util/       time (Apple + Unix epochs), mime sniffing, plist_text, nskeyedarchiver, paths │
│  testutil/   synthetic backup builder (feature = "testutil")                    │
└─────────────────────────────────────────────────────────────────────────────────┘
                                ▲
                      crates/ibex-cli/ (same core, debugging)
```

## Principles

- **Core is UI-agnostic.** Everything in `ibex-core` works from the CLI and from tests.
- **Backup folder is read-only.** All derived data lives in the cache.
- **Lazy and paginated.** Open fast, query on demand, never load a whole database into memory or across IPC. `page::Page { offset, limit }` requests a window (limit clamped to `MAX_LIMIT`), `page::Paged<T> { items, total, next_offset }` returns one.
- **Tolerant parsing.** Schemas differ by iOS/app version. Detect columns, degrade gracefully, report what was skipped.

## ibex-core

### backup

```rust
pub struct Backup {
    root: PathBuf,
    info: BackupInfo,          // from Info.plist: device name, model, iOS version, UDID, date
    encrypted: bool,           // from Manifest.plist IsEncrypted
    manifest: Manifest,        // open connection to (possibly decrypted) Manifest.db
    keys: Option<KeyBag>,      // unlocked class keys for encrypted backups
    cache: CacheDir,
}

pub struct ManifestEntry {
    pub file_id: String,       // SHA1 hex of "domain-relativePath"
    pub domain: String,
    pub relative_path: String,
    pub kind: EntryKind,       // File | Directory | Symlink (flags 1/2/4)
    pub meta: Option<FileMeta>,
}

pub struct FileMeta {
    pub size: u64,
    pub mtime: Option<DateTime<Utc>>,
    pub birth: Option<DateTime<Utc>>,
    pub mode: u32,
    pub protection_class: Option<u32>,
    pub encryption_key: Option<Vec<u8>>, // wrapped per-file key (encrypted backups)
}
```

Key operations:

- `Backup::open(path, password: Option<&str>) -> Result<Backup>`
- `Backup::open_with_cache(path, password, CacheDir) -> Result<Backup>`: same, with the cache
  redirected. Tests use it so the suite never writes to the user's real data directory; the
  app can use it to put the cache elsewhere.
- `Backup::peek(path) -> Result<BackupInfo>`: metadata without opening. `Info.plist` and
  `Manifest.plist` are in the clear even in an encrypted backup, so the password prompt can name
  the device it is unlocking.
- An encrypted backup without a password returns `BackupError::PasswordRequired`; with a wrong
  one, `CryptoError::BadPassword`. Both are distinct UI states.
- `backup.find(domain, rel_path) -> Option<ManifestEntry>`
- `backup.list_dir(domain, dir, Page) -> Paged<ManifestEntry>`: direct children only, filtered
  and ordered in SQL so `LIMIT`/`OFFSET` page over the right set. Directories sort first.
- `backup.domains() -> Vec<DomainSummary>`: **file counts only** (one `GROUP BY`).
  `DomainSummary.total_size` is `None`.
- `backup.domains_with_sizes(progress) -> Vec<DomainSummary>`: fills in `total_size`.
  A file's size lives inside its archived `file` blob, so totalling sizes means decoding
  every blob in the backup — 100k+ plist parses. That is a job with progress, not something
  a screen can call synchronously, hence the split.
- `backup.open_file(&entry) -> Result<Box<dyn ReadSeek>>`: plain file for unencrypted backups. For encrypted ones, decrypt to the cache on first access and return the cached file.
- `backup.materialize(&entry) -> Result<PathBuf>`: guarantees a real path on disk (used for SQLite and for the media protocol).

The `file` column blob is an NSKeyedArchiver plist. `util::nskeyedarchiver` resolves `$objects`/`UID` references into a plain `plist::Dictionary`.

**`backup::info` also resolves app display names**, into `BackupInfo.app_names: HashMap<String,
String>`. `Info.plist`'s `Applications` dict is keyed by bundle id, but a value there is
`{ApplicationSINF, PlaceholderIcon, iTunesMetadata}` — `iTunesMetadata` is itself a nested binary
plist (a `Data` value), and only its `itemName` field is the real App Store title. Free
first-party apps have no `Applications` entry at all; `extractors::documents::app_name` falls
back to a small built-in table for those, then to a prettified bundle id for anything neither
source names. Malformed entries for one app never fail the whole map — that app is simply absent
from it.

### workcopy

SQLite databases in a backup often have `-wal` and `-shm` siblings holding recent data. A working copy:

1. Finds `X`, `X-wal`, `X-shm` in the manifest (same domain, sibling paths).
2. Materializes all three into `cache/<backup-id>/db/<file_id>/`.
3. Opens with `SQLITE_OPEN_READ_ONLY` (the WAL is applied on read). If read-only open fails because WAL needs recovery, open read-write on the **cache copy** only.

`workcopy` takes **plain paths**, not a `Backup`, because it has two callers and one of
them cannot have a `Backup` yet:

- manifest-based: `sms.db` and friends, whose `-wal`/`-shm` siblings are separate manifest
  entries. `Backup::open_db` / `Backup::db_workcopy` assemble these.
- by path: `Manifest.db` itself is a top-level file that the manifest it indexes does not
  list, and it is copied before any `Backup` exists.

Re-copying is skipped when a `.stamp` file beside the copy still matches the sources' sizes
and mtimes. The stamp is needed because opening a WAL database read-write legitimately
checkpoints it, changing the copy's own size, which would otherwise force a copy every time.

**The check-then-copy sequence is guarded by a per-directory lock** (`lock_for`, an in-process
registry keyed by the working-copy's path). Two callers materializing the *same* database at
once — the real-backup integration tests do this on every run, since every test opens the same
backup, and a real app could too on first load — must not interleave their remove/copy steps, or
a reader can open a connection mid-replacement and find a missing or half-written database. This
was found, not designed in: a cold, parallel test run against a real backup failed most of its
tests until the lock was added. Different databases never block each other; only two callers of
the same one do.

API: `backup.open_db(domain, rel_path) -> Result<rusqlite::Connection>`

### crypto

Encrypted backups, implemented in M7. The chain, rooted in the backup password:

```text
password ──PBKDF2-SHA256(DPSL, DPIC)──┐
                                      ├─PBKDF2-SHA1(SALT, ITER)──> passcode key
                                      ┘                               │
                                                     RFC 3394 unwrap   ▼
  keybag WPKY per protection class ─────────────────────────────> class keys
                                                                       │
  ManifestKey / MBFile EncryptionKey ──────RFC 3394 unwrap─────────────▼
                                                             per-file keys
```

- `KeyBag::unlock(keybag_bytes, password)` parses the TLV keybag, derives the passcode key and
  unwraps every class key whose `WRAP & 2` is set. The AES key wrap's integrity check is what
  makes a wrong password detectable: if **every** unwrap fails, that is `CryptoError::BadPassword`.
  A single class failing is tolerated — that class is simply unavailable.
- The password is borrowed, used once and dropped. Only the class keys are kept, in `Zeroizing`
  buffers wiped on drop, and no `Debug` impl prints key bytes. Nothing is ever persisted.
- `Manifest.db` is decrypted whole into `cache/<backup-id>/manifest.db` and opened from there.
  Each file is decrypted on demand into `cache/<backup-id>/files/<file_id>`, streamed in 64 KiB
  chunks and truncated to the manifest's recorded `Size` — so padding is removed without being
  trusted, and a multi-gigabyte video is never buffered in memory. An existing decryption whose
  length already matches is reused.
- **Unlocking is deliberately expensive.** A current backup asks for ten million
  PBKDF2-HMAC-SHA256 rounds: about 0.8s in a release build. Never call it on a UI thread.
  `pbkdf2_hmac` is generic and so is monomorphized into ibex-core, which means ibex-core's own
  opt-level governs its speed; the workspace `Cargo.toml` therefore sets `opt-level = 1` for
  ibex-core in the dev profile, turning ~11s into ~1s.
- `testutil` carries the matching **encrypt-and-wrap** side, so tests build genuinely encrypted
  fixtures and prove the decrypt path against them rather than against a parallel implementation.

### extractors

Each extractor owns one data source and exposes typed, paginated queries. The common trait is intentionally small:

```rust
pub trait Extractor {
    const ID: &'static str;                       // "sms", "whatsapp", ...
    fn detect(backup: &Backup) -> Availability;   // Present { version_hint } | Absent | Unsupported(reason)
}
```

Extractor-specific APIs (examples):

- `sms::SmsDb::open(&backup)` then `.list_conversations(Page)` and
  `.list_messages(conversation_id, MessageCursor)`. Opening probes the schema once, so callers
  making several queries hold one `SmsDb` rather than reopening.
- Thread pagination is a **keyset** on `(date, ROWID)` descending, not an offset: a thread can
  hold tens of thousands of messages, where `OFFSET` degrades with depth and can skip or repeat
  rows. `MessageCursor`/`MessagePage` carry it.
- `whatsapp::list_chats`, `whatsapp::list_messages`
- `photos::PhotosDb::open(&backup)` then `.list(&PhotoFilter, PhotoCursor)`, `.timeline(&PhotoFilter)`
  for month counts, `.albums()`, `.asset(id)`. Keyset-paginated the same way as messages, on
  `(ZDATECREATED, Z_PK)`. A `DcimFallback` (`extractors::photos::dcim`) lists `Media/DCIM`
  directly with EXIF dates when `Photos.sqlite` is missing or unreadable; fixture-tested only —
  the real backup's library parses.
- `voice::VoiceMemosDb::open(&backup)` then `.list(Page)`. Primary path reads
  `CloudRecordings.db` → `ZCLOUDRECORDING`; falls back to listing `.m4a`/`.qta` directly (skipping
  `-trackN.waveform` sidecars and `.composition` metadata folders) whenever the database is
  missing **or present but fails even a basic query** — the real backup this was built against is
  the latter, independently confirmed corrupt, so the fallback is exercised by real data, not just
  a fixture.
- `documents::list_document_sources(&backup) -> Vec<DocSource>` names the browsable roots — "On My
  iPhone" plus one entry per `AppDomain-<bundle>` with a non-empty `Documents/`. Browsing itself
  needs nothing new: a `DocSource { domain, root, label, bundle_id }` is just handed to the
  existing `Backup::list_dir`/`file_info`/preview commands, the same ones the Raw browser already
  calls. App display names resolve through `BackupInfo::app_names`, built in `backup/info.rs` from
  a nested plist inside `Info.plist` — see the note under `backup::info` below.
- `contacts::resolver(&backup) -> ContactResolver` (normalizes phone numbers and emails, maps to display names; used by sms and whatsapp)

Messages normalize to shared types so the UI and exporters handle both SMS and WhatsApp the same way:

```rust
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    pub sent_at: DateTime<Utc>,
    pub from_me: bool,
    pub sender: Option<Participant>,
    pub body: Option<String>,
    pub kind: MessageKind,        // Text | Image | Video | Audio | Voice | Document | Location | Contact | Sticker | System | Unknown(i64)
    pub attachments: Vec<AttachmentRef>,
    pub reply_to: Option<String>,
    pub reactions: Vec<Reaction>,
    pub edited: bool,
    pub service: Service,         // Sms | IMessage | Rcs | WhatsApp
}

pub struct AttachmentRef {
    pub file_id: Option<String>,  // None if missing from backup
    pub file_name: String,
    pub mime: Option<String>,
    pub size: Option<u64>,
}
```

`PhotoAsset` is its own shape, not forced into `Message`: a photo has no sender or body, and
carries a resolution chain `Message` has no equivalent of —

```rust
pub struct PhotoAsset {
    pub id: String,                          // Z_PK
    pub kind: PhotoKind,                      // Photo | Video
    pub file_id: Option<String>,              // the original
    pub thumb_file_id: Option<String>,        // Apple's own 360px derivative
    pub live_file_id: Option<String>,         // Live Photo companion .MOV
    pub edited_file_id: Option<String>,       // the rendered edit, when there is one
    pub edited_live_file_id: Option<String>,
    // ... favorite, screenshot, edited, live, trashed, hidden, location, availability, ...
}
```

`display_file_id()` / `display_live_file_id()` resolve that chain to "what Photos itself would
show": the edit when there is one, else the original — decided with the project's owner, and the
rule the exporter and the lightbox both follow rather than each encoding it separately.

The grid never decodes anything: Apple already leaves a 360 px JPEG derivative in the backup for
every asset (`Media/PhotoData/Thumbnails/V2/<dir>/<file>/5005.JPG`), which `thumb_file_id`
points at. Decoding only happens for the lightbox — see `thumbs` below.

### export

`export::run(&backup, &selection, format, out, &options, progress, cancel)` dispatches on the
format:

```rust
pub trait Exporter {
    fn export(&self, backup: &Backup, sel: &Selection, out: &Path, opts: &ExportOptions,
              progress: &dyn Fn(Progress), cancel: &CancelToken) -> Result<ExportReport>;
}
```

Each exporter opens the selection's source as a `ChatSource`, so no format knows which app the
messages came from. `ChatSource` is an enum over `SmsDb` and `WhatsAppDb` (`export/source.rs`). The two have the same
shape but not the same signature — WhatsApp takes an `include_broadcasts` flag — and an enum keeps
that one difference in a single place without the lifetimes a trait object would need. Its
`Debug` prints only the source id: the databases behind it hold message content, which must never
reach a log.

**Photos is selected by `Selection.source == "photos"`, ahead of the format match** — it has one
export shape (copy the files, named and dated), not four, so `ExportFormat` is not consulted for
it. `Selection` grew one additive field for this, `photo_filter: Option<PhotoFilter>`
(`#[serde(default)]`, so every existing caller is unaffected): empty `conversation_ids` plus no
filter means the same view the grid shows by default; an explicit filter or explicit asset ids
override it, exactly as "every conversation" vs "these ids" already worked for a chat source.
`PhotosExporter` (`export/photos.rs`) writes `<year>/<YYYY-MM-DD> <name>.<ext>`, the edited render
preferred over the original, a Live Photo's companion `.MOV` copied alongside its still under the
same name. `ConversationReport`/`ExportTotals` are reused rather than given a parallel photos
shape — `messages`/`conversations` become counts of items/year-folders — and the UI relabels them
per `report.source` rather than the DTO gaining chat-specific-sounding fields photos would have to
leave blank.

**Voice Memos and Documents follow the same `Selection.source` dispatch**, selected ahead of
Photos' check in `export::run`:

- `VoiceExporter` (`export/voice.rs`, `source == "voice"`) writes
  `<YYYY-MM-DD HHmm> <title or "Recording">.<ext>` (extension matching the source file, `.m4a` or
  `.qta`). No filter field is needed — `conversation_ids` empty means every recording, exactly as
  for a chat source.
- `DocumentsExporter` (`export/documents.rs`, `source == "documents"`) copies files preserving
  their folder structure under the chosen root. `Selection` grew a second additive field,
  `document_root: Option<DocumentRoot>` (`{domain, root}`), naming which Files source
  `conversation_ids` (relative paths, or empty for everything) is relative to. Recurses through
  `Backup::list_dir` itself — there is no database to page through — and sanitizes **every path
  segment individually** through `naming::join_safe` before writing, because a manifest path is a
  real historical device path, never guaranteed filesystem-safe on this OS and never trusted as
  pre-sanitized just because it came from the backup rather than from message data.

Both write a single `ConversationReport` entry rather than one per logical group — there is no
natural grouping the way Photos has year folders — so the UI additionally hides the
"conversations" row entirely for these two sources rather than showing an always-`1` count.

- `ContactsExporter` (`export/vcard.rs`, `source == "contacts"`, M9) writes one vCard 3.0
  `.vcf` file per contact, named from its resolved display name through the same
  `NameAllocator`/`naming::join_safe` every other exporter uses. `conversation_ids` holds
  contact ids here, empty meaning every contact — the same convention as `voice`/`documents`.
  Also a single `ConversationReport` entry, same reasoning as above.

`messages_chronological` walks the extractor's existing keyset cursor 500 messages at a time and
reverses the result, because a thread reads newest-first and an export reads oldest-first.

- **HTML** (`html.rs`): one folder per conversation with `index.html` and `media/`, plus a
  top-level index listing every conversation. CSS is inlined from one Rust constant mirroring
  `src/styles/tokens.css`, dark-mode block included. **No JavaScript**, so it opens from a
  `file://` URL forever. Everything that comes out of a database — bodies, names, file names,
  `title` attributes — goes through one `escape_html` (`& < > " '`).
- **CSV** (`csv.rs`): 15 fixed columns, a stable contract. A cell starting with `=`, `+`, `-` or
  `@` is prefixed with `'` so a spreadsheet cannot execute it, and the substitutions are counted
  into the report as `formulasNeutralised` — never a silent edit.
- **JSON** (`json.rs`): `{ conversation, messages: [...] }` streamed with
  `serde_json::to_writer_pretty`, using the same `Serialize` impls as the IPC DTOs, so there is no
  second schema to keep in step.
- **Files** (`files.rs`): original attachments with original names, mtime restored via
  `Backup::copy_file_to`.

**`naming.rs` is the security-critical part**, because an export is the first thing ibex writes
that someone else will open. `safe_component` rejects path separators and NUL, strips control
characters, replaces the Windows-reserved set, refuses the reserved device names (`CON`, `AUX`,
`COM1`…, with or without an extension), trims trailing dots and spaces, and caps a component at
200 bytes on a UTF-8 boundary. `NameAllocator` de-duplicates case-insensitively with ` (2)`,
` (3)`. Paths are only ever built by joining sanitized components, so a conversation named
`../../etc/passwd` cannot escape the output folder — there is a test that tries.

**Cancellation** is an `Arc<AtomicBool>` checked between messages and before each media copy. A
cancelled export returns the report it had built, marked `cancelled: true`, and leaves its partial
folder: a half-export that claimed success would be worse than one that says what it is.

Every export writes `export-report.json` with per-conversation counts, totals, and — the honest
part — `attachmentsMissing` and `skipped`. An export that silently dropped twelve photos would
otherwise look identical to a complete one.

### jobs

`src-tauri/src/jobs.rs`. A `JobRegistry` in `AppState` maps `JobId -> { JobInfo, CancelToken }`.
`start_export` spawns onto `spawn_blocking` and returns the id immediately; the work reports by
event.

- **Progress is throttled in the job, not in the UI** (`ProgressThrottle`, 100 ms): a
  66,407-message export would otherwise emit tens of thousands of events and the IPC channel
  becomes the bottleneck. The first and last updates always go out, so a short job shows something
  and a finished bar reaches its end. The registry is updated on *every* callback even when the
  event is dropped, so `list_jobs` is never stale.
- **Finished jobs stay in the registry** for the session, with their reports, so the Exports view
  can show them after the toast has gone.
- Commands are generic over `R: tauri::Runtime` so the mock runtime can drive them in tests.

### thumbs

`thumbs::render(&backup, &entry, max_edge) -> Render { path, mime, original }`. This exists for
the lightbox alone — the camera-roll **grid** needs no decoding at all, since Apple leaves a
pre-rendered JPEG in the backup for every asset (see `extractors::photos` above), and loads that
directly over the `file` route.

- A web-safe type (JPEG/PNG/GIF/WebP, by sniffed content, not by extension — see the
  `ZUNIFORMTYPEIDENTIFIER` note in `docs/DATA_SOURCES.md`) is handed back untouched: `original:
  true`, no re-encode, no quality loss.
- HEIC/HEIF decodes through `libheif-rs` (context → primary image handle → `LibHeif::decode` into
  an interleaved RGB plane, copied out row by row because `stride` is padded) and re-encodes to
  JPEG with `image`, resized to `max_edge` with a `Triangle` filter if requested. Cached at
  `thumbs/<max_edge>/<file_id>.jpg` — the cache layout this document already specified; nothing
  changed there.
- A process-wide counting semaphore (`available_parallelism`) bounds concurrent decodes, so
  holding an arrow key in the lightbox cannot start one decode per keypress.
- The render is written to a uniquely-named temp file (`CacheDir::unique_temp_path`, not a fixed
  `.partial` name) and renamed into place — the same fix the workcopy race above needed, for the
  same reason: two concurrent renders of one photo must not be able to truncate each other.

`src-tauri/src/protocol.rs`'s `thumb/` route calls this directly; see Media protocol below.

### search

Builds a SQLite FTS5 index in `cache/<backup-id>/search.db` over SMS + WhatsApp message bodies on demand (job with progress). Queries return `SearchHit { service, conversation_id, message_id, sent_at, snippet }`.

## Cache layout

```
~/.local/share/ibex/
  backups.json                      # recently opened backups
  cache/<backup-id>/                # backup-id = SHA256 of canonical path + Info.plist UDID + backup date
    manifest.db                     # decrypted Manifest.db (encrypted backups only)
    db/<file_id>/{db,db-wal,db-shm} # SQLite working copies
    files/<file_id>                 # decrypted files (encrypted backups only)
    thumbs/<size>/<file_id>.jpg
    search.db
```

The UI offers "Clear cache for this backup". For encrypted backups, decrypted data in the cache is sensitive. Document this in the UI and clear it on request.

## src-tauri

### State

```rust
pub struct AppState {
    backups: RwLock<HashMap<BackupHandle, Arc<Backup>>>,
    // jobs: JobRegistry — milestone M4
}
```

`BackupHandle` is an opaque string id returned by `open_backup`; it is the backup's cache id, so
re-opening the same backup replaces its entry rather than leaking a second one.

Sharing one `Backup` across threads needs it to be `Sync`, and `rusqlite::Connection` is `Send` but
not `Sync`, so `Manifest` holds its connection in a `Mutex`. Reads are short and indexed, so
serializing them costs far less than giving up a shared handle.

### IPC contract (commands)

All commands are `async`, return `Result<T, IpcError>`, and run blocking work in `spawn_blocking`. `IpcError { code, message }` serializes to the frontend.

| Command | Args | Returns |
|---|---|---|
| `open_backup` | `path, password?` | `BackupSummary` (handle, device info, encrypted, available sources) |
| `close_backup` | `handle` | `()` |
| `recent_backups` | | `Vec<RecentBackup>` |
| `list_domains` | `handle` | `Vec<DomainSummary>` |
| `list_dir` | `handle, domain, path, page` | `Page<ManifestEntryDto>` |
| `file_info` | `handle, fileId` | `FileInfoDto` (incl. sniffed mime) |
| `read_text_preview` | `handle, fileId, maxBytes` | `TextPreview` |
| `read_hex_preview` | `handle, fileId, offset, len` | `HexPreview` |
| `sms_conversations` | `handle, page?` | `Paged<ConversationSummary>` |
| `sms_messages` | `handle, conversationId, cursor?` | `MessagePage` (messages newest-first, plus the cursor for older) |
| `wa_chats` | `handle, page` | `Page<ConversationSummary>` |
| `wa_messages` | `handle, chatId, cursor` | `Page<Message>` |
| `photos_list` | `handle, filter?, cursor?` | `PhotoPage` (keyset, like `MessagePage`) |
| `photos_timeline` | `handle, filter?` | `Vec<MonthCount>` (newest month first) |
| `photos_albums` | `handle` | `Vec<AlbumSummary>` (titled albums only) |
| `photo_detail` | `handle, id` | `PhotoAsset` |
| `voice_list` | `handle, page?` | `Paged<VoiceRecording>` |
| `documents_sources` | `handle` | `Vec<DocSource>` |
| `contacts_list` | `handle, page?` | `Paged<Contact>` |
| `call_history_list` | `handle, page?` | `Paged<CallRecord>` |
| `start_export` | `handle, selection, format, outDir, options?` | `JobId` (returns at once; the work reports by event) |
| `start_index` | `handle` | `JobId` (returns at once; the work reports by event) |
| `index_status` | `handle` | `IndexStatus` (whether a usable index already exists) |
| `search` | `handle, query, filters, page` | `Paged<SearchHit>` |
| `cancel_job` | `jobId` | `bool` (whether a running job was asked to stop) |
| `list_jobs` | | `Vec<JobInfo>` (this session's jobs, newest first, with their reports) |
| `clear_cache` | `handle` | `u64` (bytes freed) |
| `peek_backup` | `path` | `BackupPeek` (device info without opening; labels the password prompt) |
| `backup_stats` | `handle` | `BackupStats` (total bytes on disk, entry count, domain count, cache bytes) |
| `save_file_as` | `handle, fileId, dest` | `SaveReport` (bytes written, whether the mtime was restored) |
| `forget_recent` | `path` | `()` |
| `total_cache_size` | | `u64` |

Events: `job://progress` `{ jobId, done, total, message }`, `job://finished` `{ jobId, report }`, `job://failed` `{ jobId, error }`.

Browsing a Documents source adds no commands of its own: `documents_sources` names the roots, and
the frontend points the existing `list_dir`/`file_info`/preview commands at one source's
`domain`/`root` — the same calls the Raw browser already makes.

Keep this table in sync with the code.

### Media protocol

Registered with `register_asynchronous_uri_scheme_protocol("ibex", ...)`.

- `ibex://localhost/file/<handle>/<fileId>`: original file bytes, correct `Content-Type` (sniffed
  from content by `util::mime`, since files on disk have hashed, extensionless names),
  **HTTP Range support** (required for audio/video seeking): `bytes=a-b`, `bytes=a-` and `bytes=-n`
  answer `206` with `Content-Range`; an unsatisfiable range answers `416` with `bytes */len`;
  multiple ranges fall back to a full `200`. Unknown handle, unknown file ID and data absent from
  the backup all answer `404`; non-`GET` answers `405`. A request with no `Range` for a file over
  16 MiB is answered as a `206` first chunk, so a large video is never buffered whole.
  The handler is a pure `protocol::respond(&AppState, &Request) -> Response`, which is how the
  status and header matrix is unit-tested without a webview.
- `ibex://localhost/thumb/<handle>/<fileId>?s=<maxEdge>`: a displayable version of the file, via
  `ibex_core::thumbs::render` — a web-safe image passed through unchanged, or a HEIC/HEIF
  re-encoded to JPEG at `maxEdge` (full size when `s` is absent or unparseable). Shares the same
  Range-aware response path as `file` (`respond_file`/`respond_thumb` both call `serve_path`), so
  the status/header matrix is one tested implementation, not two. `404` for an unknown handle,
  unknown file, data absent from the backup, or a file that cannot be rendered; `405` for
  non-`GET`.

The frontend builds URLs with two helpers in `src/api/media.ts`: `fileUrl` and `thumbUrl`. On
Windows the scheme form is `http://ibex.localhost/...`, so both branch on platform even though
Linux is the primary target.

## Frontend

- `src/api/`: one typed wrapper per command, TanStack Query hooks (`useSmsMessages(handle, convId)` with infinite queries for cursors).
- `src/bindings/`: `ts-rs` output, regenerated by `cargo test export_bindings` (workspace-wide, because
  DTOs live in both crates). `TS_RS_EXPORT_DIR` is set in `.cargo/config.toml`.
  Types ibex-core already defines (`ManifestEntry`, `FileMeta`, `EntryKind`, `DomainSummary`,
  `BackupInfo`, `Page`, `Paged`, `Sniffed`, `PreviewKind`) cross IPC as themselves — they derive
  `TS` alongside `Serialize`, so there are no parallel `*Dto` copies to keep in sync. `src-tauri/dto.rs`
  holds only genuinely new wire shapes (`BackupSummary`, `FileInfo`, `TextPreview`, `HexPreview`,
  `BackupStats`, `SaveReport`, `RecentBackup`, `SourceAvailability`, `IpcError`).
  `FileMeta.encryption_key` is `#[serde(skip)]`, so wrapped key material cannot reach the frontend.
  `scripts/check.sh` regenerates the bindings before `npm run build` type-checks them, so drift
  fails the build.
- `src/views/`: one folder per sidebar section.
- `src/components/`: shared pieces: `ChatThread`, `MessageBubble`, `AttachmentView`, `Gallery`, `AudioPlayer`, `FileTree`, `HexView`, `ExportDialog`, `JobToast`.
- State: TanStack Query for server data, a small Zustand store for UI state (current backup handle, selection, theme).

## Testing

- **Synthetic fixtures** (`ibex-core` feature `testutil`): `FixtureBackup::new().with_sms(...).with_whatsapp(...).build(tmpdir)` writes a valid `Manifest.db`, `Info.plist`, `Manifest.plist` and hashed files with fake data. All unit tests use this.
- **Real-backup tests**: `#[ignore]`-free but early-return when `IBEX_TEST_BACKUP` is unset. Assert structure and counts only.
- **CLI as a test harness**: each extractor has a CLI subcommand that prints counts and a few IDs (no content by default; `--show-content` flag for local manual use).
- Frontend: type-check via `npm run build`. Component tests are optional.

## Error handling

- Core errors are typed (`BackupError`, `ExtractError`, `CryptoError`, `ExportError`).
- Missing data (attachment not in backup, iCloud-only photo) is **not an error**: it's represented in the data (`file_id: None`, `availability: CloudOnly`) and shown in the UI.
- Wrong password → `CryptoError::BadPassword`, surfaced as a specific UI state.
