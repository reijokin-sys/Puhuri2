#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Shared password-safe root command helper for phone tooling.
#
# Source this file, then call:
#
#   phone_sudo_remote 'click install --user phablet package.click'
#
# The command argument must never contain the password. The password itself
# travels only through the standard input of adb/ssh, never through argv.

SERIAL=${PUHELIN_SARJANUMERO:?aseta PUHELIN_SARJANUMERO}
HOST=${PHONE_HOST:?aseta PHONE_HOST}
KEY=${PHONE_KEY:?aseta PHONE_KEY}
MODE=${PUHURI_DEPLOY:-ssh}

phone_sudo_remote() {
    phone_sudo_command=${1:?phone_sudo_remote needs a command}
    [ -n "${PUHELIMEN_SALASANA:-}" ] || {
        echo "VIRHE: aseta PUHELIMEN_SALASANA=... (puhelimen lukituksen salasana)" >&2
        return 1
    }

    if [ "$MODE" = "adb" ]; then
        printf '%s\n' "$PUHELIMEN_SALASANA" |
            adb -s "$SERIAL" shell "sudo -S -p '' $phone_sudo_command"
    else
        # shellcheck disable=SC2086
        printf '%s\n' "$PUHELIMEN_SALASANA" |
            ssh -F /dev/null -o BatchMode=yes -o ConnectTimeout=10 \
                -i "$KEY" "$HOST" "sudo -S -p '' $phone_sudo_command"
    fi
}
