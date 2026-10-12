# Developer shortcuts for the Rust CLI.
.PHONY: cli-build cli-install cli-test cli-check cli-release-check cli-snapshot cli-publish try-proxy try-proxy-check try-proxy-stop

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
# python3 on PATH (a venv).
cli-test:
	cargo test --workspace --locked
	python3 -m unittest discover -s scripts -p 'test_*.py'

# The full pre-merge gate (rival has no hosted CI): fmt, strict clippy,
# workspace tests, startup fds, release-script tests, fixtures, the name
# check and the e2e scenarios. Logic in scripts/check.sh.
cli-check:
	scripts/check.sh

# Release builds run on a macOS host (the darwin targets need the Apple
# SDK). Logic in scripts/release-local.sh; run its `setup` once per host.
# Validates .goreleaser.yaml, allowing only the intentional brews deprecation.
# Builds nothing; invalid config and any other deprecation still fail.
cli-release-check:
	scripts/release-local.sh check

# Six archives and the formula in dist/, checked. Publishes nothing.
cli-snapshot:
	scripts/release-local.sh snapshot

# Publishes the release and the brew formula from a clean checkout of a
# v* tag. Needs GITHUB_TOKEN (a 1905 token).
cli-publish:
	scripts/release-local.sh release

# Mac: try the branch CLI and TUI against the Dell's
# CLIProxyAPI through an ssh tunnel. A throwaway RIVAL_HOME
# (~/tmp-rival-try) keeps the installed rival and ~/.rival untouched. The
# first run reads the proxy key over ssh from the proxy host (or
# PROXY_KEY=...).
# Logic in scripts/try-proxy.sh.
try-proxy:
	scripts/try-proxy.sh tui

try-proxy-check:
	scripts/try-proxy.sh check

try-proxy-stop:
	scripts/try-proxy.sh stop
