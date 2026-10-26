#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Package the computer-side ADB installer with the unchanged v1.2.0 payloads."""
import hashlib
from pathlib import Path
import shutil
import zipfile

HERE = Path(__file__).resolve().parent
PROJECT = HERE.parent.parent
OUTPUT = PROJECT / 'dist/puhuri2-1.2.3-adb-installer'
OUTPUT.mkdir(exist_ok=False)
inputs = {
    HERE / 'install-adb.py': 'install-puhuri-adb.py',
    HERE / 'ADB-README.en.md': 'README.md',
    HERE / 'ADB-OHJE.md': 'OHJE.md',
}
for name in ('LICENSE', 'THIRD_PARTY_NOTICES.md', 'THIRD_PARTY_LICENSES.txt'):
    inputs[PROJECT / name] = name
for name in ('install-puhuri-1.2.0.py', 'fi.reijo.puhuri2_1.2.0_arm64.click', 'background-1.2.0.tar.gz'):
    inputs[PROJECT / 'dist' / name] = name
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
