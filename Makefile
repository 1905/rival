# Developer shortcuts. Rival.app targets (logic in app/scripts/*.py), then
# the cli-* targets for the Rust CLI.
.PHONY: run install test soak cli-build cli-install cli-test cli-release-check try-proxy try-proxy-tui try-proxy-check try-proxy-stop

# Debug build, wrapped as "Rival (dev)", opened against your real ~/.rival.
run:
	python3 app/scripts/dev_bundle.py --build
	open -n app/.build/Rival.app

# Fresh release build (native arch) installed to /Applications/Rival.app.
# The old copy is moved to /tmp/trash, never deleted.
install:
	python3 app/scripts/install_local.py

# Unit tests, then the launch soak test: shows the dev app on screen for
# about a minute. Needs a GUI session; see app/scripts/soak_test.py.
test:
	cd app && swift test
	$(MAKE) soak

# The soak test alone (builds first).
soak:
	python3 app/scripts/soak_test.py

# CLI (Rust). `rival version` prints RIVAL_VERSION; the default is the git
# description.
RIVAL_VERSION ?= $(shell git describe --tags --always --dirty 2>/dev/null || echo dev)

# Release build of the CLI for this host: target/release/rival.
cli-build:
	RIVAL_VERSION=$(RIVAL_VERSION) cargo build --release --locked -p rival

# Installs the CLI to ~/.cargo/bin/rival.
cli-install:
	RIVAL_VERSION=$(RIVAL_VERSION) cargo install --locked --path crates/rival

# Workspace tests plus the release-script and release-config tests. The
# config tests need PyYAML: install scripts/requirements-test.txt into the
# python3 on PATH (a venv), as CI does.
cli-test:
	cargo test --workspace --locked
	python3 -m unittest discover -s scripts -p 'test_*.py'

# Validates .goreleaser.yaml. Builds nothing.
cli-release-check:
	goreleaser check

# Mac: try the branch CLI, TUI and Rival (dev) against the Dell's
# CLIProxyAPI through an ssh tunnel. A throwaway RIVAL_HOME
# (~/tmp-rival-try) keeps the installed rival and ~/.rival untouched. The
# first run reads the proxy key from the clipboard (or PROXY_KEY=...).
# Logic in scripts/try-proxy.sh.
try-proxy:
	scripts/try-proxy.sh app

try-proxy-tui:
	scripts/try-proxy.sh tui

try-proxy-check:
	scripts/try-proxy.sh check

try-proxy-stop:
	scripts/try-proxy.sh stop
