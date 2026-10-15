#!/usr/bin/env python3
"""Reject license declarations until the licensing cutover (#124).

A license grant cannot be revoked once it is pushed. Until #255 replaces this
guard with the full licensing checks, the interim notice reserves all rights
and only the allowlisted package may declare a license.
"""
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import tomllib

# The only own-code grant before the cutover. Its expression is pinned too.
ALLOWED_CARGO_LICENSES = {"crates/aidash-capability/Cargo.toml": "Apache-2.0"}
# SHA-256 of the interim notice. Changing it is a licensing decision.
ROOT_LICENSE_SHA256 = "72fc11963fe4eb1f463f5da9ee62162e28fc12c37d95c1da42311abfa778510d"
THIRD_PARTY = "third_party"
NOTICES = "THIRD_PARTY_NOTICES.md"
# Assembled so that this tracked file never contains the markers it rejects.
SPDX_MARKER = ("SPDX-" + "License-Identifier").encode()
OCI_MARKER = ("org.opencontainers.image." + "licenses").encode()
# LICENSE-MIT, COPYING.LESSER or license.md, but not license-screen.tsx.
LICENSE_NAME = re.compile(r"^(LICEN[CS]E|COPYING)|^(?i:licen[cs]e|copying)(\.(md|txt|rst))?$")
TAURI_CONFIG = re.compile(r"tauri(\..+)?\.conf\.json")
# A notices list item that starts with a backticked path names a tracked file.
LISTED_PATH = re.compile(r"^- `([^`]+)`", re.MULTILINE)
THIRD_PARTY_REFERENCE = re.compile(r"third_party/[^\s()\[\]`<>]+")


def tracked_files(root):
    output = subprocess.check_output(["git", "ls-files", "-z"], cwd=root)
    return sorted(path for path in output.decode().split("\0") if path)


def cargo_violations(path, manifest):
    allowed = ALLOWED_CARGO_LICENSES.get(path)
    package = manifest.get("package", {})
    errors = []
    if allowed is not None and package.get("license") != allowed:
        errors.append(f"{path}: the allowlisted license must stay {allowed!r}")
    tables = {
        "package": package,
        "workspace.package": manifest.get("workspace", {}).get("package", {}),
    }
    for section, table in tables.items():
        for key in ("license", "license-file"):
            if key in table and not (section == "package" and key == "license" and allowed is not None):
                errors.append(f"{path}: [{section}] declares {key}")
    return errors


def json_violations(path, document):
    if PurePosixPath(path).name == "package.json":
        return [f"{path}: declares {key}" for key in ("license", "licenses") if key in document]
    bundle = document.get("bundle", {})
    return [f"{path}: bundle declares {key}" for key in ("license", "licenseFile") if key in bundle]


def notice_violations(root, files):
    tracked = set(files)
    if NOTICES not in tracked:
        return [f"{NOTICES} is missing"]
    text = (root / NOTICES).read_text()
    errors = [
        f"{NOTICES}: lists {path}, which is not a tracked file"
        for path in LISTED_PATH.findall(text) if path not in tracked
    ]
    errors += [
        f"{NOTICES}: refers to {path}, which is not a tracked file"
        for path in THIRD_PARTY_REFERENCE.findall(text) if path not in tracked
    ]
    projects = sorted({
        PurePosixPath(path).parts[1] for path in tracked
        if PurePosixPath(path).parts[0] == THIRD_PARTY and len(PurePosixPath(path).parts) > 2
    })
    for project in projects:
        notice = f"{THIRD_PARTY}/{project}/LICENSE"
        if notice not in tracked:
            errors.append(f"{THIRD_PARTY}/{project}/ has no LICENSE")
        elif notice not in text:
            errors.append(f"{NOTICES} does not list {notice}")
    return errors


def violations(root, files):
    errors = []
    if "LICENSE" not in files:
        errors.append("the root LICENSE interim notice is missing")
    elif hashlib.sha256((root / "LICENSE").read_bytes()).hexdigest() != ROOT_LICENSE_SHA256:
        errors.append("the root LICENSE differs from the interim notice; licensing changes belong to #255")
    for path in files:
        parts = PurePosixPath(path).parts
        name = parts[-1]
        in_third_party = parts[0] == THIRD_PARTY
        if LICENSE_NAME.match(name) and not in_third_party and path != "LICENSE":
            errors.append(f"{path}: license files belong only in the root LICENSE or {THIRD_PARTY}/")
        if not (root / path).is_file():
            continue
        if name == "Cargo.toml":
            errors += cargo_violations(path, tomllib.loads((root / path).read_text()))
        elif name == "package.json" or TAURI_CONFIG.fullmatch(name):
            errors += json_violations(path, json.loads((root / path).read_text()))
        if in_third_party:
            continue
        content = (root / path).read_bytes()
        if SPDX_MARKER in content:
            errors.append(f"{path}: contains an SPDX license identifier")
        if OCI_MARKER in content:
            errors.append(f"{path}: declares an OCI image license label")
    return errors + notice_violations(root, files)


def main(argv):
    root = Path(argv[1]).resolve() if len(argv) > 1 else Path(__file__).resolve().parents[1]
    errors = violations(root, tracked_files(root))
    for error in errors:
        print(error, file=sys.stderr)
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
