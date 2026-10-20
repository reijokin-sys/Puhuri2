#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# OMEMO 0.3 against an **independent implementation** (stage O4).
#
#   tools/testserver/e2e-omemo-peer.sh
#
# The other side is slixmpp + slixmpp-omemo (`tools/testserver/omemo-peer.py`),
# which speaks the OMEMO 0.3 namespace and uses the `omemo` library with its
# Signal backend - nothing of ours. Both directions have to work:
#
#   1. Puhuri 2 sends an encrypted message; the peer opens it and says so.
#   2. The peer answers encrypted; Puhuri 2 opens it (the service log says so).
#
# A failure here means the wire format is only self-consistent - which is
# exactly what this stage exists to rule out.
#
# Huomio (2026-10-16): testitilit keräävät laitelistoja - jokainen ajo
# julkaisee uuden OMEMO-laitteen, eikä mikään siivoa vanhoja. Kun listassa on
# yli sata laitetta, vertainen joutuu hakemaan niin monta avainnippua että
# vastaus ei ehdi testin odotusaikaan ("Puhuri 2 ei purkanut vertaisen
# vastausta" vaikka vertainen lähetti sen). Silloin aja tuoreilla tileillä:
#   tools/testserver/e2e-omemo-peer.sh maija2@puhuri.test vesa2@puhuri.test
# (docs/TODO.md, "The test server accumulates device entries").
set -eu

PROJECT=$(cd "$(dirname "$0")/../.." && pwd)
DAEMON=${PUHURI_BIN:-$PROJECT/daemon/target/debug/puhurid}
CTL=${PUHURI_CTL:-$PROJECT/daemon/target/debug/puhuri-ctl}
PEER=${PUHURI_PEER_PYTHON:-$HOME/.venvs/puhuri-omemo/bin/python3}
# The test server's certificate is signed by the project's own CA.
PUHURI_CA=${PUHURI_CA:-/etc/ssl/certs/puhuri-test-ca.pem}
export PUHURI_CA
A_JID=${1:-maija@puhuri.test}
B_JID=${2:-vesa@puhuri.test}
A_PASS=${3:-${PUHURI_TEST_PASSWORD1:?set PUHURI_TEST_PASSWORD1 (setup.sh prints it)}}
B_PASS=${4:-${PUHURI_TEST_PASSWORD2:?set PUHURI_TEST_PASSWORD2 (setup.sh prints it)}}
YES="kylla, salaus toimii"
REPLY="ja vastaus perille"

WORK=$(mktemp -d)
echo "--- tyohakemisto: $WORK"

A_PID=
PEER_PID=
cleanup() {
    kill "$A_PID" "$PEER_PID" 2>/dev/null || true
    wait "$A_PID" "$PEER_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

fail() {
    echo "--- EI KELPAA: $1" >&2
    echo "--- Puhurin loki:" >&2
    grep -E "OMEMO|omemo" "$WORK/a.log" | tail -5 >&2 || true
    echo "--- vertaisen tulos:" >&2
    cat "$WORK/peer.out" >&2 || true
    echo "--- vertaisen virheet:" >&2
    tail -10 "$WORK/peer.err" >&2 || true
    exit 1
}

echo "--- Puhuri 2 sisään ($A_JID)"
PUHURI_ATTACHMENT_ALLOW_PRIVATE=1 PUHURI_ATTACHMENT_ALLOW_HTTP=1 "$DAEMON" --data-dir "$WORK/a" --socket "$WORK/a.sock" --engine xmpp \
    >"$WORK/a.log" 2>&1 &
A_PID=$!
for _ in $(seq 1 50); do
    [ -S "$WORK/a.sock" ] && break
    sleep 0.2
done
"$CTL" --socket "$WORK/a.sock" login "$A_JID" "$A_PASS" >/dev/null
for _ in $(seq 1 40); do
    "$CTL" --socket "$WORK/a.sock" status | grep -q '"connected": true' && break
    sleep 0.5
done
"$CTL" --socket "$WORK/a.sock" status | grep -q '"connected": true' || fail "Puhuri 2 ei kirjautunut"
# A build without the feature answers this with an error; say so before the
# test wanders on.
"$CTL" --socket "$WORK/a.sock" omemo "$A_JID" >/dev/null 2>&1 \
    || fail "daemon ei ole OMEMO-käännös: (cd daemon && cargo build --features omemo)"

echo "--- vertainen käyntiin (slixmpp-omemo, $B_JID)"
PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=python "$PEER" \
    "$PROJECT/tools/testserver/omemo-peer.py" \
    "$B_JID" "$B_PASS" "$WORK/peer" "$A_JID" "$REPLY" \
    >"$WORK/peer.out" 2>"$WORK/peer.err" &
PEER_PID=$!
for _ in $(seq 1 60); do
    grep -q "^VALMIS" "$WORK/peer.out" 2>/dev/null && break
    sleep 0.5
done
grep -q "^VALMIS" "$WORK/peer.out" || {
    echo "--- vertaisen virheet:" >&2
    tail -15 "$WORK/peer.err" >&2
    fail "vertaisen käynnistys ei edennyt"
}

echo "--- 1/2: Puhuri 2 lähettää salattuna, vertaisen pitää avata se"
if ! OUT=$("$CTL" --socket "$WORK/a.sock" omemosend "$B_JID" "$YES" 2>&1); then
    echo "--- omemosend sanoi:" >&2
    echo "$OUT" >&2
    fail "Puhuri 2 ei pystynyt salaamaan viestiä"
fi
for _ in $(seq 1 40); do
    grep -q "^PURETTU: $YES" "$WORK/peer.out" 2>/dev/null && break
    sleep 0.5
done
grep -q "^PURETTU: $YES" "$WORK/peer.out" \
    || fail "vertainen ei saanut/avannut Puhurin viestiä"
echo "--- vertainen avasi: $YES"

echo "--- 2/2: vertaisen vastaus salattuna, Puhurin pitää avata se"
for _ in $(seq 1 40); do
    grep -q "OMEMO-viesti purettu" "$WORK/a.log" 2>/dev/null && break
    sleep 0.5
done
grep -q "OMEMO-viesti purettu" "$WORK/a.log" || fail "Puhuri 2 ei purkanut vertaisen vastausta"
"$CTL" --socket "$WORK/a.sock" messages "$B_JID" | grep -q "$REPLY" \
    || fail "vastaus ei tullut perille"
echo "--- Puhuri 2 avasi: $REPLY"

echo "--- vertaisen tulos"
grep -E "^(VALMIS|PURETTU|LÄHETETTY|SALAAMATON|PURKU EPAONNISTUI|SALAUS EPAONNISTUI|EI LAITTEITA|EI LUOTETTU)" \
    "$WORK/peer.out" || true

echo "--- 3/3: sormenjälki tallentui ja vahvistusmerkintä jää (O6)"
TRUST=$("$CTL" --socket "$WORK/a.sock" omemo-trust "$B_JID")
echo "$TRUST"
echo "$TRUST" | grep -Eq '"fingerprint": "[0-9a-f]{64}"' \
    || fail "vertaisen sormenjälkeä ei tallennettu"
DEVICE=$(echo "$TRUST" | sed -n 's/.*"device_id": \([0-9]*\).*/\1/p' | head -1)
[ -n "$DEVICE" ] || fail "laitetunnusta ei saatu"
"$CTL" --socket "$WORK/a.sock" omemo-verify "$B_JID" "$DEVICE" >/dev/null
"$CTL" --socket "$WORK/a.sock" omemo-trust "$B_JID" | grep -q '"verified": true' \
    || fail "vahvistusmerkintä ei jäänyt talteen"
echo "--- vahvistettu laite $DEVICE"

echo "--- 4/4: kuittaus (XEP-0184) löytää oikean rivin myös salatusta viestistä"
# The peer answers the receipt request; the id it echoes is our own row id,
# because that is what `chat_stanza` put on the wire.
"$CTL" --socket "$WORK/a.sock" messages "$B_JID" >"$WORK/a.messages"
grep -q '"delivered": true' "$WORK/a.messages" \
    || fail "kuittaus ei tullut perille salatusta viestistä"
grep -q 'kuittaus ei osunut viestiin' "$WORK/a.log" \
    && fail "kuittaus ei osunut viestiriviin"
echo "--- kuittaus tallentui viestiriville"

echo "--- OK: OMEMO 0.3 toimii riippumattoman asiakkaan kanssa molempiin suuntiin, sormenjälki on tallessa ja kuittaus löytää rivin"
