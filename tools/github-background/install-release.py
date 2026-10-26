#!/usr/bin/python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Install or remove Puhuri 2 1.2.0 and its background services."""
import argparse
from contextlib import contextmanager
import hashlib
import json
import os
import pwd
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request

VERSION = '1.2.0'
BASE_URL = 'https://github.com/reijokin-sys/Puhuri2/releases/download/v' + VERSION
ASSETS = {
    'fi.reijo.puhuri2_1.2.0_arm64.click': '1f7a5efe839966f04ce14f08564b17d71de35f36cfd8f0989410dbd3bb544571',
    'background-1.2.0.tar.gz': '4ca739abda186a3d5253ff3eb814f32d750b7857b030a5edc67790df07db552b',
}
UNITS = ('puhurid2.service', 'puhuri2-indicator.service')
FILES = {'manage.py', 'launch.py', 'notifier.py', 'messaging.py', 'bundle.json',
         'README.md', 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'THIRD_PARTY_LICENSES.txt'}
CLICK_ROOT = Path('/opt/click.ubuntu.com/fi.reijo.puhuri2/current')


def ctl(*args, check=True):
    return subprocess.run(['systemctl', '--user', *args], check=check,
                          stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def download(name, directory, local_assets=None):
    print(('Using staged ' if local_assets else 'Downloading ') + name, flush=True)
    path = directory / name
    digest = hashlib.sha256()
    source = (local_assets / name).open('rb') if local_assets else urllib.request.urlopen(BASE_URL + '/' + name, timeout=60)
    with source as response, path.open('xb') as output:
        while True:
            data = response.read(1024 * 1024)
            if not data:
                break
            output.write(data)
            digest.update(data)
    if digest.hexdigest() != ASSETS[name]:
        raise RuntimeError('Checksum mismatch: ' + name)
    return path


def unpack(archive, directory):
    root = directory / ('background-' + VERSION)
    root.mkdir(mode=0o700)
    seen = set()
    # Extract only flat regular files from the exact known directory. Never
    # honor tar paths, links, ownership, permissions or special files.
    with tarfile.open(archive, 'r:gz') as source:
        for member in source:
            if member.name == root.name and member.isdir():
                continue
            prefix = root.name + '/'
            name = member.name[len(prefix):] if member.name.startswith(prefix) else ''
            if name not in FILES or name in seen or not member.isfile() or member.size > 16 * 1024 * 1024:
                raise RuntimeError('Unexpected archive entry')
            seen.add(name)
            with source.extractfile(member) as input_file, (root / name).open('xb') as output:
                shutil.copyfileobj(input_file, output)
    if seen != FILES:
        raise RuntimeError('Incomplete background bundle')
    return root


def click_already_matches(bundle):
    if registered_version() != VERSION:
        return False
    meta = json.loads((bundle / 'bundle.json').read_text())
    root = CLICK_ROOT
    for name, expected in meta['click_files'].items():
        path = root / name
        if not path.is_file():
            return False
        with path.open('rb') as stream:
            if hashlib.file_digest(stream, 'sha256').hexdigest() != expected:
                return False
    return True


def registered_version():
    result = subprocess.run(['click', 'list'], check=True, capture_output=True, text=True)
    for line in result.stdout.splitlines():
        fields = line.split()
        if len(fields) == 2 and fields[0] == 'fi.reijo.puhuri2':
            return fields[1]
    return None


def root_click_command(*arguments):
    user = pwd.getpwuid(os.getuid())
    runtime = '/run/user/' + str(os.getuid())
    return ['sudo', '--', 'env', 'HOME=' + user.pw_dir,
            'XDG_CACHE_HOME=' + user.pw_dir + '/.cache', 'XDG_RUNTIME_DIR=' + runtime,
            'DBUS_SESSION_BUS_ADDRESS=unix:path=' + runtime + '/bus', 'click', *arguments]


@contextmanager
def click_session(path=None):
    # click's user.vala reads this legacy session-env file when temporarily
    # dropping root privileges. Newer UT provides the bus socket but not the
    # file. Create it only for this operation, without changing existing files.
    path = path or Path('/run/user') / str(os.getuid()) / 'dbus-session'
    created = False
    try:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    except FileExistsError:
        pass
    else:
        with os.fdopen(fd, 'w') as stream:
            stream.write('DBUS_SESSION_BUS_ADDRESS=unix:path=' + str(path.parent / 'bus') + '\n')
        created = True
    try:
        yield
    finally:
        if created:
            path.unlink(missing_ok=True)


def click_install_command(click):
    if shutil.which('pkcon'):
        return ['pkcon', 'install-local', '--allow-untrusted', str(click)]
    if shutil.which('click') and shutil.which('sudo'):
        return root_click_command('install', '--user', pwd.getpwuid(os.getuid()).pw_name,
                                  '--allow-unauthenticated', str(click))
    raise RuntimeError('Installing a new click requires pkcon or click with sudo')


def preflight():
    if os.geteuid() == 0:
        raise RuntimeError('Run as the ordinary phone user, without sudo')
    if os.uname().machine != 'aarch64':
        raise RuntimeError('Requires arm64 Ubuntu Touch')
    for variable in ('XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'PUHURI_SOCKET'):
        if os.environ.get(variable):
            raise RuntimeError('Custom app paths are not supported')
    os.environ.setdefault('XDG_RUNTIME_DIR', '/run/user/' + str(os.getuid()))
    os.environ.setdefault('DBUS_SESSION_BUS_ADDRESS', 'unix:path=' + os.environ['XDG_RUNTIME_DIR'] + '/bus')
    subprocess.run(['/usr/bin/python3', '-c', 'import dbus; from gi.repository import Gio, GLib'], check=True)
    ctl('show-environment')
    managed = Path.home() / '.local/share/fi.reijo.puhuri2-background/installed.json'
    for name in UNITS:
        path = Path.home() / '.config/systemd/user' / name
        if path.exists() and (path.is_symlink() or not path.read_text().startswith('# Managed by Puhuri GitHub background installer\n') or not managed.exists()):
            raise RuntimeError('Old unmanaged services require separate migration')
    for process in Path('/proc').iterdir():
        if not process.name.isdigit():
            continue
        try:
            executable = os.readlink(process / 'exe')
            cwd = os.readlink(process / 'cwd')
        except OSError:
            continue
        if cwd.startswith('/opt/click.ubuntu.com/fi.reijo.puhuri2/') and 'qmlscene' in executable:
            raise RuntimeError('Close Puhuri 2 before installation')
        if executable.startswith('/opt/click.ubuntu.com/fi.reijo.puhuri2/') and not managed.exists():
            raise RuntimeError('Stop the existing unmanaged receiver before installation')
    return managed.exists()


def remove_installation():
    base = Path.home() / '.local/share/fi.reijo.puhuri2-background'
    state_path = base / 'installed.json'
    manifest_path = CLICK_ROOT / '.click/info/fi.reijo.puhuri2.manifest'
    registered = registered_version()
    click_present = registered is not None
    if click_present:
        if registered != VERSION:
            raise RuntimeError('This installer removes only Puhuri 2 v' + VERSION)
        manifest = json.loads(manifest_path.read_text())
        if manifest.get('name') != 'fi.reijo.puhuri2' or manifest.get('version') != VERSION:
            raise RuntimeError('This installer removes only Puhuri 2 v' + VERSION)
        if not shutil.which('click') or not shutil.which('sudo'):
            raise RuntimeError('Click removal requires click and sudo')
    if state_path.exists():
        state = json.loads(state_path.read_text())
        payload = Path(state['payload'])
        if state.get('version') != VERSION or payload.parent != base / 'versions' or payload.is_symlink():
            raise RuntimeError('Unexpected installed service version or payload')
        print('Removing background services', flush=True)
        subprocess.run(['/usr/bin/python3', str(payload / 'manage.py'), 'remove'], check=True)
    if click_present:
        print('Removing click application', flush=True)
        with click_session(), open('/dev/tty') as terminal:
            # First removal can reveal an @all installation. A second normal
            # per-user unregister hides that inherited version for this user;
            # never remove other users' shared application automatically.
            for attempt in range(2):
                subprocess.run(root_click_command('unregister', '--user',
                                pwd.getpwuid(os.getuid()).pw_name, 'fi.reijo.puhuri2', VERSION),
                               stdin=terminal, check=True)
                remaining = registered_version()
                if remaining is None:
                    break
                if remaining != VERSION:
                    raise RuntimeError('Another click version became visible; refusing further removal')
                if attempt == 0:
                    print('Removing inherited registration for this user', flush=True)
            else:
                raise RuntimeError('Click still registered; removal did not finish')
    # The manager has exited, so its now-unused lock and empty directories can
    # be removed. Never recursively delete a home/config/data directory.
    (base / 'manager.lock').unlink(missing_ok=True)
    for directory in (base / 'versions', base):
        try:
            directory.rmdir()
        except OSError:
            pass
    print('Puhuri 2 ' + VERSION + ' removed; account, messages, attachments and OMEMO keys retained.', flush=True)


def choose_action():
    print('1) Install or update / Asenna tai päivitä\n2) Remove / Poista\n0) Exit / Lopeta', flush=True)
    with open('/dev/tty') as terminal:
        while True:
            print('Choose / Valitse [1/2/0]: ', end='', flush=True)
            answer = terminal.readline()
            if not answer:
                raise RuntimeError('No terminal selection received')
            choice = {'1': 'install', '2': 'remove', '0': 'exit'}.get(answer.strip())
            if choice:
                return choice
            print('Choose 1, 2 or 0.', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', nargs='?', choices=['install', 'remove'])
    parser.add_argument('--plan', action='store_true', help='Show pinned assets; no downloads or changes')
    parser.add_argument('--local-assets', type=Path, help='Test with staged packages instead of network downloads')
    args = parser.parse_args()
    if args.plan:
        print(json.dumps({'action': args.action or 'ask', 'version': VERSION, 'base_url': BASE_URL, 'sha256': ASSETS,
                          'phone_archives': False}, indent=2))
        return
    print('\033[1;32m=== Puhuri 2 - v' + VERSION + ' ===\033[0m', flush=True)
    if args.action:
        run_action(args.action, args.local_assets)
        return
    while True:
        action = choose_action()
        if action == 'exit':
            print('Exit / Lopetettu.', flush=True)
            return
        run_action(action, args.local_assets)


def run_action(action, local_assets=None):
    os.umask(0o077)
    managed = preflight()
    if action == 'remove':
        remove_installation()
        return
    # Download and verify both assets before changing any installed component.
    # TemporaryDirectory removes downloads/extracted staging on success/error.
    with tempfile.TemporaryDirectory(prefix='puhuri-install-') as temporary:
        directory = Path(temporary)
        click = download('fi.reijo.puhuri2_' + VERSION + '_arm64.click', directory, local_assets)
        bundle = unpack(download('background-' + VERSION + '.tar.gz', directory, local_assets), directory)
        subprocess.run(['/usr/bin/python3', str(bundle / 'manage.py'), 'plan'],
                       check=True, stdout=subprocess.DEVNULL)
        matches = click_already_matches(bundle)
        install_command = None if matches else click_install_command(click)
        active = [name for name in UNITS if managed and ctl('is-active', name, check=False).returncode == 0]
        if managed:
            ctl('stop', *UNITS)
        try:
            # Keep installer confirmation / sudo authentication on the tty.
            # No password is collected, stored or placed in process arguments.
            if matches:
                print('Exact click already installed; updating background services only.', flush=True)
            else:
                with click_session(), open('/dev/tty') as terminal:
                    subprocess.run(install_command, stdin=terminal, check=True)
                if not click_already_matches(bundle):
                    raise RuntimeError('Click registration or installed files do not match after installation')
            subprocess.run(['/usr/bin/python3', str(bundle / 'manage.py'), 'install'], check=True)
            subprocess.run(['/usr/bin/python3', str(bundle / 'manage.py'), 'status'], check=True)
        except Exception:
            for name in active:
                ctl('start', name, check=False)
            print('Installation incomplete. Click rollback is not automatic; rerun the matching installer.', file=sys.stderr)
            raise
    print('Puhuri 2 ' + VERSION + ' installed; temporary downloads removed.', flush=True)


if __name__ == '__main__':
    try:
        main()
    except RuntimeError as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
    except Exception:
        print('Operation failed. Check network, release availability and installer output; account data was not purged.', file=sys.stderr)
        sys.exit(1)
