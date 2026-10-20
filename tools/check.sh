#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Check the installed Puhuri 2 on the phone.
#
#   tools/check.sh [--launch] [--qml]
#
# --qml loads the installed user interface with qmlscene and reports any QML
#       error. That is how a missing component (BusyIndicator is not part of
#       Lomiri.Components) was found, so the check is here now.
set -eu

PROJECT=$(cd "$(dirname "$0")/.." && pwd)
SERIAL=${PUHELIN_SARJANUMERO:?aseta PUHELIN_SARJANUMERO}
HOST=${PHONE_HOST:?aseta PHONE_HOST}
KEY=${PHONE_KEY:?aseta PHONE_KEY}
MODE=${PUHURI_DEPLOY:-ssh}
LAUNCH=0
[ "${1:-}" = "--launch" ] && LAUNCH=1
[ "${1:-}" = "--qml" ] && QML=1
[ "${2:-}" = "--qml" ] && QML=1
QML=${QML:-0}

remote() {
    if [ "$MODE" = "adb" ]; then
        adb -s "$SERIAL" shell "$1"
    else
        # shellcheck disable=SC2086
        ssh -F /dev/null -o BatchMode=yes -o ConnectTimeout=10 -i "$KEY" "$HOST" "$1"
    fi
}

echo "--- click list"
remote "click list | grep fi.reijo.puhuri2 || echo 'EI ASENNETTU'"

echo "--- confined child processes"
remote 'pgrep -x puhurid || true; pgrep -x puhuri-indicator || true'

echo "--- tila"
remote 'export XDG_RUNTIME_DIR=/run/user/$(id -u); /opt/click.ubuntu.com/fi.reijo.puhuri2/current/lib/aarch64-linux-gnu/bin/puhuri-ctl status' || true

if [ "$LAUNCH" = "1" ]; then
    echo "--- käynnistys"
    VERSION=$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["version"])' \
        "$PROJECT/app/manifest.json")
    remote "export XDG_RUNTIME_DIR=/run/user/\$(id -u); lomiri-app-launch fi.reijo.puhuri2_puhuri_$VERSION"
fi

if [ "$QML" = "1" ]; then
    echo "--- QML-lataustesti (virheet näkyvät tässä)"
    remote 'cd /opt/click.ubuntu.com/fi.reijo.puhuri2/current &&
      QT_QPA_PLATFORM=ubuntumirclient XDG_RUNTIME_DIR=/run/user/$(id -u) \
        timeout 10 qmlscene qml/Main.qml 2>&1 |
      grep -v "^dlopen failed" | grep -v "Creating a QMirClientScreen" || true'
    # The conversation view is only instantiated when a chat is opened, so it
    # is loaded here on its own as well.
    remote 'cd /opt/click.ubuntu.com/fi.reijo.puhuri2/current &&
      QT_QPA_PLATFORM=ubuntumirclient XDG_RUNTIME_DIR=/run/user/$(id -u) \
        timeout 10 qmlscene qml/ConversationPage.qml 2>&1 |
      grep -v "^dlopen failed" | grep -v "Creating a QMirClientScreen" || true'
    # The same for the pages that are only pushed on demand (the roster, where
    # a new conversation starts).
    remote 'cd /opt/click.ubuntu.com/fi.reijo.puhuri2/current &&
      QT_QPA_PLATFORM=ubuntumirclient XDG_RUNTIME_DIR=/run/user/$(id -u) \
        timeout 10 qmlscene qml/RosterPage.qml 2>&1 |
      grep -v "^dlopen failed" | grep -v "Creating a QMirClientScreen" || true'
    # And the trust view, which shows the fingerprints.
    remote 'cd /opt/click.ubuntu.com/fi.reijo.puhuri2/current &&
      QT_QPA_PLATFORM=ubuntumirclient XDG_RUNTIME_DIR=/run/user/$(id -u) \
        timeout 10 qmlscene qml/TrustPage.qml 2>&1 |
      grep -v "^dlopen failed" | grep -v "Creating a QMirClientScreen" || true'
    # The login page: the password field and its "show the password" tick.
    remote 'cd /opt/click.ubuntu.com/fi.reijo.puhuri2/current &&
      QT_QPA_PLATFORM=ubuntumirclient XDG_RUNTIME_DIR=/run/user/$(id -u) \
        timeout 10 qmlscene qml/LoginPage.qml 2>&1 |
      grep -v "^dlopen failed" | grep -v "Creating a QMirClientScreen" || true'
    # Settings: the language selector and the account rows.
    remote 'cd /opt/click.ubuntu.com/fi.reijo.puhuri2/current &&
      QT_QPA_PLATFORM=ubuntumirclient XDG_RUNTIME_DIR=/run/user/$(id -u) \
        timeout 10 qmlscene qml/SettingsPage.qml 2>&1 |
      grep -v "^dlopen failed" | grep -v "Creating a QMirClientScreen" || true'
    # The irreversible local-data confirmation.
    remote 'cd /opt/click.ubuntu.com/fi.reijo.puhuri2/current &&
      QT_QPA_PLATFORM=ubuntumirclient XDG_RUNTIME_DIR=/run/user/$(id -u) \
        timeout 10 qmlscene qml/DeleteDataDialog.qml 2>&1 |
      grep -v "^dlopen failed" | grep -v "Creating a QMirClientScreen" || true'
    # The nickname dialog, which is opened from the conversation menu.
    remote 'cd /opt/click.ubuntu.com/fi.reijo.puhuri2/current &&
      QT_QPA_PLATFORM=ubuntumirclient XDG_RUNTIME_DIR=/run/user/$(id -u) \
        timeout 10 qmlscene qml/NicknameDialog.qml 2>&1 |
      grep -v "^dlopen failed" | grep -v "Creating a QMirClientScreen" || true'
    # The round Send ball, which the writing bar and the draft preview share.
    remote 'cd /opt/click.ubuntu.com/fi.reijo.puhuri2/current &&
      QT_QPA_PLATFORM=ubuntumirclient XDG_RUNTIME_DIR=/run/user/$(id -u) \
        timeout 10 qmlscene qml/SendButton.qml 2>&1 |
      grep -v "^dlopen failed" | grep -v "Creating a QMirClientScreen" || true'
    # The writing bar's icon buttons (picture and microphone).
    remote 'cd /opt/click.ubuntu.com/fi.reijo.puhuri2/current &&
      QT_QPA_PLATFORM=ubuntumirclient XDG_RUNTIME_DIR=/run/user/$(id -u) \
        timeout 10 qmlscene qml/ComposerButton.qml 2>&1 |
      grep -v "^dlopen failed" | grep -v "Creating a QMirClientScreen" || true'
    echo "--- (tyhjä tulos = ei QML-virheitä)"
fi
