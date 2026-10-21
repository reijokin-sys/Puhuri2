#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Read-only phone storage snapshot; emit no audio, paths or message content.

Run with: adb shell -T python3 - < tools/measure-voice-storage.py
File tokens permit before/after comparison without retaining filenames.
"""
import datetime
import hashlib
import json
import os
from pathlib import Path

root = Path.home() / '.local/share/fi.reijo.puhuri2'
folders = {}
for name in ('attachments', 'recordings'):
    base = root / name
    entries = []
    for path in sorted(base.rglob('*')) if base.exists() else []:
        if not path.is_file() or path.is_symlink():
            continue
        stat = path.stat()
        codec = None
        if path.suffix.lower() in ('.ogg', '.oga', '.opus', '.wav'):
            with path.open('rb') as stream:
                header = stream.read(128)
            if b'OpusHead' in header:
                codec = 'opus'
            elif b'\x01vorbis' in header:
                codec = 'vorbis'
            elif header.startswith(b'RIFF'):
                codec = 'riff'
        entries.append({
            'token': hashlib.sha256(str(path.relative_to(root)).encode()).hexdigest(),
            'bytes': stat.st_size,
            'allocated_bytes': stat.st_blocks * 512,
            'links': stat.st_nlink,
            'codec': codec,
        })
    folders[name] = {
        'files': len(entries),
        'bytes': sum(item['bytes'] for item in entries),
        'allocated_bytes': sum(item['allocated_bytes'] for item in entries),
        'entries': entries,
    }

# An unlinked file can retain blocks until its final open descriptor closes.
open_files = []
denied = 0
for process in Path('/proc').iterdir():
    if not process.name.isdigit():
        continue
    try:
        if process.stat().st_uid != os.getuid():
            continue
        for descriptor in (process / 'fd').iterdir():
            try:
                target = os.readlink(descriptor)
                deleted = target.endswith(' (deleted)')
                clean = target.removesuffix(' (deleted)')
                path = Path(clean)
                if not any(path.is_relative_to(root / name) for name in folders):
                    continue
                stat = descriptor.stat()
                open_files.append({
                    'token': hashlib.sha256(str(path.relative_to(root)).encode()).hexdigest(),
                    'deleted': deleted,
                    'allocated_bytes': stat.st_blocks * 512,
                })
            except (FileNotFoundError, PermissionError, ProcessLookupError):
                continue
    except (FileNotFoundError, PermissionError, ProcessLookupError):
        denied += 1

disk = os.statvfs(root)
print(json.dumps({
    'measured_utc': datetime.datetime.now(datetime.timezone.utc).isoformat(),
    'folders': folders,
    'open_attachment_descriptors': open_files,
    'same_user_process_fd_directories_unreadable': denied,
    'filesystem_available_bytes': disk.f_bavail * disk.f_frsize,
}, indent=2))
