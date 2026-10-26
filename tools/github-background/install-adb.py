#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Install/remove Puhuri 2 v1.2.0 from a computer over an authorized ADB link.

All payloads are read from the computer. No phone download tool, network
connection, apt operation or root filesystem remount is required.
"""
import argparse
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys
import uuid

VERSION = '1.2.0'
PAYLOADS = {
    'install-puhuri-1.2.0.py': '22b80af0d51a72e4330ac94f8003e3167d176da15007c4429749daee7e9f18b6',
    'fi.reijo.puhuri2_1.2.0_arm64.click': '1f7a5efe839966f04ce14f08564b17d71de35f36cfd8f0989410dbd3bb544571',
    'background-1.2.0.tar.gz': '4ca739abda186a3d5253ff3eb814f32d750b7857b030a5edc67790df07db552b',
}
PHONE_PROBE = '''import json,os,sys,shutil,subprocess
from pathlib import Path
if sys.version_info < (3,11):
    raise SystemExit("Phone needs Python 3.11 or later")
if os.getuid()==0:
    raise SystemExit("ADB must use the ordinary phone user, not root")
if os.uname().machine!="aarch64":
    raise SystemExit("This package requires arm64 Ubuntu Touch")
from gi.repository import Gio,GLib
import dbus
runtime="/run/user/"+str(os.getuid())
os.environ["XDG_RUNTIME_DIR"]=runtime
os.environ["DBUS_SESSION_BUS_ADDRESS"]="unix:path="+runtime+"/bus"
if not all(shutil.which(name) for name in ("click","sudo","systemctl")):
    raise SystemExit("Phone needs click, sudo and systemctl")
subprocess.run(["systemctl","--user","show-environment"],check=True,stdout=subprocess.DEVNULL)
if not Path(runtime+"/bus").exists():
    raise SystemExit("Open the ordinary Ubuntu Touch user session first")
print(json.dumps({"architecture":"arm64","python_supported":True,
                  "ordinary_user":True,"session_bus":True,"systemd_user":True,
                  "dbus_gio_glib":True,"click_sudo":True}))
'''
OPERATION_RUNNER = r'''import collections,json,os,re,subprocess,sys
from pathlib import Path
stage=Path(sys.argv[1]); token=sys.argv[2]; command=json.loads(sys.argv[3])
verbose = False
routine = {
    'Exact click already installed; updating background services only.',
    'Removing background services', 'Removing click application',
    'Removing inherited registration for this user',
    'User services installed: 1.2.0; verify visible notification manually',
    'User services removed; click, messages and keys retained; no archived payload',
    '{"service_mode": true}',
    'Puhuri 2 1.2.0 installed; temporary downloads removed.',
    'Puhuri 2 1.2.0 removed; account, messages, attachments and OMEMO keys retained.',
    'WARNING:root:Signature check failed, but installing anyway as requested',
}
def is_routine(line):
    text=re.sub(r'\x1b\[[0-9;]*m', '', line).strip()
    return (not text or text in routine or text=='=== Puhuri 2 - v1.2.0 ==='
        or text in ('Using staged fi.reijo.puhuri2_1.2.0_arm64.click',
                    'Using staged background-1.2.0.tar.gz')
        or re.fullmatch(r"\(process:[0-9]+\): lomiri-app-launch-WARNING \*\*: [0-9:.]+: Desktop file '/opt/click.ubuntu.com/\.click/users/[^/]+/fi\.reijo\.puhuri2/puhuri-daemon\.desktop' is set to not display, not copying", text))
# Only child program output is filtered. sudo still opens /dev/tty itself;
# no terminal input/password is read, captured or stored by this wrapper.
hidden=collections.deque(maxlen=40)
with subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                      text=True, errors='replace', bufsize=1) as child:
    for line in child.stdout:
        if not verbose and is_routine(line):
            if line.strip(): hidden.append(line)
        else:
            print(line, end='', flush=True)
    code=child.wait()
if code and hidden:
    print('Operation failed; preceding details / Toiminto epäonnistui, lisätiedot:', flush=True)
    for line in hidden: print(line, end='', flush=True)
record={"token":token,"exit_code":code}
temporary=stage/"operation-result.new"
with temporary.open("x") as stream:
    json.dump(record,stream)
temporary.chmod(0o600)
temporary.replace(stage/"operation-result.json")
raise SystemExit(code)
'''
POSTCONDITIONS = '''import json,os,subprocess,sys
from pathlib import Path
action=sys.argv[1];app="fi.reijo.puhuri2";home=Path.home()
os.environ["XDG_RUNTIME_DIR"]="/run/user/"+str(os.getuid())
os.environ["DBUS_SESSION_BUS_ADDRESS"]="unix:path="+os.environ["XDG_RUNTIME_DIR"]+"/bus"
rows=subprocess.check_output(["click","list"],text=True).splitlines()
versions=[row.split()[1] for row in rows if len(row.split())==2 and row.split()[0]==app]
units=("puhurid2.service","puhuri2-indicator.service")
state=home/".local/share/fi.reijo.puhuri2-background/installed.json"
flag=home/".config/fi.reijo.puhuri2/service-managed"
if action=="install":
    if versions!=["1.2.0"] or not state.is_file() or not flag.is_file():
        raise SystemExit("Installed registration/state/flag missing")
    if json.loads(state.read_text()).get("version")!="1.2.0":
        raise SystemExit("Installed service version mismatch")
    for check in ("is-active","is-enabled"):
        subprocess.run(["systemctl","--user",check,*units],check=True,stdout=subprocess.DEVNULL)
elif action=="remove":
    if versions or state.exists() or flag.exists():
        raise SystemExit("Application registration or service state still present")
    for unit in units:
        if (home/".config/systemd/user"/unit).exists():
            raise SystemExit("Service unit still present")
        if subprocess.run(["systemctl","--user","is-active",unit],stdout=subprocess.DEVNULL).returncode==0:
            raise SystemExit("Service still active")
else:
    raise SystemExit("Unexpected action")
print("Phone operation postconditions OK")
'''


def sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def verify_assets(directory):
    for name, expected in PAYLOADS.items():
        path = directory / name
        if not path.is_file():
            raise RuntimeError('Missing computer-side payload: ' + name)
        if sha256(path) != expected:
            raise RuntimeError('Computer-side checksum mismatch: ' + name)


def select_device(adb, requested=None):
    result = subprocess.run([adb, 'devices'], check=True, capture_output=True, text=True)
    devices = {}
    for line in result.stdout.splitlines()[1:]:
        fields = line.split()
        if len(fields) >= 2:
            devices[fields[0]] = fields[1]
    if requested:
        if devices.get(requested) != 'device':
            raise RuntimeError('Requested ADB device is absent, offline or unauthorized')
        return requested
    ready = [serial for serial, state in devices.items() if state == 'device']
    if not ready:
        raise RuntimeError('No authorized ADB phone. Connect USB and approve debugging on the phone.')
    if len(ready) > 1:
        raise RuntimeError('Several ADB devices. Select the phone with --serial SERIAL.')
    return ready[0]


@contextmanager
def terminal_input():
    # curl | python consumes stdin as source. Menu choices and interactive ADB
    # must use the computer's controlling terminal, not the exhausted pipe.
    if sys.stdin.isatty():
        yield sys.stdin
    else:
        with open('CONIN$' if os.name == 'nt' else '/dev/tty', 'r') as terminal:
            yield terminal


class Phone:
    def __init__(self, adb, serial, verbose=False):
        self.verbose = verbose
        self.command = [adb, '-s', serial]

    def shell(self, arguments, interactive=False, capture=False):
        # ADB invokes the phone's POSIX shell; quote each remote argument.
        remote = ' '.join(shlex.quote(str(arg)) for arg in arguments)
        command = self.command + ['shell', '-tt' if interactive else '-T', remote]
        if interactive:
            with terminal_input() as terminal:
                return subprocess.run(command, stdin=terminal, check=True, text=True)
        return subprocess.run(command, check=True, capture_output=capture, text=True)

    def check(self):
        result = self.shell(['/usr/bin/python3', '-c', PHONE_PROBE], capture=True)
        return json.loads(result.stdout)

    def run_interactive(self, arguments, stage):
        token = uuid.uuid4().hex
        transport_error = None
        try:
            runner = OPERATION_RUNNER.replace('verbose = False', 'verbose = True') if self.verbose else OPERATION_RUNNER
            self.shell(['/usr/bin/python3', '-c', runner, stage, token,
                        json.dumps(arguments)], interactive=True)
        except subprocess.CalledProcessError as error:
            if error.returncode != 129:
                raise
            transport_error = error
        # Do not infer success from printed output or blindly ignore 129. The
        # result is atomically written after the actual child process exits.
        try:
            response = self.shell(['/usr/bin/python3', '-c',
                'import sys; from pathlib import Path; print(Path(sys.argv[1]).read_text())',
                stage + '/operation-result.json'], capture=True)
            result = json.loads(response.stdout)
        except (subprocess.CalledProcessError, ValueError) as error:
            raise RuntimeError('No verified phone completion result; operation is incomplete') from error
        if (not isinstance(result, dict) or result.get('token') != token
                or type(result.get('exit_code')) is not int or result['exit_code'] != 0):
            raise RuntimeError('Phone operation failed or completion result did not match')
        if transport_error and self.verbose:
            print('Phone operation completion independently verified after ADB terminal shutdown.', flush=True)

    def verify_postconditions(self, action):
        self.shell(['/usr/bin/python3', '-c', POSTCONDITIONS, action], capture=not self.verbose)

    def operate(self, action, directory):
        print('Checking phone / Tarkistetaan puhelin…', flush=True)
        self.check()
        stage = self.shell(['mktemp', '-d', '/tmp/puhuri-adb.XXXXXX'], capture=True).stdout.strip()
        if not re.fullmatch(r'/tmp/puhuri-adb\.[A-Za-z0-9]+', stage):
            raise RuntimeError('Unexpected phone staging directory; refusing transfer/cleanup')
        names = list(PAYLOADS) if action == 'install' else ['install-puhuri-1.2.0.py']
        try:
            print('Transferring files / Siirretään tiedostot…', flush=True)
            for name in names:
                subprocess.run(self.command + ['push', str(directory / name), stage + '/' + name], check=True,
                               stdout=None if self.verbose else subprocess.DEVNULL)
            manifest = {name: PAYLOADS[name] for name in names}
            verification = '''import hashlib,json,sys
from pathlib import Path
root=Path(sys.argv[1]); expected=json.loads(sys.argv[2])
for name,digest in expected.items():
    with (root/name).open("rb") as stream:
        if hashlib.file_digest(stream,"sha256").hexdigest()!=digest:
            raise SystemExit("ADB payload checksum mismatch")
print("ADB payload checksums OK")
'''
            self.shell(['/usr/bin/python3', '-c', verification, stage, json.dumps(manifest)], capture=not self.verbose)
            print(('Installing app and services / Asennetaan sovellus ja palvelut…' if action == 'install'
                   else 'Removing app and services / Poistetaan sovellus ja palvelut…'), flush=True)
            print('Phone password if requested / Anna puhelimen salasana pyydettäessä.', flush=True)
            # Inherit the terminal. No password is read by this program or
            # placed in process arguments, environment variables or log files.
            self.run_interactive(['env', 'PYTHONDONTWRITEBYTECODE=1', '/usr/bin/python3',
                        stage + '/install-puhuri-1.2.0.py', action,
                        '--local-assets', stage], stage)
            self.verify_postconditions(action)
        finally:
            try:
                self.shell(['rm', '-rf', '--', stage])
            except subprocess.CalledProcessError:
                print('ADB cleanup failed. Reconnect and remove this temporary phone directory: ' + stage,
                      file=sys.stderr)
                raise


def choose_action():
    print('\033[1;32m1)\033[0m Install or update / Asenna tai päivitä\n'
          '\033[1;32m2)\033[0m Remove / Poista\n'
          '\033[1;32m0)\033[0m Exit / Lopeta')
    with terminal_input() as terminal:
        while True:
            print('Choose / Valitse [1/2/0]: ', end='', flush=True)
            answer = terminal.readline()
            if not answer:
                raise RuntimeError('Run the computer installer in a terminal')
            action = {'1': 'install', '2': 'remove', '0': 'exit'}.get(answer.strip())
            if action:
                return action
            print('Choose 1, 2 or 0.')


def perform(action, args):
    verify_assets(args.assets_dir)
    adb = shutil.which('adb')
    if not adb:
        raise RuntimeError('Install Android platform-tools/adb on the COMPUTER and add adb to PATH.')
    serial = select_device(adb, args.serial)
    phone = Phone(adb, serial, verbose=getattr(args, 'verbose', False))
    if action == 'check':
        print(json.dumps(phone.check(), indent=2))
        return
    phone.operate(action, args.assets_dir)
    print('\033[1;32mOK\033[0m ' + ('Installed / Asennettu: Puhuri 2 v' + VERSION + ' + background services / taustapalvelut.'
           if action == 'install' else
           'Removed / Poistettu: Puhuri 2 + services / palvelut. Data retained / Omat tiedot säilytetty.'), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', nargs='?', choices=['install', 'remove', 'check'])
    parser.add_argument('--assets-dir', type=Path, default=Path(__file__).resolve().parent)
    parser.add_argument('--verbose', action='store_true', help='Show full installation output / Näytä kaikki tulosteet')
    parser.add_argument('--serial', help='Use this authorized ADB device when several are attached')
    args = parser.parse_args()
    if hasattr(os, 'geteuid') and os.geteuid() == 0:
        raise RuntimeError('Run on the COMPUTER as the normal user, without sudo.')
    print('\033[1;32m=== Puhuri 2 - v' + VERSION + ' ===\033[0m', flush=True)
    print('Computer installer — authorized ADB connection to the phone required.', flush=True)
    if args.action:
        perform(args.action, args)
        return
    action = choose_action()
    if action == 'exit':
        print('Exit / Lopetettu.')
        return
    perform(action, args)


if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError, OSError) as error:
        print('ADB installer: ' + str(error), file=sys.stderr)
        sys.exit(1)
