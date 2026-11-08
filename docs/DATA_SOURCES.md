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

Crates used (M7): `pbkdf2` 0.13 with the `hmac` feature, `sha1`, `sha2`, `aes-kw` 0.3, `aes` 0.9,
`cbc` 0.2, `zeroize` 1.9. Known-answer tests cover the primitives: RFC 3394 §4.6 for the 256-bit
unwrap and RFC 6070 for PBKDF2, so a dependency bump cannot silently change behaviour.

Two practical notes from implementing it:

- Decryption **truncates to the `Size` the manifest records** rather than trusting the PKCS#7
  padding. It is free, and it cannot be fooled by a damaged final block. `Manifest.db` has no
  recorded size, so there the padding is stripped, falling back to the raw blocks if it does not
  validate.
- The AES key wrap's integrity check is the only reliable wrong-password signal. CBC has none, so
  a wrong key there yields plausible-looking garbage rather than an error — which is why the
  password must be judged at the unwrap step and never by whether a file "looks right".

Never persist the password. Keep unwrapped keys in memory only, zeroize on close (`zeroize` crate).

## SMS / iMessage

- **File:** `HomeDomain` / `Library/SMS/sms.db` (+ `-wal`, `-shm`)
- **Attachments:** `MediaDomain` / `Library/SMS/Attachments/...` (the `attachment.filename` column holds `~/Library/SMS/Attachments/...`; strip `~/` and look up in `MediaDomain`).
- **Decided in M2 (2026-11-07): use the `imessage-database` crate** (5.0.1, GPL-3.0-or-later).

  *Why.* On iOS 16+ `message.text` is usually NULL and the real text lives in `attributedBody`, a
  typedstream — Apple's `NSArchiver` format. Decoding that is the hardest part of reading
  `sms.db`, and getting it subtly wrong means showing people the wrong message text, the worst
  failure this app could have. The crate also probes which columns a given iOS version has
  (`Capabilities::determine`), and already encodes the semantics of tapbacks (including iOS 18
  custom emoji), threaded replies, edits, fully-unsent messages and group actions.

  *Consequence.* Linking GPL-3.0 code makes ibex a derivative work, so the whole project is
  **GPL-3.0-or-later**: `LICENSE` at the repo root and `license` fields on all three crates.

  *What we still write ourselves.* Contact resolution from `AddressBook.sqlitedb`; attachment
  resolution into `MediaDomain` manifest entries (the crate expects a Mac's real filesystem
  paths, not a backup's hashed layout); pagination; and the mapping onto our normalized
  `Message` type.

  *How it fits the working-copy rule.* `get_connection` takes a path and opens its own read-only
  connection, so it is handed our cache copy from `Backup::db_workcopy` — the database in the
  backup is never touched.

  *Two things worth knowing about its API.* Its message query is hardcoded to
  `ORDER BY m.date` ascending with no `LIMIT`, so paging is ours: we pick each page's row IDs
  with a keyset query, narrow `QueryContext`'s `start`/`end` (same Apple-nanosecond units as
  `m.date`) to that window, and keep the rows we asked for. And `tapbacks_by_target` loads every
  tapback in the database at once, which suits a one-shot export but not paging a thread, so
  reactions are fetched per page instead.

- Reading a row runs the crate's `parse_body`, which sets `Message::text` from the typedstream,
  falling back to a legacy `streamtyped` scan and then to the `text` column. So `text` is already
  the decoded body; there is no separate call to make.
- `is_fully_unsent()` reads the `message_summary_info` plist (per-part edit history), *not*
  `date_retracted`. A message can have the column set without that plist, so ibex checks both.

Key tables:

| Table | Notes |
|---|---|
| `message` | `ROWID`, `guid`, `text`, `attributedBody` (typedstream blob; holds the text when `text` is NULL, common on iOS 16+), `handle_id`, `service` (`SMS`, `iMessage`, `RCS`), `date`, `date_read`, `date_delivered`, `is_from_me`, `cache_has_attachments`, `associated_message_guid` + `associated_message_type` (tapbacks: **1000, 2000–2007 add, 3000–3007 remove**; **0, 2 and 3 are ordinary messages**, and 4000 is a poll vote — see the 2026-11-08 note below), `thread_originator_guid` (inline replies), `date_edited`, `date_retracted`, `item_type`, `group_action_type`, `balloon_bundle_id` |
| `handle` | `ROWID`, `id` (phone number or email), `service` |
| `chat` | `ROWID`, `guid`, `chat_identifier`, `display_name`, `style` (43 = group, 45 = 1:1), `service_name` |
| `chat_message_join` | `chat_id`, `message_id`, `message_date` |
| `chat_handle_join` | `chat_id`, `handle_id` |
| `attachment` | `ROWID`, `guid`, `filename`, `mime_type`, `uti`, `transfer_name`, `total_bytes`, `created_date`, `is_sticker` |
| `message_attachment_join` | `message_id`, `attachment_id` |

Rules:
- Conversation = `chat`. Messages via `chat_message_join`, ordered by `message.date`.
- Skip tapback rows as standalone messages; attach them as reactions to their target
  (`associated_message_guid` looks like `p:0/<guid>` or `bp:<guid>`). **A non-zero
  `associated_message_type` does not mean tapback** — see the 2026-11-08 note.
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
| `ZWACHATSESSION` | `Z_PK`, `ZCONTACTJID` (`<number>@s.whatsapp.net`, groups `@g.us`), `ZPARTNERNAME`, `ZSESSIONTYPE` (0 = 1:1, 1 = group, 2 = broadcast, 3 = status), `ZLASTMESSAGEDATE`, `ZMESSAGECOUNTER` (**stale — do not trust; see the 2026-11-08 M4 note**), `ZARCHIVED` |
| `ZWAMESSAGE` | `Z_PK`, `ZCHATSESSION`, `ZISFROMME`, `ZFROMJID`, `ZTOJID`, `ZTEXT`, `ZMESSAGEDATE`, `ZSENTDATE`, `ZMESSAGETYPE`, `ZGROUPMEMBER`, `ZMEDIAITEM`, `ZPARENTMESSAGE`, `ZSTANZAID`, `ZMESSAGESTATUS`, `ZSTARRED` |
| `ZWAMEDIAITEM` | `Z_PK`, `ZMESSAGE`, `ZMEDIALOCALPATH`, `ZTHUMBNAILLOCALPATH`, `ZTITLE`, `ZFILESIZE`, `ZMOVIEDURATION`, `ZLATITUDE`, `ZLONGITUDE`, `ZVCARDNAME`, `ZVCARDSTRING` |
| `ZWAGROUPMEMBER` | `Z_PK`, `ZCHATSESSION`, `ZMEMBERJID`, `ZCONTACTNAME`, `ZFIRSTNAME` |
| `ZWAGROUPINFO` | group metadata |
| `ZWAMESSAGEINFO` | receipts, reactions (blob; parse later) |

`ZMESSAGETYPE` (**do not use this to decide the message kind** — see the 2026-11-08 note; kept as a guide to what the codes mean):

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

### 2026-11-08: `ZMESSAGECOUNTER` is stale (M4, found by exporting)

The chat list said a conversation held **4,493** messages; exporting it wrote **4,490**. The
count came from `ZWACHATSESSION.ZMESSAGECOUNTER`, which WhatsApp maintains itself. Checking it
against the rows actually present:

```sql
SELECT count(*) FROM ZWACHATSESSION s
  LEFT JOIN (SELECT ZCHATSESSION AS session, count(*) AS n
             FROM ZWAMESSAGE GROUP BY ZCHATSESSION) c ON c.session = s.Z_PK
 WHERE coalesce(c.n, 0) <> s.ZMESSAGECOUNTER;
```

**1,107 of 1,107** conversations disagreed — every single one. The counter appears to track
messages the app has seen over the chat's life, not rows in this database, so deletions,
expiring messages and partial syncs all push it out. ibex therefore derives the count from a
grouped join on `ZWAMESSAGE`, which costs one extra scan and makes the list agree with the
thread and the export. A real-data test asserts the reconciliation over 40 chats.

The general lesson for Core Data stores: **a stored aggregate is a cache, not a fact.** Prefer
counting the rows you are about to show.

### 2026-11-08: WhatsApp, measured on the real backup (M3)

`AppDomainGroup-group.net.whatsapp.WhatsApp.shared`, 34,019 manifest entries (22,359 files).
No `WhatsAppSMB` container, so Business support is detection-only and fixture-tested.

| Table | Rows |
|---|---|
| `ZWAMESSAGE` | 66,407 |
| `ZWAMEDIAITEM` | 41,182 (9,285 with a local path) |
| `ZWACHATSESSION` | 1,192 |
| `ZWAGROUPMEMBER` | 2,278 |
| `ZWAGROUPINFO` | 19 |
| `ZWAADDRESSBOOKCONTACT` (ContactsV2) | 1,521 |

Every column this document lists exists. Undocumented extras worth knowing:
`ZWACHATSESSION.ZLASTMESSAGETEXT` (a conversation preview with no extra query), `ZGROUPINFO`,
`ZREMOVED`, `ZHIDDEN`, `ZUNREADCOUNT`; `ZWAMESSAGE.ZPUSHNAME`, `ZSORT`, `ZMESSAGEINFO`;
`ZWAMEDIAITEM.ZMEDIAURL`, `ZMETADATA`, `ZXMPPTHUMBPATH`, `ZASPECTRATIO`;
`ZWAGROUPMEMBER.ZISADMIN`/`ZISACTIVE`; `ZWAADDRESSBOOKCONTACT.ZLID`/`ZBUSINESSNAME`. There is
**no `ZISDELETED`** column — a deleted message is `ZMESSAGETYPE` 10.

**Media paths take exactly one prefix.** `ZMEDIALOCALPATH` holds `Media/<jid>/<x>/<y>/<file>`
with **no** `Message/` prefix. Prepending `Message/` resolved **9,050 of 9,285 (97.5%)** against
the manifest; the raw value resolved **0%**. So this is one deterministic prefix, not the
candidate search suggested above. The remaining 2.5% are media never downloaded or since
cleared — absent, not an error.

**Paths use the newer `@lid` addressing**, not only `@s.whatsapp.net`, and group media uses
`<number>-<number>@g.us`. Contacts therefore have to be matched on `ZWHATSAPPID` *and* `ZLID`.

**A JID is a phone number**, so any error or log line carrying a WhatsApp relative path
discloses one — in tension with hard rule 3, which forbids numbers while permitting "paths
within the backup". `util::paths::redact_jid` masks the identifier while keeping the structure,
and is used wherever a path reaches a message or a log. The Raw browser still shows full paths,
since inspecting them is that view's purpose.

**Media formats present:** jpg 6,246, pdf 1,081, **opus 849**, mp4 670, webp 334 (stickers),
plus docx/pptx/xlsx/m4a/mov/mp3. Voice notes are `Ogg data, Opus audio, mono 16 kHz` — a real
Ogg container, so **no transcoding is needed**: WebKitGTK decodes them through GStreamer's
`oggdemux` + `opusdec`, both present with the full plugin set. The `symphonia`/`hound`/`ffmpeg`
option this document contemplated is not required.

#### Two classification traps, both found by real data

1. **`ZMESSAGETYPE` cannot decide the message kind.** The real database holds **38 distinct
   values**, roughly 25 of them undocumented, and — decisively — **27,813 of the 50,410 type-0
   ("text") messages carry a media item**, because a link preview is stored as a media item on an
   ordinary text message. So ibex infers the kind from the *resolved file* (extension, then
   sniffed content) and consults the type code only for what a file cannot reveal: 0 text,
   4 contact, 5 location, 6 system, 10 deleted, 15 sticker. Unknown codes become
   `MessageKind::Unknown(n)` and still render.
2. **`ZGROUPEVENTTYPE` is not a "this is a group event" flag.** Value **2 covers 35,467
   messages** — over half the database — of which **27,462 have ordinary text**. An early version
   of the extractor treated any non-zero value as a system message, which classified 62% of a
   sampled thread as grey system notices. Only `ZMESSAGETYPE` 6 marks a system message.

**Hidden sessions hold real conversations.** `ZHIDDEN` is set on 123 of the 1,107 one-to-one and
group sessions, and those 123 hold **283 messages between them, none empty**. Filtering them out
would silently lose real conversations, so ibex lists them; only `ZSESSIONTYPE` decides what is a
conversation (0 and 1) versus a feed (2 broadcast, 3 status/channel, and a 4 observed once).
Default listing: **1,107**; with feeds included: **1,192**.

**Reactions are not read.** They live in `ZWAMESSAGEINFO.ZRECEIPTINFO` as an undocumented
protobuf blob (60,112 rows). Guessing at its layout risks attributing the wrong reaction to the
wrong person, so ibex leaves `reactions` empty for WhatsApp and says so in the UI's absence
rather than inventing them.

### 2026-11-08: first *readable* real backup (iOS 27.2, iPhone19,7)

A second backup was taken with a known password, so everything below is measured against real
data rather than assumed. 45 GB, 70,534 files, **115,762 manifest entries across 1,891 domains**,
`sms.db` 87 MB with **17,588 message rows**.

**The MBFile epoch question is settled: Unix seconds, as implemented.** File dates across
`HomeDomain/Library` span 2023-04-11 to 2026-11-07 18:57:38, and that newest value matches the
backup's own timestamp. Read as Apple-epoch they would all have landed past 2050. The
`from_unix_secs` / `from_apple` split in `util::time` is correct.

**`fileID` = SHA-1 of `"<domain>-<relativePath>"` confirmed** by recomputing 437 sampled rows,
all matching.

**Decryption yields real content**, not plausible-looking garbage: 60 of 60 sampled decrypted
files in `Library/Preferences` sniff to a recognisable type.

**`associated_message_type` is not a simple "non-zero means tapback" flag.** This backup holds
**no tapbacks at all**, but 8 rows with code **2** and 8 with code **3** — ordinary messages that
are merely *associated* with another. The library classifies 0, 2 and 3 alike as normal; only
**1000** (sticker) and **2000–2007** / **3000–3007** are tapbacks, and **4000** is a poll vote.
ibex had been excluding every non-zero code, which silently dropped those 16 messages from their
threads. Fixed, with a fixture message carrying code 2 and a regression test.

**attributedBody decoding earns its keep.** 104 of 17,588 messages have `text` NULL with an
`attributedBody` blob, so they would render blank without decoding the typedstream. Measured
through the extractor on real data: 1,619 of 1,629 messages across the five chats richest in
such rows come back with body text (the remainder are legitimately attachment-only).

**Counts reconcile exactly:** 859 conversations reporting 17,449 messages between them, against
17,449 non-tapback rows joined to a chat. The other 139 message rows are joined to no chat at
all — orphans from deleted conversations — and are correctly invisible.

**Not exercised by this backup**, so still only fixture-tested:
- **Group chats.** All 859 chats are `style` 45 (one-to-one); there is not a single `style` 43.
- **Tapbacks**, per the above.
- `service` split is SMS 17,455 / iMessage 133, so iMessage-specific paths get little real
  coverage.

**iOS 27.2 `message` columns.** Every column ibex depends on is present, including
`attributedBody`, `associated_message_emoji`, `thread_originator_guid`/`_part`, `date_edited`,
**`date_retracted`** (which is why reading it directly, alongside the library's
`message_summary_info` check, is right), `cache_has_attachments`, `item_type`,
`group_action_type` and `message_summary_info`. New since the table above was written, and worth
knowing about for later milestones: `schedule_type`, `schedule_state`, `date_updated`,
`retry_count`, `is_time_sensitive`, `is_critical`, `is_sos`, `sent_or_received_off_grid`,
`is_pending_satellite_send`, `needs_relay`, `is_kt_verified`, `syndication_ranges`,
`date_recovered`, `part_count`, `index_state`, `filter_action`/`filter_sub_action`.

**Keybag**, unchanged from the first backup and exactly as this document describes: `VERS` 5,
`TYPE` 1, `ITER` 10 000, `DPWT` 1, `DPIC` 10 000 000, `DPSL`/`SALT` 20 bytes, 14 class entries,
44-byte `ManifestKey`. Unlocking the real backup takes **0.8 s** of derivation; opening it end to
end, including decrypting the 163 MB `Manifest.db` into the cache, takes about 18 s.


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
- `2026-11-07` M1: files in a backup have hashed, extensionless names, so content type has to be
  sniffed from the bytes. Order used by `util::mime`: the plist magics first (`bplist`, and an XML
  declaration or doctype mentioning `plist` — `infer` knows neither, and plists are everywhere in a
  backup), then `infer`'s magic numbers, then `mime_guess` on the *manifest* name's extension, then
  a UTF-8 text heuristic.
- `2026-11-07` M1: WebKitGTK cannot decode HEIC/HEIF, so those sniff to a distinct
  `PreviewKind::ImageUnsupported` and the UI explains itself instead of showing a broken image.
  Same for TIFF and camera raw. M5's thumbnail service (libheif) is what fixes it.

### 2026-11-07: first real backup inspected (encrypted)

A real backup became available: iPhone19,7 on iOS 27.2 (build 24B119), 45 GB, 71,780 files,
a 164 MB `Manifest.db`, 1809 installed apps. `Manifest.plist` → `IsEncrypted = true`, so
**nothing beyond the metadata plists is readable until M7 lands**. Confirmed structurally:

- `Manifest.plist` → `Version` `10.0`, `SystemDomainsVersion` `24.0`, `WasPasscodeSet` true.
- `ManifestKey` is **44 bytes** = 4-byte little-endian protection class + 40-byte wrapped key,
  exactly as the scheme above describes.
- `BackupKeyBag` is 1712 bytes and parses with the documented TLV layout, no deviations:

  | Item | Value |
  |---|---|
  | `VERS` | 5 |
  | `TYPE` | 1 (backup keybag) |
  | `HMCK` | 40 bytes |
  | `WRAP` (header) | 0 |
  | `SALT` | 20 bytes |
  | `ITER` | 10 000 (PBKDF2-HMAC-SHA1) |
  | `DPWT` | 1 |
  | `DPIC` | **10 000 000** (PBKDF2-HMAC-SHA256) |
  | `DPSL` | 20 bytes |

- 14 per-class entries, for classes 1–11 and 17–19. Every one has `KTYP` 0 (AES) and a
  40-byte `WPKY` (32-byte key plus the 8 bytes AES key wrap adds). `WRAP` is 2 or 3, so
  `WRAP & 2` holds throughout and every class key is wrapped with the passcode key.

Two consequences for M7:

1. `DPIC` is ten million SHA-256 iterations. Derivation is deliberately expensive and will
   take seconds even in release mode, so it must run off the UI thread with a progress or at
   least a busy state, and a wrong password costs that same wait before it can be reported.
   Worth measuring and recording the real number.
2. `Manifest.plist` → `Lockdown` carries `ProductType`, `ProductVersion`, `BuildVersion` and
   `DeviceClass` in the clear, and `Info.plist` is not encrypted either. The password prompt
   can therefore name the device it is unlocking. `Backup::open` currently reads `BackupInfo`
   and then discards it when it returns `PasswordRequired`; M7 should hand that info back
   (a `Backup::peek(path) -> BackupInfo`, or carry it on the error) so the modal can show it.
