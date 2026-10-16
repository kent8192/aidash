#!/usr/bin/env python3
"""Reject license declarations until the licensing cutover (#124).

A license grant cannot be revoked once it is pushed. Until #255 replaces this
guard with the full licensing checks, the interim notice reserves all rights
and only the allowlisted package may declare a license.
"""
import configparser
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import tomllib

# The only own-code grant before the cutover. Its expression is pinned too.
ALLOWED_CARGO_LICENSES = {"crates/aidash-capability/Cargo.toml": "Apache-2.0"}
# SHA-256 of the interim notice, hashed with CRLF read as LF so that a Windows
# checkout hashes the same text. Changing it is a licensing decision.
ROOT_LICENSE_SHA256 = "72fc11963fe4eb1f463f5da9ee62162e28fc12c37d95c1da42311abfa778510d"
THIRD_PARTY = "third_party"
NOTICES = "THIRD_PARTY_NOTICES.md"
# Every project whose adapted material Aidash carries: the notices section that
# describes it, its upstream provenance, the SHA-256 of its verbatim notice
# (hashed like the root notice) and the files that hold adapted material. The
# notices are checked against this inventory, not only against themselves, so
# attribution cannot disappear, move or change together with its notice.
# Changing it is a licensing decision.
REQUIRED_NOTICES = {
    "fast-jev-compaction": {
        "heading": "fast-jev-compaction",
        "source": "https://github.com/tamaratran/fast-jev-compaction",
        "revision": "e3f262a7f4d42bd8dd32ced30d26176f7cb545b0",
        "license": "MIT",
        "sha256": "06dcddbb6908a0c6dd4a9e8ec822eea41d5a460a53089fecccc8a68049e99241",
        "adapted": (
            "crates/aidash-application/src/context/compaction.rs",
            "crates/aidash-application/src/context.rs",
            "crates/aidash-integrations/src/compaction.rs",
            "crates/aidash-integrations/src/compaction/tests.rs",
            "server/src/apps/execution/services/context/jev.rs",
            "crates/aidash-application/src/decision/state.rs",
        ),
    },
    "hindsight": {
        "heading": "Hindsight",
        "source": "https://github.com/vectorize-io/hindsight",
        "revision": "b89ce287464fe10790d67ba86bfabe29d8926253",
        "license": "MIT",
        "sha256": "01fde0bedf83bdc185065d7af524a61690efe576a67f922eaff3a1280c17b63a",
        "adapted": (
            "crates/aidash-domain/src/memory.rs",
            "crates/aidash-domain/src/memory/recall.rs",
            "crates/aidash-domain/src/memory/graph.rs",
            "crates/aidash-domain/src/memory/extraction.rs",
            "crates/aidash-domain/src/memory/consolidation.rs",
            "crates/aidash-application/src/memory.rs",
            "server/src/apps/knowledge/repositories/engine_jobs.rs",
            "server/src/apps/knowledge/repositories/engine_jobs/consolidation.rs",
            "server/src/apps/knowledge/repositories/learning.rs",
            "server/src/apps/knowledge/services/memory_models.rs",
            "server/src/apps/knowledge/repositories/native_memory.rs",
            "server/src/apps/knowledge/repositories/postgres_vector.rs",
        ),
    },
}
# Assembled so that this tracked file never contains the markers it rejects.
SPDX_MARKER = ("SPDX-" + "License-Identifier").encode()
OCI_MARKER = ("org.opencontainers.image." + "licenses").encode()
# LICENSE-MIT, License-MIT, MIT-LICENSE, UNLICENSE, COPYING.LESSER or license.md,
# but not license-screen.tsx or license.rs: in any case, one dash or underscore
# prefix or suffix, then only a document extension.
LICENSE_NAME = re.compile(
    r"^(LICEN[CS]E|COPYING)"
    r"|^(?i:([^.]+[-_])?(un)?licen[cs]e|copying)([-_][^.]+)?(\.(?i:md|txt|rst))?$"
)
# A REUSE-style directory of license texts, such as LICENSES/MIT.txt.
LICENSE_DIRECTORY = "licenses"
PYTHON_MANIFESTS = ("pyproject.toml", "setup.cfg")
TAURI_CONFIG = re.compile(r"tauri(\..+)?\.conf\.json")
# A notices list item that starts with a backticked path names a tracked file.
LISTED_PATH = re.compile(r"^- `([^`]+)`", re.MULTILINE)
SECTION_HEADING = re.compile(r"^## (.+?)[ \t]*$", re.MULTILINE)
THIRD_PARTY_REFERENCE = re.compile(r"third_party/[^\s()\[\]`<>]+")


def tracked_files(root):
    output = subprocess.check_output(["git", "ls-files", "-z"], cwd=root)
    return sorted(path for path in output.decode().split("\0") if path)


def digest(path):
    return hashlib.sha256(path.read_bytes().replace(b"\r\n", b"\n")).hexdigest()


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


def python_violations(path, text):
    """Python packaging metadata that declares a license, as Cargo and npm can."""
    if PurePosixPath(path).name == "pyproject.toml":
        document = tomllib.loads(text)
        tables = {
            "project": document.get("project", {}),
            "tool.poetry": document.get("tool", {}).get("poetry", {}),
        }
    else:
        parser = configparser.ConfigParser(interpolation=None)
        parser.read_string(text)
        tables = {"metadata": dict(parser["metadata"]) if parser.has_section("metadata") else {}}
    errors = []
    for section, table in tables.items():
        errors += [
            f"{path}: [{section}] declares {key}" for key in table
            if key.replace("_", "-") in ("license", "license-file", "license-files")
        ]
        classifiers = table.get("classifiers", [])
        if isinstance(classifiers, str):
            classifiers = classifiers.splitlines()
        if any(classifier.strip().startswith("License ::") for classifier in classifiers):
            errors.append(f"{path}: [{section}] declares a License classifier")
    return errors


def project_violations(root, tracked, sections, project, expected):
    """Check one required project's pinned notice and its notices section."""
    notice = f"{THIRD_PARTY}/{project}/LICENSE"
    errors = []
    if notice not in tracked:
        errors.append(f"{notice} is missing for the required project {project}")
    elif digest(root / notice) != expected["sha256"]:
        errors.append(f"{notice} differs from the pinned upstream notice")
    heading = expected["heading"]
    section = sections.get(heading)
    if section is None:
        return errors + [f"{NOTICES} has no '## {heading}' section for {project}"]
    lines = section.splitlines()
    errors += [
        f"{NOTICES}: the {heading} section lacks '{line}'"
        for line in (
            f"- Source: <{expected['source']}>",
            f"- Revision: `{expected['revision']}`",
            f"- License: {expected['license']}, in [{notice}]({notice})",
        )
        if line not in lines
    ]
    listed = LISTED_PATH.findall(section)
    return errors + [
        f"{NOTICES} does not list {path} under {heading}, adapted from {project}"
        for path in expected["adapted"] if path not in listed
    ]


def notice_violations(root, files, required):
    tracked = set(files)
    if NOTICES not in tracked:
        return [f"{NOTICES} is missing"]
    text = (root / NOTICES).read_text()
    errors = [
        f"{NOTICES}: lists {path}, which is not a tracked file"
        for path in LISTED_PATH.findall(text) if path not in tracked
    ]
    # Each project's paths and provenance are read from its own section, so a
    # path moved under another project's heading no longer counts.
    parts = SECTION_HEADING.split(text)
    sections = dict(zip(parts[1::2], parts[2::2]))
    for project, expected in required.items():
        errors += project_violations(root, tracked, sections, project, expected)
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


def violations(root, files, required=REQUIRED_NOTICES):
    errors = []
    if "LICENSE" not in files:
        errors.append("the root LICENSE interim notice is missing")
    elif digest(root / "LICENSE") != ROOT_LICENSE_SHA256:
        errors.append("the root LICENSE differs from the interim notice; licensing changes belong to #255")
    # Only the pinned notices are exempt, not everything under third_party/.
    pinned = {f"{THIRD_PARTY}/{project}/LICENSE" for project in required}
    for path in files:
        parts = PurePosixPath(path).parts
        name = parts[-1]
        in_license_directory = any(part.lower() == LICENSE_DIRECTORY for part in parts[:-1])
        if (LICENSE_NAME.match(name) or in_license_directory) and path != "LICENSE" and path not in pinned:
            errors.append(f"{path}: license files belong only in the root LICENSE or a pinned third-party notice")
        if not (root / path).is_file():
            continue
        if name == "Cargo.toml":
            errors += cargo_violations(path, tomllib.loads((root / path).read_text()))
        elif name == "package.json" or TAURI_CONFIG.fullmatch(name):
            errors += json_violations(path, json.loads((root / path).read_text()))
        elif name in PYTHON_MANIFESTS:
            errors += python_violations(path, (root / path).read_text())
        if path in pinned:
            continue
        content = (root / path).read_bytes()
        if SPDX_MARKER in content:
            errors.append(f"{path}: contains an SPDX license identifier")
        if OCI_MARKER in content:
            errors.append(f"{path}: declares an OCI image license label")
    return errors + notice_violations(root, files, required)


def main(argv):
    root = Path(argv[1]).resolve() if len(argv) > 1 else Path(__file__).resolve().parents[1]
    errors = violations(root, tracked_files(root))
    for error in errors:
        print(error, file=sys.stderr)
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
