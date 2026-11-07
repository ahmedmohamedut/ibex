# ibex: iOS Backup Explorer

A desktop app for browsing and exporting data from iPhone backups, built with Rust and Tauri. Linux first.

ibex reads the backup folder produced by `idevicebackup2` (libimobiledevice) or Finder/iTunes, resolves the hashed file layout through `Manifest.db`, and presents the data as conversations, galleries and file trees instead of SQLite tables.

> **Status:** early development. See [ROADMAP.md](ROADMAP.md).

## Features (planned)

- **Messages:** SMS, MMS and iMessage threads with attachments, contact names, reactions
- **WhatsApp:** chats, groups, media, voice notes
- **Photos:** camera roll gallery with HEIC support, dates and locations
- **Voice Memos:** list, playback, export with titles
- **Documents:** Files app storage and per-app Documents folders
- **Raw browser:** every domain and file in the backup, with preview and hex view
- **Export:** HTML (self-contained, readable offline), CSV, JSON, or original files with original names and timestamps
- **Encrypted backups:** decrypt with the backup password
- **Search:** across SMS and WhatsApp

## Making a backup on Linux

```bash
# pair the phone first if needed
idevicepair pair
# full backup into ./backups/<UDID>/
idevicebackup2 backup --full ./backups
```

Open the `<UDID>` folder in ibex. It's the one that contains `Manifest.db`.

## Development

Setup steps are in [SETUP.md](SETUP.md).

```bash
npm install
npm run tauri dev          # run the app
cargo test --workspace     # Rust tests
cargo run -p ibex-cli -- info /path/to/backup   # debug CLI
```

## Project layout

```
crates/ibex-core/   backup parsing, extractors, exporters (no Tauri dependency)
crates/ibex-cli/    command-line tool for debugging the core
src-tauri/          Tauri app: commands, media protocol, jobs
src/                React + TypeScript UI
docs/               architecture, data source reference, UI design, prompts
```

## Documentation

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): layers, modules, IPC contract
- [docs/DATA_SOURCES.md](docs/DATA_SOURCES.md): where each data type lives in a backup, and its schema
- [docs/UI_DESIGN.md](docs/UI_DESIGN.md): screens and interaction design
- [ROADMAP.md](ROADMAP.md): milestones
- [AGENTS.md](AGENTS.md): rules for coding agents

## Privacy

ibex is offline only. It makes no network requests, never writes to the backup folder, and keeps its cache (working copies of databases, thumbnails, decrypted files) under `~/.local/share/ibex/`. The cache can be wiped from the app. Don't commit backup data or exports to this repository.

## License

To be decided. If SMS parsing uses the `imessage-database` crate (GPL-3.0), the app must be released under a GPL-3.0-compatible license.
