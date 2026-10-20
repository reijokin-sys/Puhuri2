#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Attachments between two local sessions: a picture and a voice message.
#
#   tools/testserver/e2e-attachments.sh
#
# The picture is made with Python/Pillow; the voice message is recorded with
# parec from the computer's own audio input, which is why this runs on the
# desktop rather than on the phone. Both need the local Prosody
# (tools/testserver/setup.sh) with its file upload component.
set -eu

PROJECT=$(cd "$(dirname "$0")/../.." && pwd)
DAEMON=${PUHURI_BIN:-$PROJECT/daemon/target/debug/puhurid}
CTL=${PUHURI_CTL:-$PROJECT/daemon/target/debug/puhuri-ctl}
A_JID=${1:-maija@puhuri.test}
B_JID=${2:-vesa@puhuri.test}
A_PASS=${3:-${PUHURI_TEST_PASSWORD1:?set PUHURI_TEST_PASSWORD1 (setup.sh prints it)}}
B_PASS=${4:-${PUHURI_TEST_PASSWORD2:?set PUHURI_TEST_PASSWORD2 (setup.sh prints it)}}

WORK=$(mktemp -d)
A_PID=""
B_PID=""
LISTEN_PID=""
cleanup() {
    [ -n "$A_PID" ] && kill "$A_PID" 2>/dev/null || true
    [ -n "$B_PID" ] && kill "$B_PID" 2>/dev/null || true
    [ -n "$LISTEN_PID" ] && kill "$LISTEN_PID" 2>/dev/null || true
    cp "$WORK/a.log" "${TMPDIR:-/tmp}/puhuri-att-a.log" 2>/dev/null || true
    cp "$WORK/b.log" "${TMPDIR:-/tmp}/puhuri-att-b.log" 2>/dev/null || true
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

ctl() { socket=$1; shift; "$CTL" --socket "$socket" "$@"; }

ctl "$WORK/a.sock" login "$A_JID" "$A_PASS" >/dev/null
ctl "$WORK/b.sock" login "$B_JID" "$B_PASS" >/dev/null
for _ in $(seq 1 40); do
    if ctl "$WORK/a.sock" status | grep -q '"connected": true' \
            && ctl "$WORK/b.sock" status | grep -q '"connected": true'; then
        break
    fi
    sleep 0.5
done
# The upload service is found with a disco round trip right after login.
sleep 2

CAPTION="kuvateksti kuvan mukana"

echo "--- kuva kuvatekstillä"
python3 - "$WORK/kuva.png" <<'PY'
import sys
from PIL import Image
Image.new("RGB", (64, 64), (200, 80, 40)).save(sys.argv[1])
PY
# A's own events are kept while the picture goes out: the upload's progress
# (XEP-0363, plan item 5b) and the receipt of the file message (XEP-0184, the
# leftover of item 5a) both arrive on that socket.
ctl "$WORK/a.sock" listen >"$WORK/a.events" 2>&1 &
LISTEN_PID=$!
sleep 1
ctl "$WORK/a.sock" sendimage "$B_JID" "$WORK/kuva.png" "$CAPTION" >/dev/null
sleep 8
kill "$LISTEN_PID" 2>/dev/null || true
LISTEN_PID=""
grep -o '"event":"[a-z.]*"' "$WORK/a.events" | sort | uniq -c || true

echo "--- kuvateksti perillä (5c)"
ctl "$WORK/b.sock" messages "$A_JID" >"$WORK/b.messages"
python3 - "$WORK/b.messages" "$CAPTION" <<'PY'
import json
import sys

messages = json.load(open(sys.argv[1]))["result"]
pictures = [m for m in messages if m.get("attachment", {}).get("image")]
if not pictures:
    raise SystemExit("EI KELPAA: kuva ei tullut perille")
text = pictures[-1]["text"]
print("kuvateksti: %r" % text)
if text != sys.argv[2]:
    raise SystemExit("EI KELPAA: kuvateksti ei tullut perille: %r" % text)
PY

echo "--- lähetyksen edistyminen ja liiteviestin kuittaus (5b, 5a)"
ctl "$WORK/a.sock" messages "$B_JID" >"$WORK/a.messages"
python3 - "$WORK/a.messages" "$WORK/a.events" <<'PY'
import json
import sys

messages = json.load(open(sys.argv[1]))["result"]
events = [json.loads(line) for line in open(sys.argv[2]) if line.strip()]
progress = [event for event in events if event.get("event") == "attachment.progress"]
if not progress:
    raise SystemExit("EI KELPAA: lähetyksen edistymisestä ei tullut yhtään tapahtumaa")
last = progress[-1]["data"]
print("edistymisraportteja: %d, viimeinen %s/%s tavua"
      % (len(progress), last["sent"], last["total"]))
if last["sent"] != last["total"]:
    raise SystemExit("EI KELPAA: viimeinen raportti ei ollut perillä (sent != total)")

pictures = [m for m in messages if m.get("attachment", {}).get("image")]
if not pictures:
    raise SystemExit("EI KELPAA: omaa kuvariviä ei löytynyt")
row = pictures[-1]
if row.get("delivered") is not True:
    raise SystemExit(
        "EI KELPAA: liiteviesti ei saanut kuittausta (docs/TODO.md, kohdan 5a jäänne)"
    )
print("liiteviestin kuittaus tallessa rivillä %s" % row["id"])
PY

echo "--- aaniviesti"
ctl "$WORK/a.sock" voicestart "$B_JID" >/dev/null
sleep 3
# Stopping only stops: the recording becomes a draft, and sending it is a
# second step (the same as the Send button in the interface).
ctl "$WORK/a.sock" voicestop >/dev/null
ctl "$WORK/a.sock" voicesend >/dev/null
sleep 5

echo "--- ääniviestin muoto perillä (48 kHz mono; käyttäjän laatuhavainto 16.10.)"
python3 - "$WORK/b" <<'PY'
import glob
import os
import sys
import wave

paths = sorted(glob.glob(os.path.join(sys.argv[1], "attachments", "*.wav")))
if not paths:
    raise SystemExit("EI KELPAA: ääniviestiä ei tullut perille")
with wave.open(paths[-1]) as wav:
    rate = wav.getframerate()
    channels = wav.getnchannels()
    width = wav.getsampwidth()
    frames = wav.getnframes()
print("%s: %d Hz, %d kanava, %d bittiä, %.1f s"
      % (os.path.basename(paths[-1]), rate, channels, width * 8, frames / rate))
if rate != 48000 or channels != 1:
    raise SystemExit("EI KELPAA: äänitys ei ole 48 kHz mono")
PY

echo "--- B:n vastaanottamat viestit"
ctl "$WORK/b.sock" messages "$A_JID" | grep -E '"text"|"voice"|"image"|"content_type"|"path"'
echo "--- B:n liitekansio"
ls -l "$WORK/b/attachments/" 2>/dev/null || echo "(ei liitteita)"
echo "--- A:n virheet"
grep -i 'virhe' "$WORK/a.log" || echo "(ei virheita)"
echo "--- B:n virheet"
grep -i 'virhe' "$WORK/b.log" || echo "(ei virheita)"
