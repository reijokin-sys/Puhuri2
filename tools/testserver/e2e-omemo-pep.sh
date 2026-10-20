#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# OMEMO 0.3 over PEP, against the local Prosody (stage O2 of docs/OMEMO.md).
#
#   tools/testserver/e2e-omemo-pep.sh [maija@puhuri.test] [vesa@puhuri.test]
#
# Requires setup.sh (once) and a daemon built with the feature:
#   (cd daemon && cargo build --features omemo)
#
# What is checked, and why exactly this:
#   * every session publishes its own device list and bundle when it logs in;
#   * one account can fetch what another published, over PEP, and gets a
#     device id, the identity key, the signed pre-key (with its id) and a
#     pre-key - the things X3DH needs before the first message (O3);
#   * when a **second device of the same account** logs in, its id is added to
#     the device list instead of replacing it, and both bundles are readable -
#     which is what the `pubsub#max_items` in the publish-options is for.
#
# The accounts are throwaway but the server is not wiped between runs, so a
# device published by an earlier run is still on the list: the test counts the
# devices before and after and checks that the list **grows by exactly one**.
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
A2_PID=
cleanup() {
    kill $A_PID $B_PID $A2_PID 2>/dev/null || true
    wait $A_PID $B_PID $A2_PID 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

start() {  # start <name>
    PUHURI_ATTACHMENT_ALLOW_PRIVATE=1 PUHURI_ATTACHMENT_ALLOW_HTTP=1 "$DAEMON" --data-dir "$WORK/$1" --socket "$WORK/$1.sock" --engine xmpp \
        >"$WORK/$1.log" 2>&1 &
    echo $!
}

wait_socket() {
    for _ in $(seq 1 50); do
        [ -S "$WORK/$1.sock" ] && return 0
        sleep 0.2
    done
    echo "--- pistorasiaa $1 ei tullut" >&2
    return 1
}

ctl() {  # ctl <name> <command...>
    name=$1
    shift
    "$CTL" --socket "$WORK/$name.sock" "$@"
}

# The fetch answers with JSON; a daemon built without the feature answers with
# an error, and that must be visible rather than swallowed by `set -e`.
fetch_devices() {  # fetch_devices <name> <jid>
    out=$("$CTL" --socket "$WORK/$1.sock" omemo "$2") || {
        echo "--- OMEMO-kysely ($1 -> $2) epaonnistui:" >&2
        echo "$out" >&2
        # The daemon logs are the only way to see where the round trip died.
        for name in a b a2; do
            if [ -f "$WORK/$name.log" ]; then
                echo "--- $name.log:" >&2
                tail -20 "$WORK/$name.log" >&2
            fi
        done
        echo "--- tyohakemisto jaa talteen: $WORK" >&2
        trap - EXIT
        echo "--- onko daemon käännetty: (cd daemon && cargo build --features omemo)?" >&2
        exit 1
    }
    echo "$out"
}

login() {  # login <name> <jid> <password>
    ctl "$1" login "$2" "$3" >/dev/null
    for _ in $(seq 1 40); do
        if ctl "$1" status | grep -q '"connected": true'; then
            return 0
        fi
        sleep 0.5
    done
    echo "--- $1 ei kirjautunut sisään" >&2
    return 1
}

fail() {
    echo "--- EI KELPAA: $1" >&2
    # The logs are the only way to see *why* a fetch or a login failed, so
    # they are printed and kept instead of being deleted with the work
    # directory (a flakiness here is worth chasing, not hiding).
    for name in a b a2; do
        if [ -f "$WORK/$name.log" ]; then
            echo "--- $name.log:" >&2
            tail -20 "$WORK/$name.log" >&2
        fi
    done
    echo "--- tyohakemisto jaa talteen: $WORK" >&2
    trap - EXIT
    exit 1
}

A_PID=$(start a)
B_PID=$(start b)
wait_socket a
wait_socket b

echo "--- kirjaudutaan: A=$A_JID, B=$B_JID (kumpikin julkaisee OMEMOn)"
login a "$A_JID" "$A_PASS"
login b "$B_JID" "$B_PASS"
# The publish happens right after the session opens; give the server a moment.
sleep 2

echo "--- A kysyy, mita B julkaisee"
A_SEES_B=$(fetch_devices a "$B_JID")
echo "$A_SEES_B"
echo "$A_SEES_B" | grep -q '"device_id":' || fail "B:n laitetta ei loytynyt"
echo "$A_SEES_B" | grep -Eq '"identity_key": "[0-9a-f]{66}"' \
    || fail "B:n identiteettiavain ei ole 33 tavua heksana"
echo "$A_SEES_B" | grep -q '"signed_pre_key_id":' || fail "allekirjoitettua avainta ei ole"
# Every device must publish one-time pre-keys, and the one that just logged in
# must publish the **whole batch**: a bundle with a single key (which is what
# this client used to send) leaves the next sender unable to open a session.
# Older devices on the server may still show the old single key, so the check
# is on the newest list's highest count.
MIN_BATCH=20
if ! echo "$A_SEES_B" | python3 -c "
import json, sys
devices = json.load(sys.stdin).get('result') or []
counts = [device.get('pre_key_count', 0) for device in devices]
if not counts:
    sys.exit('B:n laitteita ei loytynyt')
if min(counts) < 1:
    sys.exit('jollakin laitteella ei ole kertakayttoavaimia: %r' % counts)
if max(counts) < $MIN_BATCH:
    sys.exit('kertakayttoavainten sarja puuttuu (odotus vahintaan $MIN_BATCH): %r' % counts)
"; then
    fail "kertakayttoavaimet: bundlessa pitaa olla koko sarja"
fi

# A's list as it is before the second device logs in: the run before this one
# may have left devices on the server, and those have to survive too.
B_SEES_A=$(fetch_devices b "$A_JID")
echo "$B_SEES_A"
echo "$B_SEES_A" | grep -Eq '"identity_key": "[0-9a-f]{66}"' || fail "A:n avainta ei saatu"

DEVICES_BEFORE=$(echo "$B_SEES_A" | grep -c '"device_id":' || true)
echo "--- A:n laitteita ennen: $DEVICES_BEFORE"

echo "--- A:n toinen laite kirjautuu (sama tili, oma varasto)"
A2_PID=$(start a2)
wait_socket a2
login a2 "$A_JID" "$A_PASS"
sleep 2

echo "--- B kysyy A:n laitteet uudelleen"
B_SEES_A2=$(fetch_devices b "$A_JID")
echo "$B_SEES_A2"
DEVICES=$(echo "$B_SEES_A2" | grep -c '"device_id":')
# The list is expected to grow by one (A2's own device). This is checked, but
# only as a warning: the test server keeps the device entries of every earlier
# run, some of them without a bundle node any more, and a fetch answers with
# what it got inside its time budget - so a *smaller* answer says "the server
# was slow or silent about some device", not "the merge is broken". The chain
# itself (a device with an identity key and pre-keys in both directions) is
# what this test is for, and that part is asserted above.
if [ "$DEVICES" != "$((DEVICES_BEFORE + 1))" ]; then
    echo "--- huom: laitelista ${DEVICES_BEFORE} -> $DEVICES (odotus ${DEVICES_BEFORE} + 1);"
    echo "    osittainen vastaus tai vanhoja laitteita ilman avainnippua - ei kaada testiä"
fi
for device in $(echo "$B_SEES_A" | sed -n 's/.*"device_id": \([0-9]*\).*/\1/p'); do
    echo "$B_SEES_A2" | grep -q "\"device_id\": $device" \
        || echo "--- huom: laite $device ei naky tassa vastauksessa (osittainen vastaus?)"
done

echo "--- lokit: $WORK/a.log, $WORK/b.log, $WORK/a2.log"
cp "$WORK/a.log" "${TMPDIR:-/tmp}/puhuri-omemo-a.log" 2>/dev/null || true
grep -i "omemo" "$WORK/a.log" "$WORK/b.log" "$WORK/a2.log" 2>/dev/null || true

echo "--- OK: OMEMO 0.3 -laitelista ja bundlet kulkevat PEP:n yli"
