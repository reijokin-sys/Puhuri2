#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Remove Puhuri 2 from the phone.
#
#   PUHELIMEN_SALASANA=... ./uninstall.sh [--purge]
#
# --purge also removes Puhuri's private data, settings and cache - without it
# the state stays, which is what an upgrade wants.
set -eu

PROJECT=$(cd "$(dirname "$0")" && pwd)
. "$PROJECT/tools/phone-sudo-remote.sh"

PURGE=0
[ "${1:-}" = "--purge" ] && PURGE=1

remote() {
    if [ "$MODE" = "adb" ]; then
        adb -s "$SERIAL" shell "$1"
    else
        # shellcheck disable=SC2086
        ssh -F /dev/null -o BatchMode=yes -o ConnectTimeout=10 -i "$KEY" "$HOST" "$1"
    fi
}

echo "--- palvelut pois"
remote 'export XDG_RUNTIME_DIR=/run/user/$(id -u);
  systemctl --user disable --now puhuri2-indicator.service 2>/dev/null || true;
  systemctl --user disable --now puhurid2.service 2>/dev/null || true;
  systemctl --user reset-failed puhuri2-indicator.service puhurid2.service 2>/dev/null || true;
  rm -f ~/.config/systemd/user/puhuri2-indicator.service ~/.config/systemd/user/puhurid2.service;
  systemctl --user daemon-reload'

echo "--- click pois"
[ -n "${PUHELIMEN_SALASANA:-}" ] || {
    echo "VIRKE: aseta PUHELIMEN_SALASANA=..." >&2
    exit 1
}
phone_sudo_remote \
    "click unregister --user=phablet fi.reijo.puhuri2"
phone_sudo_remote \
    "find /var/cache/apparmor -mindepth 2 -maxdepth 2 -type d -name 'click_fi.reijo.puhuri2*' -exec rm -rf {} +"

if [ "$PURGE" = "1" ]; then
    echo "--- tila pois"
    remote 'rm -rf ~/.local/share/fi.reijo.puhuri2 ~/.config/fi.reijo.puhuri2 ~/.cache/fi.reijo.puhuri2'
fi

echo "--- jäljellä:"
remote "click list | grep fi.reijo.puhuri2 || echo 'ei asennettu'"
