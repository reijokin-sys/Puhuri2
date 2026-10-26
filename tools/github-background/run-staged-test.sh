#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-or-later
# One-use phone test wrapper. All staged packages are removed on exit.
set -eu
TEST_STAGE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
case "$TEST_STAGE" in
    /tmp/puhuri-1.2.0-test.*) ;;
    *) echo 'Refusing cleanup outside temporary test directory' >&2; exit 1 ;;
esac
trap 'rm -rf -- "$TEST_STAGE"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
case "${1:-install}" in
    install) python3 "$TEST_STAGE/install-puhuri-1.2.0.py" install --local-assets "$TEST_STAGE" ;;
    remove) python3 "$TEST_STAGE/install-puhuri-1.2.0.py" remove ;;
    cycle)
        python3 "$TEST_STAGE/install-puhuri-1.2.0.py" remove
        python3 "$TEST_STAGE/install-puhuri-1.2.0.py" install --local-assets "$TEST_STAGE"
        ;;
    *) echo 'Usage: run-staged-test.sh [install|remove|cycle]' >&2; exit 2 ;;
esac
