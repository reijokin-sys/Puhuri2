#!/bin/bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# `ranlib` for the Sähke cross build: llvm-ar's index mode.
set -eu

ZIG=$(python3 -c "import ziglang, os; print(os.path.join(os.path.dirname(ziglang.__file__), 'zig'))")
exec "$ZIG" ranlib "$@"
