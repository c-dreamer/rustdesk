#!/bin/bash
# SessionStart hook for Claude Code cloud sessions.
#
# Goal: make `cargo fmt --check`, `cargo clippy` / `cargo check` and unit tests runnable on the
# Rust workspace. Synchronous, idempotent, non-interactive, and a no-op outside a cloud session.
#
# What it does and why (measured on ubuntu-24.04, 4 cores, 2026-10-04; ~9 min cold, ~5 s warm):
#   1. `libs/hbb_common` is a git submodule that the cloud clone does not check out; nothing in
#      the workspace builds without it.
#   2. apt: the native dev packages from Dockerfile / .github/workflows/ci.yml, plus the distro
#      codec packages (libvpx, libaom, libyuv, opus) that stand in for the vcpkg ports.
#   3. `cargo fetch` of every crate in Cargo.lock.
#   4. `cargo check` of `hbb_common` and `base`: they need no native libraries (their build.rs
#      uses pure-Rust protobuf codegen, so no protoc) and their clippy/tests are cheap after this.
#      The `hbb_common` unit-test binary is built too, so `cargo test -p hbb_common --lib` is
#      instant in the session.
#   5. Best effort: `cargo check --features linux-pkg-config` of the whole root crate.
#      Upstream builds `scrap` and `magnum-opus` against vcpkg (README.md, ci.yml); vcpkg is
#      deliberately NOT installed here (bootstrapping it and building its ports does not fit this
#      hook's budget). With the non-default `linux-pkg-config` feature both crates take the same
#      headers from pkg-config instead, which the apt packages above provide. Ubuntu's libyuv-dev
#      ships no .pc file, so a minimal one is written to /usr/local/lib/pkgconfig. A failure in
#      this step is reported but does not fail the hook; steps 1-4 are the guaranteed baseline.
#
# Limits: a default-feature `cargo check`/`cargo build`/`cargo test --workspace` of the root crate
# still needs VCPKG_ROOT and is not possible here; always pass `--features linux-pkg-config` for
# anything that pulls in `scrap` or the root crate. `cargo clippy --features linux-pkg-config` on
# the root crate currently fails on pre-existing deny-by-default lints in the code itself
# (libs/enigo/src/dsl.rs, libs/scrap/src/x11/server.rs, src/platform/gtk_sudo.rs), not on the
# environment; `cargo clippy -p hbb_common -p base` is clean. The Flutter UI in flutter/ is out
# of scope.
set -euo pipefail

if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

ROOT="${CLAUDE_PROJECT_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
cd "$ROOT"

log() { printf '[session-start] %s\n' "$*"; }
started=$(date +%s)
sudo=""
if [ "$(id -u)" -ne 0 ]; then sudo="sudo -n"; fi

# 1. Submodule.
if [ ! -f libs/hbb_common/Cargo.toml ]; then
  log "initializing git submodules"
  git submodule update --init --recursive
fi

# 2. Native build inputs.
packages=(
  gcc g++ clang libclang-dev pkg-config nasm yasm
  libgtk-3-dev libasound2-dev libpulse-dev libdbus-1-dev
  libxcb-randr0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxdo-dev libxfixes-dev libxtst-dev
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev
  libunwind-dev libva-dev libvdpau-dev
  libvpx-dev libaom-dev libyuv-dev libopus-dev
)
missing=()
for p in "${packages[@]}"; do
  if ! dpkg-query -W -f='${Status}' "$p" 2>/dev/null | grep -q 'install ok installed'; then
    missing+=("$p")
  fi
done
if [ "${#missing[@]}" -gt 0 ]; then
  log "installing apt packages: ${missing[*]}"
  export DEBIAN_FRONTEND=noninteractive
  $sudo apt-get update -qq
  $sudo apt-get install -y -qq --no-install-recommends "${missing[@]}"
fi
if ! pkg-config --exists libyuv; then
  log "writing /usr/local/lib/pkgconfig/libyuv.pc (libyuv-dev ships none)"
  $sudo mkdir -p /usr/local/lib/pkgconfig
  $sudo tee /usr/local/lib/pkgconfig/libyuv.pc > /dev/null << 'PC'
prefix=/usr
libdir=${prefix}/lib/x86_64-linux-gnu
includedir=${prefix}/include

Name: libyuv
Description: YUV scaling and conversion library (written by .claude/hooks/session-start.sh)
Version: 0
Libs: -L${libdir} -lyuv
Cflags: -I${includedir}
PC
fi

# 3. Crate sources. Cargo.lock is authoritative; .cargo/config.toml already fetches git with the CLI.
log "cargo fetch"
cargo fetch --locked

# 4. Guaranteed baseline: hbb_common + base, and the hbb_common unit-test binary.
log "cargo check -p hbb_common -p base"
cargo check --locked -p hbb_common -p base
log "cargo test -p hbb_common --lib --no-run"
cargo test --locked -p hbb_common --lib --no-run

# 5. Best effort: the whole root crate without vcpkg.
log "cargo check --features linux-pkg-config (best effort)"
if ! cargo check --locked --features linux-pkg-config; then
  log "WARNING: root crate check failed; hbb_common and base are still usable (see comment at top)"
fi

if [ -n "${CLAUDE_ENV_FILE:-}" ]; then
  line='export DEBIAN_FRONTEND=noninteractive'
  grep -qxF "$line" "$CLAUDE_ENV_FILE" 2>/dev/null || echo "$line" >> "$CLAUDE_ENV_FILE"
fi

log "done in $(( $(date +%s) - started ))s"
