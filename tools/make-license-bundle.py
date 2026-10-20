#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Build the deterministic third-party licence bundle shipped in the click."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
import tempfile
from collections import defaultdict
from pathlib import Path


PROJECT = Path(__file__).resolve().parent.parent
DAEMON = PROJECT / "daemon"
OUTPUT = PROJECT / "THIRD_PARTY_LICENSES.txt"
TARGET = "aarch64-unknown-linux-gnu"
FEATURES = "omemo"
LICENSE_NAME = re.compile(
    r"^(?:licen[cs]e|copying|notice|unlicense|copyright)", re.IGNORECASE
)


def run_cargo(*args: str) -> str:
    result = subprocess.run(
        ["cargo", *args],
        cwd=DAEMON,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    return result.stdout


def package_set() -> list[tuple[str, str]]:
    output = run_cargo(
        "tree",
        "--locked",
        "--target",
        TARGET,
        "--features",
        FEATURES,
        "--edges",
        "normal,build",
        "--prefix",
        "none",
        "--format",
        "{p}",
    )
    found: set[tuple[str, str]] = set()
    pattern = re.compile(r"^(\S+) v(\S+?)(?: \(.*\))?$")
    for raw in output.splitlines():
        line = raw.strip()
        match = pattern.match(line)
        if match:
            found.add((match.group(1), match.group(2)))
    if not found:
        raise RuntimeError("cargo tree did not return any packages")
    return sorted(found, key=lambda item: (item[0].lower(), item[1]))


def metadata_packages() -> dict[tuple[str, str], dict]:
    output = run_cargo(
        "metadata",
        "--locked",
        "--format-version",
        "1",
        "--all-features",
    )
    metadata = json.loads(output)
    return {
        (package["name"], package["version"]): package
        for package in metadata["packages"]
    }


def normalise_text(path: Path) -> str:
    text = path.read_text(encoding="utf-8", errors="replace")
    text = text.replace("\r\n", "\n").replace("\r", "\n").strip()
    return text + "\n"


def license_files(package: dict) -> list[Path]:
    root = Path(package["manifest_path"]).parent
    candidates: set[Path] = set()
    explicit = package.get("license_file")
    if explicit:
        path = Path(explicit)
        candidates.add(path if path.is_absolute() else root / path)
    for path in root.iterdir():
        if path.is_file() and LICENSE_NAME.match(path.name):
            candidates.add(path)
    return sorted(path for path in candidates if path.is_file())


def external_file(path: Path, component: str, licence: str) -> dict:
    return {
        "name": component,
        "licence": licence,
        "files": [(path.name, normalise_text(path))],
    }


def build_bundle() -> str:
    wanted = package_set()
    packages = metadata_packages()
    missing_metadata = [f"{name} {version}" for name, version in wanted if (name, version) not in packages]
    if missing_metadata:
        raise RuntimeError(
            "cargo metadata did not contain: " + ", ".join(missing_metadata)
        )

    package_files: dict[tuple[str, str], list[tuple[str, str]]] = {}
    donors: dict[str, list[tuple[str, str]]] = defaultdict(list)

    for key in wanted:
        package = packages[key]
        if package["name"] == "puhurid":
            continue
        files = [
            (path.name, normalise_text(path))
            for path in license_files(package)
        ]
        package_files[key] = files
        expression = package.get("license") or "NOASSERTION"
        if files:
            donors[expression].extend(files)

    unresolved: list[str] = []
    for key in wanted:
        package = packages[key]
        if package["name"] == "puhurid" or package_files[key]:
            continue
        expression = package.get("license") or "NOASSERTION"
        package_files[key] = donors.get(expression, [])
        if not package_files[key]:
            unresolved.append(f"{package['name']} {package['version']} ({expression})")
    if unresolved:
        raise RuntimeError(
            "no full licence text found for: " + ", ".join(unresolved)
        )

    external = [
        external_file(
            PROJECT / "vendor/libsignal-protocol-c/LICENSE",
            "libsignal-protocol-c 2.3.3",
            "GPL-3.0",
        ),
        external_file(
            PROJECT / "vendor/protobuf-c-1.5.1/LICENSE",
            "protobuf-c 1.5.1",
            "BSD-2-Clause",
        ),
        external_file(
            PROJECT / "app/Lomiri/Components/Themes/LICENSE.LGPL-3.0",
            "Puhuri 2 Lomiri theme modules",
            "LGPL-3.0-only",
        ),
    ]

    texts: dict[str, str] = {}
    for files in package_files.values():
        for _, content in files:
            texts[hashlib.sha256(content.encode()).hexdigest()] = content
    for component in external:
        for _, content in component["files"]:
            texts[hashlib.sha256(content.encode()).hexdigest()] = content

    text_id = {
        digest: f"{index:04d}"
        for index, digest in enumerate(sorted(texts), start=1)
    }

    lines = [
        "Puhuri 2 third-party licence bundle",
        "=================================",
        "",
        "Generated by tools/make-license-bundle.py. Do not edit by hand.",
        f"Target: {TARGET}",
        f"Features: {FEATURES}",
        "Cargo.lock SHA-256: "
        + hashlib.sha256((DAEMON / "Cargo.lock").read_bytes()).hexdigest(),
        "Licence selection follows each package's Cargo SPDX expression.",
        "",
        "PACKAGE NOTICES",
        "===============",
        "",
    ]

    for key in wanted:
        package = packages[key]
        if package["name"] == "puhurid":
            continue
        lines.extend(
            [
                f"Package: {package['name']} ",
                f"Version: {package['version']}",
                f"Licence: {package.get('license') or 'NOASSERTION'}",
                "Authors: " + ("; ".join(package.get("authors") or []) or "not declared"),
                "Repository: " + (package.get("repository") or "not declared"),
                "Licence texts: "
                + ", ".join(
                    f"TEXT {text_id[hashlib.sha256(content.encode()).hexdigest()]}"
                    for _, content in package_files[key]
                ),
                "",
            ]
        )

    lines.extend(
        [
            "BUNDLED NON-RUST COMPONENTS",
            "===========================",
            "",
        ]
    )
    for component in external:
        lines.extend(
            [
                f"Component: {component['name']}",
                f"Licence: {component['licence']}",
                "Licence texts: "
                + ", ".join(
                    f"TEXT {text_id[hashlib.sha256(content.encode()).hexdigest()]}"
                    for _, content in component["files"]
                ),
                "",
            ]
        )

    lines.extend(["LICENCE TEXTS", "=============", ""])
    for digest in sorted(texts, key=lambda item: text_id[item]):
        lines.extend(
            [
                f"===== TEXT {text_id[digest]} =====",
                "",
                texts[digest].rstrip(),
                "",
            ]
        )
    return "\n".join(lines).rstrip() + "\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--check",
        action="store_true",
        help="fail if the checked-in bundle differs from the generated content",
    )
    args = parser.parse_args()

    try:
        generated = build_bundle()
    except (OSError, RuntimeError, subprocess.CalledProcessError, ValueError) as error:
        print(f"FAIL: licence bundle: {error}", file=sys.stderr)
        return 1

    if args.check:
        try:
            current = OUTPUT.read_text(encoding="utf-8")
        except FileNotFoundError:
            print(f"FAIL: {OUTPUT.name} is missing", file=sys.stderr)
            return 1
        if current != generated:
            print(
                f"FAIL: {OUTPUT.name} is stale; run tools/make-license-bundle.py",
                file=sys.stderr,
            )
            return 1
        print(f"PASS: {OUTPUT.name} is current")
        return 0

    with tempfile.NamedTemporaryFile(
        "w", encoding="utf-8", dir=OUTPUT.parent, delete=False
    ) as handle:
        handle.write(generated)
        temporary = Path(handle.name)
    temporary.chmod(0o644)
    temporary.replace(OUTPUT)
    print(f"wrote {OUTPUT}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
