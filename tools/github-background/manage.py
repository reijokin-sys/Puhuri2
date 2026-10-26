#!/usr/bin/python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Install/remove Puhuri's optional user services; never installs a click."""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import socket
import subprocess
import sys
import time

APP = 'fi.reijo.puhuri2'
UNITS = ('puhurid2.service', 'puhuri2-indicator.service')
MARK = '# Managed by Puhuri GitHub background installer\n'
BUNDLE = Path(__file__).resolve().parent
HOME = Path.home()
BASE = HOME / '.local/share/fi.reijo.puhuri2-background'
UNIT_DIR = HOME / '.config/systemd/user'
CONFIG = HOME / '.config' / APP
FLAG = CONFIG / 'service-managed'
STATE = BASE / 'installed.json'


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def systemctl(*args, check=True):
    # No shell and no credentials. Use this logged-in user's own manager.
    return subprocess.run(['systemctl', '--user', *args], check=check,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)


def rpc_status():
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(2)
        connection.connect(str(Path(os.environ['XDG_RUNTIME_DIR']) / APP / 'socket'))
        connection.sendall(b'{"id":1,"cmd":"status","args":{}}\n')
        with connection.makefile('rb') as stream:
            for line in stream:
                reply = json.loads(line)
                if reply.get('id') == 1:
                    if reply.get('ok'):
                        return reply['result']
                    break
    raise RuntimeError('Daemon status unavailable')


def write(path, data, mode=0o600):
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.is_symlink():
        raise RuntimeError('Refusing symlink destination')
    temporary = path.with_name(path.name + '.new')
    with temporary.open('xb') as stream:
        stream.write(data)
    temporary.chmod(mode)
    temporary.replace(path)


def metadata():
    meta = json.loads((BUNDLE / 'bundle.json').read_text())
    if meta['app_id'] != APP or not re.fullmatch(r'[0-9][0-9A-Za-z.+~-]*', meta['version']):
        raise RuntimeError('Invalid bundle identity/version')
    # Exact allowlist: bundle paths can never escape into user data.
    names = {'manage.py', 'launch.py', 'notifier.py', 'messaging.py',
             'LICENSE', 'THIRD_PARTY_NOTICES.md', 'THIRD_PARTY_LICENSES.txt', 'README.md'}
    if set(meta['files']) != names:
        raise RuntimeError('Unexpected bundle file list')
    for name in names:
        if sha(BUNDLE / name) != meta['files'][name]:
            raise RuntimeError('Bundle checksum mismatch: ' + name)
    return meta


def installed_root(meta):
    root = Path('/opt/click.ubuntu.com') / APP / 'current'
    root = root.resolve(strict=True)
    manifest = json.loads((root / '.click/info/fi.reijo.puhuri2.manifest').read_text())
    if any(manifest.get(key) != value for key, value in
           [('name', APP), ('version', meta['version']), ('architecture', 'arm64')]):
        raise RuntimeError('Install the matching arm64 click first')
    for name, digest in meta['click_files'].items():
        if name not in ('lib/aarch64-linux-gnu/bin/puhurid', 'puhuri.py', '.click/info/fi.reijo.puhuri2.manifest'):
            raise RuntimeError('Invalid click file list')
        if sha(root / name) != digest:
            raise RuntimeError('Installed click differs from bundle')
    if len(meta['click_files']) != 3:
        raise RuntimeError('Incomplete click hashes')
    return root


def unit(payload, kind):
    # systemd quotes paths, and %% is the literal percent escape.
    def quote(value):
        return '"' + str(value).replace('\\', '\\\\').replace('"', '\\"').replace('%', '%%') + '"'
    command = '/usr/bin/python3 ' + quote(payload / 'launch.py') + ' ' + kind
    after = 'After=puhurid2.service\nPartOf=puhurid2.service\n' if kind == 'notifier' else ''
    return (MARK + '[Unit]\nDescription=Puhuri 2 GitHub ' + kind + '\n' + after +
            '\n[Service]\nType=simple\nUMask=0077\nNoNewPrivileges=yes\n'
            'Environment=PYTHONDONTWRITEBYTECODE=1\n'
            'Environment=DBUS_SESSION_BUS_ADDRESS=unix:path=%t/bus\n'
            'Restart=on-failure\nRestartSec=5\nExecStart=' + command +
            '\n\n[Install]\nWantedBy=graphical-session.target\n').encode()


def snapshot(paths):
    result = {}
    for path in paths:
        if path.is_symlink():
            raise RuntimeError('Refusing symlink destination')
        result[path] = path.read_bytes() if path.exists() else None
    return result


def restore(files):
    for path, data in files.items():
        if data is None:
            path.unlink(missing_ok=True)
        else:
            write(path, data)


def discard_payload(value):
    path = Path(value)
    if path.parent != BASE / 'versions' or path.is_symlink():
        raise RuntimeError('Refusing cleanup outside managed payload directory')
    if path.exists():
        shutil.rmtree(path)


def install():
    meta = metadata()
    root = installed_root(meta)
    # A separate user installation; no /etc changes or root operation.
    subprocess.run(['/usr/bin/python3', '-c', 'import dbus; from gi.repository import Gio, GLib'],
                   check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    units = [UNIT_DIR / name for name in UNITS]
    old = snapshot([*units, FLAG, STATE])
    if old[STATE] is not None:
        previous = json.loads(old[STATE])
        if previous.get('schema_version') != meta['schema_version']:
            raise RuntimeError('Schema-changing update requires a separate migration plan')
    for path in units:
        if old[path] is not None and not old[path].startswith(MARK.encode()):
            raise RuntimeError('Existing unmanaged service; explicit migration required: ' + path.name)
    was_enabled = {name: systemctl('is-enabled', name, check=False).returncode == 0 for name in UNITS}
    was_active = {name: systemctl('is-active', name, check=False).returncode == 0 for name in UNITS}
    # Never replace a running UI child or old development receiver silently.
    if not STATE.exists():
        try:
            rpc_status()
        except (OSError, RuntimeError):
            pass
        else:
            raise RuntimeError('Close Puhuri and stop its old receiver before first installation')
    payload = BASE / 'versions' / (meta['version'] + '-' + sha(BUNDLE / 'bundle.json')[:12])
    payload.mkdir(parents=True, exist_ok=True)
    # Only the active payload is retained; rollback snapshots stay in memory.
    for name in (*meta['files'], 'bundle.json'):
        destination = payload / name
        if destination.exists():
            if sha(destination) != sha(BUNDLE / name):
                raise RuntimeError('Existing payload differs')
        else:
            write(destination, (BUNDLE / name).read_bytes())
    config = json.dumps({'root': str(root), 'version': meta['version'],
                         'daemon_sha256': meta['click_files']['lib/aarch64-linux-gnu/bin/puhurid']}).encode()
    launch_config = payload / 'runtime.json'
    if not launch_config.exists():
        write(launch_config, config)
    elif launch_config.read_bytes() != config:
        raise RuntimeError('Payload runtime configuration differs')
    try:
        write(FLAG, b'Puhuri GitHub user services\n')
        existing = [name for name, path in zip(UNITS, units) if old[path] is not None]
        if existing:
            systemctl('stop', *existing)
        # Stop failure is fatal; never unlink another live daemon's socket.
        for path, kind in zip(units, ('daemon', 'notifier')):
            write(path, unit(payload, kind))
        systemctl('daemon-reload')
        systemctl('enable', *UNITS)
        systemctl('start', UNITS[0])
        for _ in range(50):
            try:
                if rpc_status().get('service_mode') is True:
                    break
            except (OSError, RuntimeError):
                pass
            time.sleep(0.2)
        else:
            raise RuntimeError('Service-mode daemon did not become ready')
        systemctl('start', UNITS[1])
        time.sleep(1)
        systemctl('is-active', *UNITS)
        write(STATE, json.dumps({'version': meta['version'], 'payload': str(payload),
                                 'schema_version': meta['schema_version'],
                                 'click_sha256': meta['click_sha256']}).encode())
    except Exception:
        systemctl('stop', *UNITS, check=False)
        for name in UNITS:
            systemctl('disable', name, check=False)
        restore(old)
        systemctl('daemon-reload')
        for name in UNITS:
            if was_enabled[name]:
                systemctl('enable', name)
            if was_active[name]:
                systemctl('start', name)
        previous_payload = json.loads(old[STATE])['payload'] if old[STATE] else None
        if str(payload) != previous_payload:
            discard_payload(payload)
        raise
    if old[STATE]:
        previous_payload = json.loads(old[STATE])['payload']
        if previous_payload != str(payload):
            discard_payload(previous_payload)
    print('User services installed: ' + meta['version'] + '; verify visible notification manually')


def remove():
    if not STATE.exists():
        raise RuntimeError('No managed installation')
    payload = json.loads(STATE.read_text())['payload']
    for name in UNITS:
        path = UNIT_DIR / name
        if path.is_symlink() or not path.read_bytes().startswith(MARK.encode()):
            raise RuntimeError('Service changed outside installer; refusing removal')
    systemctl('stop', *UNITS)
    systemctl('disable', *UNITS)
    # Remove inactive add-on code; never touch the click or account data.
    for name in UNITS:
        (UNIT_DIR / name).unlink()
    FLAG.unlink(missing_ok=True)
    STATE.unlink()
    systemctl('daemon-reload')
    systemctl('reset-failed', *UNITS, check=False)
    discard_payload(payload)
    print('User services removed; click, messages and keys retained; no archived payload')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['plan', 'install', 'status', 'remove'])
    args = parser.parse_args()
    if args.action == 'plan':
        meta = metadata()
        print(json.dumps({'version': meta['version'], 'click_sha256': meta['click_sha256'],
                          'user_service_directory': str(UNIT_DIR), 'payload_directory': str(BASE),
                          'units': UNITS, 'root_required': False}, indent=2))
        return
    if os.geteuid() == 0:
        raise RuntimeError('Run as the logged-in phone user, without sudo')
    os.environ.setdefault('XDG_RUNTIME_DIR', '/run/user/' + str(os.getuid()))
    if any(os.environ.get(name) for name in ('XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'PUHURI_SOCKET')):
        raise RuntimeError('Custom app paths are not supported by this installer')
    os.umask(0o077)
    BASE.mkdir(parents=True, exist_ok=True)
    lock_fd = os.open(BASE / 'manager.lock', os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    # Keep the fd alive through the whole transaction, including rollback.
    with os.fdopen(lock_fd, 'w') as manager_lock:
        fcntl.flock(manager_lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        execute(args.action)


def execute(action):
    if action == 'install':
        if os.uname().machine != 'aarch64':
            raise RuntimeError('This add-on targets arm64 Ubuntu Touch')
        install()
    elif action == 'remove':
        remove()
    else:
        systemctl('is-active', *UNITS)
        # Do not expose account names or server addresses from status.
        print(json.dumps({'service_mode': rpc_status().get('service_mode')}))


if __name__ == '__main__':
    try:
        main()
    except RuntimeError as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
    except Exception:
        # Subprocess/RPC errors may contain private data. Local journal holds details.
        print('Background operation failed; no account data was printed. Check user-service journal.', file=sys.stderr)
        sys.exit(1)
