#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# End-to-end test against the local Prosody: two accounts, two sessions.
#
#   tools/testserver/e2e.sh [maija@puhuri.test] [vesa@puhuri.test]
#
# Requires tools/testserver/setup.sh to have been run once (that creates the
# accounts and the server). The two daemons run with their own data
# directories and sockets under a temporary directory, so a test never touches
# the real ~/.local/share/fi.reijo.puhuri2.
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

cleanup() {
    kill "$A_PID" "$B_PID" 2>/dev/null || true
    wait "$A_PID" "$B_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

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

ctl() {  # ctl <socket> <command...>
    socket=$1
    shift
    "$CTL" --socket "$socket" "$@"
}

echo "--- kirjaudutaan sisään"
ctl "$WORK/a.sock" login "$A_JID" "$A_PASS" >/dev/null
ctl "$WORK/b.sock" login "$B_JID" "$B_PASS" >/dev/null

for _ in $(seq 1 40); do
    a_state=$(ctl "$WORK/a.sock" status | grep -c '"connected": true' || true)
    b_state=$(ctl "$WORK/b.sock" status | grep -c '"connected": true' || true)
    [ "$a_state" = "1" ] && [ "$b_state" = "1" ] && break
    sleep 0.5
done

echo "--- A:n tila"
ctl "$WORK/a.sock" status
echo "--- B:n tila"
ctl "$WORK/b.sock" status

echo "--- B lahettaa A:lle"
ctl "$WORK/b.sock" send "$A_JID" "Tervehdys Puhurista" >/dev/null
sleep 2

echo "--- A:n keskustelut"
ctl "$WORK/a.sock" conversations
echo "--- A:n viestit"
ctl "$WORK/a.sock" messages "$B_JID"

echo "--- A vastaa"
ctl "$WORK/a.sock" send "$B_JID" "Vastaus perille" >/dev/null
sleep 2
echo "--- B:n viestit"
ctl "$WORK/b.sock" messages "$A_JID"

echo "--- kirjoitustila ja kuittaukset (XEP-0085 / XEP-0184)"
ctl "$WORK/a.sock" listen >"$WORK/a.events" 2>&1 &
LISTEN_PID=$!
sleep 1
ctl "$WORK/b.sock" typing "$A_JID" >/dev/null
sleep 1
ctl "$WORK/b.sock" typing "$A_JID" --stop >/dev/null
ctl "$WORK/a.sock" send "$B_JID" "kuittaatko tämän" >/dev/null
sleep 3
kill "$LISTEN_PID" 2>/dev/null || true
grep -o '"event":"[a-z.]*"' "$WORK/a.events" | sort | uniq -c || true
grep -o '"event":"typing","data":{"thread":"[^"]*","typing":[a-z]*' "$WORK/a.events" || true
grep -o '"event":"message.receipt","data":{"thread":"[^"]*","id":"[^"]*"' "$WORK/a.events" || true

echo "--- kuittaus osuu oikealle riville ja jaa talteen (XEP-0184, 2026-10-16)"
ctl "$WORK/a.sock" messages "$B_JID" >"$WORK/a.messages"
python3 - "$WORK/a.messages" "$WORK/a.events" <<'PY'
import json
import sys

messages = json.load(open(sys.argv[1]))["result"]
events = [json.loads(line) for line in open(sys.argv[2]) if line.strip()]
receipts = [event for event in events if event.get("event") == "message.receipt"]
if not receipts:
    raise SystemExit("EI KELPAA: kuittaustapahtumaa ei tullut lainkaan")

sent = [message for message in messages if message["outgoing"]][-1]
echoed = receipts[0]["data"]["id"]
print("viestirivin id: %s / kuittauksen id: %s" % (sent["id"], echoed))
if echoed != sent["id"]:
    raise SystemExit(
        "EI KELPAA: kuittaus tuli tunnisteella jota ei ole omalla rivilla "
        "(kirjaston satunnainen id?)"
    )
if sent.get("delivered") is not True:
    raise SystemExit("EI KELPAA: kuittaus ei tallentunut viestiriville")
print("kuittaus osui oikealle riville ja on tallessa")
PY

echo "--- (lokit: $WORK/a.log, $WORK/b.log - kopioidaan talteen ennen siivousta)"
cp "$WORK/a.log" "${TMPDIR:-/tmp}/puhuri-e2e-a.log"
cp "$WORK/b.log" "${TMPDIR:-/tmp}/puhuri-e2e-b.log"
