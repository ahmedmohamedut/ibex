# ibex: iOS Backup Explorer

A desktop app for browsing and exporting data from iPhone backups, built with Rust and Tauri. Linux first.

ibex reads the backup folder produced by Finder/iTunes or `idevicebackup2` (libimobiledevice), resolves the hashed file layout through `Manifest.db`, and presents the data as conversations, galleries and file trees instead of SQLite tables.

> **Status:** 0.1.0, the first public release. It has been tested against synthetic fixtures and one real iOS backup; expect rough edges on other iOS versions and app versions. See [ROADMAP.md](ROADMAP.md) and the [known limitations](#known-limitations).

## Features

- **Messages:** SMS, MMS and iMessage threads with attachments, contact names, tapback reactions, replies, edited and unsent markers
- **WhatsApp:** chats and groups (WhatsApp and WhatsApp Business), media, locations, contact cards, voice notes
- **Photos:** camera roll grid with month headers, albums, Live Photos, HEIC in the lightbox, recently deleted and hidden
- **Voice Memos:** list, playback, export with their titles as file names
- **Files:** "On My iPhone" storage and per-app Documents folders, with previews
- **Contacts and Call History:** browse the address book, export to vCard; calls with FaceTime separated from cellular
- **Search:** full-text search across SMS and WhatsApp (Ctrl+K), jumping to the message
- **Raw browser:** every domain and file in the backup, with image, text, plist, PDF, audio/video and hex previews
- **Export:** HTML (self-contained, no JavaScript, readable offline), CSV, JSON, or original files with original names and timestamps
- **Encrypted backups:** decrypt with the backup password

## Install

Download the `.deb` or `.rpm` from the [releases page](https://github.com/ahmedmohamedut/ibex/releases).

```bash
# Debian / Ubuntu
sudo apt install ./ibex_0.1.0_amd64.deb

# Fedora
sudo dnf install ./ibex-0.1.0-1.x86_64.rpm
```

Runtime dependencies are WebKitGTK 4.1, GTK 3 and libheif 1.17+, which the package manager pulls in. The packages are built on Ubuntu 24.04 and need a distribution at least that new (Debian 13, Fedora 40, or later); on older systems, [build from source](BUILDING.md).

To build from source instead, see [BUILDING.md](BUILDING.md).

## Making a backup on Linux

```bash
# pair the phone first if needed
idevicepair pair
# full backup into ./backups/<UDID>/
idevicebackup2 backup --full ./backups
```

Open the `<UDID>` folder in ibex. It's the one that contains `Manifest.db`.

A backup made by Finder (macOS) or iTunes (Windows) works too: copy the `<UDID>` folder from `~/Library/Application Support/MobileSync/Backup/` or `%APPDATA%\Apple Computer\MobileSync\Backup\`.

## Command-line tool

`ibex-cli` exposes the same core library for scripting and debugging. It prints counts and IDs by default; names, text and numbers only appear when you pass the flag that asks for them.

```bash
cargo run -p ibex-cli -- info /path/to/backup
cargo run -p ibex-cli -- sms convs /path/to/backup
cargo run -p ibex-cli -- export sms all --format html --out ./out /path/to/backup
cargo run -p ibex-cli -- --help
```

For an encrypted backup, pass `--password`. The password is read from `IBEX_PASSWORD` or prompted for; it is never accepted as an argument.

## Privacy

ibex is offline only. It makes no network requests, never writes to the backup folder, and keeps its cache (working copies of databases, decrypted files, the search index) under `~/.local/share/ibex/`. The cache can be wiped from the app. The backup password is held in memory only while the backup is open and is never stored.

## Known limitations

- Linux only for now. macOS and Windows builds are on the roadmap.
- Apple Notes is not supported yet: its storage format is an undocumented protobuf.
- Photos: no rubber-band selection, and HEIC files are exported as-is rather than converted to JPEG.
- WhatsApp reactions are not shown.

## Project layout

```
crates/ibex-core/   backup parsing, extractors, exporters (no Tauri dependency)
crates/ibex-cli/    command-line tool over the core
src-tauri/          Tauri app: commands, media protocol, jobs
src/                React + TypeScript UI
docs/               architecture, data source reference, UI design
```

## Documentation

- [BUILDING.md](BUILDING.md): building from source and running the checks
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): layers, modules, IPC contract
- [docs/DATA_SOURCES.md](docs/DATA_SOURCES.md): where each data type lives in a backup, and its schema
- [docs/UI_DESIGN.md](docs/UI_DESIGN.md): screens and interaction design
- [ROADMAP.md](ROADMAP.md): milestones
- [AGENTS.md](AGENTS.md): rules for contributors and coding agents (hard rules apply to humans too)

## Contributing

Issues and pull requests are welcome. Please read the hard rules in [AGENTS.md](AGENTS.md) first, in particular: **never attach real backup data, exported content, or screenshots containing personal messages to an issue or PR.** Report schema problems with structure only (`PRAGMA table_info`, row counts, `typeof()`), and run `scripts/check.sh` before opening a PR.

## License

ibex is licensed under the [GNU General Public License v3.0 or later](LICENSE).

SMS and iMessage parsing uses the [`imessage-database`](https://crates.io/crates/imessage-database) crate, which is GPL-3.0; that is why ibex is GPL. The reasoning is recorded in [docs/DATA_SOURCES.md](docs/DATA_SOURCES.md).
