#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# L1b end to end against the local Prosody: one receive path (carbon and
# archive), the duplicate rules and the crash MAM heals.
#
#   tools/testserver/e2e-l1b.sh
#
# Requires setup.sh (the local test server) and a host build. What it checks:
#
#   1. a carbon is decrypted: a second session of the *sender's* account sees
#      the copy of what was sent, in plaintext (not the fallback line);
#   2. a crash between the decrypt and the commit loses nothing: the receiver
#      is started with PUHURI_FAILPOINT=recv_after_decrypt_before_persist, the
#      message is sent, the daemon aborts - and at the next start the archive
#      read brings the same message back, decrypted and stored exactly once;
#   3. the archive anchor works: `mam_last_id` is stored, the next login asks
#      with it and the server answers without an IQ error.
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

# The crash phase needs the failpoint build; the other daemons run the normal
# one, so the failpoint binary is copied aside first.
echo "--- käännetään failpoints-käännös kaatumistestiä varten"
(cd "$PROJECT/daemon" && cargo build --features "omemo failpoints" --offline >/dev/null)
cp "$DAEMON" "$WORK/puhurid-failpoints"
(cd "$PROJECT/daemon" && cargo build --features omemo --offline >/dev/null)

A_PID=; B_PID=; A2_PID=
cleanup() {
    for pid in $A_PID $B_PID $A2_PID; do
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
    done
    rm -rf "$WORK"
}
trap cleanup EXIT

fail() {
    echo "--- EI KELPAA: $1" >&2
    for name in a b a2; do
        [ -f "$WORK/$name.log" ] && { echo "--- $name:n loki:" >&2; tail -6 "$WORK/$name.log" >&2; }
    done
    exit 1
}

start() {  # start <name> <binary>
    name=$1
    binary=${2:-$DAEMON}
    "$binary" --data-dir "$WORK/$name" --socket "$WORK/$name.sock" --engine xmpp \
        >"$WORK/$name.log" 2>&1 &
    case "$name" in
        a) A_PID=$! ;;
        b) B_PID=$! ;;
        a2) A2_PID=$! ;;
    esac
    for _ in $(seq 1 50); do
        [ -S "$WORK/$name.sock" ] && return 0
        sleep 0.2
    done
    fail "$name ei avannut socketia"
}

ctl() {  # ctl <name> <command...>
    name=$1
    shift
    "$CTL" --socket "$WORK/$name.sock" "$@"
}

login() {
    ctl "$1" login "$2" "$3" >/dev/null 2>&1 || true
    for _ in $(seq 1 40); do
        ctl "$1" status | grep -q '"connected": true' && return 0
        sleep 0.5
    done
    fail "$1 ei kirjautunut sisään"
}

seen() {  # seen <name> <thread> <text>
    ctl "$1" messages "$2" | grep -q "$3"
}

echo "--- käynnistetään: a (lähettäjä), a2 (saman tilin toinen laite), b (vastaanottaja)"
start a
start a2
start b
login a "$A_JID" "$A_PASS"
login a2 "$A_JID" "$A_PASS"
login b "$B_JID" "$B_PASS"
sleep 3

CANARY_CARBON="CARBON-$(date +%s)"
echo "--- A lähettää salatun viestin B:lle (A2:n pitää nähdä kopio purettuna)"
ctl a omemosend "$B_JID" "$CANARY_CARBON" >/dev/null
sleep 5
seen b "$A_JID" "$CANARY_CARBON" || fail "B ei saanut viestiä"
seen a2 "$B_JID" "$CANARY_CARBON" || fail "A2 ei nähnyt carbonia purettuna (fallback-teksti?)"
grep -q "OMEMO encrypted message" "$WORK/a2.log" && fail "A2 tallensi fallback-tekstin"
echo "--- OK: carbon purettiin samalla polulla (A2 näkee viestin)"

CANARY_CRASH="CRASH-$(date +%s)"
echo "--- kaatumistesti: B uudelleen failpointilla, viesti kaataa sen ennen committia"
kill "$B_PID" 2>/dev/null || true
wait "$B_PID" 2>/dev/null || true
B_PID=
PUHURI_FAILPOINT=recv_after_decrypt_before_persist \
    "$WORK/puhurid-failpoints" --data-dir "$WORK/b" --socket "$WORK/b.sock" \
    --engine xmpp >"$WORK/b.log" 2>&1 &
B_PID=$!
for _ in $(seq 1 50); do
    [ -S "$WORK/b.sock" ] && break
    sleep 0.2
done
login b "$B_JID" "$B_PASS"
sleep 2
ctl a omemosend "$B_JID" "$CANARY_CRASH" >/dev/null
sleep 4
if kill -0 "$B_PID" 2>/dev/null; then
    fail "failpoint ei kaatanut B:n prosessia"
fi
wait "$B_PID" 2>/dev/null || true
echo "--- B kaatui failpointiin; käynnistetään uudelleen - arkiston pitää tuoda viesti"
start b
login b "$B_JID" "$B_PASS"
sleep 8
seen b "$A_JID" "$CANARY_CRASH" || fail "arkisto ei tuonut kaatunutta viestiä takaisin"
count=$(ctl b messages "$A_JID" | grep -c "$CANARY_CRASH" || true)
[ "$count" = "1" ] || fail "viesti tallentui $count kertaa (pitää olla 1)"
echo "--- OK: kaatunut viesti palasi arkistosta ja tallentui tasan kerran"

echo "--- arkistotunnus ja seuraava haku (after-polku)"
kill "$B_PID" 2>/dev/null || true
wait "$B_PID" 2>/dev/null || true
start b
login b "$B_JID" "$B_PASS"
sleep 6
grep -q "IQ-virhe: mam-history" "$WORK/b.log" && fail "MAM-haku hylättiin (after-polku)"
python3 - "$WORK/b/puhuri.db" <<'PY' || fail "mam_last_id puuttuu"
import sqlite3, sys
db = sqlite3.connect(sys.argv[1])
value = db.execute("SELECT value FROM settings WHERE key = 'mam_last_id'").fetchone()
print("--- mam_last_id:", value[0] if value else "(ei ole)")
sys.exit(0 if value and value[0] else 1)
PY
echo "--- OK: L1b toimii palvelinta vasten (carbon, kaatuminen, arkisto, after)"
