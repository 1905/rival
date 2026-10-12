#!/usr/bin/env bash
# Builds and publishes a rival release from a local macOS host (rival has no
# hosted CI). The darwin targets need the Apple SDK, so this runs on macOS
# only. It installs pinned tools under $RIVAL_RELEASE_TOOLS and runs the
# same GoReleaser command for every mode.
#
#   scripts/release-local.sh setup     # pinned tools (once per host)
#   scripts/release-local.sh check     # goreleaser config check
#   scripts/release-local.sh snapshot  # six archives + formula; publishes nothing
#   scripts/release-local.sh release   # publishes the release + brew formula
#
# `release` needs a clean checkout at an exact v* tag and a token of the
# `1905` account in GITHUB_TOKEN (or one line on stdin). HOMEBREW_TAP_TOKEN
# defaults to the same token.
set -euo pipefail

# Pinned release tools.
RUST_TOOLCHAIN=1.98
ZIG_VERSION=0.16.0
CARGO_ZIGBUILD_VERSION=0.23.4
CARGO_XWIN_VERSION=0.23.1
GORELEASER_VERSION=2.18.2

TOOLS="${RIVAL_RELEASE_TOOLS:-$HOME/tmp-rival-release/tools}"
MODE="${1:-}"

[ "$(uname -s)" = Darwin ] || { echo "error: run on macOS (the darwin targets need the Apple SDK)" >&2; exit 1; }
[ "$(uname -m)" = arm64 ] || { echo "error: the pinned Zig and GoReleaser downloads are arm64 builds" >&2; exit 1; }
export PATH="$TOOLS/bin:$TOOLS/zig:$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"

setup() {
  mkdir -p "$TOOLS/bin"
  if [ "$("$TOOLS/zig/zig" version 2>/dev/null)" != "$ZIG_VERSION" ]; then
    rm -rf "$TOOLS/zig" && mkdir -p "$TOOLS/zig"
    curl -fsSL "https://ziglang.org/download/$ZIG_VERSION/zig-aarch64-macos-$ZIG_VERSION.tar.xz" |
      tar -xJ -C "$TOOLS/zig" --strip-components 1
  fi
  if ! "$TOOLS/bin/goreleaser" --version 2>/dev/null | grep -q "GitVersion:  *$GORELEASER_VERSION$"; then
    curl -fsSL "https://github.com/goreleaser/goreleaser/releases/download/v$GORELEASER_VERSION/goreleaser_Darwin_arm64.tar.gz" |
      tar -xz -C "$TOOLS/bin" goreleaser
  fi
  rustup toolchain install "$RUST_TOOLCHAIN" --profile minimal
  rustup target add --toolchain "$RUST_TOOLCHAIN" \
    x86_64-apple-darwin aarch64-apple-darwin \
    x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu \
    x86_64-pc-windows-msvc aarch64-pc-windows-msvc
  cargo "+$RUST_TOOLCHAIN" install --locked --root "$TOOLS" \
    "cargo-zigbuild@$CARGO_ZIGBUILD_VERSION" "cargo-xwin@$CARGO_XWIN_VERSION"
  # cargo-xwin runs in clang mode and needs clang, llvm-lib and lld-link.
  brew install llvm lld
}

# The environment .goreleaser.yaml reads. SDKROOT is set per darwin target
# by the config, never for the whole run.
build_env() {
  export RUSTUP_TOOLCHAIN="$RUST_TOOLCHAIN"
  export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
  RIVAL_LLVM_BIN="$(brew --prefix llvm)/bin:$(brew --prefix lld)/bin"
  RIVAL_APPLE_SDKROOT="$(xcrun --sdk macosx --show-sdk-path)"
  export RIVAL_LLVM_BIN RIVAL_APPLE_SDKROOT
  unset SDKROOT
  cd "$(dirname "$0")/.."
  rustc --version
  zig version
  cargo-zigbuild --version
  cargo-xwin --version
  goreleaser --version | grep GitVersion
}

case "$MODE" in
  setup)
    setup ;;
  check)
    build_env
    python3 scripts/check_goreleaser.py ;;
  snapshot)
    build_env
    HOMEBREW_TAP_TOKEN="" goreleaser release --snapshot --clean --parallelism 1
    python3 scripts/check_release_archives.py dist --formula --run ;;
  release)
    build_env
    if [ -n "$(git status --porcelain)" ]; then
      echo "error: the checkout has changes; release from a clean checkout of the tag" >&2
      exit 1
    fi
    TAG="$(git describe --tags --exact-match)" || { echo "error: HEAD is not at a tag" >&2; exit 1; }
    case "$TAG" in v*) ;; *) echo "error: $TAG is not a v* tag" >&2; exit 1 ;; esac
    if [ -z "${GITHUB_TOKEN:-}" ]; then
      IFS= read -r GITHUB_TOKEN
    fi
    export GITHUB_TOKEN
    export HOMEBREW_TAP_TOKEN="${HOMEBREW_TAP_TOKEN:-$GITHUB_TOKEN}"
    goreleaser release --clean --parallelism 1 ;;
  *)
    echo "usage: $0 setup|check|snapshot|release" >&2
    exit 2 ;;
esac
echo "RELEASE_LOCAL_OK $MODE"
