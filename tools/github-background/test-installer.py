#!/usr/bin/python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Isolated install/remove/rollback checks; never calls systemd or sudo."""
import importlib.util
from contextlib import nullcontext
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
PROJECT = HERE.parent.parent


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, HERE / filename)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


class InstallerTests(unittest.TestCase):
    def test_service_install_rollback_and_removal_preserve_data(self):
        manager = module('manager_under_test', 'manage.py')
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary)
            manager.BASE = home / '.local/share/fi.reijo.puhuri2-background'
            manager.UNIT_DIR = home / '.config/systemd/user'
            manager.CONFIG = home / '.config/fi.reijo.puhuri2'
            manager.FLAG = manager.CONFIG / 'service-managed'
            manager.STATE = manager.BASE / 'installed.json'
            manager.BUNDLE = PROJECT / 'dist/background-1.2.0'
            data = home / '.local/share/fi.reijo.puhuri2/example-data'
            data.parent.mkdir(parents=True)
            data.write_text('synthetic retained data')
            active, enabled = set(), set()
            fail_notifier = [False]

            def ctl(action, *names, check=True):
                if action == 'start' and manager.UNITS[1] in names and fail_notifier[0]:
                    fail_notifier[0] = False
                    raise RuntimeError('synthetic notifier failure')
                table = active if action in ('is-active', 'start', 'stop') else enabled
                if action in ('start', 'enable'):
                    table.update(names)
                elif action in ('stop', 'disable'):
                    table.difference_update(names)
                code = int(not all(n in table for n in names)) if action.startswith('is-') else 0
                return subprocess.CompletedProcess([], code)

            def status():
                if manager.UNITS[0] not in active:
                    raise OSError('synthetic offline socket')
                return {'service_mode': True}

            with patch.object(manager, 'systemctl', side_effect=ctl), \
                 patch.object(manager, 'rpc_status', side_effect=status), \
                 patch.object(manager, 'installed_root', return_value=home / 'click'), \
                 patch.object(manager.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)), \
                 patch.object(manager.time, 'sleep'):
                manager.install()
                self.assertEqual(set(manager.UNITS), active)
                self.assertEqual(set(manager.UNITS), enabled)
                before = manager.STATE.read_bytes()
                fail_notifier[0] = True
                with self.assertRaises(RuntimeError):
                    manager.install()
                self.assertEqual(before, manager.STATE.read_bytes())
                self.assertEqual(set(manager.UNITS), active)
                self.assertFalse((manager.BASE / 'transactions').exists())
                manager.remove()
                self.assertFalse(manager.FLAG.exists())
                self.assertFalse(manager.STATE.exists())
                self.assertFalse(any((manager.BASE / 'versions').iterdir()))
                self.assertFalse(active)
                self.assertFalse(enabled)
                self.assertEqual('synthetic retained data', data.read_text())
                self.assertTrue(all(not (manager.UNIT_DIR / n).exists() for n in manager.UNITS))

    def test_bootstrap_removes_only_matching_click_without_package_download(self):
        installer = module('bootstrap_under_test', 'install-release.py')
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary)
            root = home / 'click'
            manifest = root / '.click/info/fi.reijo.puhuri2.manifest'
            manifest.parent.mkdir(parents=True)
            manifest.write_text(json.dumps({'name': 'fi.reijo.puhuri2', 'version': '1.2.0'}))
            installer.CLICK_ROOT = root
            with patch.object(Path, 'home', return_value=home), \
                 patch.object(installer.shutil, 'which', return_value='/usr/bin/tool'), \
                 patch('builtins.open', create=True) as tty, \
                 patch.object(installer, 'click_session', side_effect=lambda: nullcontext()), \
                 patch.object(installer, 'registered_version', side_effect=['1.2.0', None]) as registration, \
                 patch.object(installer.subprocess, 'run') as run, \
                 patch.object(installer, 'download', side_effect=AssertionError('No download during removal')):
                installer.remove_installation()
                command = run.call_args.args[0]
                self.assertEqual(['sudo', '--', 'env'], command[:3])
                self.assertIn('unregister', command)
                self.assertTrue(any(x.startswith('HOME=') for x in command))
                self.assertTrue(any(x.startswith('DBUS_SESSION_BUS_ADDRESS=') for x in command))
                self.assertEqual(['fi.reijo.puhuri2', '1.2.0'], command[-2:])
                manifest.write_text(json.dumps({'name': 'fi.reijo.puhuri2', 'version': '9.0.0'}))
                run.reset_mock()
                registration.side_effect = None
                registration.return_value = '9.0.0'
                with self.assertRaises(RuntimeError):
                    installer.remove_installation()
                run.assert_not_called()

    def test_leftover_click_files_do_not_count_as_user_registration(self):
        installer = module('registration_under_test', 'install-release.py')
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            installer.CLICK_ROOT = root
            (root / 'puhuri.py').write_bytes(b'synthetic source')
            (root / 'bundle.json').write_text(json.dumps({'click_files': {
                'puhuri.py': hashlib.sha256(b'synthetic source').hexdigest()}}))
            with patch.object(installer, 'registered_version', return_value=None):
                self.assertFalse(installer.click_already_matches(root))
            with patch.object(installer, 'registered_version', return_value='1.2.0'):
                self.assertTrue(installer.click_already_matches(root))

    def test_menu_retries_invalid_input_and_returns_requested_action(self):
        installer = module('menu_under_test', 'install-release.py')
        with patch('builtins.open', return_value=io.StringIO('invalid\n2\n')):
            self.assertEqual('remove', installer.choose_action())
        with patch('builtins.open', return_value=io.StringIO('1\n')):
            self.assertEqual('install', installer.choose_action())
        with patch('builtins.open', return_value=io.StringIO('0\n')):
            self.assertEqual('exit', installer.choose_action())

    def test_click_session_compatibility_is_temporary_and_preserves_existing_file(self):
        installer = module('session_under_test', 'install-release.py')
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'dbus-session'
            with installer.click_session(path):
                self.assertEqual('DBUS_SESSION_BUS_ADDRESS=unix:path=' + str(path.parent / 'bus') + '\n', path.read_text())
                self.assertEqual(0o600, path.stat().st_mode & 0o777)
            self.assertFalse(path.exists())
            path.write_text('existing-session')
            with installer.click_session(path):
                self.assertEqual('existing-session', path.read_text())
            self.assertEqual('existing-session', path.read_text())

    def test_menu_session_runs_install_remove_then_exits_without_extra_action(self):
        installer = module('session_menu_under_test', 'install-release.py')
        with patch.object(installer.sys, 'argv', ['installer']), \
             patch.object(installer, 'choose_action', side_effect=['install', 'remove', 'exit']), \
             patch.object(installer, 'run_action') as action:
            installer.main()
            self.assertEqual([('install', None), ('remove', None)], [c.args for c in action.call_args_list])

    def test_inherited_shared_registration_is_hidden_only_for_current_user(self):
        installer = module('inherited_under_test', 'install-release.py')
        with tempfile.TemporaryDirectory() as temporary:
            home = Path(temporary)
            installer.CLICK_ROOT = home / 'click'
            manifest = installer.CLICK_ROOT / '.click/info/fi.reijo.puhuri2.manifest'
            manifest.parent.mkdir(parents=True)
            manifest.write_text(json.dumps({'name': 'fi.reijo.puhuri2', 'version': '1.2.0'}))
            with patch.object(Path, 'home', return_value=home), \
                 patch.object(installer.shutil, 'which', return_value='/usr/bin/tool'), \
                 patch.object(installer, 'click_session', side_effect=lambda: nullcontext()), \
                 patch.object(installer, 'registered_version', side_effect=['1.2.0', '1.2.0', None]), \
                 patch('builtins.open', create=True), \
                 patch.object(installer.subprocess, 'run') as run:
                installer.remove_installation()
                self.assertEqual(2, run.call_count)
                self.assertTrue(all('--user' in c.args[0] for c in run.call_args_list))
                self.assertTrue(all('--all-users' not in c.args[0] for c in run.call_args_list))


if __name__ == '__main__':
    unittest.main()
