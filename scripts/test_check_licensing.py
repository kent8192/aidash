"""Exercise every licensing check against accepted and rejected fixtures.

The fixture trees live under `scripts/fixtures/licensing/`. A test copies one
to a temporary directory and initialises a Git repository there, because
`scope` and `attribution` read `git ls-files` and `history` compares a working
tree with a Git ref.
"""
import contextlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("check-licensing.py")
SPEC = importlib.util.spec_from_file_location("check_licensing", SCRIPT)
LICENSING = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = LICENSING
SPEC.loader.exec_module(LICENSING)

FIXTURES = Path(__file__).with_name("fixtures") / "licensing"


def git(root: Path, *arguments: str, env: dict[str, str] | None = None) -> str:
    result = subprocess.run(
        ["git", *arguments], cwd=root, capture_output=True, text=True,
        env={**os.environ, **(env or {})},
    )
    if result.returncode != 0:
        raise AssertionError(f"git {' '.join(arguments)} failed: {result.stderr}")
    return result.stdout.strip()


def init_repository(root: Path) -> None:
    git(root, "init", "-q")
    git(root, "config", "user.email", "licensing-test@example.com")
    git(root, "config", "user.name", "Licensing Test")
    git(root, "config", "commit.gpgsign", "false")


@contextlib.contextmanager
def fixture_tree(subcommand: str, verdict: str):
    """Copy a fixture to a temporary repository root and track every file."""
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        shutil.copytree(FIXTURES / subcommand / verdict, root, dirs_exist_ok=True)
        init_repository(root)
        git(root, "add", "-A")
        yield root


@contextlib.contextmanager
def bare_fixture(subcommand: str, verdict: str):
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        shutil.copytree(FIXTURES / subcommand / verdict, root, dirs_exist_ok=True)
        yield root


def commit(root: Path, message: str, date: str, paths: list[str]) -> str:
    git(root, "add", *paths)
    git(
        root, "commit", "-q", "-m", message,
        env={"GIT_AUTHOR_DATE": f"{date}T12:00:00+00:00",
             "GIT_COMMITTER_DATE": f"{date}T12:00:00+00:00"},
    )
    return git(root, "rev-parse", "HEAD")


class ScopeTests(unittest.TestCase):
    def test_accepts_component_and_third_party_licenses(self):
        with fixture_tree("scope", "accepted") as root:
            self.assertEqual(LICENSING.check_scope(root), [])

    def test_rejects_an_enterprise_file_under_the_community_license(self):
        with fixture_tree("scope", "rejected") as root:
            errors = LICENSING.check_scope(root)
        self.assertTrue(any("enterprise/code.rs" in error for error in errors), errors)

    def test_rejects_an_own_file_with_a_conflicting_header(self):
        with fixture_tree("scope", "rejected") as root:
            errors = LICENSING.check_scope(root)
        self.assertTrue(any("wrong-header.rs" in error for error in errors), errors)

    def test_rejects_a_header_that_disagrees_with_a_specific_annotation(self):
        with fixture_tree("scope", "accepted") as root:
            (root / "src" / "annotated.rs").write_text(
                "// SPDX-License-" + "Identifier: Apache-2.0\npub fn annotated() {}\n"
            )
            reuse = root / "REUSE.toml"
            reuse.write_text(
                reuse.read_text()
                + '\n[[annotations]]\npath = "src/annotated.rs"\n'
                  'precedence = "closest"\nSPDX-License-Identifier = "MIT"\n'
            )
            git(root, "add", "-A")
            errors = LICENSING.check_scope(root)
        self.assertTrue(any("disagrees with REUSE annotation" in error for error in errors), errors)

    def test_rejects_a_third_party_file_without_an_override(self):
        with fixture_tree("scope", "rejected") as root:
            errors = LICENSING.check_scope(root)
        self.assertTrue(any("third_party/upstream/LICENSE" in error for error in errors), errors)

    def test_rejects_a_vendored_enterprise_license_without_an_override(self):
        with fixture_tree("scope", "rejected") as root:
            errors = LICENSING.check_scope(root)
        self.assertTrue(any("enterprise/vendor/LICENSE" in error for error in errors), errors)

    def test_ignores_a_license_name_in_prose(self):
        with fixture_tree("scope", "accepted") as root:
            (root / "docs").mkdir()
            (root / "docs" / "example.md").write_text(
                "Every file carries an SPDX-" + "License-Identifier: MIT header.\n"
            )
            git(root, "add", "-A")
            self.assertEqual(LICENSING.check_scope(root), [])

    def test_a_specific_closest_annotation_carves_out_a_broader_override(self):
        with fixture_tree("scope", "accepted") as root:
            (root / "enterprise" / "vendor").mkdir()
            (root / "enterprise" / "vendor" / "README.md").write_text("own material\n")
            (root / "enterprise" / "vendor" / "code.js").write_text("third-party code\n")
            reuse = root / "REUSE.toml"
            reuse.write_text(
                reuse.read_text()
                + '\n[[annotations]]\npath = "enterprise/vendor/**"\n'
                  'precedence = "override"\nSPDX-License-Identifier = "Apache-2.0"\n'
                  '\n[[annotations]]\npath = "enterprise/vendor/README.md"\n'
                  'precedence = "closest"\nSPDX-License-Identifier = "BUSL-1.1"\n'
            )
            git(root, "add", "-A")
            self.assertEqual(LICENSING.check_scope(root), [])

    def test_rejects_an_annotation_that_matches_nothing(self):
        with fixture_tree("scope", "accepted") as root:
            reuse = root / "REUSE.toml"
            reuse.write_text(
                reuse.read_text()
                + '\n[[annotations]]\npath = "missing/**"\n'
                  'SPDX-License-Identifier = "Apache-2.0"\n'
            )
            git(root, "add", "REUSE.toml")
            errors = LICENSING.check_scope(root)
        self.assertTrue(any("matches no tracked file" in error for error in errors), errors)


class ExpressionTests(unittest.TestCase):
    def test_accepts_exact_expressions_and_inheritance(self):
        with fixture_tree("expression", "accepted") as root:
            self.assertEqual(LICENSING.check_expression(root), [])

    def test_rejects_compound_and_mismatched_expressions(self):
        with fixture_tree("expression", "rejected") as root:
            errors = LICENSING.check_expression(root)
        joined = "\n".join(errors)
        for expected in (
            "compound license expression",
            "license-file",
            "web/package.json",
            "tauri.conf.json",
            "BUSL-1.1",
            "SPDX header",
        ):
            self.assertIn(expected, joined)

    def test_rejects_a_license_file_field(self):
        with fixture_tree("expression", "accepted") as root:
            manifest = root / "app" / "Cargo.toml"
            manifest.write_text(manifest.read_text() + 'license-file = "LICENSE"\n')
            git(root, "add", "app/Cargo.toml")
            errors = LICENSING.check_expression(root)
        self.assertTrue(any("license-file" in error for error in errors), errors)


class TextTests(unittest.TestCase):
    def test_accepts_pinned_texts_and_matching_terms(self):
        with bare_fixture("text", "accepted") as root:
            self.assertEqual(LICENSING.check_text(root), [])

    def test_rejects_a_modified_standard_text(self):
        with bare_fixture("text", "rejected") as root:
            errors = LICENSING.check_text(root)
        joined = "\n".join(errors)
        self.assertIn("SHA-256", joined)
        self.assertIn("enterprise/LICENSE", joined)

    def test_rejects_an_unpinned_license_text(self):
        with bare_fixture("text", "accepted") as root:
            (root / "LICENSES" / "MPL-2.0.txt").write_text("MPL\n")
            errors = LICENSING.check_text(root)
        self.assertTrue(any("no SHA-256 pin" in error for error in errors), errors)

    def test_rejects_enterprise_terms_that_were_edited(self):
        with bare_fixture("text", "accepted") as root:
            license_path = root / "enterprise" / "LICENSE"
            license_path.write_text(
                license_path.read_text().replace(
                    "The Licensor hereby grants you the right",
                    "The Licensor does not grant you the right",
                )
            )
            errors = LICENSING.check_text(root)
        self.assertTrue(any("terms differ" in error for error in errors), errors)


class ParamsTests(unittest.TestCase):
    def test_accepts_the_fixed_parameters(self):
        with bare_fixture("params", "accepted") as root:
            self.assertEqual(LICENSING.check_params(root), [])

    def test_rejects_wrong_parameters_and_a_mismatched_date(self):
        with bare_fixture("params", "rejected") as root:
            errors = LICENSING.check_params(root)
        joined = "\n".join(errors)
        self.assertIn("Additional Use Grant", joined)
        self.assertIn("Change License", joined)
        self.assertIn("Change Date", joined)

    def test_rejects_a_missing_versions_file(self):
        with bare_fixture("params", "accepted") as root:
            (root / "enterprise" / "VERSIONS.toml").unlink()
            errors = LICENSING.check_params(root)
        self.assertTrue(any("VERSIONS.toml" in error for error in errors), errors)


class HistoryTests(unittest.TestCase):
    def make_license_repository(self) -> tuple[Path, str, tempfile.TemporaryDirectory]:
        directory = tempfile.TemporaryDirectory()
        root = Path(directory.name)
        (root / "enterprise").mkdir()
        (root / "enterprise" / "LICENSE").write_text("Business Source License 1.1\n")
        init_repository(root)
        commit_hash = commit(root, "add the Enterprise license", "2026-10-26",
                             ["enterprise/LICENSE"])
        return root, commit_hash, directory

    def write_versions(self, root: Path, verdict: str, commit_hash: str) -> None:
        text = (FIXTURES / "history" / verdict / "enterprise" / "VERSIONS.toml").read_text()
        text = text.replace("REPLACE_COMMIT", commit_hash).replace("REPLACE_DATE", "2026-10-26")
        (root / "enterprise" / "VERSIONS.toml").write_text(text)

    def test_accepts_the_four_year_boundary_and_the_derived_commit(self):
        root, commit_hash, directory = self.make_license_repository()
        try:
            self.write_versions(root, "accepted", commit_hash)
            self.assertEqual(LICENSING.check_history(root, commit_hash), [])
        finally:
            directory.cleanup()

    def test_rejects_one_day_after_the_four_year_boundary(self):
        root, commit_hash, directory = self.make_license_repository()
        try:
            self.write_versions(root, "rejected", commit_hash)
            errors = LICENSING.check_history(root, commit_hash)
        finally:
            directory.cleanup()
        self.assertTrue(any("later than" in error for error in errors), errors)

    def test_accepts_an_earlier_explicit_change_date(self):
        root, commit_hash, directory = self.make_license_repository()
        try:
            (root / "enterprise" / "VERSIONS.toml").write_text(
                '[[version]]\nversion = "0.1.0"\nchange_date = "2028-01-01"\n'
                'first_public_date = "2026-10-26"\n'
            )
            self.assertEqual(LICENSING.check_history(root, commit_hash), [])
        finally:
            directory.cleanup()

    def test_rejects_a_change_date_that_moves_later(self):
        root, commit_hash, directory = self.make_license_repository()
        try:
            base = (
                '[[version]]\nversion = "0.1.0"\nchange_date = "2029-01-01"\n'
                'first_public_date = "2026-10-26"\n'
            )
            (root / "enterprise" / "VERSIONS.toml").write_text(base)
            base_commit = commit(root, "record the version", "2026-10-27",
                                 ["enterprise/VERSIONS.toml"])
            (root / "enterprise" / "VERSIONS.toml").write_text(
                base.replace("2029-01-01", "2029-06-01")
            )
            errors = LICENSING.check_history(root, base_commit)
        finally:
            directory.cleanup()
        self.assertTrue(any("moved later" in error for error in errors), errors)

    def test_rejects_a_disappeared_version(self):
        root, commit_hash, directory = self.make_license_repository()
        try:
            base = (
                '[[version]]\nversion = "0.1.0"\nchange_date = "2030-10-26"\n'
                'first_public_date = "2026-10-26"\n'
            )
            (root / "enterprise" / "VERSIONS.toml").write_text(base)
            base_commit = commit(root, "record 0.1.0", "2026-10-27",
                                 ["enterprise/VERSIONS.toml"])
            (root / "enterprise" / "VERSIONS.toml").write_text(
                '[[version]]\nversion = "0.2.0"\nchange_date = "2031-01-01"\n'
                'first_public_date = "2026-11-01"\n'
            )
            errors = LICENSING.check_history(root, base_commit)
        finally:
            directory.cleanup()
        self.assertTrue(any("disappeared" in error for error in errors), errors)

    def test_rejects_a_first_public_commit_that_git_does_not_derive(self):
        root, commit_hash, directory = self.make_license_repository()
        try:
            (root / "enterprise" / "VERSIONS.toml").write_text(
                '[[version]]\nversion = "0.1.0"\nchange_date = "2030-10-26"\n'
                'first_public_commit = "deadbeef"\nfirst_public_date = "2026-10-26"\n'
            )
            errors = LICENSING.check_history(root, commit_hash)
        finally:
            directory.cleanup()
        self.assertTrue(any("does not match" in error for error in errors), errors)

    def test_rejects_a_first_public_date_that_differs_from_the_commit(self):
        root, commit_hash, directory = self.make_license_repository()
        try:
            (root / "enterprise" / "VERSIONS.toml").write_text(
                '[[version]]\nversion = "0.1.0"\nchange_date = "2030-10-26"\n'
                f'first_public_commit = "{commit_hash}"\n'
                'first_public_date = "2026-10-27"\n'
            )
            errors = LICENSING.check_history(root, commit_hash)
        finally:
            directory.cleanup()
        self.assertTrue(any("committer date" in error for error in errors), errors)


class BoundaryTests(unittest.TestCase):
    def test_accepts_the_documented_slots_alias(self):
        with fixture_tree("boundary", "accepted") as root:
            self.assertEqual(LICENSING.check_boundary(root), [])

    def test_rejects_package_vite_tsconfig_and_dockerfile_references(self):
        with fixture_tree("boundary", "rejected") as root:
            errors = LICENSING.check_boundary(root)
        joined = "\n".join(errors)
        for expected in ("package.json", "vite.config.ts", "tsconfig.json", "Dockerfile"):
            self.assertIn(expected, joined)

    @unittest.skipUnless(shutil.which("cargo"), "cargo is required")
    def test_delegates_to_the_existing_workspace_check(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "src").mkdir()
            (root / "Cargo.toml").write_text(
                '[package]\nname = "fixture"\nversion = "0.1.0"\nedition = "2024"\n'
            )
            (root / "src" / "lib.rs").write_text("pub fn fixture() {}\n")
            init_repository(root)
            git(root, "add", "-A")
            subprocess.run(
                ["cargo", "generate-lockfile", "--manifest-path", "Cargo.toml"],
                cwd=root, check=True, capture_output=True,
            )
            git(root, "add", "-A")
            fake = root / "fake-boundary.py"
            fake.write_text('import sys\nprint("fake boundary failure", file=sys.stderr)\nsys.exit(1)\n')
            errors = LICENSING.check_boundary(root, fake)
        self.assertIn("fake boundary failure", errors)


class ArtifactTests(unittest.TestCase):
    def test_accepts_a_community_filesystem(self):
        with bare_fixture("artifact", "accepted") as root:
            errors = LICENSING.check_artifact(LICENSING.artifact_paths(root))
        self.assertEqual(errors, [])

    def test_accepts_the_documented_doc_root(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "usr" / "share" / "doc" / "aidash"
            (root / "LICENSES").mkdir(parents=True)
            (root / "LICENSE").write_text("Apache-2.0\n")
            (root / "LICENSES" / "Apache-2.0.txt").write_text("Apache-2.0\n")
            (root / "LICENSES" / "MIT.txt").write_text("MIT\n")
            (root / "LICENSES" / "OFL-1.1.txt").write_text("OFL\n")
            (root / "THIRD_PARTY_NOTICES.md").write_text("# notices\n")
            errors = LICENSING.check_artifact(LICENSING.artifact_paths(Path(directory)))
        self.assertEqual(errors, [])

    def test_accepts_the_nginx_document_root(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "usr" / "share" / "nginx" / "html"
            (root / "LICENSES").mkdir(parents=True)
            (root / "LICENSE").write_text("Apache-2.0\n")
            (root / "LICENSES" / "Apache-2.0.txt").write_text("Apache-2.0\n")
            (root / "LICENSES" / "MIT.txt").write_text("MIT\n")
            (root / "LICENSES" / "OFL-1.1.txt").write_text("OFL\n")
            (root / "THIRD_PARTY_NOTICES.md").write_text("# notices\n")
            errors = LICENSING.check_artifact(LICENSING.artifact_paths(Path(directory)))
        self.assertEqual(errors, [])

    def test_rejects_enterprise_paths_and_missing_notices(self):
        with bare_fixture("artifact", "rejected") as root:
            errors = LICENSING.check_artifact(LICENSING.artifact_paths(root))
        joined = "\n".join(errors)
        self.assertIn("enterprise", joined)
        self.assertIn("aidash-broker", joined)
        self.assertIn("THIRD_PARTY_NOTICES.md", joined)

    def test_command_exit_codes(self):
        with bare_fixture("artifact", "accepted") as root:
            accepted = subprocess.run(
                [sys.executable, str(SCRIPT), "--root", str(root), "artifact", "--dir", str(root)],
                capture_output=True,
            ).returncode
        with bare_fixture("artifact", "rejected") as root:
            rejected = subprocess.run(
                [sys.executable, str(SCRIPT), "--root", str(root), "artifact", "--dir", str(root)],
                capture_output=True,
            ).returncode
        self.assertEqual((accepted, rejected), (0, 1))


class AttributionTests(unittest.TestCase):
    def test_accepts_tracked_paths_and_licensed_inventories(self):
        with fixture_tree("attribution", "accepted") as root:
            errors = LICENSING.check_attribution(root, root / "inventory")
        self.assertEqual(errors, [])

    def test_rejects_an_untracked_path_and_an_unreferenced_notice(self):
        with fixture_tree("attribution", "rejected") as root:
            errors = LICENSING.check_attribution(root, root / "inventory")
        joined = "\n".join(errors)
        self.assertIn("src/missing.rs", joined)
        self.assertIn("third_party/other/LICENSE", joined)

    def test_rejects_an_inventory_package_without_a_license(self):
        with fixture_tree("attribution", "rejected") as root:
            errors = LICENSING.check_attribution(root, root / "inventory")
        joined = "\n".join(errors)
        self.assertIn("has no license", joined)

    def test_rejects_an_unreviewed_npm_license_exception(self):
        with fixture_tree("attribution", "rejected") as root:
            errors = LICENSING.check_attribution(root, root / "inventory")
        self.assertTrue(any("not in the reviewed list" in error for error in errors), errors)

    def test_accepts_a_reviewed_npm_license_exception(self):
        exception = {
            "license": "MIT",
            "source": "https://github.com/substack/node-buffers",
            "reason": (
                "The upstream README states MIT/X11; the npm package.json "
                "omits the license field."
            ),
        }
        inventory = {
            "workspace": "web",
            "ecosystem": "npm",
            "license_exceptions": [
                {"package": "buffers", "version": "0.1.1", **exception},
            ],
            "packages": [
                {
                    "name": "buffers",
                    "version": "0.1.1",
                    "license": "MIT",
                    "license_source": "exception",
                    "license_exception": exception,
                },
            ],
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "inventory-npm-exception.json"
            path.write_text(json.dumps(inventory))
            errors = LICENSING.check_inventory(Path(directory))
        self.assertEqual(errors, [])

    def test_rejects_an_exception_with_a_changed_source(self):
        exception = {
            "license": "MIT",
            "source": "https://example.com/not-the-upstream",
            "reason": "A local claim.",
        }
        inventory = {
            "workspace": "web",
            "ecosystem": "npm",
            "license_exceptions": [
                {"package": "buffers", "version": "0.1.1", **exception},
            ],
            "packages": [
                {
                    "name": "buffers",
                    "version": "0.1.1",
                    "license": "MIT",
                    "license_source": "exception",
                    "license_exception": exception,
                },
            ],
        }
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "inventory-npm-exception.json"
            path.write_text(json.dumps(inventory))
            errors = LICENSING.check_inventory(Path(directory))
        self.assertTrue(any("expected" in error for error in errors), errors)

    def test_requires_an_inventory_when_the_option_is_given(self):
        with fixture_tree("attribution", "accepted") as root:
            errors = LICENSING.check_attribution(root, root / "missing")
        self.assertTrue(any("inventory directory is missing" in error for error in errors), errors)


class HelpersTests(unittest.TestCase):
    def test_reuse_globs_stay_inside_one_segment(self):
        self.assertTrue(LICENSING.glob_matches("enterprise/**", "enterprise/a/b.rs"))
        self.assertTrue(LICENSING.glob_matches("**/*.rs", "enterprise/a/b.rs"))
        self.assertFalse(LICENSING.glob_matches("*.txt", "LICENSES/Apache-2.0.txt"))
        self.assertTrue(LICENSING.glob_matches("LICENSES/*.txt", "LICENSES/MIT.txt"))
        self.assertTrue(LICENSING.glob_matches("enterprise/auth/fonts", "enterprise/auth/fonts/a.woff2"))

    def test_parameters_ignore_wrapped_values_and_stop_at_the_next_section(self):
        text = (
            "Parameters\n\nLicensor:             kent8192\n"
            "Licensed Work:        Aidash Enterprise 0.1.0\n"
            "                      original material\n"
            "Additional Use Grant: None\nChange Date:          2030-10-26\n"
            "Change License:       Apache License, Version 2.0\n\n"
            "For information, see COMMERCIAL-LICENSING.md.\n\n"
            "Terms\n"
        )
        self.assertEqual(
            LICENSING.parse_parameters(text),
            {
                "Licensor": "kent8192",
                "Licensed Work": "Aidash Enterprise 0.1.0 original material",
                "Additional Use Grant": "None",
                "Change Date": "2030-10-26",
                "Change License": "Apache License, Version 2.0",
            },
        )

    def test_four_year_boundary(self):
        first = LICENSING.parse_date("2026-10-26")
        self.assertEqual(LICENSING.add_four_years(first), LICENSING.parse_date("2030-10-26"))
        self.assertLess(LICENSING.parse_date("2030-10-26"),
                        LICENSING.add_four_years(first) + LICENSING.datetime.timedelta(days=1))


if __name__ == "__main__":
    unittest.main()
