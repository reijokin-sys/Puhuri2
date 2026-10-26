#!/usr/bin/python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Version-pinned service launcher; updates require re-running the installer."""
import hashlib
import json
import os
from pathlib import Path
import sys

base = Path(__file__).resolve().parent
config = json.loads((base / 'runtime.json').read_text())
root = Path(config['root'])
current = root.parent / 'current'
if not root.is_dir() or current.resolve() != root:
    print('Puhuri click removed or updated; reinstall matching background bundle', flush=True)
    sys.exit(0)
daemon = root / 'lib/aarch64-linux-gnu/bin/puhurid'
with daemon.open('rb') as stream:
    if hashlib.file_digest(stream, 'sha256').hexdigest() != config['daemon_sha256']:
        print('Puhuri daemon changed; reinstall matching background bundle', flush=True)
        sys.exit(0)
os.environ['PUHURI_CLICK_ROOT'] = str(root)
if sys.argv[1:] == ['daemon']:
    os.environ['PUHURI_CLICK_WATCHDOG'] = '1'
    os.chdir(root)
    os.execv(daemon, [str(daemon), '--engine', 'xmpp', '--service-mode'])
elif sys.argv[1:] == ['notifier']:
    os.execv('/usr/bin/python3', ['/usr/bin/python3', str(base / 'notifier.py')])
else:
    sys.exit(2)
