#!/usr/bin/env python3
"""Check the Aidash licensing cutover: scope, expressions, texts, history and artifacts.

The script is the single entry point for the checks decided in #124 decision 12.
Every subcommand accepts a repository root with `--root`, so the tests can run
the checks against fixture trees instead of the real worktree:

- `scope`       every tracked own file resolves to exactly one license.
- `expression`  own manifests, headers and image labels are exactly the license
                of their component, never a compound expression.
- `text`        the standard license texts match their SHA-256 pins, and
                `enterprise/LICENSE` carries the same terms.
- `params`      the BUSL parameters are complete and match `VERSIONS.toml`.
- `history`     recorded versions and dates only move earlier, and each Change
                Date is at most four years after first publication.
- `boundary`    the Community tree never references `enterprise/`, except for
                the documented `@slots` mechanism.
- `artifact`    a Community image holds no Enterprise source paths and ships
                the license and notice files.
- `attribution` the third-party notices and generated inventories are complete.

`reuse lint` is a separate CI step: it enforces the REUSE specification itself,
while this script applies the Aidash policy on top of the same annotations.

The fixture trees under `scripts/fixtures/licensing/` are deliberately excluded
from a real-tree run; the unit tests copy them to a temporary repository root.
"""
from __future__ import annotations

import argparse
import dataclasses
import datetime
import hashlib
import json
import re
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
FIXTURE_PREFIX = "scripts/fixtures/licensing/"

COMMUNITY_LICENSE = "Apache-2.0"
ENTERPRISE_LICENSE = "BUSL-1.1"
PROJECT_LICENSES = {COMMUNITY_LICENSE, ENTERPRISE_LICENSE}

# SHA-256 of the unmodified standard texts. All four are the SPDX
# license-list texts; the Apache-2.0 file is the license-list copy, not the
# apache.org page with its appendix. A new LICENSES/*.txt file needs a pin
# here before it can land.
LICENSE_PINS = {
    "LICENSES/Apache-2.0.txt": "074e6e32c86a4c0ef8b3ed25b721ca23aca83df277cd88106ef7177c354615ff",
    "LICENSES/BUSL-1.1.txt": "d90560217049db1d6020ec7cf482da4198bfa372dffd22270ba08fd8088cd93b",
    "LICENSES/MIT.txt": "b05785f9f18e6716bab63424b11454513b9943a222595b70411009202fc592b5",
    "LICENSES/OFL-1.1.txt": "8eea8287e5876b539670cadb82e99f9a7afddec6f6730811be1daf25d2e9bcfd",
}

# A Community image must ship these files under /app (the runtime image) or
# under the nginx document root (the frontend image). The image root is also
# accepted for images that put the application elsewhere.
ARTIFACT_FILES = (
    "LICENSE",
    "LICENSES/Apache-2.0.txt",
    "LICENSES/MIT.txt",
    "LICENSES/OFL-1.1.txt",
    "THIRD_PARTY_NOTICES.md",
)
ARTIFACT_PREFIXES = ("", "app/", "usr/share/doc/aidash/", "usr/share/nginx/html/")

# The only npm packages allowed to lack a license in package-lock.json. The
# inventory generator imports this table, and check_inventory rejects any
# exception that is not here with the same SPDX id and source URL.
NPM_LICENSE_EXCEPTIONS = {
    ("buffers", "0.1.1"): {
        "license": "MIT",
        "source": "https://github.com/substack/node-buffers",
        "reason": (
            "The upstream README states MIT/X11; the npm package.json omits "
            "the license field."
        ),
    },
}

ENTERPRISE_MARKERS = ("aidash-broker", "aidash-gcp", "aidash-enterprise-server")
ENTERPRISE_PATH = re.compile(r"(^|/)enterprise(/|$)")

# A header starts its own comment line. The tag inside a sentence is prose, and
# the licensing guard must never turn a license name in prose into a failure.
SPDX_HEADER = re.compile(
    r"(?m)^[ \t]*(?:(?://|/\*|\*|#|<!--|;|--|REM)\s*)?"
    r"SPDX-License-Identifier\s*:\s*([^\r\n]+)",
    re.IGNORECASE,
)
SPDX_COMBINATOR = re.compile(r"\b(?:OR|AND|WITH)\b")
COMMENT_CLOSERS = ("*/", "-->", "---", "#", "*")


class CheckError(Exception):
    """A setup problem that prevents a check from running."""


@dataclasses.dataclass
class Annotation:
    patterns: list[str]
    precedence: str
    license: str | None
    copyright: str | None

    def matches(self, path: str) -> bool:
        return any(glob_matches(pattern, path) for pattern in self.patterns)

    def specificity(self) -> int:
        return max(specificity(pattern) for pattern in self.patterns)


@dataclasses.dataclass
class Resolution:
    license: str | None
    source: str
    third_party: bool
    error: str | None = None


def is_enterprise(path: str) -> bool:
    return path == "enterprise" or path.startswith("enterprise/")


def is_third_party_path(path: str) -> bool:
    return path.startswith("third_party/")


def expected_license(path: str) -> str:
    return ENTERPRISE_LICENSE if is_enterprise(path) else COMMUNITY_LICENSE


def excluded(path: str) -> bool:
    return path.startswith(FIXTURE_PREFIX)


def tracked(root: Path) -> list[str]:
    """Tracked paths, excluding the check fixtures themselves."""
    result = subprocess.run(["git", "ls-files", "-z"], cwd=root, capture_output=True)
    if result.returncode != 0:
        raise CheckError(f"git ls-files failed in {root}: {result.stderr.decode().strip()}")
    paths = [path for path in result.stdout.decode("utf-8", "surrogateescape").split("\0") if path]
    return sorted(path for path in paths if not excluded(path))


def text_file(path: Path, limit: int | None = None) -> str | None:
    """Read a UTF-8 text file, returning None for binary or unreadable files."""
    try:
        with path.open("rb") as handle:
            data = handle.read() if limit is None else handle.read(limit)
    except OSError:
        return None
    if b"\0" in data:
        return None
    return data.decode("utf-8", "replace")


def glob_matches(pattern: str, path: str) -> bool:
    """Match a REUSE.toml path glob against a repository-relative path.

    `*` and `?` stay inside one path segment, `**` crosses segments. A pattern
    without a wildcard also matches everything below that directory, because
    annotations often name a directory rather than `directory/**`.
    """
    pattern = pattern.replace("\\", "/")
    while pattern.startswith("./"):
        pattern = pattern[2:]
    path = path.replace("\\", "/")
    if not pattern:
        return False
    if not any(character in pattern for character in "*?["):
        return path == pattern.rstrip("/") or path.startswith(pattern.rstrip("/") + "/")
    if pattern.endswith("/"):
        pattern = pattern.rstrip("/") + "/**"
    return re.fullmatch(glob_regex(pattern), path) is not None


def glob_regex(pattern: str) -> str:
    pieces: list[str] = []
    index = 0
    while index < len(pattern):
        character = pattern[index]
        if character == "*":
            if index + 1 < len(pattern) and pattern[index + 1] == "*":
                index += 2
                if index < len(pattern) and pattern[index] == "/":
                    pieces.append("(?:.*/)?")
                    index += 1
                else:
                    pieces.append(".*")
            else:
                pieces.append("[^/]*")
                index += 1
        elif character == "?":
            pieces.append("[^/]")
            index += 1
        elif character == "[":
            end = pattern.find("]", index + 1)
            if end == -1:
                pieces.append(re.escape(character))
                index += 1
            else:
                pieces.append(pattern[index:end + 1])
                index = end + 1
        else:
            pieces.append(re.escape(character))
            index += 1
    return "".join(pieces)


def specificity(pattern: str) -> int:
    return len(pattern.replace("*", "").replace("?", ""))


def load_reuse(root: Path) -> tuple[list[Annotation], list[str]]:
    path = root / "REUSE.toml"
    if not path.is_file():
        return [], []
    try:
        data = tomllib.loads(path.read_text())
    except (OSError, tomllib.TOMLDecodeError) as error:
        return [], [f"REUSE.toml: {error}"]
    errors: list[str] = []
    annotations: list[Annotation] = []
    for index, entry in enumerate(data.get("annotations", [])):
        patterns = entry.get("path", [])
        if isinstance(patterns, str):
            patterns = [patterns]
        patterns = [str(pattern) for pattern in patterns if isinstance(pattern, str)]
        if not patterns:
            errors.append(f"REUSE.toml annotation {index}: path is missing")
            continue
        precedence = str(entry.get("precedence", "closest"))
        if precedence not in {"closest", "aggregate", "override"}:
            errors.append(
                f"REUSE.toml annotation {index}: unknown precedence {precedence!r}; "
                "use closest, aggregate or override"
            )
            continue
        license = entry.get("SPDX-License-Identifier")
        annotations.append(Annotation(
            patterns=patterns,
            precedence=precedence,
            license=str(license).strip() if license else None,
            copyright=str(entry["SPDX-FileCopyrightText"]).strip()
            if entry.get("SPDX-FileCopyrightText") else None,
        ))
    return annotations, errors


def spdx_headers(path: Path) -> list[str]:
    """SPDX-License-Identifier values in the first 8 KiB of a file."""
    text = text_file(path, 8192)
    if text is None:
        return []
    values: list[str] = []
    for match in SPDX_HEADER.finditer(text):
        value = match.group(1).strip()
        for closer in COMMENT_CLOSERS:
            if value.endswith(closer):
                value = value[: -len(closer)].strip()
        if value and value not in values:
            values.append(value)
    return values


def resolve_annotations(path: str, annotations: list[Annotation]) -> Resolution:
    matches = [annotation for annotation in annotations if annotation.matches(path)]
    aggregates = [annotation for annotation in matches
                  if annotation.precedence == "aggregate" and annotation.license]
    # `override` and `closest` are both path annotations. The most specific
    # matching pattern wins, so a specific `closest` BUSL annotation can carve
    # a README out of a broader `override` vendor directory.
    candidates = [annotation for annotation in matches
                  if annotation.precedence in {"override", "closest"} and annotation.license]
    third_party = is_third_party_path(path)
    if candidates:
        best = max(annotation.specificity() for annotation in candidates)
        chosen = [annotation for annotation in candidates
                  if annotation.specificity() == best]
        licenses = {annotation.license for annotation in chosen}
        if len(licenses) != 1:
            return Resolution(None, "annotation", third_party,
                              f"{path}: annotations at the same specificity disagree: "
                              f"{sorted(licenses)!r}")
        chosen.extend(aggregates)
        licenses = {annotation.license for annotation in chosen}
        if len(licenses) > 1:
            return Resolution(None, "annotation", third_party,
                              f"{path}: several annotations resolve to {sorted(licenses)!r}")
        overridden = any(annotation.precedence == "override" for annotation in chosen)
        return Resolution(
            licenses.pop(),
            "override" if overridden else "annotation",
            third_party or overridden,
        )
    return Resolution(expected_license(path), "default", third_party)


def resolve(path: str, annotations: list[Annotation], header: str | None) -> Resolution:
    result = resolve_annotations(path, annotations)
    if header:
        if result.error:
            return result
        return Resolution(header, "header", is_third_party_path(path) or result.third_party)
    return result


def check_scope(root: Path) -> list[str]:
    annotations, errors = load_reuse(root)
    files = tracked(root)
    for annotation in annotations:
        for pattern in annotation.patterns:
            if not any(glob_matches(pattern, path) for path in files):
                errors.append(f"REUSE.toml: annotation path {pattern!r} matches no tracked file")
    for path in files:
        absolute = root / path
        if not absolute.is_file():
            errors.append(f"{path}: tracked file is missing")
            continue
        headers = spdx_headers(absolute)
        if len(headers) > 1:
            errors.append(f"{path}: several SPDX license headers: {headers!r}")
            continue
        resolution = resolve(path, annotations, headers[0] if headers else None)
        if resolution.error:
            errors.append(resolution.error)
            continue
        if resolution.license is None:
            errors.append(f"{path}: no license")
            continue
        if headers:
            # A header wins over a blanket `**` default, but it must agree with
            # a specific annotation or an override; otherwise the file has two
            # different answers.
            annotation_result = resolve(path, annotations, None)
            specific = any(
                annotation.matches(path)
                and annotation.license
                and (annotation.precedence == "override" or annotation.specificity() > 0)
                for annotation in annotations
            )
            if annotation_result.error:
                errors.append(annotation_result.error)
                continue
            if specific and annotation_result.license \
                    and annotation_result.license != headers[0]:
                errors.append(
                    f"{path}: SPDX header {headers[0]!r} disagrees with REUSE "
                    f"annotation {annotation_result.license!r}"
                )
                continue
        if is_third_party_path(path):
            # `third_party/` material may be annotated with a specific path and
            # any precedence. A blanket `**` annotation is not enough: it would
            # silently apply the Community license to an MIT or OFL file.
            annotated = any(
                annotation.matches(path)
                and annotation.license
                and (annotation.precedence == "override" or annotation.specificity() > 0)
                for annotation in annotations
            )
            if resolution.source == "default" or not (annotated or resolution.source == "header"):
                errors.append(
                    f"{path}: third-party file needs a specific REUSE annotation or an SPDX header"
                )
            continue
        if is_enterprise(path) and Path(path).name.startswith("LICENSE") \
                and path != "enterprise/LICENSE":
            # Every Enterprise LICENSE other than the Licensed Work's own file
            # belongs to vendored material and needs an explicit override; the
            # `enterprise/**` default would otherwise label it BUSL-1.1.
            if not resolution.third_party:
                errors.append(
                    f"{path}: a third-party Enterprise LICENSE needs a "
                    "'precedence = \"override\"' annotation"
                )
            continue
        if resolution.third_party:
            # An override annotation is how Enterprise vendors a third-party
            # font or vendor file without changing the Licensed Work's license.
            continue
        expected = expected_license(path)
        if resolution.license != expected:
            errors.append(
                f"{path}: license {resolution.license!r} from {resolution.source}, "
                f"expected {expected}"
            )
        elif SPDX_COMBINATOR.search(resolution.license):
            errors.append(f"{path}: compound license expression {resolution.license!r}")
    return errors


def check_expression(root: Path) -> list[str]:
    annotations, errors = load_reuse(root)
    files = tracked(root)
    for path in files:
        if is_third_party_path(path):
            continue
        absolute = root / path
        if path.endswith("Cargo.toml"):
            errors.extend(manifest_expression(root, path, absolute))
        elif path.endswith("package.json"):
            errors.extend(npm_expression(path, absolute))
        elif path.endswith("tauri.conf.json"):
            errors.extend(tauri_expression(path, absolute))
        elif Path(path).name.startswith("Dockerfile") or path.endswith(".Dockerfile"):
            errors.extend(dockerfile_expression(path, absolute))
        headers = spdx_headers(absolute)
        if not headers:
            continue
        resolution = resolve(path, annotations, None)
        if resolution.third_party:
            continue
        if len(headers) > 1:
            errors.append(f"{path}: several SPDX license headers: {headers!r}")
            continue
        header = headers[0]
        expected = expected_license(path)
        if SPDX_COMBINATOR.search(header):
            errors.append(f"{path}: compound SPDX license expression {header!r}")
        elif header != expected:
            errors.append(f"{path}: SPDX header {header!r}, expected {expected}")
    return errors


def inherited_workspace_license(root: Path, manifest: Path) -> str | None:
    """The nearest enclosing `[workspace.package] license` for a member."""
    directory = manifest.parent
    while True:
        candidate = directory / "Cargo.toml"
        if candidate.is_file() and candidate != manifest:
            try:
                data = tomllib.loads(candidate.read_text())
            except (OSError, tomllib.TOMLDecodeError):
                data = {}
            if "workspace" in data:
                return data.get("workspace", {}).get("package", {}).get("license")
        if directory == root or directory.parent == directory:
            return None
        directory = directory.parent


def manifest_expression(root: Path, path: str, absolute: Path) -> list[str]:
    try:
        data = tomllib.loads(absolute.read_text())
    except (OSError, tomllib.TOMLDecodeError) as error:
        return [f"{path}: unreadable manifest ({error})"]
    errors: list[str] = []
    expected = expected_license(path)
    package = data.get("package", {})
    workspace = data.get("workspace", {}).get("package", {})
    for table, name in ((package, "package"), (workspace, "workspace.package")):
        if "license-file" in table:
            errors.append(f"{path}: [{name}] uses license-file; use license = {expected!r}")
    if "license-file" in data.get("workspace", {}):
        errors.append(f"{path}: [workspace] uses license-file; use a license expression")
    license = package.get("license")
    own_workspace_license = workspace.get("license")
    effective = None
    if isinstance(license, dict):
        if license.get("workspace") is True:
            effective = own_workspace_license
            if effective is None:
                effective = inherited_workspace_license(root, absolute)
            if effective is None:
                errors.append(
                    f"{path}: license.workspace = true but no enclosing "
                    "[workspace.package] declares a license"
                )
        else:
            errors.append(f"{path}: [package] license is not a string")
    elif isinstance(license, str):
        effective = license
    elif license is not None:
        errors.append(f"{path}: [package] license is not a string")
    if package and effective is None:
        errors.append(f"{path}: [package] license is missing")
    if not package and own_workspace_license is None:
        errors.append(f"{path}: [workspace.package] license is missing")
    candidates = [effective]
    if own_workspace_license is not None:
        candidates.append(own_workspace_license)
    for value in candidates:
        if value is None:
            continue
        if value != expected:
            errors.append(f"{path}: license {value!r}, expected {expected}")
        if SPDX_COMBINATOR.search(value):
            errors.append(f"{path}: compound license expression {value!r}")
    return errors


def npm_expression(path: str, absolute: Path) -> list[str]:
    try:
        data = json.loads(absolute.read_text())
    except (OSError, ValueError) as error:
        return [f"{path}: unreadable package.json ({error})"]
    expected = expected_license(path)
    license = data.get("license")
    if not isinstance(license, str) or not license:
        return [f"{path}: license is missing or not a string"]
    errors = []
    if license != expected:
        errors.append(f"{path}: license {license!r}, expected {expected}")
    if SPDX_COMBINATOR.search(license):
        errors.append(f"{path}: compound license expression {license!r}")
    return errors


def tauri_expression(path: str, absolute: Path) -> list[str]:
    try:
        data = json.loads(absolute.read_text())
    except (OSError, ValueError) as error:
        return [f"{path}: unreadable tauri.conf.json ({error})"]
    bundle = data.get("bundle", {})
    if not isinstance(bundle, dict):
        return [f"{path}: bundle is not an object"]
    expected = expected_license(path)
    license = bundle.get("license")
    if license is None:
        if bundle.get("active"):
            return [f"{path}: bundle.license is missing, expected {expected}"]
        return []
    if license != expected:
        return [f"{path}: bundle.license {license!r}, expected {expected}"]
    if SPDX_COMBINATOR.search(license):
        return [f"{path}: compound bundle.license {license!r}"]
    return []


def dockerfile_lines(text: str) -> list[str]:
    """Join Dockerfile continuation lines and drop comments."""
    logical: list[str] = []
    pending = ""
    for raw in text.splitlines():
        line = raw.rstrip()
        if not pending and line.lstrip().startswith("#"):
            continue
        if line.endswith("\\"):
            pending += line[:-1] + " "
            continue
        logical.append((pending + line).strip())
        pending = ""
    if pending:
        logical.append(pending.strip())
    return logical


def dockerfile_expression(path: str, absolute: Path) -> list[str]:
    text = text_file(absolute)
    if text is None:
        return [f"{path}: unreadable Dockerfile"]
    expected = expected_license(path)
    labels: list[str] = []
    for line in dockerfile_lines(text):
        if not re.match(r"(?i)LABEL\b", line):
            continue
        for match in re.finditer(
            r"""org\.opencontainers\.image\.licenses\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s]+))""",
            line,
        ):
            labels.append(next(group for group in match.groups() if group is not None))
    errors: list[str] = []
    if not labels and path in {"Dockerfile", "enterprise/Dockerfile"}:
        errors.append(f"{path}: org.opencontainers.image.licenses label is missing")
    for label in labels:
        if label != expected:
            errors.append(f"{path}: OCI licenses label {label!r}, expected {expected}")
        if SPDX_COMBINATOR.search(label):
            errors.append(f"{path}: compound OCI licenses label {label!r}")
    return errors


def check_text(root: Path) -> list[str]:
    errors: list[str] = []
    licenses = root / "LICENSES"
    if not licenses.is_dir():
        return ["LICENSES/: directory is missing"]
    present = sorted(path.name for path in licenses.glob("*.txt"))
    for name in sorted(LICENSE_PINS):
        if not (root / name).is_file():
            errors.append(f"{name}: standard text is missing")
    for name in present:
        relative = f"LICENSES/{name}"
        if relative not in LICENSE_PINS:
            errors.append(f"{relative}: no SHA-256 pin in check-licensing.py")
            continue
        digest = hashlib.sha256((root / relative).read_bytes()).hexdigest()
        if digest != LICENSE_PINS[relative]:
            errors.append(
                f"{relative}: SHA-256 {digest} does not match the standard text pin"
            )
    enterprise = root / "enterprise/LICENSE"
    standard = root / "LICENSES/BUSL-1.1.txt"
    if not enterprise.is_file():
        errors.append("enterprise/LICENSE: file is missing")
    elif standard.is_file():
        enterprise_text = text_file(enterprise)
        standard_text = text_file(standard)
        if enterprise_text is None or standard_text is None:
            errors.append("enterprise/LICENSE: cannot be read as text")
        else:
            if not license_terms(enterprise_text):
                errors.append("enterprise/LICENSE: no 'Terms' section")
            if not license_terms(standard_text):
                errors.append("LICENSES/BUSL-1.1.txt: no 'Terms' section")
            if license_terms(enterprise_text) != license_terms(standard_text):
                errors.append(
                    "enterprise/LICENSE: terms differ from LICENSES/BUSL-1.1.txt "
                    "after whitespace normalisation"
                )
            for notice in (
                "License text copyright",
                '"Business Source License" is a trademark',
            ):
                if notice not in enterprise_text:
                    errors.append(f"enterprise/LICENSE: missing the standard notice {notice!r}")
    if enterprise.is_file():
        parameters = parse_parameters(text_file(enterprise) or "")
        if not parameters:
            errors.append("enterprise/LICENSE: Parameters block is missing")
    return errors


def license_terms(text: str) -> str:
    """Whitespace-normalised BUSL terms body, with any Parameters block removed."""
    text = re.sub(r"\r\n?", "\n", text)
    match = re.search(r"^\s*Terms\s*$", text, re.MULTILINE)
    if not match:
        return ""
    return " ".join(text[match.end():].split())


def parse_parameters(text: str) -> dict[str, str]:
    """Parse the BUSL Parameters block into a key -> value mapping."""
    text = re.sub(r"\r\n?", "\n", text)
    lines = text.splitlines()
    start = next((index for index, line in enumerate(lines)
                  if line.strip() == "Parameters"), None)
    if start is None:
        return {}
    keys = ("Licensor", "Licensed Work", "Additional Use Grant", "Change Date", "Change License")
    values: dict[str, list[str]] = {}
    current: str | None = None
    for line in lines[start + 1:]:
        stripped = line.strip()
        if not stripped:
            continue
        if stripped in {"Terms", "Business Source License 1.1", "Notice"}:
            break
        matched = next((key for key in keys if stripped.startswith(key + ":")), None)
        if matched:
            current = matched
            values[current] = [stripped.split(":", 1)[1].strip()]
        elif current and line[:1].isspace():
            # Wrapped values are indented. An unindented line is a new
            # section, such as the alternative-licensing paragraph.
            values[current].append(stripped)
        else:
            break
    return {key: " ".join(part for part in parts if part) for key, parts in values.items()}


def check_params(root: Path) -> list[str]:
    errors: list[str] = []
    license_path = root / "enterprise/LICENSE"
    if not license_path.is_file():
        return ["enterprise/LICENSE: file is missing"]
    parameters = parse_parameters(text_file(license_path) or "")
    if not parameters:
        return ["enterprise/LICENSE: Parameters block is missing"]
    for key in ("Licensor", "Licensed Work", "Additional Use Grant", "Change Date", "Change License"):
        if not parameters.get(key):
            errors.append(f"enterprise/LICENSE: parameter {key!r} is empty or missing")
    if parameters.get("Additional Use Grant") != "None":
        errors.append(
            "enterprise/LICENSE: Additional Use Grant must be 'None', found "
            f"{parameters.get('Additional Use Grant')!r}"
        )
    change_date = parameters.get("Change Date")
    if change_date:
        parsed = parse_date(change_date)
        if parsed is None:
            errors.append(
                f"enterprise/LICENSE: Change Date {change_date!r} is not an ISO date"
            )
    if parameters.get("Change License") != "Apache License, Version 2.0":
        errors.append(
            "enterprise/LICENSE: Change License must be 'Apache License, Version 2.0', found "
            f"{parameters.get('Change License')!r}"
        )
    versions_path = root / "enterprise/VERSIONS.toml"
    if not versions_path.is_file():
        errors.append("enterprise/VERSIONS.toml: file is missing")
        return errors
    versions, version_errors = load_versions(versions_path)
    errors.extend(version_errors)
    if not versions:
        errors.append("enterprise/VERSIONS.toml: no [[version]] entries")
        return errors
    latest = versions[-1]
    licensed_work = parameters.get("Licensed Work", "")
    match = re.search(r"\b(\d+\.\d+\.\d+)\b", licensed_work)
    if not match:
        errors.append(
            f"enterprise/LICENSE: Licensed Work {licensed_work!r} has no x.y.z version"
        )
    elif match.group(1) != latest.get("version"):
        errors.append(
            "enterprise/LICENSE: Licensed Work version "
            f"{match.group(1)!r} does not match VERSIONS.toml {latest.get('version')!r}"
        )
    recorded_change_date = value_text(latest.get("change_date"))
    if change_date and recorded_change_date != change_date:
        errors.append(
            "enterprise/LICENSE: Change Date "
            f"{change_date!r} does not match VERSIONS.toml {recorded_change_date!r}"
        )
    return errors


def value_text(value) -> str | None:
    if value is None:
        return None
    if isinstance(value, datetime.date):
        return value.isoformat()
    return str(value)


def parse_date(value: str) -> datetime.date | None:
    try:
        return datetime.date.fromisoformat(value.strip())
    except ValueError:
        return None


def add_four_years(value: datetime.date) -> datetime.date:
    try:
        return value.replace(year=value.year + 4)
    except ValueError:
        # 29 February has no counterpart in a non-leap target year.
        return datetime.date(value.year + 4, 2, 28)


def load_versions(path: Path) -> tuple[list[dict], list[str]]:
    try:
        data = tomllib.loads(path.read_text())
    except (OSError, tomllib.TOMLDecodeError) as error:
        return [], [f"{path}: unreadable VERSIONS.toml ({error})"]
    entries = data.get("version", [])
    if not isinstance(entries, list):
        return [], [f"{path}: [[version]] is not an array of tables"]
    errors: list[str] = []
    seen: set[str] = set()
    for index, entry in enumerate(entries):
        if not isinstance(entry, dict):
            errors.append(f"{path}: version entry {index} is not a table")
            continue
        version = entry.get("version")
        if not isinstance(version, str) or not version:
            errors.append(f"{path}: version entry {index} has no version")
            continue
        if version in seen:
            errors.append(f"{path}: version {version!r} is recorded twice")
        seen.add(version)
        for field in ("change_date", "first_public_date"):
            if field in entry and parse_date(str(entry[field])) is None:
                errors.append(f"{path}: version {version!r} has an invalid {field}")
    return entries, errors


def git_show(root: Path, ref: str, path: str) -> str | None:
    result = subprocess.run(
        ["git", "show", f"{ref}:{path}"], cwd=root, capture_output=True, text=True
    )
    return result.stdout if result.returncode == 0 else None


def licensed_work_version(text: str) -> str | None:
    """Return the x.y.z version named by the Licensed Work parameter, if any."""
    match = re.search(r"\b(\d+\.\d+\.\d+)\b", parse_parameters(text).get("Licensed Work", ""))
    return match.group(1) if match else None


def git_version_publication(root: Path, version: str) -> str | None:
    """Return the first commit whose enterprise/LICENSE names `version` as the Licensed Work.

    Each Enterprise version is first published by the commit that puts its
    version into the Licensed Work parameter: for the first version that is the
    commit that added enterprise/LICENSE, and for a later version it is the
    commit that bumped the Licensed Work to that version.
    """
    result = subprocess.run(
        ["git", "log", "--format=%H", "--reverse", "--", "enterprise/LICENSE"],
        cwd=root, capture_output=True, text=True,
    )
    if result.returncode != 0:
        return None
    for commit in (line.strip() for line in result.stdout.splitlines()):
        if not commit:
            continue
        text = git_show(root, commit, "enterprise/LICENSE")
        if text is not None and licensed_work_version(text) == version:
            return commit
    return None


def git_committer_date(root: Path, commit: str) -> datetime.date | None:
    result = subprocess.run(
        ["git", "show", "-s", "--format=%ct", commit],
        cwd=root, capture_output=True, text=True,
    )
    if result.returncode != 0:
        return None
    try:
        return datetime.datetime.fromtimestamp(int(result.stdout.strip()), datetime.UTC).date()
    except (ValueError, OSError):
        return None


def check_history(root: Path, base: str) -> list[str]:
    errors: list[str] = []
    working_path = root / "enterprise/VERSIONS.toml"
    if not working_path.is_file():
        return ["enterprise/VERSIONS.toml: file is missing"]
    current, current_errors = load_versions(working_path)
    errors.extend(current_errors)
    base_text = git_show(root, base, "enterprise/VERSIONS.toml")
    base_entries: list[dict] | None = None
    if base_text is None:
        print(f"note: {base} has no enterprise/VERSIONS.toml; treating the base as empty")
    else:
        try:
            base_data = tomllib.loads(base_text)
            base_entries = base_data.get("version", [])
        except tomllib.TOMLDecodeError as error:
            errors.append(f"{base}:enterprise/VERSIONS.toml is unreadable ({error})")
    current_by_version = {entry.get("version"): entry for entry in current
                          if isinstance(entry, dict) and entry.get("version")}
    if base_entries:
        for entry in base_entries:
            if not isinstance(entry, dict) or not entry.get("version"):
                continue
            version = entry["version"]
            replacement = current_by_version.get(version)
            if replacement is None:
                errors.append(f"version {version!r} disappeared from VERSIONS.toml")
                continue
            for field in ("change_date", "first_public_date"):
                if field not in entry:
                    continue
                if field not in replacement:
                    errors.append(f"version {version!r}: {field} was removed")
                    continue
                before = parse_date(str(entry[field]))
                after = parse_date(str(replacement[field]))
                if before and after and after > before:
                    errors.append(
                        f"version {version!r}: {field} moved later, {before} -> {after}"
                    )
    for entry in current:
        if not isinstance(entry, dict) or not entry.get("version"):
            continue
        version = entry["version"]
        first = parse_date(str(entry.get("first_public_date"))) \
            if entry.get("first_public_date") else None
        change = parse_date(str(entry.get("change_date"))) \
            if entry.get("change_date") else None
        if first and change:
            limit = add_four_years(first)
            if change > limit:
                errors.append(
                    f"version {version!r}: Change Date {change} is later than "
                    f"{first} + 4 years ({limit})"
                )
        commit = entry.get("first_public_commit")
        if commit:
            derived = git_version_publication(root, version)
            if derived is None:
                errors.append(
                    f"version {version!r}: first_public_commit is recorded but git has "
                    f"no commit whose enterprise/LICENSE names Licensed Work {version}"
                )
                continue
            if commit != derived:
                errors.append(
                    f"version {version!r}: first_public_commit {commit!r} does not match "
                    f"git {derived!r}"
                )
            if first:
                committer = git_committer_date(root, derived)
                if committer is None:
                    errors.append(
                        f"version {version!r}: cannot read the committer date of {derived}"
                    )
                elif first != committer:
                    errors.append(
                        f"version {version!r}: first_public_date {first} does not match "
                        f"the committer date {committer}"
                    )
    return errors


def check_boundary(root: Path, boundary_script: Path | None = None) -> list[str]:
    errors: list[str] = []
    script = boundary_script or Path(__file__).with_name("check-workspace-boundaries.py")
    manifest = root / "Cargo.toml"
    if manifest.is_file():
        metadata = subprocess.run(
            ["cargo", "metadata", "--locked", "--format-version", "1"],
            cwd=root, capture_output=True,
        )
        if metadata.returncode != 0:
            detail = metadata.stderr.decode(errors="replace").strip()
            errors.append(f"cargo metadata failed: {detail}" if detail else "cargo metadata failed")
        elif not script.is_file():
            errors.append(f"{script}: workspace boundary check is missing")
        else:
            checked = subprocess.run(
                [sys.executable, str(script), "--workspace", "community"],
                cwd=root, input=metadata.stdout, capture_output=True,
            )
            if checked.returncode != 0:
                output = (checked.stderr + checked.stdout).decode(errors="replace").strip()
                errors.extend(line for line in output.splitlines() if line.strip())
    else:
        print("note: no Cargo.toml; skipping the cargo-metadata workspace boundary check")
    files = tracked(root)
    for path in files:
        if is_enterprise(path):
            continue
        absolute = root / path
        name = Path(path).name
        if name == "package.json":
            errors.extend(package_json_enterprise(path, absolute))
        elif name == "Cargo.toml" or name.startswith("Dockerfile") or name.endswith(".Dockerfile"):
            text = text_file(absolute)
            if text and re.search(r"(^|[^A-Za-z0-9_-])enterprise/", text):
                errors.append(f"{path}: Community input names enterprise/")
        elif name.startswith("vite.config.") or name.startswith("vite.config"):
            errors.extend(vite_enterprise(path, absolute))
        elif name.startswith("tsconfig") and name.endswith(".json"):
            errors.extend(tsconfig_enterprise(path, absolute))
    return list(dict.fromkeys(errors))


def walk_json_strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, list):
        for item in value:
            yield from walk_json_strings(item)
    elif isinstance(value, dict):
        for item in value.values():
            yield from walk_json_strings(item)


def package_json_enterprise(path: str, absolute: Path) -> list[str]:
    try:
        data = json.loads(absolute.read_text())
    except (OSError, ValueError):
        return [f"{path}: unreadable package.json"]
    return [
        f"{path}: Community package.json names enterprise/: {value!r}"
        for value in walk_json_strings(data)
        if "enterprise/" in value
    ]


def vite_enterprise(path: str, absolute: Path) -> list[str]:
    text = text_file(absolute)
    if not text:
        return []
    errors: list[str] = []
    alias = re.compile(
        r"""["']?(@?[A-Za-z0-9_@/.-]+)["']?\s*:\s*[`"']([^`"']*enterprise/[^`"']*)[`"']"""
    )
    for key, value in alias.findall(text):
        if key == "@slots" or key.startswith("@slots/"):
            continue
        errors.append(f"{path}: Community alias {key!r} points at {value!r} (enterprise/)")
    for line in text.splitlines():
        if "enterprise/" in line and "@slots" not in line:
            errors.append(f"{path}: Community input names enterprise/: {line.strip()}")
    return list(dict.fromkeys(errors))


def tsconfig_enterprise(path: str, absolute: Path) -> list[str]:
    text = text_file(absolute)
    if not text:
        return []
    try:
        data = json.loads(text)
    except ValueError:
        data = None
    errors: list[str] = []
    if isinstance(data, dict):
        paths = data.get("compilerOptions", {}).get("paths", {})
        if isinstance(paths, dict):
            for key, values in paths.items():
                if not isinstance(values, list):
                    continue
                for value in values:
                    if isinstance(value, str) and "enterprise/" in value:
                        if key == "@slots" or key.startswith("@slots/"):
                            continue
                        errors.append(
                            f"{path}: Community tsconfig path {key!r} points at "
                            f"{value!r} (enterprise/)"
                        )
    if not isinstance(data, dict):
        for line in text.splitlines():
            if "enterprise/" in line and "@slots" not in line:
                errors.append(f"{path}: Community input names enterprise/: {line.strip()}")
    return list(dict.fromkeys(errors))


def normalize_member(name: str) -> str:
    name = name.replace("\\", "/")
    while name.startswith("./"):
        name = name[2:]
    return name.lstrip("/")


def artifact_paths(root: Path) -> list[str]:
    return sorted(path.relative_to(root).as_posix() for path in root.rglob("*"))


def image_paths(image: str) -> list[str]:
    if shutil.which("docker") is None:
        raise CheckError("docker is not installed; the artifact --image check needs it")
    created = subprocess.run(["docker", "create", image], capture_output=True, text=True)
    if created.returncode != 0:
        raise CheckError(f"docker create {image} failed: {created.stderr.strip()}")
    container = created.stdout.strip()
    try:
        with tempfile.NamedTemporaryFile(suffix=".tar") as archive:
            exported = subprocess.run(
                ["docker", "export", container], stdout=archive, stderr=subprocess.PIPE
            )
            if exported.returncode != 0:
                raise CheckError(
                    f"docker export {container} failed: "
                    f"{exported.stderr.decode(errors='replace').strip()}"
                )
            archive.flush()
            with tarfile.open(archive.name) as tar:
                return [normalize_member(member.name) for member in tar.getmembers()]
    finally:
        subprocess.run(["docker", "rm", "-f", container], capture_output=True)


def check_artifact(paths: list[str]) -> list[str]:
    errors: list[str] = []
    for path in paths:
        if ENTERPRISE_PATH.search(path):
            errors.append(f"{path}: Enterprise path in a Community artifact")
            continue
        for marker in ENTERPRISE_MARKERS:
            if marker in path:
                errors.append(f"{path}: Enterprise crate {marker!r} in a Community artifact")
                break
    present = set(paths)
    for name in ARTIFACT_FILES:
        if not any(prefix + name in present for prefix in ARTIFACT_PREFIXES):
            locations = ", ".join(f"/{prefix}{name}" for prefix in ARTIFACT_PREFIXES)
            errors.append(f"artifact: {name} is missing (expected at {locations})")
    return errors


def check_attribution(root: Path, inventory: Path | None) -> list[str]:
    errors: list[str] = []
    notices = root / "THIRD_PARTY_NOTICES.md"
    if not notices.is_file():
        return ["THIRD_PARTY_NOTICES.md: file is missing"]
    text = notices.read_text()
    files = tracked(root)
    tracked_set = set(files)
    listed = {
        path for path in listed_notice_paths(text)
        # A bare name in a bullet is usually prose, such as `tokenizer.json`;
        # a repository path has a slash or is an actual tracked file.
        if "/" in path or path.rstrip("/") in tracked_set
    }
    for path in sorted(listed):
        if path.rstrip("/") in tracked_set:
            continue
        prefix = path.rstrip("/") + "/"
        if any(candidate.startswith(prefix) for candidate in tracked_set):
            continue
        errors.append(f"THIRD_PARTY_NOTICES.md: listed path {path!r} is not tracked")
    for path in files:
        if re.fullmatch(r"third_party/[^/]+/LICENSE", path) and path not in text:
            errors.append(
                f"THIRD_PARTY_NOTICES.md: {path} is not referenced"
            )
    if inventory is not None:
        errors.extend(check_inventory(inventory))
    return errors


def listed_notice_paths(text: str) -> set[str]:
    """Paths named by the notice's attribution bullets and relative links.

    Prose in the "How the files were traced" sections names historical paths
    that no longer exist; only bullets and links make a current attribution
    claim, so only those are checked.
    """
    paths: set[str] = set()
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith(("- Source:", "- Revision:")):
            continue
        for target in re.findall(r"\]\(([^)]+)\)", stripped):
            if not target.startswith(("http://", "https://", "#")):
                paths.add(target.split("#", 1)[0])
        if stripped.startswith("- "):
            for candidate in re.findall(r"`([^`]+)`", stripped):
                if "://" in candidate or " " in candidate:
                    continue
                if "/" in candidate or re.search(r"\.[A-Za-z0-9]+$", candidate):
                    paths.add(candidate)
    return paths


def check_inventory(directory: Path) -> list[str]:
    if not directory.is_dir():
        return [f"{directory}: inventory directory is missing"]
    files = sorted(directory.glob("inventory-*.json"))
    if not files:
        return [f"{directory}: no inventory-*.json files"]
    errors: list[str] = []
    for path in files:
        try:
            data = json.loads(path.read_text())
        except (OSError, ValueError) as error:
            errors.append(f"{path}: unreadable inventory ({error})")
            continue
        packages = data.get("packages", data) if isinstance(data, dict) else data
        if not isinstance(packages, list):
            errors.append(f"{path}: packages is not a list")
            continue
        listed = reviewed_exceptions(path, data, errors)
        for package in packages:
            if not isinstance(package, dict):
                errors.append(f"{path}: package entry is not an object")
                continue
            name = str(package.get("name", "<unknown>"))
            version = str(package.get("version", ""))
            license = package.get("license")
            if package.get("license_source") == "exception":
                check_package_exception(path, name, version, license, package, listed, errors)
                continue
            if not isinstance(license, str) or not license.strip():
                errors.append(f"{path}: package {name!r} has no license")
            elif license.strip().upper() in {"UNLICENSED", "UNKNOWN", "NONE"}:
                errors.append(f"{path}: package {name!r} has license {license!r}")
    return errors


def reviewed_exceptions(path: Path, data, errors: list[str]) -> dict[tuple[str, str], dict]:
    """Validate the inventory's reviewed exception list and return it by key."""
    if not isinstance(data, dict):
        return {}
    entries = data.get("license_exceptions", [])
    if not isinstance(entries, list):
        errors.append(f"{path}: license_exceptions is not a list")
        return {}
    listed: dict[tuple[str, str], dict] = {}
    for entry in entries:
        if not isinstance(entry, dict):
            errors.append(f"{path}: license exception entry is not an object")
            continue
        name = str(entry.get("package", ""))
        version = str(entry.get("version", ""))
        expected = NPM_LICENSE_EXCEPTIONS.get((name, version))
        if expected is None:
            errors.append(
                f"{path}: license exception {name}@{version} is not in the reviewed list"
            )
            continue
        if entry.get("license") != expected["license"]:
            errors.append(
                f"{path}: license exception {name}@{version} has license "
                f"{entry.get('license')!r}, expected {expected['license']!r}"
            )
        if entry.get("source") != expected["source"]:
            errors.append(
                f"{path}: license exception {name}@{version} has source "
                f"{entry.get('source')!r}, expected {expected['source']!r}"
            )
        if not isinstance(entry.get("reason"), str) or not entry["reason"].strip():
            errors.append(f"{path}: license exception {name}@{version} has no reason")
        listed[(name, version)] = entry
    return listed


def check_package_exception(
    path: Path,
    name: str,
    version: str,
    license: str | None,
    package: dict,
    listed: dict[tuple[str, str], dict],
    errors: list[str],
) -> None:
    expected = NPM_LICENSE_EXCEPTIONS.get((name, version))
    if expected is None:
        errors.append(
            f"{path}: package {name}@{version} uses an exception that is not in the reviewed list"
        )
        return
    if (name, version) not in listed:
        errors.append(
            f"{path}: package {name}@{version} exception is missing from license_exceptions"
        )
    metadata = package.get("license_exception")
    if not isinstance(metadata, dict):
        errors.append(f"{path}: package {name}@{version} exception has no metadata")
        return
    if license != expected["license"]:
        errors.append(
            f"{path}: package {name}@{version} license {license!r}, "
            f"expected {expected['license']!r}"
        )
    if metadata.get("license") != expected["license"]:
        errors.append(
            f"{path}: package {name}@{version} exception license "
            f"{metadata.get('license')!r}, expected {expected['license']!r}"
        )
    if metadata.get("source") != expected["source"]:
        errors.append(
            f"{path}: package {name}@{version} exception source "
            f"{metadata.get('source')!r}, expected {expected['source']!r}"
        )
    if not isinstance(metadata.get("reason"), str) or not metadata["reason"].strip():
        errors.append(f"{path}: package {name}@{version} exception has no reason")


def main(arguments: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT, help="repository root")
    subparsers = parser.add_subparsers(dest="command", required=True)

    subparsers.add_parser("scope", help="every tracked file resolves to one license")
    subparsers.add_parser("expression", help="license expressions are exact")
    subparsers.add_parser("text", help="standard texts match their checksums")
    subparsers.add_parser("params", help="BUSL parameters are complete")
    history = subparsers.add_parser("history", help="recorded dates never move later")
    history.add_argument("--base", required=True, help="git ref to compare against")
    subparsers.add_parser("boundary", help="Community never references enterprise/")
    artifact = subparsers.add_parser("artifact", help="Community image contents")
    artifact.add_argument("--image", help="image reference to inspect with Docker")
    artifact.add_argument("--dir", type=Path, help="already-exported filesystem to inspect")
    attribution = subparsers.add_parser("attribution", help="notices and inventories")
    attribution.add_argument("--inventory", type=Path, help="directory of inventory-*.json")

    args = parser.parse_args(arguments)
    root = args.root.resolve()
    try:
        if args.command == "scope":
            errors = check_scope(root)
        elif args.command == "expression":
            errors = check_expression(root)
        elif args.command == "text":
            errors = check_text(root)
        elif args.command == "params":
            errors = check_params(root)
        elif args.command == "history":
            errors = check_history(root, args.base)
        elif args.command == "boundary":
            errors = check_boundary(root)
        elif args.command == "artifact":
            if bool(args.image) == bool(args.dir):
                parser.error("artifact needs exactly one of --image or --dir")
            if args.image:
                paths = image_paths(args.image)
            else:
                directory = args.dir if args.dir.is_absolute() else root / args.dir
                paths = artifact_paths(directory.resolve())
            errors = check_artifact(paths)
        elif args.command == "attribution":
            inventory = None
            if args.inventory:
                inventory = args.inventory if args.inventory.is_absolute() else root / args.inventory
            errors = check_attribution(root, inventory)
        else:  # pragma: no cover - argparse enforces the choice
            parser.error(f"unknown command {args.command}")
    except CheckError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1
    print(f"{args.command}: ok ({root})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
