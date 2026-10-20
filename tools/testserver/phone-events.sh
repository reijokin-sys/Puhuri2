#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Watch the XEP-0085 / XEP-0184 events between the phone and this computer.
#
#   tools/testserver/phone-events.sh
#
# The phone must already be logged in (its service keeps the account, so it
# usually is). This starts a peer on this computer, tells it to say it is
# writing and then to send a message, and prints what both sides saw:
#
#   * the phone should see `typing` from the peer,
#   * the computer should see `message.receipt` for its own message, because
#     the phone answers a receipt request as soon as the message arrives.
set -eu

PROJECT=$(cd "$(dirname "$0")/../.." && pwd)
KEY=${PHONE_KEY:?aseta PHONE_KEY}
HOST=${PHONE_HOST:?aseta PHONE_HOST}
PHONE_CTL='export XDG_RUNTIME_DIR=/run/user/$(id -u); /opt/click.ubuntu.com/fi.reijo.puhuri2/current/lib/aarch64-linux-gnu/bin/puhuri-ctl'
A_JID=${1:-maija@puhuri.test}
B_JID=${2:-vesa@puhuri.test}
B_PASS=${3:-${PUHURI_TEST_PASSWORD2:?set PUHURI_TEST_PASSWORD2 (setup.sh prints it)}}

SSH="ssh -F /dev/null -o BatchMode=yes -o ConnectTimeout=10 -i $KEY"
WORK=$(mktemp -d)
V_PID=""
cleanup() {
    [ -n "$V_PID" ] && kill "$V_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

"$PROJECT/daemon/target/debug/puhurid" \
    --data-dir "$WORK/peer" --socket "$WORK/peer.sock" --engine xmpp \
    >"$WORK/peer.log" 2>&1 &
V_PID=$!
for _ in $(seq 1 50); do [ -S "$WORK/peer.sock" ] && break; sleep 0.2; done
"$PROJECT/daemon/target/debug/puhuri-ctl" --socket "$WORK/peer.sock" \
    login "$B_JID" "$B_PASS" >/dev/null
sleep 3

echo "--- kuunnellaan molempia"
"$PROJECT/daemon/target/debug/puhuri-ctl" --socket "$WORK/peer.sock" listen \
    >"$WORK/peer.events" 2>&1 &
PEER_LISTEN=$!
$SSH "$HOST" "$PHONE_CTL listen" >"$WORK/phone.events" 2>&1 &
PHONE_LISTEN=$!
sleep 1

echo "--- vertaisclient sanoo kirjoittavansa ja lahettaa viestin"
"$PROJECT/daemon/target/debug/puhuri-ctl" --socket "$WORK/peer.sock" \
    typing "$A_JID" >/dev/null
sleep 1
"$PROJECT/daemon/target/debug/puhuri-ctl" --socket "$WORK/peer.sock" \
    typing "$A_JID" --stop >/dev/null
"$PROJECT/daemon/target/debug/puhuri-ctl" --socket "$WORK/peer.sock" \
    send "$A_JID" "kuittaatko taman" >/dev/null
sleep 4

kill "$PEER_LISTEN" "$PHONE_LISTEN" 2>/dev/null || true

echo "--- puhelimen nakevat tapahtumat"
grep -o '"event":"[a-z.]*"' "$WORK/phone.events" | sort | uniq -c || true
grep -o '"typing":[a-z]*' "$WORK/phone.events" || true
echo "--- koneen nakevat tapahtumat"
grep -o '"event":"[a-z.]*"' "$WORK/peer.events" | sort | uniq -c || true
