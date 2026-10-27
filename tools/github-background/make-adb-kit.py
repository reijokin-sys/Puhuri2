#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Package the computer-side ADB installer with the versioned application payloads."""
import hashlib
from pathlib import Path
import shutil
import zipfile

HERE = Path(__file__).resolve().parent
PROJECT = HERE.parent.parent
VERSION = '1.2.5'
APP_VERSION = '1.2.4'
OUTPUT = PROJECT / ('dist/puhuri2-' + VERSION + '-adb-installer')
OUTPUT.mkdir(exist_ok=False)
inputs = {
    HERE / 'ADB-README.en.md': 'README.md',
    HERE / 'ADB-OHJE.md': 'OHJE.md',
}
for name in ('LICENSE', 'THIRD_PARTY_NOTICES.md', 'THIRD_PARTY_LICENSES.txt'):
    inputs[PROJECT / name] = name
click = PROJECT / ('dist/fi.reijo.puhuri2_' + APP_VERSION + '_arm64.click')
background = PROJECT / ('dist/background-' + APP_VERSION + '.tar.gz')
click_hash = hashlib.sha256(click.read_bytes()).hexdigest()
background_hash = hashlib.sha256(background.read_bytes()).hexdigest()
def stamp(source):
    return source.replace('1.2.0', APP_VERSION).replace(
        '1f7a5efe839966f04ce14f08564b17d71de35f36cfd8f0989410dbd3bb544571', click_hash).replace(
        '4ca739abda186a3d5253ff3eb814f32d750b7857b030a5edc67790df07db552b', background_hash)
phone = stamp((HERE / 'install-release.py').read_text())
(OUTPUT / ('install-puhuri-' + APP_VERSION + '.py')).write_text(phone)
host = stamp((HERE / 'install-adb.py').read_text()).replace(
    '22b80af0d51a72e4330ac94f8003e3167d176da15007c4429749daee7e9f18b6',
    hashlib.sha256(phone.encode()).hexdigest())
(OUTPUT / 'install-puhuri-adb.py').write_text(host)
inputs[click] = click.name
inputs[background] = background.name
for source, name in inputs.items():
    shutil.copy2(source, OUTPUT / name)
checksums = ''.join(hashlib.sha256(path.read_bytes()).hexdigest() + '  ' + path.name + '\n'
                    for path in sorted(OUTPUT.iterdir()))
(OUTPUT / 'SHA256SUMS').write_text(checksums)
with zipfile.ZipFile(str(OUTPUT) + '.zip', 'x', compression=zipfile.ZIP_DEFLATED) as archive:
    for path in sorted(OUTPUT.iterdir()):
        info = zipfile.ZipInfo(OUTPUT.name + '/' + path.name, date_time=(1980, 1, 1, 0, 0, 0))
        info.external_attr = 0o100644 << 16
        archive.writestr(info, path.read_bytes(), compress_type=zipfile.ZIP_DEFLATED)
print('Built ' + str(OUTPUT.relative_to(PROJECT)) + '.zip')
print('SHA-256 ' + hashlib.sha256(Path(str(OUTPUT) + '.zip').read_bytes()).hexdigest())

# The launcher is a separate release asset to avoid a circular ZIP checksum.
archive_digest = hashlib.sha256(Path(str(OUTPUT) + '.zip').read_bytes()).hexdigest()
launcher = (HERE / 'install.sh.in').read_text().replace('@VERSION@', VERSION).replace('@SHA256@', archive_digest)
assert '@VERSION@' not in launcher and '@SHA256@' not in launcher
(PROJECT / 'dist' / ('install-puhuri-' + VERSION + '.sh')).write_text(launcher)
print('Built pinned launcher install-puhuri-' + VERSION + '.sh (publish as install.sh)')

# Workstation-only test entry: same hash/flow, ZIP read locally, no release URL.
release_zip_url = ('https://github.com/reijokin-sys/Puhuri2/releases/download/v' + VERSION
                   + '/puhuri2-' + VERSION + '-adb-installer.zip')
local_launcher = launcher.replace(release_zip_url, Path(str(OUTPUT) + '.zip').as_uri())
(PROJECT / 'dist' / ('install-puhuri-' + VERSION + '-local-test.sh')).write_text(local_launcher)
