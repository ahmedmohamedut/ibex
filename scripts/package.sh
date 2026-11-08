#!/usr/bin/env bash
# Build the Linux packages: ibex_<v>_amd64.deb and ibex-<v>-1.x86_64.rpm.
#
# Pass bundle targets as arguments to narrow the build, e.g. `package.sh deb`.
# Defaults to both.
set -euo pipefail

cd "$(dirname "$0")/.."

# The rpm bundler copies each file's mode from disk, unlike the deb bundler,
# which normalizes to 755/644. A developer umask of 002 therefore ships a
# group-writable /usr/bin/ibex in the rpm only. Forcing the umask here keeps
# the two packages identical in everything but their metadata.
umask 022

bundles="${*:-deb,rpm}"
bundles="${bundles// /,}"

npx tauri build --bundles "$bundles"

printf '\n\033[1m==> packages in target/release/bundle\033[0m\n'
find target/release/bundle -maxdepth 2 -type f \( -name '*.deb' -o -name '*.rpm' \) \
  -printf '%10s  %p\n' | sort -k2
