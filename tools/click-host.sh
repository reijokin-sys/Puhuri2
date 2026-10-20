#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Run the Ubuntu "click" tool on this computer without installing it.
#
#   tools/click-host.sh build --no-validate DIRECTORY
#
# Same tool as in the mail project: the Ubuntu package is unpacked into
# /tmp/click-tool instead of being installed system wide (this machine has no
# root for apt). The first run downloads eight small packages and needs
# network access; later runs work offline. CLICK_TOOL_DIR overrides the
# unpack directory.
set -eu

TOOL_DIR=${CLICK_TOOL_DIR:-/tmp/click-tool}
ROOT="$TOOL_DIR/root"

PACKAGES="click python3-click-package libclick-0.4-0 gir1.2-click-0.4 \
gir1.2-json-1.0 libjson-glib-1.0-0 gir1.2-gee-0.8 libgee-0.8-2"

if [ ! -x "$ROOT/usr/bin/click" ]; then
    echo "--- unpacking the click tool into $TOOL_DIR (first run, needs the network)" >&2
    mkdir -p "$TOOL_DIR"
    (
        cd "$TOOL_DIR"
        # shellcheck disable=SC2086
        apt-get download $PACKAGES >/dev/null
    )
    for deb in "$TOOL_DIR"/*.deb; do
        dpkg-deb -x "$deb" "$ROOT"
    done
fi

GIREPO=$(ls -d "$ROOT"/usr/lib/*/girepository-1.0 2>/dev/null | head -1)
[ -n "$GIREPO" ] || {
    echo "VIRHE: click-työkalun purku näyttää rikkinäiseltä ($ROOT)" >&2
    exit 1
}
LIBDIR=$(dirname "$GIREPO")

PYTHONPATH="$ROOT/usr/lib/python3/dist-packages${PYTHONPATH:+:$PYTHONPATH}" \
GI_TYPELIB_PATH="$GIREPO${GI_TYPELIB_PATH:+:$GI_TYPELIB_PATH}" \
LD_LIBRARY_PATH="$LIBDIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" \
exec python3 "$ROOT/usr/bin/click" "$@"
