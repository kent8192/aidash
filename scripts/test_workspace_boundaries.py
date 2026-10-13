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
            ("aidash-capability", "aidash-broker"),
            ("aidash-capability", "aidash-integrations"),
            ("aidash-capability", "reqwest"),
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

    def test_test_support_features_stay_out_of_production_dependencies(self):
        server = next(package for package in self.metadata["packages"]
                      if package["name"] == "aidash-server")
        dev = [dependency for dependency in server["dependencies"]
               if dependency["name"] == "aidash-application" and dependency["kind"] == "dev"]
        self.assertTrue(dev and "test-support" in dev[0]["features"],
                        "server tests use the test-only write port")
        for kind in (None, "build"):
            with self.subTest(kind=kind):
                metadata = copy.deepcopy(self.metadata)
                package = next(package for package in metadata["packages"]
                               if package["name"] == "aidash-server")
                package["dependencies"].append({
                    "name": "aidash-application", "kind": kind, "features": ["test-support"],
                })
                self.assertIn(
                    "production dependency enables test-support: aidash-server -> aidash-application",
                    BOUNDARIES.violations(metadata, ROOT),
                )
        # Any feature other than test-support itself that enables it, directly
        # or through a dependency, would enable the test-only cfg in a
        # production build, including a non-default alias.
        for crate, feature, values in (
                ("aidash-server", "default", ["test-support"]),
                ("aidash-server", "fixtures", ["aidash-integrations/test-support"]),
                ("aidash-server", "fixtures", ["aidash-application?/test-support"]),
                ("aidash-application", "fixtures", ["test-support"])):
            with self.subTest(crate=crate, feature=feature, values=values):
                metadata = copy.deepcopy(self.metadata)
                package = next(package for package in metadata["packages"]
                               if package["name"] == crate)
                package["features"][feature] = values
                self.assertIn(
                    f"feature {crate}/{feature} must not enable test-support",
                    BOUNDARIES.violations(metadata, ROOT),
                )
        # The test-support feature itself may chain to a dependency's.
        metadata = copy.deepcopy(self.metadata)
        package = next(package for package in metadata["packages"]
                       if package["name"] == "aidash-integrations")
        package["features"]["test-support"] = ["aidash-application/test-support"]
        self.assertEqual(BOUNDARIES.violations(metadata, ROOT), [])


if __name__ == "__main__":
    unittest.main()
