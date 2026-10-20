#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Send a message from the phone to the computer's Prosody test server.
#
#   tools/testserver/phone-e2e.sh
#
# Assumes tools/testserver/setup.sh has been run (Prosody, the two accounts,
# the local CA) and that the phone can reach this computer on the LAN. The
# phone's root filesystem is read only, so the test CA is not installed
# system-wide there: the daemon is started by hand with PUHURI_TLS_CA, which
# makes it add the certificate to its own trust store for that run.
#
# Nothing of the phone's permanent installation is touched: the test daemon
# uses /tmp/puhuri-phone and its own socket, and it is stopped at the end.
set -eu

PROJECT=$(cd "$(dirname "$0")/../.." && pwd)
KEY=${PHONE_KEY:?aseta PHONE_KEY}
HOST=${PHONE_HOST:?aseta PHONE_HOST}
POINT=${PUHURI_POINT:?aseta PUHURI_POINT}
A_JID=${1:-maija@puhuri.test}
B_JID=${2:-vesa@puhuri.test}
A_PASS=${3:-${PUHURI_TEST_PASSWORD1:?set PUHURI_TEST_PASSWORD1 (setup.sh prints it)}}
B_PASS=${4:-${PUHURI_TEST_PASSWORD2:?set PUHURI_TEST_PASSWORD2 (setup.sh prints it)}}

SSH="ssh -F /dev/null -o BatchMode=yes -o ConnectTimeout=10 -i $KEY"
SCP="scp -F /dev/null -o BatchMode=yes -i $KEY"

WORK=$(mktemp -d)
V_PID=""
cleanup() {
    [ -n "$V_PID" ] && kill "$V_PID" 2>/dev/null || true
    $SSH "$HOST" 'pkill -f "^/tmp/puhurid" 2>/dev/null || true' || true
    rm -rf "$WORK"
}
trap cleanup EXIT

# setup.sh leaves a world-readable copy of the test CA next to the
# certificate, so no root is needed here.
cp /usr/local/share/puhuri-test-ca.crt "$WORK/ca.crt"

echo "--- koneen vertaisclient ($B_JID)"
"$PROJECT/daemon/target/debug/puhurid" \
    --data-dir "$WORK/server" --socket "$WORK/server.sock" --engine xmpp \
    >"$WORK/server.log" 2>&1 &
V_PID=$!
for _ in $(seq 1 50); do [ -S "$WORK/server.sock" ] && break; sleep 0.2; done
"$PROJECT/daemon/target/debug/puhuri-ctl" --socket "$WORK/server.sock" \
    login "$B_JID" "$B_PASS" >/dev/null
sleep 3
"$PROJECT/daemon/target/debug/puhuri-ctl" --socket "$WORK/server.sock" status

echo "--- binäärit ja CA puhelimeen"
$SCP "$PROJECT/build/puhurid" "$PROJECT/build/puhuri-ctl" "$WORK/ca.crt" "$HOST:/tmp/"

echo "--- puhelin kirjautuu ($A_JID)"
$SSH "$HOST" "chmod 755 /tmp/puhurid /tmp/puhuri-ctl;
  pkill -f '^/tmp/puhurid' 2>/dev/null || true; sleep 1;
  rm -rf /tmp/puhuri-phone;
  PUHURI_TLS_CA=/tmp/ca.crt PUHURI_ATTACHMENT_ALLOW_PRIVATE=1 PUHURI_ATTACHMENT_ALLOW_HTTP=1 nohup /tmp/puhurid --data-dir /tmp/puhuri-phone \
      --socket /tmp/puhuri-phone.sock --engine xmpp >/tmp/puhuri-phone.log 2>&1 &
  sleep 1;
  /tmp/puhuri-ctl --socket /tmp/puhuri-phone.sock login $A_JID $A_PASS"

for _ in $(seq 1 30); do
    if $SSH "$HOST" '/tmp/puhuri-ctl --socket /tmp/puhuri-phone.sock status' \
            | grep -q '"connected": true'; then
        break
    fi
    sleep 1
done
echo "--- puhelimen tila"
$SSH "$HOST" '/tmp/puhuri-ctl --socket /tmp/puhuri-phone.sock status'

echo "--- puhelin lähettää"
$SSH "$HOST" "/tmp/puhuri-ctl --socket /tmp/puhuri-phone.sock send $B_JID 'Tervehdys puhelimesta'"
sleep 3

echo "--- koneen vastaanotto"
"$PROJECT/daemon/target/debug/puhuri-ctl" --socket "$WORK/server.sock" conversations

echo "--- puhelimen loki"
$SSH "$HOST" 'tail -6 /tmp/puhuri-phone.log'
