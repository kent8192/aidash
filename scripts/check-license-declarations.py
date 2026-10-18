#!/usr/bin/env python3
"""Reject license declarations until the licensing cutover (#124).

A license grant cannot be revoked once it is pushed. Until #255 replaces this
guard with the full licensing checks, the interim notice reserves all rights
and only the allowlisted package may declare a license.
"""
import ast
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
# SHA-256 of the README's License section, from its heading to the next
# section, hashed like the notices. It is the repository's primary licensing
# text, so changing it is a licensing decision too.
README_LICENSE_SHA256 = "ec9f495abc237cd41bb94bbc6ca2da2c101d0a733a559d5e8dad382839fdc1c4"
THIRD_PARTY = "third_party"
NOTICES = "THIRD_PARTY_NOTICES.md"
# SHA-256 of the whole notices file, hashed like the root notice. Any sentence
# in it can place material under a project's terms, so changing it is a
# licensing decision. The checks below explain what a change broke.
NOTICES_SHA256 = "a086a7facfc2dfcd0a9e7e44ef6cee445100caf19d05df94040de9f73ab34fc0"
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
    # The embedded tokenizer files of the Token Encodings (#276, ADR 0017).
    "gpt-oss": {
        "heading": "gpt-oss",
        "source": "https://huggingface.co/openai/gpt-oss-120b",
        "revision": "b5c939de8f754692c1647ca79fbf85e8c1e70f8a",
        "license": "Apache-2.0",
        "sha256": "58d1e17ffe5109a7ae296caafcadfdbe6a7d176f0bc4ab01e12a689b0499d8bd",
        "adapted": (
            "crates/aidash-domain/assets/token-encodings/o200k_base.json.zst",
        ),
    },
    "gemma-4": {
        "heading": "Gemma 4",
        "source": "https://huggingface.co/google/gemma-4-E4B-it",
        "revision": "ee0ef6023621cff504d758262d4e04895a5af4a2",
        "license": "Apache-2.0",
        "sha256": "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30",
        "adapted": (
            "crates/aidash-domain/assets/token-encodings/gemma4.json.zst",
        ),
    },
}
# Assembled so that this tracked file never contains the markers it rejects.
SPDX_MARKER = ("SPDX-" + "License-Identifier").encode()
OCI_MARKER = ("org.opencontainers.image." + "licenses").encode()
# The Helm chart license annotation that Artifact Hub reads.
ARTIFACTHUB_MARKER = ("artifacthub.io/" + "license").encode()
# LICENSE-MIT, License-MIT, MIT-LICENSE, UNLICENSE, COPYING.LESSER or license.md,
# but not license-screen.tsx, LICENSE-screen.tsx or license.rs: in any case, one
# dash or underscore prefix or suffix, then only a document extension.
LICENSE_NAME = re.compile(
    r"^(?i:([^.]+[-_])?(un)?licen[cs]e|copying)([-_][^.]+)?"
    # A document extension in any case, or an uppercase qualifier such as
    # COPYING.LESSER or LICENSE.APACHE; never a source extension.
    r"(\.((?i:md|txt|rst)|[A-Z][A-Z0-9-]*))?$"
)
# A REUSE-style directory of license texts, such as LICENSES/MIT.txt.
LICENSE_DIRECTORY = "licenses"
PYTHON_MANIFESTS = ("pyproject.toml", "setup.cfg")
SETUP_LICENSE_KEYWORDS = ("license", "license_file", "license_files", "license_expression")
README = "README.md"
SECTION = re.compile(r"^## .*$", re.MULTILINE)
# Tauri bundles these keys as package-license metadata (`license-file` is an
# alias). It reads them from tauri.conf.json, tauri.conf.json5, Tauri.toml and
# their platform variants, and `tauri --config` accepts any such file, so every
# JSON, JSON5 and TOML file's top-level bundle table is checked.
CONFIG_SUFFIXES = (".json", ".json5", ".toml")
TAURI_LICENSE_KEYS = ("license", "licenseFile", "license-file")
# Such a key, quoted or bare, in a file that JSON and TOML parsers cannot read,
# such as JSON5, which Tauri also accepts under a .json name.
TAURI_LICENSE_KEY = re.compile(r"""(?<![\w$-])(["']?)(license|licenseFile|license-file)\1\s*[:=]""")
DOCUMENT_SUFFIXES = (".md", ".mdx", ".markdown", ".rst", ".txt", ".adoc")
# A license named in prose, such as the "Community, Apache-2.0" grant that the
# broker README carried. Documentation may name one only in the pinned texts:
# the README License section and each notice's pinned License line. Source
# files are not held to this, because Agent Skills carry license metadata.
PROSE_LICENSE = re.compile(
    r"\b(?:MIT|BSD|ISC|[AL]?GPL|MPL|EPL|BUSL|SSPL|CC0|CC-BY)(?:-[\w.]+)*\b"
    r"|(?i:\b(?:apache|mozilla|eclipse|elastic|business\s+source|(?:lesser\s+|affero\s+)?general\s+public"
    r"|server\s+side\s+public)(?:\s+public)?\s+licen[cs]e\b"
    r"|\bapache[-\s]2(?:\.0)?\b|\bunlicense\b)"
)
# The wording with which license headers and texts grant rights, checked in
# every text file: the Apache and MIT headers and their released or
# distributed forms, the GPL and MPL headers, the ISC, BUSL and zlib grants,
# the BSD conditions and public-domain dedications.
GRANT_WORDING = re.compile(
    r"(?i:\b(?:licen[cs]ed|released|distributed)\s+under\b|\b(?:under|subject\s+to)\s+the\s+terms\s+of\b"
    r"|\bhereby\s+grant(?:s|ed)?\b|\bpermission\s+(?:is\s+)?granted\b"
    r"|\bredistribution\s+and\s+use\s+in\s+source\b|\bpublic\s+domain\b)"
)
# Docker and shells join a line that ends in the escape character (`\`, or a
# backtick after an escape directive) with the next, skipping comment lines.
CONTINUATION = re.compile(rb"[\\`]\r?\n(?:[ \t]*#[^\n]*\n)*[ \t]*")
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


def npm_violations(path, document):
    return [f"{path}: declares {key}" for key in ("license", "licenses") if key in document]


def config_violations(path, text):
    """Tauri bundle license metadata in a JSON, JSON5 or TOML file."""
    suffix = PurePosixPath(path).suffix.lower()
    try:
        document = tomllib.loads(text) if suffix == ".toml" else json.loads(text) if suffix == ".json" else None
    except (tomllib.TOMLDecodeError, json.JSONDecodeError):
        document = None
    if document is None:
        # Never fail on an unreadable file, such as a commented tsconfig.json:
        # look for the keys themselves instead.
        return [f"{path}: declares {match[1]}" for match in TAURI_LICENSE_KEY.findall(text)]
    # Only the top-level table: package-lock.json nests dependency licenses.
    bundle = document.get("bundle") if isinstance(document, dict) else None
    if not isinstance(bundle, dict):
        return []
    return [f"{path}: bundle declares {key}" for key in TAURI_LICENSE_KEYS if key in bundle]


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


def setup_violations(path, text):
    """License metadata in setup.py, however it reaches the `setup()` call.

    License keywords are rejected in any call and as keys of any dict, so a
    `setup` imported under another name or metadata gathered beforehand is
    caught too, and so is any License classifier string in the file.
    """
    try:
        tree = ast.parse(text)
    except SyntaxError:
        return [f"{path}: cannot be parsed to check its setup() metadata"]
    # setup itself and the names it is imported under, such as
    # `from setuptools import setup as package`.
    setup_names = {"setup"} | {
        alias.asname for node in ast.walk(tree) if isinstance(node, ast.ImportFrom)
        for alias in node.names if alias.name == "setup" and alias.asname
    }
    errors = []
    for node in ast.walk(tree):
        if isinstance(node, ast.Call):
            name = node.func.attr if isinstance(node.func, ast.Attribute) else getattr(node.func, "id", "")
            for keyword in node.keywords:
                if keyword.arg in SETUP_LICENSE_KEYWORDS:
                    errors.append(f"{path}: setup() declares {keyword.arg}")
                elif keyword.arg is None and name in setup_names:
                    # setup(**metadata) can carry any of the keywords above.
                    errors.append(f"{path}: setup() expands ** metadata, which cannot be checked")
        elif isinstance(node, ast.Dict):
            errors += [
                f"{path}: setup() declares {key.value}" for key in node.keys
                if isinstance(key, ast.Constant) and key.value in SETUP_LICENSE_KEYWORDS
            ]
        elif (isinstance(node, ast.Constant) and isinstance(node.value, str)
              and node.value.strip().startswith("License ::")):
            errors.append(f"{path}: setup() declares a License classifier")
    return errors


def read_text(path):
    """A text file with CRLF read as LF, as the notices are hashed."""
    return path.read_bytes().replace(b"\r\n", b"\n").decode(errors="replace")


def readme_license_section(text):
    """The README's License section, from its heading to the next section."""
    start = re.search(r"^## License[ \t]*$", text, re.MULTILINE)
    if start is None:
        return None
    following = SECTION.search(text, start.end())
    return text[start.start():following.start() if following else len(text)].rstrip()


def readme_violations(root, tracked):
    """The README's License section must stay the interim wording."""
    if README not in tracked:
        return [f"{README} is missing"]
    section = readme_license_section(read_text(root / README))
    if section is None:
        return [f"{README} has no License section"]
    if hashlib.sha256(section.encode()).hexdigest() != README_LICENSE_SHA256:
        return [f"{README}: the License section differs from the interim wording; licensing changes belong to #255"]
    return []


def notice_prose(text, required):
    """The notices without each project's License line, exempt only once and
    only inside that project's own section."""
    pinned_lines = {expected["heading"]: license_line(project, expected) for project, expected in required.items()}
    parts = SECTION_HEADING.split(text)
    kept = [parts[0]]
    for heading, body in zip(parts[1::2], parts[2::2]):
        lines = body.split("\n")
        if pinned_lines.get(heading) in lines:
            lines.remove(pinned_lines[heading])
        kept += [heading, "\n".join(lines)]
    return "\n".join(kept)


def prose_violations(path, text, required):
    """Text may grant a license, and documentation name one, only in the pinned texts."""
    if path == README:
        text = text.replace(readme_license_section(text) or "", "")
    elif path == NOTICES:
        text = notice_prose(text, required)
    errors = [
        f"{path}: '{phrase}' grants a license; licensing changes belong to #255"
        for phrase in dict.fromkeys(match.group() for match in GRANT_WORDING.finditer(text))
    ]
    if PurePosixPath(path).suffix.lower() not in DOCUMENT_SUFFIXES:
        return errors
    return errors + [
        f"{path}: names the license '{name}'; licensing changes belong to #255"
        for name in dict.fromkeys(match.group() for match in PROSE_LICENSE.finditer(text))
    ]


def license_line(project, expected):
    """The notices line that names a required project's license."""
    notice = f"{THIRD_PARTY}/{project}/LICENSE"
    return f"- License: {expected['license']}, in [{notice}]({notice})"


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
            license_line(project, expected),
        )
        if line not in lines
    ]
    listed = LISTED_PATH.findall(section)
    errors += [
        f"{NOTICES} does not list {path} under {heading}, adapted from {project}"
        for path in expected["adapted"] if path not in listed
    ]
    # An extra path would place original Aidash material under the project's
    # license, so the section names exactly the pinned files.
    return errors + [
        f"{NOTICES} lists {path} under {heading}, which the pinned inventory does not name"
        for path in listed if path not in expected["adapted"]
    ]


def notice_violations(root, files, required, notices_sha256):
    tracked = set(files)
    if NOTICES not in tracked:
        return [f"{NOTICES} is missing"]
    text = read_text(root / NOTICES)
    errors = [] if digest(root / NOTICES) == notices_sha256 else [
        f"{NOTICES} differs from the pinned notices; licensing changes belong to #255"
    ]
    errors += [
        f"{NOTICES}: lists {path}, which is not a tracked file"
        for path in LISTED_PATH.findall(text) if path not in tracked
    ]
    # Each project's paths and provenance are read from its own section, so a
    # path moved under another project's heading no longer counts.
    parts = SECTION_HEADING.split(text)
    headings = parts[1::2]
    sections = dict(zip(headings, parts[2::2]))
    # Any other section, a second copy of a pinned one or a path listed before
    # the first section could place original material under third-party terms.
    pinned_headings = [expected["heading"] for expected in required.values()]
    errors += [
        f"{NOTICES}: '## {heading}' is not the section of a pinned project"
        for heading in headings if heading not in pinned_headings
    ]
    errors += [
        f"{NOTICES}: '## {heading}' appears more than once"
        for heading in pinned_headings if headings.count(heading) > 1
    ]
    errors += [
        f"{NOTICES}: lists {path} before the first project section"
        for path in LISTED_PATH.findall(parts[0])
    ]
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


def violations(root, files, required=REQUIRED_NOTICES, notices_sha256=NOTICES_SHA256):
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
        elif name == "package.json":
            errors += npm_violations(path, json.loads((root / path).read_text()))
        elif name in PYTHON_MANIFESTS:
            errors += python_violations(path, (root / path).read_text())
        elif name == "setup.py":
            errors += setup_violations(path, (root / path).read_text())
        suffix = PurePosixPath(name).suffix.lower()
        if suffix in CONFIG_SUFFIXES:
            errors += config_violations(path, read_text(root / path))
        # The root LICENSE and the pinned notices are checked against digests.
        if path == "LICENSE" or path in pinned:
            continue
        content = (root / path).read_bytes()
        if b"\0" not in content:
            errors += prose_violations(path, content.replace(b"\r\n", b"\n").decode(errors="replace"), required)
        content = CONTINUATION.sub(b"", content)
        if SPDX_MARKER in content:
            errors.append(f"{path}: contains an SPDX license identifier")
        if OCI_MARKER in content:
            errors.append(f"{path}: declares an OCI image license label")
        if ARTIFACTHUB_MARKER in content:
            errors.append(f"{path}: declares an Artifact Hub chart license")
    return (errors + readme_violations(root, set(files))
            + notice_violations(root, files, required, notices_sha256))


def main(argv):
    root = Path(argv[1]).resolve() if len(argv) > 1 else Path(__file__).resolve().parents[1]
    errors = violations(root, tracked_files(root))
    for error in errors:
        print(error, file=sys.stderr)
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
