#!/usr/bin/env bash
# The pre-merge gate, run locally (rival has no hosted CI). Linux or macOS.
# Every step must pass before a merge or a release tag.
#
#   scripts/check.sh            # all steps
#   SKIP_E2E=1 scripts/check.sh # without the e2e scenarios
#
# Nothing here calls a real reviewer CLI, Docker, a provider API or
# inference: every test isolates HOME / RIVAL_HOME itself. Rival.app
# (`swift build --product RivalApp`, `swift test`) needs a macOS host with
# Xcode and is not part of this script.
set -euo pipefail
cd "$(dirname "$0")/.."

step() { printf '\n== %s\n' "$*"; }

step "cargo fmt"
cargo fmt --all --check

step "cargo clippy"
cargo clippy --workspace --all-targets --locked -- -D warnings

# Includes crates/rival-core/tests/contract.rs, which compares fresh writer
# bytes with the committed testdata/written golden files.
step "cargo test"
cargo test --workspace --locked --no-fail-fast

# The fd-state loader constructor must survive the final link in both the
# debug and the LTO release binary.
step "startup fd constructor (debug + LTO release)"
python3 scripts/check-startup-fds.py

# PyYAML comes from scripts/requirements-test.txt, in the ignored .venv.
step "release script tests"
VENV=.venv
[ -x "$VENV/bin/python3" ] || python3 -m venv "$VENV"
"$VENV/bin/python3" -m pip install -q -r scripts/requirements-test.txt
"$VENV/bin/python3" -m unittest discover -s scripts -p 'test_*.py'

step "fixtures unchanged"
git status --porcelain -- testdata/
test -z "$(git status --porcelain -- testdata/)"

# rival is Rust-only. Allowed: release history, plans, captured review text
# in fixtures, the command reference, and the retired skill names that
# install must keep deleting.
step "no Go, STE or antislop names"
! git grep -n -I -E '\bGo\b|gostd|gojson|winpath|\bgo_[a-z]|[a-z]_go\b|_go_|\bGO_|\bste_|ste100|antislop' -- . \
  ':!plans' ':!CHANGELOG.md' ':!README.md' ':!docs/command-reference.md' ':!scripts/check.sh' \
  ':!crates/rival-core/data/lang' ':!testdata/lang' \
  ':!testdata/logs/consilium_echoed_schema.log' \
  ':!crates/rival-core/src/review/testdata/consilium_echoed_schema.log' \
  ':!app/Tests/Fixtures' \
  ':!crates/rival-core/src/skills.rs' ':!crates/rival-core/src/skills/tests.rs' \
  ':!crates/rival-core/src/session/tests.rs'

step "e2e runner self-tests"
python3 -m unittest discover -s e2e -p 'test_*.py'

if [ -n "${SKIP_E2E:-}" ]; then
  step "e2e scenarios: SKIPPED (SKIP_E2E is set)"
else
  step "e2e scenarios"
  cargo build -p rival --locked
  TARGET_DIR="$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
  python3 e2e/run.py --bin "$TARGET_DIR/debug/rival"
fi

printf '\nCHECK OK\n'
