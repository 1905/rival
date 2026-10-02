#!/usr/bin/env bash
# Bump version in all rival skill SKILL.md files (Go and Rust embedded trees).
# Usage: ./scripts/bump-skill-versions.sh 3.7.0

set -euo pipefail
# Byte edits need no locale; an unsupported one makes perl warn per file.
export LC_ALL=C

VERSION="${1:?Usage: $0 <version>}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Source of truth: the embedded skills compiled into the binary. The repo-root
# .claude/skills/ copies were removed — `rival install` copies these out to
# Claude skills and renders Codex skills on install/update. Both inherit this
# version, so there is no second version list to keep in sync.
# Deprecated skill directories are skipped because they are removed on install.
# Derived from the Names lists rather than repeated here: a static copy
# silently skips a newly added skill, and a skill whose version never moves is
# never reinstalled, because `rival install` skips files whose versions match.
#
# Until the Go tree is removed, the Go binary embeds rival/internal/skills and
# the Rust binary embeds crates/rival-core/skills, a byte-for-byte copy. Both
# trees are bumped. Each list is read from its own source (embed.go Names,
# skills.rs NAMES), and the two must match.
GO_SKILLS_DIR="$ROOT/rival/internal/skills"
RUST_SKILLS_DIR="$ROOT/crates/rival-core/skills"
GO_NAMES_FILE="$GO_SKILLS_DIR/embed.go"
RUST_NAMES_FILE="$ROOT/crates/rival-core/src/skills.rs"

go_names() {
	sed -n 's/^var Names = \[\]string{\(.*\)}$/\1/p' "$GO_NAMES_FILE" |
		tr ',' '\n' | tr -d ' "'
}

# rustfmt keeps the array one name per line: `pub const NAMES: [&str; N] = [`,
# then `    "rival-x",` lines, then `];`.
rust_names() {
	sed -n '/^pub const NAMES: \[&str; [0-9]*\] = \[$/,/^\];$/p' "$RUST_NAMES_FILE" |
		sed -n 's/^ *"\([^"]*\)",$/\1/p'
}

GO_NAMES="$(go_names)" || GO_NAMES=""
if [ -z "$GO_NAMES" ]; then
	echo "error: could not read skill names from $GO_NAMES_FILE" >&2
	exit 1
fi
RUST_NAMES="$(rust_names)" || RUST_NAMES=""
if [ -z "$RUST_NAMES" ]; then
	echo "error: could not read skill names from $RUST_NAMES_FILE" >&2
	exit 1
fi
if [ "$GO_NAMES" != "$RUST_NAMES" ]; then
	echo "error: skill names differ between $GO_NAMES_FILE and $RUST_NAMES_FILE" >&2
	echo "  Go:   $(tr '\n' ' ' <<<"$GO_NAMES")" >&2
	echo "  Rust: $(tr '\n' ' ' <<<"$RUST_NAMES")" >&2
	exit 1
fi

FILES=()
while IFS= read -r name; do
	FILES+=("$GO_SKILLS_DIR/$name/SKILL.md" "$RUST_SKILLS_DIR/$name/SKILL.md")
done <<<"$GO_NAMES"

missing=0
for file in "${FILES[@]}"; do
  if [[ -f "$file" ]]; then
    # perl -pi edits in place on both BSD and GNU systems; the version is
    # passed as data, never as part of the expression.
    VERSION="$VERSION" perl -pi -e 's/^version: .*/version: $ENV{VERSION}/' "$file"
    echo "  ✓ $file → $VERSION"
  else
    echo "  ✗ $file not found"
    missing=1
  fi
done

if [ "$missing" -ne 0 ]; then
	echo "error: some skill files were not found" >&2
	exit 1
fi
echo "Done."
