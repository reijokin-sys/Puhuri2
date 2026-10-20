#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Set up the local Prosody test server for Puhuri 2.
#
#   sudo tools/testserver/setup.sh
#
# What it does (all of it additive and reversible):
#   1. makes a local CA (once) and a server certificate for puhuri.test,
#   2. installs tools/testserver/puhuri.test.cfg.lua into Prosody's conf.d,
#   3. adds "127.0.0.1 puhuri.test" to /etc/hosts if it is missing,
#   4. trusts the CA in the system store (so our own client accepts it),
#   5. restarts Prosody and creates the two test accounts if they are missing.
#
# Undo: remove /etc/prosody/conf.d/puhuri.test.cfg.lua, the certificate pair,
# the hosts line, /usr/local/share/ca-certificates/puhuri-test-ca.crt and
# /etc/puhuri-test-ca, then run update-ca-certificates and restart Prosody.
set -eu

DOMAIN=puhuri.test
DIR=$(cd "$(dirname "$0")" && pwd)
CERTDIR=/etc/prosody/certs
CADIR=/etc/puhuri-test-ca
USER1=maija
USER2=vesa
# Test-only credentials are generated per setup run; they are never committed
# to the repository. The e2e scripts take the same values from the environment.
PASSWORD1=${PUHURI_TEST_PASSWORD1:-$(openssl rand -hex 12)}
PASSWORD2=${PUHURI_TEST_PASSWORD2:-$(openssl rand -hex 12)}

if [ "$(id -u)" != "0" ]; then
    echo "aja rootina: sudo tools/testserver/setup.sh" >&2
    exit 1
fi

echo "--- 1/5 paikallinen CA"
mkdir -p "$CADIR"
if [ ! -f "$CADIR/ca.key" ]; then
    openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
        -keyout "$CADIR/ca.key" -out "$CADIR/ca.crt" \
        -subj "/CN=Puhuri 2 test CA"
fi

echo "--- 2/5 palvelimen varmenne ($DOMAIN)"
if [ ! -f "$CADIR/$DOMAIN.crt" ]; then
    openssl req -newkey rsa:2048 -nodes \
        -keyout "$CADIR/$DOMAIN.key" -out "$CADIR/$DOMAIN.csr" \
        -subj "/CN=$DOMAIN" \
        -addext "subjectAltName=DNS:$DOMAIN,DNS:upload.$DOMAIN"
    openssl x509 -req -in "$CADIR/$DOMAIN.csr" \
        -CA "$CADIR/ca.crt" -CAkey "$CADIR/ca.key" -CAcreateserial \
        -out "$CADIR/$DOMAIN.crt" -days 3650 -copy_extensions copy
fi
install -m 640 -o prosody -g prosody "$CADIR/$DOMAIN.key" "$CERTDIR/$DOMAIN.key"
install -m 644 "$CADIR/$DOMAIN.crt" "$CERTDIR/$DOMAIN.crt"
# The file upload component (XEP-0363) is a subdomain and has its own
# certificate lookup, so it gets a copy under its own name (the SAN in the
# certificate covers both).
install -m 640 -o prosody -g prosody "$CADIR/$DOMAIN.key" "$CERTDIR/upload.$DOMAIN.key"
install -m 644 "$CADIR/$DOMAIN.crt" "$CERTDIR/upload.$DOMAIN.crt"
# A world-readable copy of the CA, so the test scripts can copy it to the
# phone without root on the computer.
install -m 644 "$CADIR/ca.crt" /usr/local/share/puhuri-test-ca.crt

echo "--- 3/5 Prosodyn asetukset"
install -m 644 "$DIR/$DOMAIN.cfg.lua" "/etc/prosody/conf.d/$DOMAIN.cfg.lua"

echo "--- 4/5 nimi ja luottamus"
grep -q "$DOMAIN" /etc/hosts || echo "127.0.0.1 $DOMAIN" >> /etc/hosts
grep -q "upload.$DOMAIN" /etc/hosts || echo "127.0.0.1 upload.$DOMAIN" >> /etc/hosts
install -m 644 "$CADIR/ca.crt" /usr/local/share/ca-certificates/puhuri-test-ca.crt
update-ca-certificates >/dev/null

echo "--- 5/5 kaynnistys ja testitilit"
systemctl restart prosody
sleep 1
systemctl is-active prosody

if ! prosodyctl register "$USER1" "$DOMAIN" "$PASSWORD1" 2>/dev/null; then
    echo "huom: $USER1@$DOMAIN on jo olemassa; salasana ei muuttunut."
    echo "      vaihda se tarvittaessa: prosodyctl passwd $USER1 $DOMAIN"
fi
if ! prosodyctl register "$USER2" "$DOMAIN" "$PASSWORD2" 2>/dev/null; then
    echo "huom: $USER2@$DOMAIN on jo olemassa; salasana ei muuttunut."
    echo "      vaihda se tarvittaessa: prosodyctl passwd $USER2 $DOMAIN"
fi

echo "valmis: $USER1@$DOMAIN / $USER2@$DOMAIN"
echo "test credentials (export these for the e2e scripts):"
echo "  export PUHURI_TEST_PASSWORD1='$PASSWORD1'"
echo "  export PUHURI_TEST_PASSWORD2='$PASSWORD2'"
