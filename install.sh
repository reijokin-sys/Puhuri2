#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# Build the confined child process and click UI, then install the click.
#
#   PUHELIMEN_SALASANA=... ./install.sh
#
# PUHURI_FEATURES defaults to `omemo` (libsignal + the envelope), because the
# phone uses OMEMO and a plain build silently loses encrypted conversations.
# Set PUHURI_FEATURES= (empty) explicitly for a plain test build.
#
# The daemon and indicator are started as children by the confined UI.
set -eu

PROJECT=$(cd "$(dirname "$0")" && pwd)

echo "--- rakennetaan palvelu (aarch64)"
PUHURI_FEATURES=${PUHURI_FEATURES:-omemo} sh "$PROJECT/tools/build-daemon.sh"

echo "--- rakennetaan click (sisältää palvelun)"
sh "$PROJECT/tools/build.sh"

echo "--- asennetaan puhelimeen"
sh "$PROJECT/tools/deploy.sh"
