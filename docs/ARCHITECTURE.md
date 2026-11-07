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
│  extractors/ contacts · sms · whatsapp · photos · voice · documents              │
│  export/     html · csv · json · files                                           │
│  thumbs/     thumbnail generation + cache (HEIC, JPEG, PNG, video poster later)  │
│  search/     FTS5 index over messages                                            │
│  util/       time (Apple epoch), mime sniffing, nskeyedarchiver, paths           │
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
- Until M7, an encrypted backup returns `BackupError::PasswordRequired` without a password and
  `BackupError::EncryptionUnsupported` with one, rather than pretending to open.
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

API: `backup.open_db(domain, rel_path) -> Result<rusqlite::Connection>`

### extractors

Each extractor owns one data source and exposes typed, paginated queries. The common trait is intentionally small:

```rust
pub trait Extractor {
    const ID: &'static str;                       // "sms", "whatsapp", ...
    fn detect(backup: &Backup) -> Availability;   // Present { version_hint } | Absent | Unsupported(reason)
}
```

Extractor-specific APIs (examples):

- `sms::list_conversations(&backup, Page) -> Vec<ConversationSummary>`
- `sms::list_messages(&backup, conv_id, Cursor) -> Page<Message>`
- `whatsapp::list_chats`, `whatsapp::list_messages`
- `photos::list_assets(&backup, Filter, Page)`
- `voice::list_recordings(&backup)`
- `documents::list_sources(&backup)`, `documents::list_dir(...)`
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

### export

Exporters take a selection plus options and report progress through a callback:

```rust
pub trait Exporter {
    fn export(&self, backup: &Backup, sel: &Selection, out: &Path,
              opts: &ExportOptions, progress: &dyn Fn(Progress), cancel: &CancelToken) -> Result<ExportReport>;
}
```

- **HTML:** one folder per conversation with `index.html`, inline CSS, a `media/` folder with relative links. Opens offline in any browser.
- **CSV / JSON:** one row/object per message, attachments as relative paths.
- **Files:** copies original files with original names, restores mtime (`filetime`), de-duplicates name clashes with a suffix.

Every export writes an `export-report.json` listing counts, skipped items and missing attachments.

### thumbs

`thumbs::get(&backup, file_id, size) -> PathBuf`: generates JPEG thumbnails into `cache/<backup-id>/thumbs/<size>/<file_id>.jpg`. HEIC via `libheif-rs`, others via `image`. Generation runs on a bounded worker pool.

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
    jobs: JobRegistry,
}
```

`BackupHandle` is an opaque string id returned by `open_backup`.

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
| `sms_conversations` | `handle, page` | `Page<ConversationSummary>` |
| `sms_messages` | `handle, conversationId, cursor` | `Page<Message>` |
| `wa_chats` | `handle, page` | `Page<ConversationSummary>` |
| `wa_messages` | `handle, chatId, cursor` | `Page<Message>` |
| `photos_list` | `handle, filter, page` | `Page<PhotoAsset>` |
| `voice_list` | `handle` | `Vec<VoiceRecording>` |
| `docs_sources` | `handle` | `Vec<DocSource>` |
| `start_export` | `handle, selection, format, outDir, options` | `JobId` |
| `start_index` | `handle` | `JobId` |
| `search` | `handle, query, filters, page` | `Page<SearchHit>` |
| `cancel_job` | `jobId` | `()` |
| `clear_cache` | `handle` | `()` |

Events: `job://progress` `{ jobId, done, total, message }`, `job://finished` `{ jobId, report }`, `job://failed` `{ jobId, error }`.

Keep this table in sync with the code.

### Media protocol

Registered with `register_asynchronous_uri_scheme_protocol("ibex", ...)`.

- `ibex://localhost/file/<handle>/<fileId>`: original file bytes, correct `Content-Type` (sniffed), **HTTP Range support** (required for audio/video seeking).
- `ibex://localhost/thumb/<handle>/<fileId>?s=256`: thumbnail JPEG.

The frontend builds URLs with one helper, `src/api/media.ts`. On Windows the scheme form is `http://ibex.localhost/...`, so the helper must branch on platform even though Linux is the primary target.

## Frontend

- `src/api/`: one typed wrapper per command, TanStack Query hooks (`useSmsMessages(handle, convId)` with infinite queries for cursors).
- `src/bindings/`: `ts-rs` output. Regenerate with `cargo test -p ibex-core export_bindings` (or whatever test exports them).
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
