#!/usr/bin/python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Build a background add-on matched to one exact local click; never publish."""
import argparse
import gzip
import hashlib
import io
import json
import re
from pathlib import Path
import shutil
import subprocess
import tarfile

HERE = Path(__file__).resolve().parent
PROJECT = HERE.parent.parent


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('click', type=Path)
parser.add_argument('output', type=Path)
args = parser.parse_args()
payload = subprocess.check_output(['ar', 'p', str(args.click), 'data.tar.gz'])
control = subprocess.check_output(['ar', 'p', str(args.click), 'control.tar.gz'])
with tarfile.open(fileobj=io.BytesIO(control), mode='r:gz') as archive:
    manifest_bytes = archive.extractfile('./manifest').read()
manifest = json.loads(manifest_bytes)
with tarfile.open(fileobj=io.BytesIO(payload), mode='r:gz') as archive:
    def content(name):
        return archive.extractfile('./' + name).read()
    if manifest['name'] != 'fi.reijo.puhuri2' or manifest['architecture'] != 'arm64':
        raise ValueError('Unexpected click identity/architecture')
    if not all(manifest.get(k) == v for k, v in json.loads((PROJECT / 'app/manifest.json').read_text()).items()):
        raise ValueError('Click is not from current manifest')
    daemon = content('lib/aarch64-linux-gnu/bin/puhurid')
    if b'--service-mode' not in daemon:
        raise ValueError('Click daemon lacks service mode')
    if b'service-managed' not in content('puhuri.py'):
        raise ValueError('Click UI lacks service ownership guard')
    click_files = {name: hashlib.sha256(content(name)).hexdigest() for name in
                   ('puhuri.py', 'lib/aarch64-linux-gnu/bin/puhurid')}
    click_files['.click/info/fi.reijo.puhuri2.manifest'] = hashlib.sha256(manifest_bytes).hexdigest()
    licence_files = {name: content(name) for name in
                     ('LICENSE', 'THIRD_PARTY_NOTICES.md', 'THIRD_PARTY_LICENSES.txt')}
args.output.mkdir(parents=True, exist_ok=False)
for name in ('manage.py', 'launch.py', 'README.md'):
    shutil.copy2(HERE / name, args.output / name)
for name in ('notifier.py', 'messaging.py'):
    shutil.copy2(PROJECT / 'tools/device-background' / name, args.output / name)
for name, data in licence_files.items():
    (args.output / name).write_bytes(data)
files = {path.name: digest(path) for path in sorted(args.output.iterdir())}
schema = int(re.search(r'pub const SCHEMA_VERSION: i64 = (\d+);', (PROJECT / 'daemon/src/store.rs').read_text())[1])
meta = {'app_id': manifest['name'], 'version': manifest['version'], 'schema_version': schema,
        'architecture': manifest['architecture'], 'click_sha256': digest(args.click),
        'click_files': click_files, 'files': files}
(args.output / 'bundle.json').write_text(json.dumps(meta, indent=2) + '\n')
def clean_info(info):
    info.uid = info.gid = 0
    info.uname = info.gname = ''
    info.mtime = 0
    info.mode = 0o755 if info.isdir() else 0o644
    return info


with Path(str(args.output) + '.tar.gz').open('xb') as raw:
    with gzip.GzipFile(fileobj=raw, filename='', mode='wb', mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode='w') as output:
            output.add(args.output, arcname=args.output.name, filter=clean_info)
print(json.dumps({'bundle': str(args.output), 'archive_sha256': digest(Path(str(args.output) + '.tar.gz'))}, indent=2))
