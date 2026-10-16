"""Exercise the interim license guard against generated repositories.

Fixtures are written to temporary Git repositories rather than tracked, because
a tracked rejected fixture would fail the guard on this repository itself.
"""

import hashlib
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

## Upstream

- Source: <https://example.com/upstream>
- Revision: `0123abc`
- License: MIT, in [third_party/upstream/LICENSE](third_party/upstream/LICENSE)

- `src/adapted.rs`: adapted code (lineage)
"""
# Pinned notices are not scanned for markers, so this one may carry one.
UPSTREAM_NOTICE = f"{SPDX}: MIT\n"
# The third-party inventory the fixtures must carry.
REQUIRED = {
    "upstream": {
        "heading": "Upstream",
        "source": "https://example.com/upstream",
        "revision": "0123abc",
        "license": "MIT",
        "sha256": hashlib.sha256(UPSTREAM_NOTICE.encode()).hexdigest(),
        "adapted": ("src/adapted.rs",),
    },
}
# The smallest repository that the guard accepts.
ACCEPTED = {
    "LICENSE": (ROOT / "LICENSE").read_text(),
    "THIRD_PARTY_NOTICES.md": NOTICES,
    "third_party/upstream/LICENSE": UPSTREAM_NOTICE,
    "src/adapted.rs": "fn adapted() {}\n",
    "crates/aidash-capability/Cargo.toml": '[package]\nname = "aidash-capability"\nlicense = "Apache-2.0"\n',
    "crates/other/Cargo.toml": '[package]\nname = "other"\n',
    "Cargo.toml": '[workspace]\nmembers = ["crates/*"]\n',
    "web/package.json": '{"name": "web", "private": true}\n',
    "web/package-lock.json": '{"packages": {"node_modules/x": {"license": "MIT"}}}\n',
    "web/src/license-screen.tsx": "export {};\n",
    "desktop/src-tauri/tauri.conf.json": '{"bundle": {"active": true}}\n',
    "runner/pyproject.toml": '[project]\nname = "runner"\nclassifiers = ["Programming Language :: Python"]\n',
    "tools/setup.cfg": "[metadata]\nname = tools\n",
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
        "crates/other/LICENSE: license files belong only in the root LICENSE or a pinned third-party notice",
    ),
    "a suffixed root license file": (
        {"LICENSE-MIT": "MIT License\n"},
        "LICENSE-MIT: license files belong only in the root LICENSE or a pinned third-party notice",
    ),
    "a copying file": (
        {"docs/COPYING": "GPL\n"},
        "docs/COPYING: license files belong only in the root LICENSE or a pinned third-party notice",
    ),
    "a lowercase license document": (
        {"web/license.md": "MIT\n"},
        "web/license.md: license files belong only in the root LICENSE or a pinned third-party notice",
    ),
    "a case-variant suffixed license file": (
        {"License-MIT": "MIT License\n"},
        "License-MIT: license files belong only in the root LICENSE or a pinned third-party notice",
    ),
    "a lowercase suffixed license file": (
        {"docs/license-MIT": "MIT License\n"},
        "docs/license-MIT: license files belong only in the root LICENSE or a pinned third-party notice",
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
    "a third-party project outside the pinned inventory": (
        {"third_party/other/LICENSE": "MIT License\n"},
        "third_party/other/LICENSE: license files belong only in the root LICENSE or a pinned third-party notice",
    ),
    "a license file at the third-party root": (
        {"third_party/LICENSE": "MIT License\n"},
        "third_party/LICENSE: license files belong only in the root LICENSE or a pinned third-party notice",
    ),
    "a second license beside a pinned notice": (
        {"third_party/upstream/LICENSE-MIT": "MIT License\n"},
        "third_party/upstream/LICENSE-MIT: license files belong only in the root LICENSE or a pinned third-party notice",
    ),
    "an SPDX identifier beside a pinned notice": (
        {"third_party/upstream/NOTES.md": f"{SPDX}: MIT\n"},
        "third_party/upstream/NOTES.md: contains an SPDX license identifier",
    ),
    "a replaced third-party notice": (
        {"third_party/upstream/LICENSE": "Anything goes\n"},
        "third_party/upstream/LICENSE differs from the pinned upstream notice",
    ),
    "an unlicense file": (
        {"UNLICENSE": "This is free and unencumbered software\n"},
        "UNLICENSE: license files belong only in the root LICENSE or a pinned third-party notice",
    ),
    "a prefixed license file": (
        {"docs/MIT-LICENSE": "MIT License\n"},
        "docs/MIT-LICENSE: license files belong only in the root LICENSE or a pinned third-party notice",
    ),
    "a REUSE license directory": (
        {"LICENSES/MIT.txt": "MIT License\n"},
        "LICENSES/MIT.txt: license files belong only in the root LICENSE or a pinned third-party notice",
    ),
    "a Python project license": (
        {"runner/pyproject.toml": '[project]\nname = "runner"\nlicense = "MIT"\n'},
        "runner/pyproject.toml: [project] declares license",
    ),
    "Python project license files": (
        {"runner/pyproject.toml": '[project]\nname = "runner"\nlicense-files = ["LICEN[CS]E*"]\n'},
        "runner/pyproject.toml: [project] declares license-files",
    ),
    "a Python license classifier": (
        {"runner/pyproject.toml":
            '[project]\nname = "runner"\nclassifiers = ["License :: OSI Approved :: MIT License"]\n'},
        "runner/pyproject.toml: [project] declares a License classifier",
    ),
    "a Poetry license": (
        {"runner/pyproject.toml": '[tool.poetry]\nname = "runner"\nlicense = "MIT"\n'},
        "runner/pyproject.toml: [tool.poetry] declares license",
    ),
    "a setuptools license": (
        {"tools/setup.cfg": "[metadata]\nname = tools\nlicense_files = LICENSE\n"},
        "tools/setup.cfg: [metadata] declares license_files",
    ),
    "a third-party project without its notice": (
        {"third_party/other/README.md": "upstream\n"},
        "third_party/other/ has no LICENSE",
    ),
    "a stale adapted path": (
        {"src/adapted.rs": None},
        "THIRD_PARTY_NOTICES.md: lists src/adapted.rs, which is not a tracked file",
    ),
    "a required project removed with its notice": (
        {"third_party/upstream/LICENSE": None,
         "THIRD_PARTY_NOTICES.md": "# Third-party notices\n\n- `src/adapted.rs`: adapted code (lineage)\n"},
        "third_party/upstream/LICENSE is missing for the required project upstream",
    ),
    "an adapted file dropped from the notices": (
        {"THIRD_PARTY_NOTICES.md": NOTICES.replace("- `src/adapted.rs`: adapted code (lineage)\n", "")},
        "THIRD_PARTY_NOTICES.md does not list src/adapted.rs under Upstream, adapted from upstream",
    ),
    "an adapted file moved under another project": (
        {"THIRD_PARTY_NOTICES.md": NOTICES.replace("- `src/adapted.rs`: adapted code (lineage)\n", "")
            + "\n## Elsewhere\n\n- `src/adapted.rs`: adapted code (lineage)\n"},
        "THIRD_PARTY_NOTICES.md does not list src/adapted.rs under Upstream, adapted from upstream",
    ),
    "a missing project section": (
        {"THIRD_PARTY_NOTICES.md": NOTICES.replace("## Upstream", "## Renamed")},
        "THIRD_PARTY_NOTICES.md has no '## Upstream' section for upstream",
    ),
    "a replaced upstream source": (
        {"THIRD_PARTY_NOTICES.md": NOTICES.replace("example.com/upstream", "example.com/fork")},
        "THIRD_PARTY_NOTICES.md: the Upstream section lacks '- Source: <https://example.com/upstream>'",
    ),
    "a removed upstream revision": (
        {"THIRD_PARTY_NOTICES.md": NOTICES.replace("- Revision: `0123abc`\n", "")},
        "THIRD_PARTY_NOTICES.md: the Upstream section lacks '- Revision: `0123abc`'",
    ),
    "a changed license label": (
        {"THIRD_PARTY_NOTICES.md": NOTICES.replace("- License: MIT,", "- License: BSD-3-Clause,")},
        "THIRD_PARTY_NOTICES.md: the Upstream section lacks "
        "'- License: MIT, in [third_party/upstream/LICENSE](third_party/upstream/LICENSE)'",
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
        return LICENSES.violations(root, LICENSES.tracked_files(root), REQUIRED)


class LicenseDeclarationTests(unittest.TestCase):
    def test_this_repository_declares_no_license_beyond_the_allowlist(self):
        self.assertEqual(LICENSES.violations(ROOT, LICENSES.tracked_files(ROOT)), [])

    def test_accepted_fixture_passes(self):
        self.assertEqual(errors_for(), [])

    def test_crlf_checkout_passes(self):
        # A Windows checkout with core.autocrlf converts the notices to CRLF.
        crlf = {
            path: ACCEPTED[path].replace("\n", "\r\n")
            for path in ("LICENSE", "third_party/upstream/LICENSE")
        }
        self.assertEqual(errors_for(crlf), [])

    def test_rejected_fixtures_fail(self):
        for case, (changes, expected) in REJECTED.items():
            with self.subTest(case=case):
                self.assertIn(expected, errors_for(changes))

    def test_command_exits_nonzero_on_a_rejected_fixture(self):
        # The command checks the pinned inventory, which only this repository carries.
        with tempfile.TemporaryDirectory() as directory:
            rejected = repository(directory, {"crates/other/LICENSE": "MIT License\n"})
            for root, status in ((ROOT, 0), (rejected, 1)):
                with self.subTest(status=status):
                    result = subprocess.run(
                        [sys.executable, str(SCRIPT), str(root)], capture_output=True, text=True,
                    )
                    self.assertEqual(result.returncode, status, result.stderr)


if __name__ == "__main__":
    unittest.main()
