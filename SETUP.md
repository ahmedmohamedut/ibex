# Setup: scaffolding the ibex project

Follow these steps once. After step 8, hand the project to your coding agent using the prompts in `docs/PROMPTS.md`.

> **Name:** the project is called **ibex** (iOS Backup EXplorer). To rename, do it now: search-and-replace `ibex` in these docs before scaffolding.

## 1. System dependencies (Tauri 2 on Linux)

**Debian / Ubuntu**
```bash
sudo apt update
sudo apt install -y build-essential curl wget file pkg-config \
  libwebkit2gtk-4.1-dev libxdo-dev libssl-dev \
  libayatana-appindicator3-dev librsvg2-dev \
  libheif-dev sqlite3
```

**Fedora**
```bash
sudo dnf check-update
sudo dnf install -y webkit2gtk4.1-devel openssl-devel curl wget file \
  libappindicator-gtk3-devel librsvg2-devel libxdo-devel libheif-devel sqlite
sudo dnf group install -y "c-development"
```

**Arch**
```bash
sudo pacman -Syu --needed webkit2gtk-4.1 base-devel curl wget file openssl \
  appmenu-gtk-module libappindicator-gtk3 librsvg xdotool libheif sqlite
```

`libheif` is needed later for HEIC thumbnails (milestone M5). `sqlite3` is the CLI, handy for inspecting schemas.

## 2. Toolchains

```bash
# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup component add clippy rustfmt

# Node LTS (via fnm; nvm works too)
curl -fsSL https://fnm.vercel.app/install | bash
fnm install --lts && fnm use lts-latest
```

Check: `rustc --version`, `cargo --version`, `node --version`, `npm --version`.

## 3. Create the Tauri app

```bash
npm create tauri-app@latest ibex
```

Answer the prompts:

| Prompt | Answer |
|---|---|
| Identifier | `dev.ibex.app` (or your own reverse-domain id) |
| Frontend language | TypeScript / JavaScript |
| Package manager | npm |
| UI template | React |
| UI flavor | TypeScript |

```bash
cd ibex
npm install
```

You now have `src/` (React frontend) and `src-tauri/` (Rust app).

## 4. Turn the repo into a Cargo workspace with a core crate

```bash
cargo new --lib crates/ibex-core
cargo new crates/ibex-cli
```

Create `Cargo.toml` at the **repo root**:

```toml
[workspace]
resolver = "2"
members = ["src-tauri", "crates/ibex-core", "crates/ibex-cli"]

[workspace.package]
edition = "2024"
version = "0.1.0"

[workspace.dependencies]
ibex-core = { path = "crates/ibex-core" }
```

In `src-tauri/Cargo.toml`, add under `[dependencies]`:
```toml
ibex-core = { workspace = true }
```

In `crates/ibex-cli/Cargo.toml`, add:
```toml
ibex-core = { workspace = true }
```

Build once to make sure the workspace resolves:
```bash
cargo build
```

## 5. Add Tauri plugins

```bash
npm run tauri add dialog     # folder/file pickers
```

## 6. Drop in the docs

Copy these files into the repo root, keeping the folder layout:

```
AGENTS.md
CLAUDE.md
README.md        (replace the generated one)
ROADMAP.md
SETUP.md
docs/ARCHITECTURE.md
docs/DATA_SOURCES.md
docs/UI_DESIGN.md
docs/PROMPTS.md
```

## 7. Git and privacy guardrails

Append to `.gitignore`:

```gitignore
# Never commit real backup data or extracted content
/backups/
/exports/
*.sqlite
*.sqlite-wal
*.sqlite-shm
*.db
*.db-wal
*.db-shm
Manifest.plist
Status.plist
Info.plist
!src-tauri/**/Info.plist
.env
```

Then:
```bash
git init
git add -A
git commit -m "chore: scaffold ibex (Tauri 2 + React + core workspace)"
```

## 8. Verify and point to your backup

```bash
npm run tauri dev
```

A window with the Tauri starter page should open. Close it.

For local-only integration tests against your real backup, set an environment variable (never commit it):

```bash
echo 'export IBEX_TEST_BACKUP="/path/to/backup/<UDID>"' >> ~/.bashrc
```

The path should be the folder that contains `Manifest.db`, `Manifest.plist`, `Info.plist` and the two-character hex subfolders.

## 9. Start building

Open `docs/PROMPTS.md` and paste the **Kickoff prompt** into your agent, then work through milestone prompts in order.
