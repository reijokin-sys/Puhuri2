#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Install Puhuri 2's click package on the phone.
#
#   PUHELIMEN_SALASANA=... tools/deploy.sh
#
# The click contains the UI, the confined daemon and the notification helper.
# Deployment installs persistent user services that keep XMPP alive while the
# UI is closed.
#
# Everything goes over SSH (the phone is usually on Wi-Fi without USB);
# PUHURI_DEPLOY=adb switches to adb when the cable is plugged in.
set -eu

PROJECT=$(cd "$(dirname "$0")/.." && pwd)
. "$PROJECT/tools/phone-sudo-remote.sh"

SERIAL=${PUHELIN_SARJANUMERO:?aseta PUHELIN_SARJANUMERO}
HOST=${PHONE_HOST:?aseta PHONE_HOST}
KEY=${PHONE_KEY:?aseta PHONE_KEY}
MODE=${PUHURI_DEPLOY:-ssh}

SSH="ssh -F /dev/null -o BatchMode=yes -o ConnectTimeout=10 -i $KEY"

for binary in puhurid puhuri-ctl; do
    [ -x "$PROJECT/build/$binary" ] || {
        echo "Puuttuu $PROJECT/build/$binary. Aja ensin tools/build-daemon.sh" >&2
        exit 1
    }
done

# The click is rebuilt here on purpose. Installing whatever .click happened to
# be newest in dist/ once shipped a stale user interface to the phone: the
# linking page still showed a QR code the daemon had already replaced, and
# every scan failed. Set PUHURI_SKIP_BUILD=1 to install the package as it is.
if [ "${PUHURI_SKIP_BUILD:-0}" != "1" ]; then
    echo "--- click-paketti rakennetaan lähdekoodista"
    "$PROJECT/tools/build.sh"
fi

send() {  # send <local file> <remote path>
    if [ "$MODE" = "adb" ]; then
        adb -s "$SERIAL" push "$1" "$2" >/dev/null
    else
        # shellcheck disable=SC2086
        $SSH "$HOST" "cat > '$2'" < "$1"
    fi
}

remote() {  # remote <shell command>
    if [ "$MODE" = "adb" ]; then
        adb -s "$SERIAL" shell "$1"
    else
        # shellcheck disable=SC2086
        $SSH "$HOST" "$1"
    fi
}

PACKAGE=$(ls -t "$PROJECT"/dist/*.click 2>/dev/null | head -1 || true)
[ -n "$PACKAGE" ] || {
    echo "Ei .click-pakettia hakemistossa dist/. Aja ensin tools/build.sh" >&2
    exit 1
}
# The Rust binary is not rebuilt automatically (it takes a long time), but a
# newer source tree than the binary is worth saying out loud.
if [ -n "$(find "$PROJECT/daemon/src" -newer "$PROJECT/build/puhurid" -name '*.rs' -print -quit)" ]; then
    echo "VAROITUS: daemon/src on uudempi kuin build/puhurid - aja tools/build-daemon.sh" >&2
fi

NAME=$(basename "$PACKAGE")
VERSION=$(printf '%s\n' "$NAME" | sed -n \
    's/^fi\.reijo\.puhuri2_\(.*\)_arm64\.click$/\1/p')
[ -n "$VERSION" ] || {
    echo "VIRHE: clickin nimestä ei saatu versiota: $NAME" >&2
    exit 1
}

echo "--- aiemmat palvelut pysäytetään"
remote \
    'export XDG_RUNTIME_DIR=/run/user/$(id -u);
     systemctl --user stop puhuri2-indicator.service puhurid2.service 2>/dev/null || true'

echo "--- click-paketti"
send "$PACKAGE" "/home/phablet/$NAME"
[ -n "${PUHELIMEN_SALASANA:-}" ] || {
    echo "VIRHE: aseta PUHELIMEN_SALASANA=... (click-asennus tarvitsee rootin)" >&2
    exit 1
}
phone_sudo_remote \
    "click install --user phablet --allow-unauthenticated /home/phablet/$NAME"
remote "rm -f /home/phablet/$NAME"

# Reinstalling the same version does not regenerate the AppArmor profile, so a
# change to the click's policy groups (for example adding `audio`) would never
# take effect. That cost an evening: the sound was denied silently.
echo "--- AppArmor-profiili uudistetaan"
phone_sudo_remote "aa-clickhook -f >/dev/null 2>&1; echo uudistettu"

echo "--- pysyvät käyttäjäpalvelut"
ROOT="/opt/click.ubuntu.com/fi.reijo.puhuri2/$VERSION"
UI_PROFILE="fi.reijo.puhuri2_puhuri_$VERSION"
DAEMON_PROFILE="fi.reijo.puhuri2_puhuri-daemon_$VERSION"
RENDERED=$(mktemp -d)
trap 'rm -rf "$RENDERED"' EXIT INT TERM
for unit in puhurid2 puhuri2-indicator; do
    sed -e "s|@ROOT@|$ROOT|g" \
        -e "s|@UI_PROFILE@|$UI_PROFILE|g" \
        -e "s|@DAEMON_PROFILE@|$DAEMON_PROFILE|g" \
        "$PROJECT/service/$unit.service.in" >"$RENDERED/$unit.service"
    send "$RENDERED/$unit.service" \
        "/home/phablet/.config/systemd/user/$unit.service"
done

# A running fallback child owns the socket and would prevent the persistent
# daemon from starting. Stop only this app's known process paths.
phone_sudo_remote \
    "sh -c 'for p in \$(pgrep -f \"^/usr/lib/qt5/bin/qmlscene qml/Main[.]qml\$\" || true); do kill -9 \$p; done; for p in \$(pgrep -f \"^$ROOT/lib/aarch64-linux-gnu/bin/puhurid --engine xmpp\$\" || true); do kill -9 \$p; done; for p in \$(pgrep -f \"^$ROOT/lib/aarch64-linux-gnu/bin/puhuri-indicator --notifications-only\$\" || true); do kill -9 \$p; done'"

remote \
    'export XDG_RUNTIME_DIR=/run/user/$(id -u);
     systemctl --user daemon-reload
     systemctl --user enable --now puhurid2.service
     systemctl --user enable --now puhuri2-indicator.service
     for i in $(seq 1 100); do
         [ -S "$XDG_RUNTIME_DIR/fi.reijo.puhuri2/socket" ] && break
         sleep 0.1
     done'

echo "--- sovellus käynnistetään"
remote \
    'export XDG_RUNTIME_DIR=/run/user/$(id -u);
     version=$(basename "$(readlink -f /opt/click.ubuntu.com/fi.reijo.puhuri2/current)");
     setsid lomiri-app-launch "fi.reijo.puhuri2_puhuri_$version" </dev/null >/dev/null 2>&1 &
     sleep 2
     systemctl --user is-active puhurid2.service
     systemctl --user is-active puhuri2-indicator.service'

echo "--- tila"
remote \
    'export XDG_RUNTIME_DIR=/run/user/$(id -u); /opt/click.ubuntu.com/fi.reijo.puhuri2/current/lib/aarch64-linux-gnu/bin/puhuri-ctl status'
