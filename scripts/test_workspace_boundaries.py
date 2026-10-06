"""Exercise portable-layer contracts against the actual locked Cargo graph."""

import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import unittest


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "workspace_boundaries", Path(__file__).with_name("check-workspace-boundaries.py")
)
BOUNDARIES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BOUNDARIES)


class WorkspaceBoundaryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.metadata = json.loads(subprocess.check_output(
            ["cargo", "metadata", "--locked", "--format-version", "1"], cwd=ROOT,
        ))
        cls.identities = {
            package["name"]: package["id"] for package in cls.metadata["packages"]
        }

    def add_dependency(self, metadata, owner, dependency, kind=None):
        node = next(node for node in metadata["resolve"]["nodes"]
                    if node["id"] == self.identities[owner])
        node["deps"].append({
            "pkg": self.identities[dependency],
            "dep_kinds": [{"kind": kind}],
        })

    def test_locked_workspace_obeys_portable_layer_contracts(self):
        self.assertEqual(BOUNDARIES.violations(self.metadata, ROOT), [])

    def test_portable_layers_reject_reverse_and_adapter_dependencies(self):
        for owner, dependency in (
            ("aidash-domain", "aidash-application"),
            ("aidash-application", "aidash-harness"),
            ("aidash-harness", "aidash-runtime"),
            ("aidash-harness", "aidash-integrations"),
            ("aidash-harness", "aidash-server"),
        ):
            with self.subTest(owner=owner, dependency=dependency):
                metadata = copy.deepcopy(self.metadata)
                self.add_dependency(metadata, owner, dependency)
                self.assertIn(
                    f"forbidden production dependency: {owner} -> {dependency}",
                    BOUNDARIES.violations(metadata, ROOT),
                )

    def test_transitive_transport_dependency_is_rejected(self):
        metadata = copy.deepcopy(self.metadata)
        # A build dependency is still part of the production dependency closure.
        self.add_dependency(metadata, "aidash-application", "reqwest", kind="build")
        errors = BOUNDARIES.violations(metadata, ROOT)
        self.assertIn(
            "forbidden production dependency: aidash-harness -> aidash-application -> reqwest",
            errors,
        )


if __name__ == "__main__":
    unittest.main()
