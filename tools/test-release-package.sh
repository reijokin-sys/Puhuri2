#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Inspect the built Puhuri 2 click for confined hooks and required artifacts.
set -eu

PROJECT=$(cd "$(dirname "$0")/.." && pwd)
PACKAGE=$(ls -t "$PROJECT"/dist/fi.reijo.puhuri2_*.click 2>/dev/null | head -1 || true)
[ -f "$PACKAGE" ] || {
    echo "FAIL: dist/-hakemistossa ei ole click-pakettia" >&2
    exit 1
}

python3 "$PROJECT/tools/make-license-bundle.py" --check
manifest=$(cat "$PROJECT/app/manifest.json")
maintainer=$(python3 -c \
    'import json,sys; print(json.load(open(sys.argv[1])).get("maintainer", ""))' \
    "$PROJECT/app/manifest.json")
case "$maintainer" in
    *"<"*"@"*">"*) ;;
    *)
        echo "FAIL: manifestin maintainer ei sisällä sähköpostiosoitetta" >&2
        exit 1
        ;;
esac
contents=$(ar p "$PACKAGE" data.tar.gz | tar -tz)

for hook in '"puhuri"' '"puhuri-daemon"' '"push"'; do
    printf '%s\n' "$manifest" | grep -q "$hook" || {
        echo "FAIL: manifestista puuttuu hook $hook" >&2
        exit 1
    }
done
if printf '%s\n' "$manifest" | grep -q 'unconfined\|puhuri-setup\|puhuri-indicator'; then
    echo "FAIL: manifestissa on poistettu tai unconfined hook" >&2
    exit 1
fi

for path in \
    ./LICENSE \
    ./THIRD_PARTY_NOTICES.md \
    ./THIRD_PARTY_LICENSES.txt \
    ./fi.reijo.puhuri2.apparmor \
    ./puhuri-daemon.apparmor \
    ./puhuri-daemon.desktop \
    ./push.apparmor \
    ./push.json \
    ./lib/aarch64-linux-gnu/bin/puhurid \
    ./lib/aarch64-linux-gnu/bin/puhuri-indicator \
    ./lib/aarch64-linux-gnu/bin/puhuri-push-helper \
    ./lib/aarch64-linux-gnu/bin/puhuri-postal-job
do
    printf '%s\n' "$contents" | grep -qx "$path" || {
        echo "FAIL: clickistä puuttuu $path" >&2
        exit 1
    }
done

push_helper_shebang=$(ar p "$PACKAGE" data.tar.gz | tar -xzO \
    ./lib/aarch64-linux-gnu/bin/puhuri-push-helper | head -n 1)
[ "$push_helper_shebang" = '#!/usr/bin/python3' ] || {
    echo "FAIL: push-helperin shebang ei sovi ubuntu-push-helper-profiiliin" >&2
    exit 1
}

for forbidden in puhuri-setup.apparmor puhuri-service-setup puhurid2.service; do
    if printf '%s\n' "$contents" | grep -q "$forbidden"; then
        echo "FAIL: click sisältää poistettavan helperin tai yksikön: $forbidden" >&2
        exit 1
    fi
done

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
ar p "$PACKAGE" data.tar.gz | tar -xz -C "$work"
if grep -R -I -F -q '/opt/click.ubuntu.com/' "$work"; then
    echo "FAIL: click sisältää OpenStoren hylkäämän kovan click-asennuspolun" >&2
    exit 1
fi
for file in LICENSE THIRD_PARTY_NOTICES.md THIRD_PARTY_LICENSES.txt; do
    cmp -s "$PROJECT/$file" "$work/$file" || {
        echo "FAIL: clickin $file ei vastaa projektin tiedostoa" >&2
        exit 1
    }
done

main_profile=$(ar p "$PACKAGE" data.tar.gz | tar -xzO ./fi.reijo.puhuri2.apparmor)
if printf '%s\n' "$main_profile" | grep -q '"template": "unconfined"'; then
    echo "FAIL: pääsovellus ei saa olla unconfined" >&2
    exit 1
fi
daemon_profile=$(ar p "$PACKAGE" data.tar.gz | tar -xzO ./puhuri-daemon.apparmor)
printf '%s\n' "$daemon_profile" | grep -q '"networking"' || {
    echo "FAIL: daemon-profiilista puuttuu networking" >&2
    exit 1
}
if printf '%s\n' "$daemon_profile" | grep -q '"template": "unconfined"'; then
    echo "FAIL: daemon-profiili ei saa olla unconfined" >&2
    exit 1
fi
push_profile=$(ar p "$PACKAGE" data.tar.gz | tar -xzO ./push.apparmor)
printf '%s\n' "$push_profile" | grep -q '"template": "ubuntu-push-helper"' || {
    echo "FAIL: push-hookin confined template puuttuu" >&2
    exit 1
}
echo "PASS: clickin hookit, helperit ja lisenssitarkistus täsmäävät"
