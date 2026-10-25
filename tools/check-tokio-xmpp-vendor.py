#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Reproduce the pinned tokio-xmpp and xmpp-parsers trees from its registry archive + local patch."""
from pathlib import Path
import hashlib
import subprocess
import tarfile
import tempfile

PROJECT = Path(__file__).resolve().parents[1]
def verify(name: str, checksum: str, patch: str) -> None:
    vendor = PROJECT / "vendor" / name
    archive_name = "upstream-" + name.rsplit("-", 1)[1] + ".crate"
    archive_path = vendor / archive_name
    if hashlib.sha256(archive_path.read_bytes()).hexdigest() != checksum:
        raise SystemExit("FAIL: registry archive checksum differs: " + name)
    with tempfile.TemporaryDirectory(prefix="puhuri-vendor-check-") as temporary:
        base = Path(temporary)
        originals = []
        with tarfile.open(archive_path) as archive:
            for member in archive.getmembers():
                relative = Path(member.name)
                if relative.is_absolute() or ".." in relative.parts or relative.parts[0] != name:
                    raise SystemExit("FAIL: unexpected archive path")
                if member.isdir():
                    continue
                if not member.isfile():
                    raise SystemExit("FAIL: non-regular archive member")
                destination = base / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes(archive.extractfile(member).read())
                originals.append(Path(*relative.parts[1:]))
        pristine = base / name
        subprocess.run(["patch", "--batch", "--fuzz=0", "-p1", "-i", str(vendor / patch)], cwd=pristine, check=True)
        for relative in originals:
            if (pristine / relative).read_bytes() != (vendor / relative).read_bytes():
                raise SystemExit(f"FAIL: vendor file does not match archive + patch: {name}/{relative}")
        allowed = {"LICENSE", "VENDOR.md", archive_name, patch}
        if any(p.is_symlink() for p in vendor.rglob("*")):
            raise SystemExit("FAIL: unexpected symlink in pinned vendor tree")
        unexpected = {str(p.relative_to(vendor)) for p in vendor.rglob("*") if p.is_file()} - {str(p) for p in originals} - allowed
        if unexpected:
            raise SystemExit("FAIL: unexplained vendor additions: " + ", ".join(sorted(unexpected)))
    print("PASS: registry checksum and complete source match archive + patch: " + name)


# Registry checksums retained from daemon/Cargo.lock before path overrides.
verify("tokio-xmpp-6.0.0", "8921a165ab1eefd447cb1661423c582ac77b9c6960e123a71c8ad9cebc40b026", "patches/0001-durable-ingress.patch")
verify("xmpp-parsers-0.23.0", "92b5d63bbd122ce3b5287d83efdd6916fb88bb274be23f6a9140238e0e1465b0", "patches/0001-sm-failed-optional-h.patch")
