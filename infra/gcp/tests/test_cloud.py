"""Plan review must enforce authorization even for drift after preflight."""

import json
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "control"))
from cloud import Terraform


class PlanTests(unittest.TestCase):
    def apply(self, resource_type, actions, **authorization):
        with TemporaryDirectory() as directory:
            terraform = Terraform.__new__(Terraform)
            terraform.root = Path(directory)
            terraform.configuration = dict.fromkeys(
                (
                    "project_id",
                    "cloudflare_zone_id",
                    "release_bucket",
                    "deploy_service_account",
                    "domain",
                ),
                "fixture",
            )
            plan = {
                "resource_changes": [
                    {
                        "type": resource_type,
                        "change": {
                            "actions": actions,
                            "before": {"labels": {"environment": "test"}},
                            "after": {"labels": {"environment": "test"}},
                        },
                    }
                ]
            }
            with patch("cloud.run", return_value=json.dumps(plan).encode()) as command:
                try:
                    terraform.apply({}, **authorization)
                finally:
                    self.assertFalse(
                        (terraform.root / "controller.auto.tfvars.json").exists()
                    )
                    self.assertFalse((terraform.root / "controller.tfplan").exists())
            self.assertTrue(
                any("apply" in call.args for call in command.call_args_list)
            )

    def test_refresh_cannot_recreate_a_vm_deleted_after_preflight(self):
        with self.assertRaisesRegex(RuntimeError, "power authorization"):
            self.apply("google_compute_instance", ["create"])
        with self.assertRaises(RuntimeError):
            self.apply(
                "google_compute_instance", ["delete", "create"], starting={"pr-1"}
            )
        self.apply("google_compute_instance", ["create"], starting={"test"})

    def test_disk_replacement_requires_explicit_retirement_for_that_owner(self):
        with self.assertRaisesRegex(RuntimeError, "retained disk"):
            self.apply("google_compute_disk", ["delete", "create"])
        with self.assertRaises(RuntimeError):
            self.apply("google_compute_disk", ["delete"], retiring={"pr-1"})
        self.apply("google_compute_disk", ["delete"], retiring={"test"})


if __name__ == "__main__":
    unittest.main()
