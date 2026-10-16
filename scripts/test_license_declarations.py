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
GRANTED = "licensed " + "under"
RELEASED = "Released " + "under"
DISTRIBUTED = "distributed " + "under"
PERMISSION = "Permission is " + "granted"
ARTIFACTHUB = "artifacthub.io/" + "license"
NOTICES = """# Third-party notices

## Upstream

- Source: <https://example.com/upstream>
- Revision: `0123abc`
- License: MIT, in [third_party/upstream/LICENSE](third_party/upstream/LICENSE)

- `src/adapted.rs`: adapted code (lineage)
"""
# Pinned notices are not scanned for markers, so this one may carry one.
UPSTREAM_NOTICE = f"{SPDX}: MIT\n"
# The pinned digest of the fixture notices, hashed like the guard hashes them.
NOTICES_SHA256 = hashlib.sha256(NOTICES.encode()).hexdigest()
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
README_TEXT = (ROOT / "README.md").read_text()
# The interim License section, as this repository's README carries it.
README_LICENSE = README_TEXT[README_TEXT.index("## License"):]
# The smallest repository that the guard accepts.
ACCEPTED = {
    "LICENSE": (ROOT / "LICENSE").read_text(),
    "README.md": "# Fixture\n\nIntroduction.\n\n" + README_LICENSE,
    "THIRD_PARTY_NOTICES.md": NOTICES,
    "third_party/upstream/LICENSE": UPSTREAM_NOTICE,
    "src/adapted.rs": "fn adapted() {}\n",
    "crates/aidash-capability/Cargo.toml": '[package]\nname = "aidash-capability"\nlicense = "Apache-2.0"\n',
    "crates/other/Cargo.toml": '[package]\nname = "other"\n',
    "Cargo.toml": '[workspace]\nmembers = ["crates/*"]\n',
    "web/package.json": '{"name": "web", "private": true}\n',
    "web/package-lock.json": '{"packages": {"node_modules/x": {"license": "MIT"}}}\n',
    "web/src/license-screen.tsx": "export {};\n",
    "web/src/LICENSE-screen.tsx": "export {};\n",
    "tools/COPYING-tools.py": "pass\n",
    "infra/setup.py": (
        "def setup(name):\n    return name\n\noptions = dict({'enabled': False}, **{'name': 'tool'})\n"
        "setup(name='tool')\n"
    ),
    # Source may name a license as data, as Agent Skill metadata does.
    "web/tests/skill.spec.ts": 'const skill = "---\\nname: research\\nlicense: MIT\\n---\\n";\n',
    "desktop/src-tauri/tauri.conf.json": '{"bundle": {"active": true}}\n',
    # Neither parses as JSON or TOML; the guard reads them without failing.
    "web/tsconfig.json": '{\n  // comments make this JSONC\n  "compilerOptions": {"strict": true}\n}\n',
    "tests/template.toml": "name = {{ name }}\n",
    "crates/other/README.md": "# Other\n\nA crate whose documentation names no license.\n",
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
    "a Tauri bundle license file alias": (
        {"desktop/src-tauri/tauri.conf.json": '{"bundle": {"license-file": "../LICENSE"}}\n'},
        "desktop/src-tauri/tauri.conf.json: bundle declares license-file",
    ),
    "a Tauri TOML bundle license": (
        {"desktop/src-tauri/Tauri.toml": '[bundle]\nlicense-file = "../LICENSE"\n'},
        "desktop/src-tauri/Tauri.toml: bundle declares license-file",
    ),
    "a Tauri JSON5 bundle license": (
        {"desktop/src-tauri/tauri.conf.json5": "{\n  // JSON5\n  bundle: { license: 'MIT' },\n}\n"},
        "desktop/src-tauri/tauri.conf.json5: declares license",
    ),
    "JSON5 under a Tauri .json name": (
        {"desktop/src-tauri/tauri.conf.json": "{\n  // JSON5\n  bundle: { licenseFile: '../LICENSE' },\n}\n"},
        "desktop/src-tauri/tauri.conf.json: declares licenseFile",
    ),
    "a bundle license in a file for tauri --config": (
        {"desktop/src-tauri/release.json": '{"bundle": {"license": "MIT"}}\n'},
        "desktop/src-tauri/release.json: bundle declares license",
    ),
    "an SPDX identifier in own code": (
        {"src/lib.rs": f"// {SPDX}: Apache-2.0\n"},
        "src/lib.rs: contains an SPDX license identifier",
    ),
    "an OCI image license label": (
        {"Dockerfile": f'LABEL {OCI}="Apache-2.0"\n'},
        "Dockerfile: declares an OCI image license label",
    ),
    "an OCI image license label split across lines": (
        {"Dockerfile": "LABEL " + OCI.replace(".licenses", ".\\\n# a comment\nlicenses") + '="MIT"\n'},
        "Dockerfile: declares an OCI image license label",
    ),
    "an Artifact Hub chart license": (
        {"deploy/helm/x/Chart.yaml": f"apiVersion: v2\nname: x\nannotations:\n  {ARTIFACTHUB}: MIT\n"},
        "deploy/helm/x/Chart.yaml: declares an Artifact Hub chart license",
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
    "a setup.py license": (
        {"infra/setup.py": "from setuptools import setup\nsetup(name='tool', license='MIT')\n"},
        "infra/setup.py: setup() declares license",
    ),
    "a setup.py License classifier": (
        {"infra/setup.py":
            "import setuptools\nsetuptools.setup(name='tool', classifiers=['License :: OSI Approved :: MIT License'])\n"},
        "infra/setup.py: setup() declares a License classifier",
    ),
    "setup.py metadata expanded into setup()": (
        {"infra/setup.py":
            "from setuptools import setup\nmetadata = {'name': 'tool', 'license': 'MIT'}\nsetup(**metadata)\n"},
        "infra/setup.py: setup() expands ** metadata, which cannot be checked",
    ),
    "an aliased setup() license": (
        {"infra/setup.py": "from setuptools import setup as package\npackage(name='tool', license='MIT')\n"},
        "infra/setup.py: setup() declares license",
    ),
    "metadata expanded into an aliased setup()": (
        {"infra/setup.py": "import json\nfrom setuptools import setup as package\npackage(**json.load(open('m.json')))\n"},
        "infra/setup.py: setup() expands ** metadata, which cannot be checked",
    ),
    "a License classifier kept in a variable": (
        {"infra/setup.py":
            "from setuptools import setup\nCLASSIFIERS = ['License :: OSI Approved :: MIT License']\n"
            "setup(name='tool', classifiers=CLASSIFIERS)\n"},
        "infra/setup.py: setup() declares a License classifier",
    ),
    "a license grant in a component README": (
        {"crates/other/README.md": "# Other\n\nCommunity, Apache-2.0. A crate.\n"},
        "crates/other/README.md: names the license 'Apache-2.0'; licensing changes belong to #255",
    ),
    "a license grant in a source header": (
        {"src/lib.rs": f"// This file is {GRANTED} the MIT License.\n"},
        f"src/lib.rs: '{GRANTED}' grants a license; licensing changes belong to #255",
    ),
    "a released-under source header": (
        {"src/lib.rs": f"// {RELEASED} the MIT License.\n"},
        f"src/lib.rs: '{RELEASED}' grants a license; licensing changes belong to #255",
    ),
    "a distributed-under source header": (
        {"src/main.ts": f"// This file is {DISTRIBUTED} the MIT License.\n"},
        f"src/main.ts: '{DISTRIBUTED}' grants a license; licensing changes belong to #255",
    ),
    "a zlib-style permission grant": (
        {"src/zlib.c": f"/* {PERMISSION} to anyone to use this software. */\n"},
        f"src/zlib.c: '{PERMISSION}' grants a license; licensing changes belong to #255",
    ),
    "a license grant outside the README License section": (
        {"README.md": f"# Fixture\n\nAidash is {GRANTED} the MIT License.\n\n" + README_LICENSE},
        "README.md: names the license 'MIT'; licensing changes belong to #255",
    ),
    "a README license grant": (
        {"README.md": f"# Fixture\n\n## License\n\nAidash is {GRANTED} the MIT License.\n"},
        "README.md: the License section differs from the interim wording; licensing changes belong to #255",
    ),
    "an uppercase license qualifier": (
        {"docs/LICENSE.APACHE": "Apache License\n"},
        "docs/LICENSE.APACHE: license files belong only in the root LICENSE or a pinned third-party notice",
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
    "an original file listed as adapted": (
        {"THIRD_PARTY_NOTICES.md": NOTICES + "- `src/original.rs`: not adapted at all\n",
         "src/original.rs": "fn original() {}\n"},
        "THIRD_PARTY_NOTICES.md lists src/original.rs under Upstream, which the pinned inventory does not name",
    ),
    "an extra notices section": (
        {"THIRD_PARTY_NOTICES.md": NOTICES + "\n## Elsewhere\n\n- `src/original.rs`: original code\n",
         "src/original.rs": "fn original() {}\n"},
        "THIRD_PARTY_NOTICES.md: '## Elsewhere' is not the section of a pinned project",
    ),
    "a duplicate notices section": (
        {"THIRD_PARTY_NOTICES.md": NOTICES.replace(
            "## Upstream", "## Upstream\n\n- `src/original.rs`: original code\n\n## Upstream"),
         "src/original.rs": "fn original() {}\n"},
        "THIRD_PARTY_NOTICES.md: '## Upstream' appears more than once",
    ),
    "a path listed before the first notices section": (
        {"THIRD_PARTY_NOTICES.md": NOTICES.replace(
            "# Third-party notices\n", "# Third-party notices\n\n- `src/original.rs`: original code\n"),
         "src/original.rs": "fn original() {}\n"},
        "THIRD_PARTY_NOTICES.md: lists src/original.rs before the first project section",
    ),
    "a license named outside a pinned License line": (
        {"THIRD_PARTY_NOTICES.md": NOTICES + "\nThe rest of `src/` is MIT as well.\n"},
        "THIRD_PARTY_NOTICES.md: names the license 'MIT'; licensing changes belong to #255",
    ),
    "a pinned License line copied before the first section": (
        {"THIRD_PARTY_NOTICES.md": NOTICES.replace(
            "# Third-party notices\n",
            "# Third-party notices\n\nOriginal Aidash code is covered by:\n\n"
            "- License: MIT, in [third_party/upstream/LICENSE](third_party/upstream/LICENSE)\n")},
        "THIRD_PARTY_NOTICES.md: names the license 'MIT'; licensing changes belong to #255",
    ),
    "a file claimed in a project section's prose": (
        {"THIRD_PARTY_NOTICES.md": NOTICES.replace(
            "- `src/adapted.rs`: adapted code (lineage)\n",
            "- `src/adapted.rs`: adapted code (lineage)\n\nThis license also applies to src/original.rs.\n"),
         "src/original.rs": "fn original() {}\n"},
        "THIRD_PARTY_NOTICES.md differs from the pinned notices; licensing changes belong to #255",
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
        return LICENSES.violations(root, LICENSES.tracked_files(root), REQUIRED, NOTICES_SHA256)


class LicenseDeclarationTests(unittest.TestCase):
    def test_this_repository_declares_no_license_beyond_the_allowlist(self):
        self.assertEqual(LICENSES.violations(ROOT, LICENSES.tracked_files(ROOT)), [])

    def test_accepted_fixture_passes(self):
        self.assertEqual(errors_for(), [])

    def test_crlf_checkout_passes(self):
        # A Windows checkout with core.autocrlf converts the notices to CRLF.
        crlf = {
            path: ACCEPTED[path].replace("\n", "\r\n")
            for path in ("LICENSE", "README.md", "THIRD_PARTY_NOTICES.md", "third_party/upstream/LICENSE")
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
