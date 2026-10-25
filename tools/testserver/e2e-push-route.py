#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Exercise UI Python -> private RPC -> saved route with a disposable daemon."""
import importlib.util
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import types

ROOT = Path(__file__).resolve().parents[2]
ARTIFACTS = ROOT / 'docs/artifacts/push-route-ui-20261025'
TOKEN = 'SYNTHETIC-UI-TOKEN-0123456789-0123456789-0123456789-0123456789-0123456789'
ROTATED = TOKEN + '-ROTATED'


def main():
    result = {'passed': False, 'phone_used': False, 'real_accounts_used': False}
    ARTIFACTS.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='puhuri-route-') as temp:
        work = Path(temp)
        os.environ.update(PUHURI_SOCKET=str(work / 'daemon.sock'),
                          XDG_CONFIG_HOME=str(work / 'config'), XDG_CACHE_HOME=str(work / 'cache'))
        stub = types.ModuleType('pyotherside')
        stub.send = lambda *args: None
        sys.modules['pyotherside'] = stub
        spec = importlib.util.spec_from_file_location('puhuri_route_app', ROOT / 'app/puhuri.py')
        app = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(app)
        log = (work / 'daemon.log').open('w')
        processes = []
        def start():
            process = subprocess.Popen([str(ROOT / 'daemon/target/debug/puhurid'), '--engine', 'fake',
                                        '--data-dir', str(work / 'data'), '--socket', os.environ['PUHURI_SOCKET']],
                                       stdout=log, stderr=log)
            processes.append(process)
            for _ in range(100):
                try:
                    with socket.socket(socket.AF_UNIX) as stream:
                        stream.connect(os.environ['PUHURI_SOCKET'])
                    return process
                except OSError:
                    time.sleep(.05)
            raise AssertionError('daemon did not start')
        def rpc(command, **args):
            response = app._backend.request(command, args)
            assert response.get('ok'), 'RPC failed: ' + command
            return response['result']
        def ok(response):
            assert response.get('ok'), 'UI adapter request failed'
            text = json.dumps(response)
            assert TOKEN not in text and ROTATED not in text, 'registration response exposes token'
            return response['result']
        try:
            process = start()
            rpc('account.login', jid='alpha@example.invalid', password='public-synthetic-fixture')
            ok(app.background_setup('push.example.invalid', 'operator-module'))
            ok(app.push_register(TOKEN))
            state = rpc('push.get')
            assert state['module'] == 'operator-module' and state['enabled']
            assert state['settings']['module'] == 'operator-module'
            result['ui_route_selected'] = True
            # A token refresh must use the saved name, never the old ubports default.
            ok(app.push_register(ROTATED))
            assert rpc('push.get')['settings']['module'] == 'operator-module'
            result['token_rotation_preserves_module'] = True
            # Disable, deliver a new token signal, then restart the process.
            ok(app.push_set_enabled(False))
            assert ok(app.push_register(TOKEN))['reason'] == 'disabled'
            state = rpc('push.get')
            assert not state['enabled'] and not state['settings']
            assert state['module'] == 'operator-module'
            app._backend.close()
            process.terminate()
            process.wait(timeout=5)
            process = start()
            assert not rpc('push.get')['enabled']
            assert ok(app.push_register(ROTATED))['reason'] == 'disabled'
            assert not rpc('push.get')['settings']
            result['disabled_survives_restart_and_token_signal'] = True
            ok(app.push_set_enabled(True))
            assert rpc('push.get')['settings']['module'] == 'operator-module'
            result['explicit_reenable_uses_saved_module'] = True
            before = rpc('push.get')
            response = app.background_setup('push.example.invalid', 'invalid module')
            assert not response['ok']
            assert rpc('push.get')['module'] == before['module']
            assert not app.background_setup('push.example.invalid/resource', 'other')['ok']
            assert rpc('push.get')['service'] == before['service']
            result['invalid_route_preserves_settings'] = True
            # Operator registration reads token from stdin and does not echo it.
            command = [str(ROOT / 'daemon/target/debug/puhuri-ctl'), '--socket', os.environ['PUHURI_SOCKET'],
                       'push-enable', 'push.example.invalid', '--token-stdin', '--module', 'stdin-module']
            output = subprocess.run(command, input=TOKEN+'\n', text=True, capture_output=True, check=True)
            assert TOKEN not in output.stdout + output.stderr
            assert rpc('push.get')['settings']['module'] == 'stdin-module'
            result['stdin_registration_no_token_output'] = True
            # Direct operator disable must also retain the selected route.
            rpc('push.disable')
            assert rpc('push.get')['module'] == 'stdin-module'
            assert ok(app.push_register(ROTATED))['reason'] == 'disabled'
            rpc('account.logout')
            assert ok(app.push_register(TOKEN))['reason'] == 'no-account'
            assert rpc('push.get')['token'] is None
            result['logged_out_token_not_stored'] = True
            result['passed'] = True
        finally:
            app._backend.close()
            for process in processes:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)
            log.close()
            text = (work / 'daemon.log').read_text()
            result['token_absent_from_daemon_log'] = TOKEN not in text and ROTATED not in text
            result['passed'] = result['passed'] and result['token_absent_from_daemon_log']
            (ARTIFACTS / 'route-daemon.log').write_text(text.replace(ROTATED, '[REDACTED]').replace(TOKEN, '[REDACTED]'))
            (ARTIFACTS / 'route-result.json').write_text(json.dumps(result, indent=2)+'\n')
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
