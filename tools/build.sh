#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Build the Puhuri 2 click package into dist/.
#
#   tools/build.sh
#
# The UI is QML + Python. Its confined daemon child is a separate Rust binary,
# built for the phone by tools/build-daemon.sh.
set -eu

PROJECT=$(cd "$(dirname "$0")/.." && pwd)
cd "$PROJECT"

if command -v click >/dev/null 2>&1; then
    CLICK=click
else
    CLICK="$PROJECT/tools/click-host.sh"
fi

python3 tools/make-icon.py
python3 tools/make-license-bundle.py --check

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

stage="$work/package"
mkdir -p "$stage"
cp app/manifest.json app/fi.reijo.puhuri2.apparmor \
   app/puhuri-daemon.apparmor app/puhuri-daemon.desktop \
   app/push.apparmor app/push.json \
   app/puhuri.desktop app/puhuri.py "$stage/"
cp LICENSE THIRD_PARTY_NOTICES.md THIRD_PARTY_LICENSES.txt "$stage/"
# `app/Lomiri` carries Puhuri 2's own theme modules. They belong in the click's
# **root** - not under `qml/` - because the toolkit's theme loader finds a theme
# module on the QML import paths, and `qmlscene` finds it in the working
# directory, which for a click is the click root (measured on the phone
# 2026-10-16, docs/TEEMAT-TOTEUTUS.md).
cp -r app/qml app/assets app/lib app/Lomiri app/Puhuri "$stage/"
[ -f app/Puhuri/Audio/libpuhuriaudio.so ] || {
    echo "Puuttuu app/Puhuri/Audio/libpuhuriaudio.so. Aja tools/build-qaudio-plugin.sh" >&2
    exit 1
}
# Python test and source imports can leave bytecode beside bundled helpers.
# Never put interpreter caches into the click: apart from being regenerated,
# they can make removed helper names appear to remain in the package.
find "$stage" -type d -name __pycache__ -prune -exec rm -rf {} +
find "$stage" -type f \( -name '*.pyc' -o -name '*.pyo' \) -delete
# The confined UI starts the daemon and indicator as children. AppArmor
# inheritance and both child labels must be checked on the target device.
for binary in puhurid puhuri-ctl; do
    [ -x "$PROJECT/build/$binary" ] || {
        echo "Puuttuu $PROJECT/build/$binary. Aja ensin tools/build-daemon.sh" >&2
        exit 1
    }
    cp "$PROJECT/build/$binary" "$stage/lib/aarch64-linux-gnu/bin/$binary"
done
chmod -R u=rwX,go=rX "$stage"
# The push helper is an executable: the push service runs it directly.
chmod u+x,go+x "$stage/lib/aarch64-linux-gnu/bin/puhuri-push-helper"
chmod u+x,go+x "$stage/lib/aarch64-linux-gnu/bin/puhurid" \
    "$stage/lib/aarch64-linux-gnu/bin/puhuri-ctl" \
    "$stage/lib/aarch64-linux-gnu/bin/puhuri-indicator"

cd "$work"
"$CLICK" build --no-validate "$stage"

mkdir -p "$PROJECT/dist"
for package in "$work"/*.click; do
    target="$PROJECT/dist/$(basename "$package")"
    [ ! -e "$target" ] || {
        echo "Kohdepaketti on jo olemassa, en korvaa sitä: $target" >&2
        exit 1
    }
    mv "$package" "$target"
done

echo "--- built:"
ls -l "$PROJECT/dist/"
