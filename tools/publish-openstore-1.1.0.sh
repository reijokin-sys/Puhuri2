#!/bin/bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# Authorized 1.1.0 submission; enter the key only at the hidden prompt.
set -euo pipefail
cd -- "$(dirname -- "$(realpath -- "$0")")/.."
if [[ -e docs/artifacts/release-1.1.0/openstore-result.json ]]; then
    echo 'A submission result already exists. Inspect the public store before retrying.' >&2
    exit 1
fi
exec python3 docs/artifacts/release-1.1.0/openstore-submit.py --key-stdin \
    --package dist/fi.reijo.puhuri2_1.1.0_arm64.click \
    --changelog-file docs/GITHUB-RELEASE-1.1.0.md \
    --description-file docs/artifacts/release-1.1.0/store-description.txt \
    --result-file docs/artifacts/release-1.1.0/openstore-result.json
