"""Exercise the Enterprise alignment check against this repository and copies of it."""

import importlib.util
from pathlib import Path
import shutil
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "enterprise_alignment", Path(__file__).with_name("check-enterprise-alignment.py")
)
ALIGNMENT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ALIGNMENT)
COPIED = ("Cargo.toml", "Cargo.lock", *ALIGNMENT.SHARED_FILES)


class EnterpriseAlignmentTests(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.community = Path(directory.name)
        self.enterprise = self.community / "enterprise"
        self.enterprise.mkdir()
        for name in COPIED:
            shutil.copy(ROOT / name, self.community / name)
            shutil.copy(ROOT / "enterprise" / name, self.enterprise / name)
        # Workspace paths resolve against each copy, as they do in the repository.
        (self.community / "crates").symlink_to(ROOT / "crates")
        (self.community / "server").symlink_to(ROOT / "server")

    def edit(self, path, old, new):
        text = path.read_text()
        self.assertIn(old, text)
        path.write_text(text.replace(old, new, 1))

    def test_this_repository_is_aligned(self):
        self.assertEqual(ALIGNMENT.misalignments(ROOT, ROOT / "enterprise"), [])

    def test_a_copy_of_this_repository_is_aligned(self):
        self.assertEqual(ALIGNMENT.misalignments(self.community, self.enterprise), [])

    def test_a_different_locked_version_names_the_fix(self):
        self.edit(self.enterprise / "Cargo.lock", 'name = "serde"\nversion = "', 'name = "serde"\nversion = "0.0.0-')
        errors = ALIGNMENT.misalignments(self.community, self.enterprise)
        self.assertEqual(len(errors), 1, errors)
        self.assertIn("serde (registry+https://github.com/rust-lang/crates.io-index) is locked at 0.0.0-", errors[0])
        self.assertIn("cargo update --manifest-path enterprise/Cargo.toml -p serde --precise", errors[0])

    def test_a_different_shared_file_is_reported(self):
        for name in ALIGNMENT.SHARED_FILES:
            with self.subTest(name=name):
                (self.enterprise / name).write_text((self.community / name).read_text() + "\n# drift\n")
                self.assertIn(f"enterprise/{name} differs from {name}",
                              ALIGNMENT.misalignments(self.community, self.enterprise))
                shutil.copy(self.community / name, self.enterprise / name)

    def test_different_workspace_tables_are_reported(self):
        self.edit(self.enterprise / "Cargo.toml", 'rust-version = "1.96"', 'rust-version = "1.95"')
        self.edit(self.enterprise / "Cargo.toml", 'lto = "thin"', 'lto = "fat"')
        self.assertEqual(ALIGNMENT.misalignments(self.community, self.enterprise), [
            "[workspace.package] differs between the workspaces",
            "[profile] differs between the workspaces",
        ])

    def test_the_component_license_is_fixed_per_workspace(self):
        self.edit(self.community / "Cargo.toml", 'license = "Apache-2.0"', 'license = "MIT"')
        self.edit(self.enterprise / "Cargo.toml", 'license = "BUSL-1.1"', 'license = "MIT"')
        self.assertEqual(ALIGNMENT.misalignments(self.community, self.enterprise), [
            '[workspace.package] license in Cargo.toml must be "Apache-2.0"',
            '[workspace.package] license in enterprise/Cargo.toml must be "BUSL-1.1"',
        ])

    def test_a_shared_dependency_must_name_the_same_crate(self):
        self.edit(self.community / "Cargo.toml", 'aidash-domain = { path = "crates/aidash-domain" }',
                  'aidash-domain = { path = "crates/aidash-application" }')
        self.assertIn("[workspace.dependencies] aidash-domain differs between the workspaces",
                      ALIGNMENT.misalignments(self.community, self.enterprise))


if __name__ == "__main__":
    unittest.main()
