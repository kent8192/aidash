"""Exercise portable-layer contracts against the actual locked Cargo graph."""

import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
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


class LayoutTests(unittest.TestCase):
    def test_cloud_infrastructure_outside_enterprise_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.assertEqual(BOUNDARIES.layout_violations(root), [])
            (root / "infra").mkdir()
            self.assertEqual(
                BOUNDARIES.layout_violations(root),
                ["Cloud infrastructure must live under enterprise/: infra/"],
            )


class EnterpriseReferenceTests(unittest.TestCase):
    def references(self, files):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for path, content in files.items():
                (root / path).parent.mkdir(parents=True, exist_ok=True)
                (root / path).write_text(content)
            return BOUNDARIES.enterprise_references(root)

    def test_community_build_inputs_may_not_name_enterprise(self):
        for path, content in (
            ("Cargo.toml", '[workspace]\nmembers = ["crates/*", "enterprise/server"]\n'),
            ("crates/a/Cargo.toml", '[dependencies]\nb = { path = "../../enterprise/b" }\n'),
            ("desktop/Cargo.lock", 'source = "enterprise"\n'),
            (".cargo/config.toml", '[patch.crates-io]\nb = { path = "enterprise/b" }\n'),
            ("Dockerfile", "COPY enterprise ./enterprise\n"),
        ):
            with self.subTest(path=path):
                self.assertEqual(
                    self.references({path: content}),
                    [f"Community build input names enterprise/: {path}"],
                )

    def test_enterprise_files_and_package_names_are_not_references(self):
        self.assertEqual(self.references({
            "Cargo.toml": "# aidash-enterprise-server composes this server.\n",
            "enterprise/Cargo.toml": '[workspace]\nmembers = ["server"]\n',
            "web/node_modules/x/Cargo.toml": 'path = "enterprise"\n',
        }), [])


class EnterpriseWorkspaceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.root = ROOT / "enterprise"
        cls.metadata, cls.community = (json.loads(subprocess.check_output(
            ["cargo", "metadata", "--locked", "--format-version", "1"], cwd=root,
        )) for root in (cls.root, ROOT))

    def test_locked_enterprise_workspace_composes_only_community_crates(self):
        self.assertEqual(BOUNDARIES.enterprise_violations(self.metadata, self.root), [])

    def test_standalone_services_cannot_compose_the_cloud_runtime(self):
        identities = {package["name"]: package["id"] for package in self.metadata["packages"]}
        for service in ("aidash-auth-server", "aidash-marketplace-server"):
            for kind in (None, "build"):
                with self.subTest(service=service, kind=kind):
                    metadata = copy.deepcopy(self.metadata)
                    node = next(node for node in metadata["resolve"]["nodes"]
                                if node["id"] == identities[service])
                    node["deps"].append({"pkg": identities["aidash-server"],
                                         "dep_kinds": [{"kind": kind}]})
                    self.assertIn(
                        f"standalone service composes Cloud runtime: {service} -> aidash-server",
                        BOUNDARIES.enterprise_violations(metadata, self.root),
                    )

    def test_local_dependencies_outside_community_crates_are_rejected(self):
        metadata = copy.deepcopy(self.metadata)
        metadata["packages"].append({
            "id": "path+file:///runner#aidash-runner@0.1.0", "name": "aidash-runner",
            "source": None, "manifest_path": str(ROOT / "runner" / "Cargo.toml"),
        })
        self.assertIn(
            "Enterprise may depend locally only on Community crates: aidash-runner",
            BOUNDARIES.enterprise_violations(metadata, self.root),
        )

    def test_community_may_not_resolve_a_google_cloud_client(self):
        metadata = copy.deepcopy(self.community)
        metadata["packages"].append({
            "id": "registry+https://github.com/rust-lang/crates.io-index#gcp_auth@0.12.7",
            "name": "gcp_auth", "source": "registry+https://github.com/rust-lang/crates.io-index",
            "manifest_path": "/registry/gcp_auth-0.12.7/Cargo.toml", "dependencies": [], "features": {},
        })
        self.assertIn(
            "Community resolves a Google Cloud client: gcp_auth",
            BOUNDARIES.violations(metadata, ROOT),
        )

    def test_community_may_not_resolve_an_enterprise_package(self):
        metadata = copy.deepcopy(self.community)
        metadata["packages"].append({
            "id": "path+file:///enterprise/server#aidash-enterprise-server@0.1.0",
            "name": "aidash-enterprise-server", "source": None,
            "manifest_path": str(self.root / "server" / "Cargo.toml"),
        })
        self.assertIn(
            "Community resolves an Enterprise package: aidash-enterprise-server",
            BOUNDARIES.violations(metadata, ROOT),
        )


if __name__ == "__main__":
    unittest.main()
