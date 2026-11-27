# Building ibex from source

## 1. System dependencies (Tauri 2 on Linux)

**Debian / Ubuntu**
```bash
sudo apt update
sudo apt install -y build-essential curl wget file pkg-config \
  libwebkit2gtk-4.1-dev libxdo-dev libssl-dev \
  libayatana-appindicator3-dev librsvg2-dev libheif-dev
```

**Fedora**
```bash
sudo dnf install -y webkit2gtk4.1-devel openssl-devel curl wget file \
  libappindicator-gtk3-devel librsvg2-devel libxdo-devel libheif-devel
sudo dnf group install -y "c-development"
```

**Arch**
```bash
sudo pacman -Syu --needed webkit2gtk-4.1 base-devel curl wget file openssl \
  appmenu-gtk-module libappindicator-gtk3 librsvg xdotool libheif
```

`libheif` 1.17 or newer is required for HEIC images. SQLite is bundled, so no system SQLite is needed; the `sqlite3` CLI is still handy for inspecting schemas.

## 2. Toolchains

- **Rust** (stable, edition 2024 support: 1.85 or newer) via [rustup](https://rustup.rs), with `clippy` and `rustfmt`
- **Node.js** LTS with npm

Check: `rustc --version`, `cargo --version`, `node --version`, `npm --version`.

## 3. Build and run

```bash
npm install
npm run tauri dev               # run the app with hot reload
cargo run -p ibex-cli -- --help # the command-line tool
```

## 4. Checks

A change is not done until the full check sequence passes:

```bash
scripts/check.sh   # cargo fmt --check, clippy -D warnings, cargo test, npm run build
```

TypeScript types in `src/bindings/` are generated from the Rust DTOs by `ts-rs` when the tests run. Don't edit them by hand.

### Testing against a real backup

Unit tests use a synthetic backup builder, so no real data is needed. To also run the integration tests in `crates/ibex-core/tests/real_backup.rs` against your own backup, set (and never commit):

```bash
export IBEX_TEST_BACKUP="/path/to/backup/<UDID>"   # folder containing Manifest.db
export IBEX_TEST_PASSWORD="..."                    # only for an encrypted backup
cargo test --workspace
```

These tests assert counts and structure, never content.

To exercise the CLI without a real backup, write a synthetic one:

```bash
cargo run -p ibex-cli --features fixture -- fixture --out /tmp/ibex-fixture
```

## 5. Packaging

```bash
scripts/package.sh          # .deb and .rpm
scripts/package.sh deb      # just one
```

Packages land in `target/release/bundle/`.
