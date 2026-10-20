#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# OMEMO 0.3 end to end: the encrypted envelope between two installations.
#
#   tools/testserver/e2e-omemo-envelope.sh [maija@puhuri.test] [vesa@puhuri.test]
#
# Requires setup.sh (once) and a daemon built with the feature:
#   (cd daemon && cargo build --features omemo)
#
# What is checked: A sends a message encrypted to every device B publishes,
# and B opens it (the plaintext arrives in the conversation and the log says
# the envelope was opened); then B answers the same way. The plaintext never
# appears on the wire, so the proof is that the receiving side *decrypted* -
# a message that arrived as its fallback text would fail this test.
set -eu

PROJECT=$(cd "$(dirname "$0")/../.." && pwd)
DAEMON=${PUHURI_BIN:-$PROJECT/daemon/target/debug/puhurid}
CTL=${PUHURI_CTL:-$PROJECT/daemon/target/debug/puhuri-ctl}
A_JID=${1:-maija@puhuri.test}
B_JID=${2:-vesa@puhuri.test}
A_PASS=${3:-${PUHURI_TEST_PASSWORD1:?set PUHURI_TEST_PASSWORD1 (setup.sh prints it)}}
B_PASS=${4:-${PUHURI_TEST_PASSWORD2:?set PUHURI_TEST_PASSWORD2 (setup.sh prints it)}}

WORK=$(mktemp -d)
echo "--- tyohakemisto: $WORK"

A_PID=
B_PID=
cleanup() {
    kill "$A_PID" "$B_PID" 2>/dev/null || true
    wait "$A_PID" "$B_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

fail() {
    echo "--- EI KELPAA: $1" >&2
    echo "--- A:n loki (häntä)" >&2
    tail -5 "$WORK/a.log" >&2 || true
    echo "--- B:n loki (häntä)" >&2
    tail -5 "$WORK/b.log" >&2 || true
    exit 1
}

PUHURI_ATTACHMENT_ALLOW_PRIVATE=1 PUHURI_ATTACHMENT_ALLOW_HTTP=1 "$DAEMON" --data-dir "$WORK/a" --socket "$WORK/a.sock" --engine xmpp \
    >"$WORK/a.log" 2>&1 &
A_PID=$!
PUHURI_ATTACHMENT_ALLOW_PRIVATE=1 PUHURI_ATTACHMENT_ALLOW_HTTP=1 "$DAEMON" --data-dir "$WORK/b" --socket "$WORK/b.sock" --engine xmpp \
    >"$WORK/b.log" 2>&1 &
B_PID=$!

for _ in $(seq 1 50); do
    [ -S "$WORK/a.sock" ] && [ -S "$WORK/b.sock" ] && break
    sleep 0.2
done

ctl() {  # ctl <name> <command...>
    name=$1
    shift
    "$CTL" --socket "$WORK/$name.sock" "$@"
}

login() {
    ctl "$1" login "$2" "$3" >/dev/null
    for _ in $(seq 1 40); do
        ctl "$1" status | grep -q '"connected": true' && return 0
        sleep 0.5
    done
    fail "$1 ei kirjautunut sisään"
}

# A message the receiver can find in its own store, or fail loudly.
expect_message() {  # expect_message <name> <thread> <text>
    if ! ctl "$1" messages "$2" | grep -q "$3"; then
        fail "$1 ei saanut viestiä '$3'"
    fi
    echo "--- $1 näkee viestin: $3"
}

echo "--- kirjaudutaan: A=$A_JID, B=$B_JID (kumpikin julkaisee OMEMOn)"
login a "$A_JID" "$A_PASS"
login b "$B_JID" "$B_PASS"
sleep 2

echo "--- A lähettää salatun viestin B:lle"
ctl a omemosend "$B_JID" "salainen tervehdys" >/dev/null
grep -q "OMEMO-viesti salattu" "$WORK/a.log" || fail "A ei salannut viestiä"
sleep 3
expect_message b "$A_JID" "salainen tervehdys"
grep -q "OMEMO-viesti purettu" "$WORK/b.log" || fail "B ei purkanut kirjekuorta"

echo "--- B vastaa salattuna"
ctl b omemosend "$A_JID" "salainen vastaus" >/dev/null
grep -q "OMEMO-viesti salattu" "$WORK/b.log" || fail "B ei salannut vastausta"
sleep 3
expect_message a "$B_JID" "salainen vastaus"
grep -q "OMEMO-viesti purettu" "$WORK/a.log" || fail "A ei purkanut kirjekuorta"

echo "--- kumpaankin suuntaan salattu ja purettu"
grep -o "OMEMO-viesti salattu [0-9]* laitteelle" "$WORK/a.log" "$WORK/b.log" | tail -4 || true
grep -o "OMEMO-viesti purettu (sid [0-9]*)" "$WORK/a.log" "$WORK/b.log" | tail -4 || true

echo "--- OK: OMEMO 0.3 -kirjekuori toimii molempiin suuntiin"
