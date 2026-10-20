#!/bin/bash
# SPDX-License-Identifier: AGPL-3.0-or-later
# `ar` for the Sähke cross build, backed by zig's bundled llvm-ar.
#
# cc-rs and OpenSSL's build want a single executable in $AR (a value with a
# space in it is passed on verbatim and zig then sees the ar flags as its own
# subcommand: "error: unknown command: qc").
set -eu

ZIG=$(python3 -c "import ziglang, os; print(os.path.join(os.path.dirname(ziglang.__file__), 'zig'))")
exec "$ZIG" ar "$@"
