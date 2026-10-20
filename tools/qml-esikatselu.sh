#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Show the conversation view on the phone with made-up messages.
#
#   tools/qml-esikatselu.sh          # start it (the writing bar, i.e. the
#                                    # round Send ball, is the default view)
#   tools/qml-esikatselu.sh luonnos  # with a draft waiting for Send
#   tools/qml-esikatselu.sh --stop   # stop it
#
# Why: the conversation view only exists while a chat is open, so a layout
# mistake there (bubbles on top of each other, for instance) is otherwise only
# seen after tapping through the interface. This loads the *installed* page
# with a ListModel of long texts and a voice message.
set -eu

PROJECT=$(cd "$(dirname "$0")/.." && pwd)
HOST=${PHONE_HOST:?aseta PHONE_HOST}
KEY=${PHONE_KEY:?aseta PHONE_KEY}
SSH="ssh -F /dev/null -o BatchMode=yes -o ConnectTimeout=10 -i $KEY"

# `luonnos` asks the preview to show a draft waiting for Send; anything else
# (or nothing) shows the writing bar, which is where the round Send ball is.
MODE=${1:-keskustelu}

if [ "${1:-}" = "--stop" ]; then
    # shellcheck disable=SC2086
    # Every preview is stopped, not only the last: they used to pile up on the
    # screen (four were running at once on 2026-10-16, and the topmost, empty
    # one is what a screenshot then showed). The pattern is anchored: an
    # unanchored one also matches the remote shell's own command line, and
    # pkill then kills the session that is running it (docs/DEVICE_WORKFLOW.md).
    $SSH "$HOST" 'pkill -f "^/usr/lib/qt5/bin/qmlscene /tmp/puhuri-esikatselu" || true;
      sleep 1; pgrep -c -f "^/usr/lib/qt5/bin/qmlscene /tmp/puhuri-esikatselu" || true'
    echo "esikatselu pysäytetty (0 = ei enää yhtään)"
    exit 0
fi

# shellcheck disable=SC2086
# `luonnos` asks the preview to show the draft (the picture or voice message
# waiting for Send, with the caption field above it). The choice is written
# into the **copy** of the page that goes to the phone: `qmlscene` reads every
# extra argument as another QML file, so a word on the command line cannot
# carry it - it tried to open `/tmp/luonnos` and the screen stayed empty
# (found 2026-10-16, and it is why the draft view had never actually been
# seen).
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
if [ "$MODE" = "luonnos" ]; then
    sed 's/^\( *\)property bool withDraft: false$/\1property bool withDraft: true/' \
        "$PROJECT/tools/qml-esikatselu.qml" >"$WORK/qml-esikatselu.qml"
    if ! grep -q "property bool withDraft: true" "$WORK/qml-esikatselu.qml"; then
        echo "EI KELPAA: luonnostilan rivin korvaus ei osunut" >&2
        exit 1
    fi
else
    cp "$PROJECT/tools/qml-esikatselu.qml" "$WORK/qml-esikatselu.qml"
fi

# shellcheck disable=SC2086
scp -F /dev/null -o BatchMode=yes -i "$KEY" \
    "$WORK/qml-esikatselu.qml" "$HOST:/tmp/puhuri-esikatselu.qml" >/dev/null
# The picture the preview shows: a made-up file, so the real account is not
# touched and no picture of anybody's is used.
# shellcheck disable=SC2086
scp -F /dev/null -o BatchMode=yes -i "$KEY" \
    "$PROJECT/tools/esikatselu-kuva.jpg" "$HOST:/tmp/puhuri-esikatselu-kuva.jpg" >/dev/null

# shellcheck disable=SC2086
$SSH "$HOST" 'export XDG_RUNTIME_DIR=/run/user/$(id -u); cd /tmp &&
  (QT_QPA_PLATFORM=ubuntumirclient setsid qmlscene /tmp/puhuri-esikatselu.qml \
    < /dev/null > /tmp/puhuri-esikatselu.log 2>&1 &) ; sleep 8;
  tail -3 /tmp/puhuri-esikatselu.log'

echo "--- esikatselu näkyy puhelimen ruudulla (virheet yllä, tyhjä = ei virheitä)"
