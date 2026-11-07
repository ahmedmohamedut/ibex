#!/usr/bin/env bash
# Full check sequence for ibex. A task is not done until this is green.
# See AGENTS.md.
set -euo pipefail

cd "$(dirname "$0")/.."

step() { printf '\n\033[1m==> %s\033[0m\n' "$1"; }

step "cargo fmt --all --check"
cargo fmt --all --check

step "cargo clippy --workspace --all-targets -- -D warnings"
cargo clippy --workspace --all-targets -- -D warnings

step "cargo test --workspace"
cargo test --workspace

step "npm run build"
if [ ! -d node_modules ]; then
  echo "node_modules missing; running npm install" >&2
  npm install
fi
npm run build

printf '\n\033[1;32mAll checks passed.\033[0m\n'
