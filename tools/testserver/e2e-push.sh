#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# XEP-0357 push notifications, against the local Prosody (mod_cloud_notify).
#
#   tools/testserver/e2e-push.sh
#
# The chain that is tested here is the client's half of push:
#
#   Puhuri 2 enables push  ->  the server remembers it  ->  Puhuri 2 goes offline
#   ->  a message arrives for the offline account  ->  the server sends the
#   push notification to the push service  ->  the service (here: a stand-in,
#   tools/testserver/push-collector.py) receives and answers it.
#
# What is *not* here is the other half: a push service that can reach the
# phone (the UBports/Lomiri push system, or a bridge to it). See docs/PUSH.md.
set -eu

PROJECT=$(cd "$(dirname "$0")/../.." && pwd)
DAEMON=${PUHURI_BIN:-$PROJECT/daemon/target/debug/puhurid}
CTL=${PUHURI_CTL:-$PROJECT/daemon/target/debug/puhuri-ctl}
PYTHON=${PUHURI_PEER_PYTHON:-$HOME/.venvs/puhuri-omemo/bin/python3}
A_JID=${1:-maija@puhuri.test}
B_JID=${2:-vesa@puhuri.test}
PUSH_SERVICE=${PUHURI_PUSH_SERVICE:-push.puhuri.test}
A_PASS=${3:-${PUHURI_TEST_PASSWORD1:?set PUHURI_TEST_PASSWORD1 (setup.sh prints it)}}
B_PASS=${4:-${PUHURI_TEST_PASSWORD2:?set PUHURI_TEST_PASSWORD2 (setup.sh prints it)}}
export PUHURI_CA=${PUHURI_CA:-/etc/ssl/certs/puhuri-test-ca.pem}

WORK=$(mktemp -d)
echo "--- tyohakemisto: $WORK"

A_PID=
cleanup() {
    [ -n "$A_PID" ] && kill "$A_PID" 2>/dev/null || true
    [ -n "$A_PID" ] && wait "$A_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

fail() {
    echo "--- EI KELPAA: $1" >&2
    echo "--- Puhurin loki:" >&2
    tail -5 "$WORK/a.log" >&2 || true
    echo "--- kerääjän tulos:" >&2
    cat "$WORK/collector.out" 2>/dev/null >&2 || true
    echo "--- kerääjän virheet:" >&2
    tail -8 "$WORK/collector.err" 2>/dev/null >&2 || true
    exit 1
}

start_puhuri() {
    PUHURI_ATTACHMENT_ALLOW_PRIVATE=1 PUHURI_ATTACHMENT_ALLOW_HTTP=1 "$DAEMON" --data-dir "$WORK/a" --socket "$WORK/a.sock" --engine xmpp \
        >"$WORK/a.log" 2>&1 &
    A_PID=$!
    for _ in $(seq 1 50); do
        [ -S "$WORK/a.sock" ] && break
        sleep 0.2
    done
}

stop_puhuri() {
    # A clean logout first: then the account is really offline and the server
    # stores the message (mod_offline), which is what makes it push.
    ctl logout >/dev/null 2>&1 || true
    kill "$A_PID" 2>/dev/null || true
    wait "$A_PID" 2>/dev/null || true
    A_PID=
    # Give the server a moment to notice the connection is gone.
    sleep 3
}

ctl() {
    "$CTL" --socket "$WORK/a.sock" "$@"
}

login() {
    ctl login "$A_JID" "$A_PASS" >/dev/null
    for _ in $(seq 1 40); do
        ctl status | grep -q '"connected": true' && return 0
        sleep 0.5
    done
    return 1
}

collector() {  # collector <expect-push yes|no> <outfile>
    PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=python "$PYTHON" \
        "$PROJECT/tools/testserver/push-collector.py" \
        --send-to "$A_JID" --send-as "$B_JID" "$B_PASS" --after 10 \
        >"$2" 2>"$WORK/collector.err" || true
    echo "--- kerääjä valmis: $(wc -l < "$2" 2>/dev/null || echo 0) riviä tulostetta" >&2
    if [ "$1" = "yes" ]; then
        grep -q "^PUSH node=" "$2" || fail "palvelin ei lähettänyt push-ilmoitusta"
        grep -q '"message-count": "1"' "$2" || fail "push-ilmoituksesta puuttuu viestilaskuri"
    else
        grep -q "^PUSH node=" "$2" && fail "push-ilmoitus tuli, vaikka push on pois päältä"
        true
    fi
}

echo "--- 1/3: Puhuri 2 päälle ja push käyttöön (palvelu: $PUSH_SERVICE)"
start_puhuri
login || fail "Puhuri 2 ei kirjautunut"
ctl push-enable "$PUSH_SERVICE" test-token --module ubports
ctl push
grep -q "\"jid\": \"$PUSH_SERVICE\"" "$WORK/a.log" 2>/dev/null || true

echo "--- 2/3: Puhuri 2 pois päältä, viesti saapuu - palvelimen pitää pushata"
stop_puhuri
collector yes "$WORK/collector.out"
echo "--- push tuli perille:"
grep "^PUSH" "$WORK/collector.out"

echo "--- 3/3: push pois päältä - ilmoitusta ei saa enää tulla"
start_puhuri
login || fail "Puhuri 2 ei kirjautunut uudelleen"
ctl push-disable >/dev/null
stop_puhuri
collector no "$WORK/collector2.out"

echo "--- OK: XEP-0357 toimii: palvelin pushaa kun client on offline, ja lopettaa kun push kytketään pois"
