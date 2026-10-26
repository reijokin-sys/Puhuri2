#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Real curl-pipe/PTY launcher checks with local release files; no phone/network."""
import json
import os
from pathlib import Path
import pty
import select
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[2]
LAUNCHER = ROOT / 'dist/install-puhuri-1.2.4.sh'
ARCHIVE = ROOT / 'dist/puhuri2-1.2.4-adb-installer.zip'


class LauncherTests(unittest.TestCase):
    def launch(self, mode='success', arguments=''):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            tools = directory / 'bin'
            tools.mkdir()
            archive = directory / 'test.zip'
            archive.write_bytes(ARCHIVE.read_bytes() if mode != 'corrupt' else b'corrupt')
            curl = tools / 'curl'
            # Replace only the pinned GitHub ZIP URL with a local file; real
            # curl still executes both the source pipe and archive transfer.
            curl.write_text('#!' + sys.executable + '\n' +
                'import os,subprocess,sys\n' +
                'args=sys.argv[1:]\n' +
                'for index,value in enumerate(args):\n' +
                ' if value.startswith("https://github.com/reijokin-sys/Puhuri2/releases/download/"):\n' +
                ('  raise SystemExit(22)\n' if mode == 'download_failure' else
                 '  args[index]=' + repr(archive.as_uri()) + '\n') +
                'raise SystemExit(subprocess.call([' + repr(shutil.which('curl')) + ']+args))\n')
            curl.chmod(0o755)
            adb = tools / 'adb'
            adb.write_text('#!/bin/sh\nprintf called > ' + shlex.quote(str(directory / 'adb-called')) + '\nexit 99\n')
            adb.chmod(0o755)
            env = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ['PATH'], TMPDIR=str(directory))
            command = 'curl -fsSL ' + shlex.quote(LAUNCHER.as_uri()) + ' | sh' + arguments
            pid, terminal = pty.fork()
            if pid == 0:
                os.execve('/bin/sh', ['sh', '-c', command], env)
            output = b''
            answered = False
            deadline = time.monotonic() + 20
            try:
                while time.monotonic() < deadline:
                    ready, _, _ = select.select([terminal], [], [], 0.1)
                    if ready:
                        try:
                            chunk = os.read(terminal, 8192)
                        except OSError:
                            break
                        if not chunk:
                            break
                        output += chunk
                        if b'Choose / Valitse [1/2/0]:' in output and not answered:
                            os.write(terminal, b'0\n')
                            answered = True
                else:
                    os.kill(pid, 9)
                    self.fail('Launcher PTY timeout')
            finally:
                os.close(terminal)
                _, status = os.waitpid(pid, 0)
            self.assertFalse((directory / 'adb-called').exists(), 'Exit must never call adb')
            self.assertEqual([], list(directory.glob('puhuri-launch.*')), 'Temporary files must be removed')
            return os.waitstatus_to_exitcode(status), output.decode(errors='replace'), answered

    def test_curl_pipe_menu_accepts_terminal_choice_and_cleans_up(self):
        code, output, answered = self.launch()
        self.assertEqual(0, code, output)
        self.assertTrue(answered, output)
        self.assertIn('Exit / Lopetettu.', output)
        self.assertIn('Ladataan asenninta', output)
        self.assertNotIn('% Total', output)

    def test_corrupt_zip_never_starts_installer(self):
        code, output, answered = self.launch('corrupt')
        self.assertNotEqual(0, code, output)
        self.assertFalse(answered)
        self.assertIn('checksum mismatch', output)
        self.assertNotIn('=== Puhuri', output)

    def test_failed_download_never_starts_installer(self):
        code, output, answered = self.launch('download_failure')
        self.assertNotEqual(0, code, output)
        self.assertFalse(answered)
        self.assertNotIn('=== Puhuri', output)

    def test_arguments_forwarded_to_installer(self):
        code, output, answered = self.launch(arguments=' -s -- --help')
        self.assertEqual(0, code, output)
        self.assertFalse(answered)
        self.assertIn('--verbose', output)


if __name__ == '__main__':
    unittest.main()
