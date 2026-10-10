#!/usr/bin/env bash
# Try the branch build against the Dell's CLIProxyAPI from a Mac, without
# touching the installed rival or ~/.rival.
#
#   scripts/try-proxy.sh app     # CLI build, tunnel, config, check, dev app
#   scripts/try-proxy.sh tui     # the same, then the TUI config window
#   scripts/try-proxy.sh check   # the same, then stop after the model check
#   scripts/try-proxy.sh stop    # close the tunnel
#
# Environment:
#   TRY_HOME      throwaway RIVAL_HOME (default ~/tmp-rival-try)
#   PROXY_HOST    ssh alias of the proxy host (default dell)
#   PROXY_PORT    proxy port on both ends (default 8317)
#   CLAUDE_PREFIX model prefix for Claude (default emcd2_)
#   PROXY_KEY     the proxy key; if unset and no key is saved yet, it is
#                 read over ssh from KEY_FILE on PROXY_HOST
#   KEY_FILE      key file on PROXY_HOST (default ~/cliproxyapi/t3-emcd-client.key)
set -euo pipefail

cd "$(dirname "$0")/.."

mode=${1:-app}
TRY_HOME=${TRY_HOME:-$HOME/tmp-rival-try}
PROXY_HOST=${PROXY_HOST:-dell}
PROXY_PORT=${PROXY_PORT:-8317}
CLAUDE_PREFIX=${CLAUDE_PREFIX:-emcd2_}
KEY_FILE=${KEY_FILE:-cliproxyapi/t3-emcd-client.key}
tunnel="ssh -f -N -o ExitOnForwardFailure=yes -L ${PROXY_PORT}:127.0.0.1:${PROXY_PORT} ${PROXY_HOST}"

if [ "$mode" = stop ]; then
    pkill -f "$tunnel" && echo "tunnel closed" || echo "no tunnel"
    exit 0
fi

case "$mode" in
    app | tui | check) ;;
    *) echo "usage: $0 app|tui|check|stop" >&2; exit 2 ;;
esac

echo "→ build the branch CLI"
cargo build --release --locked -p rival
R="$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')/release/rival"

echo "→ tunnel ${PROXY_HOST}:${PROXY_PORT}"
if nc -z 127.0.0.1 "$PROXY_PORT" 2>/dev/null; then
    echo "  port ${PROXY_PORT} already open"
else
    $tunnel
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        nc -z 127.0.0.1 "$PROXY_PORT" 2>/dev/null && break
        sleep 0.5
    done
fi

export RIVAL_HOME="$TRY_HOME"
mkdir -p "$RIVAL_HOME"
echo "→ config in $RIVAL_HOME"
"$R" config set proxy.url "http://127.0.0.1:${PROXY_PORT}" >/dev/null
"$R" config set proxy.claude.enabled true >/dev/null
"$R" config set proxy.claude.model_prefix "$CLAUDE_PREFIX" >/dev/null
"$R" config set proxy.codex.enabled true >/dev/null
if [ -n "${PROXY_KEY:-}" ]; then
    printf %s "$PROXY_KEY" | "$R" config key set
elif [ ! -s "$RIVAL_HOME/proxy.key" ]; then
    echo "  no key saved: reading ${PROXY_HOST}:~/${KEY_FILE#\~/}"
    ssh "$PROXY_HOST" "cat ~/${KEY_FILE#\~/}" | "$R" config key set
fi

echo "→ model check"
"$R" config check || true

case "$mode" in
    tui) exec "$R" config ;;
    app)
        echo "→ dev app"
        python3 app/scripts/dev_bundle.py --build
        open -n --env RIVAL_BIN="$R" --env RIVAL_HOME="$RIVAL_HOME" app/.build/Rival.app
        echo "Rival (dev) is open. ⌘, opens Settings."
        echo "CLI: RIVAL_HOME=$RIVAL_HOME $R config"
        echo "Close the tunnel: make try-proxy-stop"
        ;;
esac
