#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Host-side ADB installer safety checks; never contacts a real phone."""
import argparse
from contextlib import nullcontext
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('adb_installer', Path(__file__).with_name('install-adb.py'))
HOST = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(HOST)


class ADBInstallerTests(unittest.TestCase):
    def test_exit_never_contacts_adb_or_checks_payloads(self):
        with patch.object(HOST.sys, 'argv', ['installer']), \
             patch.object(HOST, 'choose_action', return_value='exit'), \
             patch.object(HOST, 'perform') as perform:
            HOST.main()
            perform.assert_not_called()

    def test_requires_authorized_unique_device(self):
        with patch.object(HOST.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0,
                          stdout='List of devices attached\na unauthorized\nb offline\n')):
            with self.assertRaises(RuntimeError):
                HOST.select_device('adb')
        with patch.object(HOST.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0,
                          stdout='List of devices attached\na device\nb device\n')):
            with self.assertRaises(RuntimeError):
                HOST.select_device('adb')
            self.assertEqual('b', HOST.select_device('adb', 'b'))

    def test_changed_payload_fails_before_adb_contact(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            for name in HOST.PAYLOADS:
                (directory / name).write_bytes(b'corrupted')
            with patch.object(HOST, 'select_device') as select:
                with self.assertRaises(RuntimeError):
                    HOST.perform('install', argparse.Namespace(assets_dir=directory, serial=None))
                select.assert_not_called()

    def test_interactive_failure_still_cleans_only_validated_stage(self):
        phone = HOST.Phone('adb', 'synthetic-device')
        calls = []

        def shell(arguments, interactive=False, capture=False):
            calls.append((arguments, interactive))
            if arguments[0] == 'mktemp':
                return subprocess.CompletedProcess([], 0, stdout='/tmp/puhuri-adb.TEST123\n')
            if interactive:
                raise subprocess.CalledProcessError(1, 'synthetic failure')
            return subprocess.CompletedProcess([], 0, stdout='')

        with patch.object(phone, 'check', return_value={}), \
             patch.object(phone, 'shell', side_effect=shell), \
             patch.object(HOST.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)):
            with self.assertRaises(subprocess.CalledProcessError):
                phone.operate('install', Path('/synthetic-computer-assets'))
        self.assertEqual((['rm', '-rf', '--', '/tmp/puhuri-adb.TEST123'], False), calls[-1])
        self.assertTrue(any(interactive for _, interactive in calls))
        operation = json.loads(next(arguments for arguments, interactive in calls if interactive)[-1])
        self.assertEqual(['install', '--local-assets', '/tmp/puhuri-adb.TEST123'], operation[-3:])

    def test_untrusted_stage_path_is_not_used_for_cleanup(self):
        phone = HOST.Phone('adb', 'synthetic-device')
        with patch.object(phone, 'check', return_value={}), \
             patch.object(phone, 'shell', return_value=subprocess.CompletedProcess([], 0, stdout='/home/phablet\n')) as shell:
            with self.assertRaises(RuntimeError):
                phone.operate('remove', Path('/synthetic-computer-assets'))
            self.assertEqual(1, shell.call_count)

    def test_remote_arguments_are_shell_quoted(self):
        phone = HOST.Phone('adb', 'synthetic-device')
        terminal = io.StringIO()
        with patch.object(HOST.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)) as run, \
             patch.object(HOST, 'terminal_input', side_effect=lambda: nullcontext(terminal)):
            phone.shell(['python3', '/tmp/a file;echo nope.py'], interactive=True)
            args = run.call_args.args[0]
            self.assertIn('-tt', args)
            self.assertEqual("python3 '/tmp/a file;echo nope.py'", args[-1])
            self.assertIs(terminal, run.call_args.kwargs['stdin'])

    def test_curl_pipe_menu_uses_controlling_terminal(self):
        with patch.object(HOST.sys.stdin, 'isatty', return_value=False), \
             patch('builtins.open', return_value=io.StringIO('1\n')) as opened:
            self.assertEqual('install', HOST.choose_action())
            self.assertIn(opened.call_args.args[0], ('/dev/tty', 'CONIN$'))

    def test_129_requires_matching_successful_completion_record(self):
        for mode in ('success', 'missing', 'child_failed', 'wrong_token'):
            with self.subTest(mode=mode):
                phone = HOST.Phone('adb', 'synthetic-device')

                def shell(arguments, interactive=False, capture=False):
                    if interactive:
                        raise subprocess.CalledProcessError(129, 'synthetic PTY shutdown')
                    if mode == 'missing':
                        raise subprocess.CalledProcessError(1, 'missing record')
                    return subprocess.CompletedProcess([], 0, stdout=json.dumps({
                        'token': 'other' if mode == 'wrong_token' else 'operation-token',
                        'exit_code': 1 if mode == 'child_failed' else 0}))

                with patch.object(phone, 'shell', side_effect=shell), \
                     patch.object(HOST.uuid, 'uuid4') as operation_id:
                    operation_id.return_value.hex = 'operation-token'
                    if mode == 'success':
                        phone.run_interactive(['python3', 'synthetic'], '/tmp/puhuri-adb.TEST123')
                    else:
                        with self.assertRaises(RuntimeError):
                            phone.run_interactive(['python3', 'synthetic'], '/tmp/puhuri-adb.TEST123')

    def test_success_marker_does_not_bypass_postconditions(self):
        phone = HOST.Phone('adb', 'synthetic-device')
        calls = []

        def shell(arguments, interactive=False, capture=False):
            calls.append(arguments)
            return subprocess.CompletedProcess([], 0, stdout='/tmp/puhuri-adb.TEST123\n')

        with patch.object(phone, 'check', return_value={}), \
             patch.object(phone, 'shell', side_effect=shell), \
             patch.object(phone, 'run_interactive'), \
             patch.object(phone, 'verify_postconditions', side_effect=RuntimeError('synthetic bad install')), \
             patch.object(HOST.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)):
            with self.assertRaises(RuntimeError):
                phone.operate('install', Path('/synthetic-computer-assets'))
        self.assertEqual(['rm', '-rf', '--', '/tmp/puhuri-adb.TEST123'], calls[-1])


if __name__ == '__main__':
    unittest.main()
