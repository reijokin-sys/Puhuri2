#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# The archive (MAM, XEP-0313) and the other devices' copies (carbons, XEP-0280).
#
#   tools/testserver/e2e-history.sh
#
# Two things are checked:
#   1. a fresh session gets the older conversation back from the server's
#      archive, and
#   2. when the same account is open twice (two resources), what one of them
#      sends shows up in the other.
set -eu

PROJECT=$(cd "$(dirname "$0")/../.." && pwd)
DAEMON=${PUHURI_BIN:-$PROJECT/daemon/target/debug/puhurid}
CTL=${PUHURI_CTL:-$PROJECT/daemon/target/debug/puhuri-ctl}
A_JID=${1:-maija@puhuri.test}
B_JID=${2:-vesa@puhuri.test}
A_PASS=${3:-${PUHURI_TEST_PASSWORD1:?set PUHURI_TEST_PASSWORD1 (setup.sh prints it)}}
B_PASS=${4:-${PUHURI_TEST_PASSWORD2:?set PUHURI_TEST_PASSWORD2 (setup.sh prints it)}}

WORK=$(mktemp -d)
PIDS=""
cleanup() {
    for pid in $PIDS; do kill "$pid" 2>/dev/null || true; done
    cp "$WORK"/*.log "${TMPDIR:-/tmp}/" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

start() {  # start <name> <jid> <password>
    PUHURI_ATTACHMENT_ALLOW_PRIVATE=1 PUHURI_ATTACHMENT_ALLOW_HTTP=1 "$DAEMON" --data-dir "$WORK/$1" --socket "$WORK/$1.sock" --engine xmpp \
        >"$WORK/$1.log" 2>&1 &
    PIDS="$PIDS $!"
    for _ in $(seq 1 50); do [ -S "$WORK/$1.sock" ] && break; sleep 0.2; done
    "$CTL" --socket "$WORK/$1.sock" login "$2" "$3" >/dev/null
    for _ in $(seq 1 40); do
        if "$CTL" --socket "$WORK/$1.sock" status | grep -q '"connected": true'; then
            return
        fi
        sleep 0.5
    done
}

ctl() { socket=$1; shift; "$CTL" --socket "$socket" "$@"; }

echo "--- A ja B saavat viestit arkistoon"
start a "$A_JID" "$A_PASS"
start b "$B_JID" "$B_PASS"
ctl "$WORK/a.sock" send "$B_JID" "historia yksi" >/dev/null
sleep 1
ctl "$WORK/a.sock" send "$B_JID" "historia kaksi" >/dev/null
sleep 3

echo "--- uusi istunto samalla tililla (tyhja tietokanta): tuleeko arkisto?"
start c "$B_JID" "$B_PASS"
sleep 6
ctl "$WORK/c.sock" messages "$A_JID" | grep -E '"text"|"outgoing"' || true

echo "--- carbons: sama tili kahdesti, lahetaan yhdesta"
start d "$B_JID" "$B_PASS"
sleep 2
ctl "$WORK/d.sock" send "$A_JID" "lahetetty toisesta resurssista" >/dev/null
sleep 3
echo "--- C:n nakevat (pitaa sisaltaa carbon-viesti):"
ctl "$WORK/c.sock" messages "$A_JID" | grep -E '"text"|"outgoing"' || true

echo "--- virheet"
grep -i virhe "$WORK"/*.log || echo "(ei virheita)"
