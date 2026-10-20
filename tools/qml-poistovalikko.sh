#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Show a delete menu (the dialog a long press opens) on the phone.
#
#   tools/qml-poistovalikko.sh [viesti|keskustelu]   # start it (default: viesti)
#   tools/qml-poistovalikko.sh --stop                # stop it
#
# Why: `backkeyd` can tap and swipe but not keep a finger down, so the delete
# menu cannot be opened through it. This shows the installed
# MessageActionsDialog.qml or ConversationActionsDialog.qml as they are, so
# the menus and their buttons can be looked at.
set -eu

PROJECT=$(cd "$(dirname "$0")/.." && pwd)
HOST=${PHONE_HOST:?aseta PHONE_HOST}
KEY=${PHONE_KEY:?aseta PHONE_KEY}
SSH="ssh -F /dev/null -o BatchMode=yes -o ConnectTimeout=10 -i $KEY"

WHICH=${1:-viesti}

if [ "$WHICH" = "--stop" ]; then
    # shellcheck disable=SC2086
    # The pattern is anchored, or pkill would also match the remote shell
    # running it (docs/DEVICE_WORKFLOW.md).
    $SSH "$HOST" 'pkill -f "^/usr/lib/qt5/bin/qmlscene /tmp/puhuri-.*valikko.qml" || true'
    echo "poistovalikko pysäytetty"
    exit 0
fi

case "$WHICH" in
    viesti)       FILE=qml-poistovalikko.qml ;;
    keskustelu)   FILE=qml-keskusteluvalikko.qml ;;
    *) echo "käyttö: $0 [viesti|keskustelu|--stop]" >&2; exit 2 ;;
esac

# shellcheck disable=SC2086
scp -F /dev/null -o BatchMode=yes -i "$KEY" \
    "$PROJECT/tools/$FILE" "$HOST:/tmp/$FILE" >/dev/null

# shellcheck disable=SC2086
$SSH "$HOST" 'export XDG_RUNTIME_DIR=/run/user/$(id -u); cd /tmp &&
  (QT_QPA_PLATFORM=ubuntumirclient setsid qmlscene /tmp/'"$FILE"' \
     < /dev/null > /tmp/puhuri-poistovalikko.log 2>&1 &) ; sleep 8;
  grep -v "^dlopen failed" /tmp/puhuri-poistovalikko.log | tail -3'

echo "--- $WHICH-valikko näkyy puhelimen ruudulla (virheet yllä, tyhjä = ei virheitä)"
