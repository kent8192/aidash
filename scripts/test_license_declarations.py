"""Exercise the interim license guard against generated repositories.

Fixtures are written to temporary Git repositories rather than tracked, because
a tracked rejected fixture would fail the guard on this repository itself.
"""

import importlib.util
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = Path(__file__).with_name("check-license-declarations.py")
SPEC = importlib.util.spec_from_file_location("license_declarations", SCRIPT)
LICENSES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(LICENSES)

SPDX = "SPDX-" + "License-Identifier"
OCI = "org.opencontainers.image." + "licenses"
NOTICES = """# Third-party notices

- License: MIT, in [third_party/upstream/LICENSE](third_party/upstream/LICENSE)

- `src/adapted.rs`: adapted code (lineage)
"""
# The smallest repository that the guard accepts.
ACCEPTED = {
    "LICENSE": (ROOT / "LICENSE").read_text(),
    "THIRD_PARTY_NOTICES.md": NOTICES,
    "third_party/upstream/LICENSE": f"{SPDX}: MIT\n",
    "src/adapted.rs": "fn adapted() {}\n",
    "crates/aidash-capability/Cargo.toml": '[package]\nname = "aidash-capability"\nlicense = "Apache-2.0"\n',
    "crates/other/Cargo.toml": '[package]\nname = "other"\n',
    "Cargo.toml": '[workspace]\nmembers = ["crates/*"]\n',
    "web/package.json": '{"name": "web", "private": true}\n',
    "web/package-lock.json": '{"packages": {"node_modules/x": {"license": "MIT"}}}\n',
    "web/src/license-screen.tsx": "export {};\n",
    "desktop/src-tauri/tauri.conf.json": '{"bundle": {"active": true}}\n',
    "Dockerfile": 'LABEL org.opencontainers.image.title="aidash"\n',
}
# Each rejected fixture changes the accepted repository by these files.
REJECTED = {
    "a package license": (
        {"crates/other/Cargo.toml": '[package]\nname = "other"\nlicense = "Apache-2.0"\n'},
        "crates/other/Cargo.toml: [package] declares license",
    ),
    "a package license file": (
        {"crates/other/Cargo.toml": '[package]\nname = "other"\nlicense-file = "LICENSE"\n'},
        "crates/other/Cargo.toml: [package] declares license-file",
    ),
    "an inherited license": (
        {"crates/other/Cargo.toml": '[package]\nname = "other"\nlicense.workspace = true\n'},
        "crates/other/Cargo.toml: [package] declares license",
    ),
    "a workspace license": (
        {"Cargo.toml": '[workspace]\nmembers = ["crates/*"]\n[workspace.package]\nlicense = "MIT"\n'},
        "Cargo.toml: [workspace.package] declares license",
    ),
    "a changed allowlisted expression": (
        {"crates/aidash-capability/Cargo.toml":
            '[package]\nname = "aidash-capability"\nlicense = "Apache-2.0 OR MIT"\n'},
        "crates/aidash-capability/Cargo.toml: the allowlisted license must stay 'Apache-2.0'",
    ),
    "an npm license": (
        {"web/package.json": '{"name": "web", "license": "MIT"}\n'},
        "web/package.json: declares license",
    ),
    "legacy npm licenses": (
        {"web/package.json": '{"name": "web", "licenses": [{"type": "MIT"}]}\n'},
        "web/package.json: declares licenses",
    ),
    "a Tauri bundle license": (
        {"desktop/src-tauri/tauri.conf.json": '{"bundle": {"license": "MIT"}}\n'},
        "desktop/src-tauri/tauri.conf.json: bundle declares license",
    ),
    "a Tauri bundle license file": (
        {"desktop/src-tauri/tauri.conf.json": '{"bundle": {"licenseFile": "../LICENSE"}}\n'},
        "desktop/src-tauri/tauri.conf.json: bundle declares licenseFile",
    ),
    "an SPDX identifier in own code": (
        {"src/lib.rs": f"// {SPDX}: Apache-2.0\n"},
        "src/lib.rs: contains an SPDX license identifier",
    ),
    "an OCI image license label": (
        {"Dockerfile": f'LABEL {OCI}="Apache-2.0"\n'},
        "Dockerfile: declares an OCI image license label",
    ),
    "a license file in a crate": (
        {"crates/other/LICENSE": "MIT License\n"},
        "crates/other/LICENSE: license files belong only in the root LICENSE or third_party/",
    ),
    "a suffixed root license file": (
        {"LICENSE-MIT": "MIT License\n"},
        "LICENSE-MIT: license files belong only in the root LICENSE or third_party/",
    ),
    "a copying file": (
        {"docs/COPYING": "GPL\n"},
        "docs/COPYING: license files belong only in the root LICENSE or third_party/",
    ),
    "a lowercase license document": (
        {"web/license.md": "MIT\n"},
        "web/license.md: license files belong only in the root LICENSE or third_party/",
    ),
    "a changed root notice": (
        {"LICENSE": "MIT License\n"},
        "the root LICENSE differs from the interim notice; licensing changes belong to #255",
    ),
    "a missing root notice": (
        {"LICENSE": None},
        "the root LICENSE interim notice is missing",
    ),
    "an unlisted third-party project": (
        {"third_party/other/LICENSE": "MIT License\n"},
        "THIRD_PARTY_NOTICES.md does not list third_party/other/LICENSE",
    ),
    "a third-party project without its notice": (
        {"third_party/other/README.md": "upstream\n"},
        "third_party/other/ has no LICENSE",
    ),
    "a stale adapted path": (
        {"src/adapted.rs": None},
        "THIRD_PARTY_NOTICES.md: lists src/adapted.rs, which is not a tracked file",
    ),
    "a stale notice link": (
        {"third_party/upstream/LICENSE": None, "third_party/upstream/NOTICE": "upstream\n"},
        "THIRD_PARTY_NOTICES.md: refers to third_party/upstream/LICENSE, which is not a tracked file",
    ),
}


def repository(directory, changes=None):
    files = {**ACCEPTED, **(changes or {})}
    for path, content in files.items():
        if content is not None:
            target = Path(directory, path)
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(content)
    subprocess.run(["git", "init", "-q", directory], check=True)
    subprocess.run(["git", "add", "-A"], cwd=directory, check=True)
    return Path(directory)


def errors_for(changes=None):
    with tempfile.TemporaryDirectory() as directory:
        root = repository(directory, changes)
        return LICENSES.violations(root, LICENSES.tracked_files(root))


class LicenseDeclarationTests(unittest.TestCase):
    def test_this_repository_declares_no_license_beyond_the_allowlist(self):
        self.assertEqual(LICENSES.violations(ROOT, LICENSES.tracked_files(ROOT)), [])

    def test_accepted_fixture_passes(self):
        self.assertEqual(errors_for(), [])

    def test_rejected_fixtures_fail(self):
        for case, (changes, expected) in REJECTED.items():
            with self.subTest(case=case):
                self.assertIn(expected, errors_for(changes))

    def test_command_exits_nonzero_on_a_rejected_fixture(self):
        for changes, status in ((None, 0), ({"crates/other/LICENSE": "MIT License\n"}, 1)):
            with self.subTest(status=status), tempfile.TemporaryDirectory() as directory:
                root = repository(directory, changes)
                result = subprocess.run(
                    [sys.executable, str(SCRIPT), str(root)], capture_output=True, text=True,
                )
                self.assertEqual(result.returncode, status, result.stderr)


if __name__ == "__main__":
    unittest.main()
