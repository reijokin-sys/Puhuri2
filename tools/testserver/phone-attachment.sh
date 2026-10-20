#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Send a picture from the phone to a peer on this computer.
#
#   tools/testserver/phone-attachment.sh
#
# The phone must be logged in (its service remembers the account). The file
# upload component is a subdomain, so the phone needs a hosts entry for it as
# well; the script adds it (test-only, like the rest of the test server).
#
# The device password comes from `PUHELIMEN_SALASANA`, like the other tools;
# it is never written into the repository.
set -eu

PROJECT=$(cd "$(dirname "$0")/../.." && pwd)
. "$PROJECT/tools/phone-sudo-remote.sh"

KEY=${PHONE_KEY:?aseta PHONE_KEY}
HOST=${PHONE_HOST:?aseta PHONE_HOST}
POINT=${PUHURI_POINT:?aseta PUHURI_POINT}
A_JID=${1:-maija@puhuri.test}
B_JID=${2:-vesa@puhuri.test}
B_PASS=${3:-${PUHURI_TEST_PASSWORD2:?set PUHURI_TEST_PASSWORD2 (setup.sh prints it)}}

if [ -z "${PUHELIMEN_SALASANA:-}" ]; then
    echo "VIRKE: aseta PUHELIMEN_SALASANA=... (puhelimen lukituksen salasana)" >&2
    exit 1
fi

SSH="ssh -F /dev/null -o BatchMode=yes -o ConnectTimeout=10 -i $KEY"
WORK=$(mktemp -d)
V_PID=""
cleanup() {
    [ -n "$V_PID" ] && kill "$V_PID" 2>/dev/null || true
    rm -rf "$WORK"
}
trap cleanup EXIT

echo "--- upload-nimen ohitus puhelimessa (vain testi)"
phone_sudo_remote \
    "sh -c 'grep -q upload.puhuri.test /etc/hosts || echo \"$POINT upload.puhuri.test\" >> /etc/hosts'; getent hosts upload.puhuri.test"

echo "--- koneen vertaisclient ($B_JID)"
"$PROJECT/daemon/target/debug/puhurid" \
    --data-dir "$WORK/peer" --socket "$WORK/peer.sock" --engine xmpp \
    >"$WORK/peer.log" 2>&1 &
V_PID=$!
for _ in $(seq 1 50); do [ -S "$WORK/peer.sock" ] && break; sleep 0.2; done
"$PROJECT/daemon/target/debug/puhuri-ctl" --socket "$WORK/peer.sock" \
    login "$B_JID" "$B_PASS" >/dev/null
sleep 3

echo "--- pieni kuva puhelimeen"
python3 - "$WORK/kuva.png" <<'PY'
import sys
from PIL import Image
Image.new("RGB", (96, 96), (40, 120, 200)).save(sys.argv[1])
PY
$SSH "$HOST" "cat > /tmp/puhuri-kuva.png" < "$WORK/kuva.png"

echo "--- puhelin lahettaa kuvan"
$SSH "$HOST" "export XDG_RUNTIME_DIR=/run/user/\$(id -u);
  /opt/click.ubuntu.com/fi.reijo.puhuri2/current/lib/aarch64-linux-gnu/bin/puhuri-ctl sendimage $B_JID /tmp/puhuri-kuva.png"
sleep 8

echo "--- koneen vastaanotto"
"$PROJECT/daemon/target/debug/puhuri-ctl" --socket "$WORK/peer.sock" \
    messages "$A_JID" | grep -E '"text"|"image"|"content_type"|"path"|"voice"'
ls -l "$WORK/peer/attachments/" 2>/dev/null || echo "(ei liitteita)"
echo "--- koneen virheet"
grep -i virhe "$WORK/peer.log" || echo "(ei virheita)"
