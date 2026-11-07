# Data sources

Where each kind of data lives in an iOS backup, and what we know about its schema. **Schemas change across iOS and app versions.** Treat everything here as a starting point; verify with `PRAGMA table_info` against real backups and update this file when you learn something.

## Backup folder layout

```
<UDID>/
  Info.plist        device name, product type, iOS version, last backup date, installed apps
  Manifest.plist    IsEncrypted, BackupKeyBag, ManifestKey (encrypted), Lockdown info
  Status.plist      backup state, IsFullBackup, Version
  Manifest.db       SQLite index of every file (encrypted when IsEncrypted = true)
  00/ … ff/         256 folders; each file stored as <first 2 hex of fileID>/<fileID>
```

### Manifest.db

```sql
CREATE TABLE Files (
  fileID       TEXT PRIMARY KEY,  -- SHA1 hex of "<domain>-<relativePath>"
  domain       TEXT,
  relativePath TEXT,
  flags        INTEGER,           -- 1 = file, 2 = directory, 4 = symlink
  file         BLOB               -- NSKeyedArchiver plist (MBFile)
);
CREATE TABLE Properties (key TEXT PRIMARY KEY, value BLOB);
```

The `file` blob unpacks (after resolving `$objects` UIDs) to an `MBFile` with keys including `Size`, `LastModified`, `Birth`, `LastStatusChange`, `Mode`, `UserID`, `GroupID`, `InodeNumber`, `ProtectionClass`, `RelativePath`, and for encrypted backups `EncryptionKey` (an `NSMutableData`/`NSData` object whose bytes start with a 4-byte little-endian class, then the wrapped key).

Physical path: `root / fileID[0..2] / fileID`. Compute `fileID` to look up a known path without a query; use the table for listings.

### Domains (common)

| Domain | Contents |
|---|---|
| `HomeDomain` | `Library/SMS/sms.db`, `Library/AddressBook/`, `Library/CallHistoryDB/`, Notes, Safari, preferences |
| `MediaDomain` | `Library/SMS/Attachments/`, older `Media/Recordings/` |
| `CameraRollDomain` | `Media/DCIM/`, `Media/PhotoData/Photos.sqlite` |
| `AppDomain-<bundle>` | an app's sandbox (`Documents/`, `Library/`) |
| `AppDomainGroup-<group>` | a shared app-group container |
| `AppDomainPlugin-<bundle>` | app extension containers |
| `SysContainerDomain-…`, `SysSharedContainerDomain-…` | system app containers |
| `KeychainDomain`, `WirelessDomain`, `ManagedPreferencesDomain`, `RootDomain`, `SystemPreferencesDomain` | system data, mostly not needed |

## Dates

Apple's Core Data / Cocoa epoch is **2001-01-01T00:00:00Z** (Unix `978307200`).

- Seconds (float or int): most Core Data databases (WhatsApp, Photos, Voice Memos).
- Nanoseconds (int): `sms.db` on iOS 11+ (`message.date`, `date_read`, `date_delivered`). Heuristic: absolute value > 1e12 → nanoseconds.
- 0 or NULL means "not set".

All conversions go through `ibex_core::util::time::from_apple(value) -> Option<DateTime<Utc>>`, which applies the heuristic. Unit-test both forms.

## Encrypted backups

Only needed when `Manifest.plist` → `IsEncrypted` is true. Overview of the scheme (iOS 10.2+):

1. `Manifest.plist` → `BackupKeyBag` is a TLV blob: header items (`VERS`, `TYPE`, `UUID`, `HMCK`, `WRAP`, `SALT`, `ITER`, `DPWT`, `DPIC`, `DPSL`) followed by per-class entries (`UUID`, `CLAS`, `WRAP`, `KTYP`, `WPKY`).
2. Derive the passcode key: `PBKDF2-HMAC-SHA256(password, DPSL, DPIC, 32)` then `PBKDF2-HMAC-SHA1(that, SALT, ITER, 32)`.
3. Unwrap each class key's `WPKY` with AES key wrap (RFC 3394) using the passcode key, for classes where `WRAP & 2`. A failure on unwrap means wrong password.
4. `Manifest.plist` → `ManifestKey`: first 4 bytes little-endian = protection class, rest = wrapped key. Unwrap with that class key; decrypt `Manifest.db` with AES-256-CBC, zero IV.
5. Per file: from the manifest `file` blob, `EncryptionKey` (skip first 4 bytes) unwrapped with the file's `ProtectionClass` key; decrypt the file with AES-256-CBC, zero IV; strip PKCS#7 padding, then truncate to `Size`.

Crates: `pbkdf2`, `sha1`, `sha2`, `hmac`, `aes-kw`, `aes`, `cbc`. Reference implementation for cross-checking: the Python `iphone_backup_decrypt` project (read its logic; don't copy its code).

Never persist the password. Keep unwrapped keys in memory only, zeroize on close (`zeroize` crate).

## SMS / iMessage

- **File:** `HomeDomain` / `Library/SMS/sms.db` (+ `-wal`, `-shm`)
- **Attachments:** `MediaDomain` / `Library/SMS/Attachments/...` (the `attachment.filename` column holds `~/Library/SMS/Attachments/...`; strip `~/` and look up in `MediaDomain`).
- **Option:** use the `imessage-database` crate (GPL-3.0) for table parsing and `attributedBody` decoding, or implement directly. Decide in M2 and record the decision here.

Key tables:

| Table | Notes |
|---|---|
| `message` | `ROWID`, `guid`, `text`, `attributedBody` (typedstream blob; holds the text when `text` is NULL, common on iOS 16+), `handle_id`, `service` (`SMS`, `iMessage`, `RCS`), `date`, `date_read`, `date_delivered`, `is_from_me`, `cache_has_attachments`, `associated_message_guid` + `associated_message_type` (tapbacks: 2000–2005 add, 3000–3005 remove), `thread_originator_guid` (inline replies), `date_edited`, `date_retracted`, `item_type`, `group_action_type`, `balloon_bundle_id` |
| `handle` | `ROWID`, `id` (phone number or email), `service` |
| `chat` | `ROWID`, `guid`, `chat_identifier`, `display_name`, `style` (43 = group, 45 = 1:1), `service_name` |
| `chat_message_join` | `chat_id`, `message_id`, `message_date` |
| `chat_handle_join` | `chat_id`, `handle_id` |
| `attachment` | `ROWID`, `guid`, `filename`, `mime_type`, `uti`, `transfer_name`, `total_bytes`, `created_date`, `is_sticker` |
| `message_attachment_join` | `message_id`, `attachment_id` |

Rules:
- Conversation = `chat`. Messages via `chat_message_join`, ordered by `message.date`.
- Skip tapback rows as standalone messages; attach them as reactions to their target (`associated_message_guid` looks like `p:0/<guid>` or `bp:<guid>`).
- Body text: `text` if non-empty, else decode `attributedBody`. The object replacement character `\u{FFFC}` marks attachment positions.
- Sender for group messages: `message.handle_id` → `handle.id` → contact resolver.

## Contacts

- **File:** `HomeDomain` / `Library/AddressBook/AddressBook.sqlitedb`
- `ABPerson` (`ROWID`, `First`, `Last`, `Organization`, `Nickname`) and `ABMultiValue` (`record_id`, `property` 3 = phone, 4 = email, `value`).
- Normalize phone numbers for matching: strip everything but digits and leading `+`; match on the last 9–10 digits as a fallback when country codes differ. Emails lowercased.
- Contact images in `AddressBookImages.sqlitedb` (optional).

## WhatsApp

- **Container:** `AppDomainGroup-group.net.whatsapp.WhatsApp.shared` (WhatsApp Business: `AppDomainGroup-group.net.whatsapp.WhatsAppSMB.shared`; support both).
- **Database:** `ChatStorage.sqlite` (+ wal/shm). Core Data, so tables are prefixed `Z` and columns `Z`; `Z_PK` is the primary key.
- **Media:** `Message/Media/<jid>/<x>/<y>/<uuid>.<ext>` in the same domain. `ZWAMEDIAITEM.ZMEDIALOCALPATH` holds the relative path (sometimes with a `Media/` or `Message/` prefix; try candidates).
- **Contacts/names:** `ContactsV2.sqlite` (`ZWAADDRESSBOOKCONTACT`) when present; else fall back to `ZPARTNERNAME` / push names.

Key tables:

| Table | Notes |
|---|---|
| `ZWACHATSESSION` | `Z_PK`, `ZCONTACTJID` (`<number>@s.whatsapp.net`, groups `@g.us`), `ZPARTNERNAME`, `ZSESSIONTYPE` (0 = 1:1, 1 = group, 2 = broadcast, 3 = status), `ZLASTMESSAGEDATE`, `ZMESSAGECOUNTER`, `ZARCHIVED` |
| `ZWAMESSAGE` | `Z_PK`, `ZCHATSESSION`, `ZISFROMME`, `ZFROMJID`, `ZTOJID`, `ZTEXT`, `ZMESSAGEDATE`, `ZSENTDATE`, `ZMESSAGETYPE`, `ZGROUPMEMBER`, `ZMEDIAITEM`, `ZPARENTMESSAGE`, `ZSTANZAID`, `ZMESSAGESTATUS`, `ZSTARRED` |
| `ZWAMEDIAITEM` | `Z_PK`, `ZMESSAGE`, `ZMEDIALOCALPATH`, `ZTHUMBNAILLOCALPATH`, `ZTITLE`, `ZFILESIZE`, `ZMOVIEDURATION`, `ZLATITUDE`, `ZLONGITUDE`, `ZVCARDNAME`, `ZVCARDSTRING` |
| `ZWAGROUPMEMBER` | `Z_PK`, `ZCHATSESSION`, `ZMEMBERJID`, `ZCONTACTNAME`, `ZFIRSTNAME` |
| `ZWAGROUPINFO` | group metadata |
| `ZWAMESSAGEINFO` | receipts, reactions (blob; parse later) |

`ZMESSAGETYPE` (commonly observed; verify):

| Value | Kind |
|---|---|
| 0 | text |
| 1 | image |
| 2 | video |
| 3 | voice/audio |
| 4 | contact (vCard) |
| 5 | location |
| 6 | system / group event |
| 7 | link |
| 8 | document |
| 10 | deleted / revoked |
| 11 | GIF |
| 15 | sticker |
| 38, 39 | view-once media |
| 46 | poll |
| 59 | system |

Unknown values map to `MessageKind::Unknown(n)`; log the value (not the content) and keep going. Voice notes are usually `.opus`; WebKitGTK can't always play Opus in an `<audio>` tag, so M3 must check playback and transcode to a cached `.ogg`/`.wav` if needed (e.g. `symphonia` + `hound`, or an optional system `ffmpeg`).

## Voice Memos

Location changed over iOS versions; probe in order:

1. `AppDomainGroup-group.com.apple.VoiceMemos.shared` / `Recordings/CloudRecordings.db` + `Recordings/*.m4a` (iOS 14+ typical)
2. `MediaDomain` / `Media/Recordings/Recordings.db` (or `CloudRecordings.db`) + `Media/Recordings/*.m4a` (older)
3. Fallback: any `.m4a` / `.qta` under the domains above, listed without titles.

`CloudRecordings.db` → `ZCLOUDRECORDING`: `ZPATH` (file name), `ZENCRYPTEDTITLE` (the title; the name is historical, it's plain text), `ZCUSTOMLABEL`, `ZDATE`, `ZDURATION`, `ZUNIQUEID`. Folders in `ZFOLDER`.

Export name: `<YYYY-MM-DD HHmm> <title>.m4a`, sanitized.

## Photos

- **Files:** `CameraRollDomain` / `Media/DCIM/1xxAPPLE/IMG_xxxx.{HEIC,JPG,PNG,MOV,MP4}`; edited versions in `Media/PhotoData/Mutations/...`.
- **Library DB:** `CameraRollDomain` / `Media/PhotoData/Photos.sqlite`.
- Main asset table is `ZASSET` (iOS 14+; `ZGENERICASSET` on older). Columns: `Z_PK`, `ZDIRECTORY`, `ZFILENAME`, `ZDATECREATED`, `ZMODIFICATIONDATE`, `ZKIND` (0 photo, 1 video), `ZWIDTH`, `ZHEIGHT`, `ZDURATION`, `ZLATITUDE`, `ZLONGITUDE` (-180.0 = none), `ZFAVORITE`, `ZHIDDEN`, `ZTRASHEDSTATE`, `ZUUID`.
- `ZADDITIONALASSETATTRIBUTES.ZORIGINALFILENAME` holds the original import name.
- Albums: `ZGENERICALBUM` + a join table whose name varies (`Z_26ASSETS`, `Z_27ASSETS`, …). Find it by scanning `sqlite_master` for `Z_%ASSETS`.
- **iCloud "Optimize Storage":** many assets in `ZASSET` have no file in the backup. Mark them `availability: CloudOnly` and show a placeholder. Small derivatives may exist in `Media/PhotoData/Thumbnails/`.
- Live Photos: a `.HEIC` + `.MOV` pair with the same base name.
- Fallback when `Photos.sqlite` is missing or unparseable: list DCIM files directly, read EXIF dates (`kamadak-exif`).

## Documents

Sources to present as browsable roots:

1. **Files app "On My iPhone":** `AppDomainGroup-group.com.apple.FileProvider.LocalStorage` / `File Provider Storage/…`
2. **Per-app `Documents/`:** every `AppDomain-<bundle>` with a non-empty `Documents/` folder. Show the app name from `Info.plist` → `Applications` → bundle id when available.
3. **Notes attachments** (later): `AppDomainGroup-group.com.apple.notes` / `Accounts/<id>/Media/…`.

Note: if the backup is iOS 13+ and an app opted out of backup, its documents simply aren't there.

## Later sources (not in the first milestones)

| Source | Location |
|---|---|
| Call history | `HomeDomain` / `Library/CallHistoryDB/CallHistory.storedata` (`ZCALLRECORD`) |
| Notes | `AppDomainGroup-group.com.apple.notes` / `NoteStore.sqlite` (`ZICNOTEDATA.ZDATA` is gzipped protobuf) |
| Safari history | `HomeDomain` / `Library/Safari/History.db` |
| Calendar | `HomeDomain` / `Library/Calendar/Calendar.sqlitedb` |
| Telegram, Signal | not in backups in a readable form (Signal excludes itself; Telegram stores in its own encrypted DB) |

## Learned facts log

Add dated notes here when a real backup contradicts or extends the above, e.g.:

- `YYYY-MM-DD` iOS X.Y: `ZWAMESSAGE.ZMESSAGETYPE` value N observed for …
- `2026-11-07` M0, **not yet checked against a real backup**: `MBFile`'s `LastModified`,
  `Birth` and `LastStatusChange` are read as **Unix** epoch seconds, not Apple-epoch
  seconds, since they come from the filesystem rather than from Core Data. They go through
  `util::time::from_unix_secs`, kept in the same module as `from_apple` so date handling
  still lives in one place. If this is wrong, every file mtime is off by 31 years — the
  symptom would be `ibex-cli ls` showing dates in 2055+. Confirm on a real backup.
- `2026-11-07` M0: a `fileID` is `SHA1("<domain>-<relativePath>")` with **no escaping**, so
  `("AppDomain", "com.foo-x")` and `("AppDomain-com.foo", "x")` collide. `Manifest::find`
  therefore treats the computed ID as a fast path and falls back to querying `domain` and
  `relativePath`, which the `Files` table stores separately and unambiguously.
- `2026-11-07` M0: `plist` 1.10.3 both reads and **writes** real binary-plist UIDs, so the
  synthetic fixture produces genuine `NSKeyedArchiver` blobs with UID indirection rather
  than a flattened approximation.
