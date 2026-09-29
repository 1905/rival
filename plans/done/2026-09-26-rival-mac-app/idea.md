# Rival Mac app — idea

**Date:** 2026-09-26
**Status:** done

User (2026-09-26): "remove 'server' feature. write cool looking swift app instead and allow to install it with cask via homebrew."

Brainstorm answers:
- **Signing:** ad-hoc signed, with quarantine stripped by the cask. No Developer ID. The user accepts the trust trade-off.
- **Shape:** menu bar extra (live count + mini list) plus a full window with TUI parity.
- **Data:** read `~/.rival/sessions` directly with FSEvents. No server, no daemon.
- **Order:** finish the prompt-core job first (P4-P5 + gates). The README rewrite moves here, so it is written once.

Found:
- Homebrew removed `--no-quarantine`, and ended support for casks that fail Gatekeeper, on 2026-09-01. Sources: Homebrew/brew#20755, the Homebrew 5.0 notes.
- The local brew is 7.0.6, on macOS 14.8.9 arm64.
- Whether a personal-tap cask may strip quarantine in `postflight` is unverified. Hence the P0 spike.
- The toolchain is Xcode 16.2 / Swift 6.0.3. The only signing identity is self-signed ("Image Studio Local Signing"); there is no Developer ID.
