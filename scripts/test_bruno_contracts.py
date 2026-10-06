"""Regression checks for completeness and privacy of real-API evidence."""

import json
from pathlib import Path
import tempfile
import unittest
from bruno_contracts import inventory

# The executable script uses a hyphen, so load its public evidence reducer explicitly.
import importlib.util

SPEC = importlib.util.spec_from_file_location(
    "bruno_api", Path(__file__).with_name("test-bruno-api.py")
)
API = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(API)


class ContractEvidenceTests(unittest.TestCase):
    def test_native_inventory_retains_methods_and_wildcards(self):
        actual = inventory(
            "[INFO] /api/tasks GET task-list -\n[INFO] /{<path:asset>} GET, HEAD frontend -\n[INFO] Total: 3"
        )
        self.assertEqual(
            dict(actual),
            {
                ("GET", "/api/tasks"): 1,
                ("GET", "/{<path:asset>}"): 1,
                ("HEAD", "/{<path:asset>}"): 1,
            },
        )

    def evidence(self, duplicate=False, skipped=False, missing_check=False):
        scenarios = [
            {
                "name": f"case-{endpoint}-{scenario}",
                "endpoint": f"GET /endpoint-{endpoint}",
                "required_checks": ["response contract"],
            }
            for endpoint in range(269)
            for scenario in range(3)
        ]
        results = [
            {
                "name": case["name"],
                "response": {
                    "status": 200,
                    "headers": {"Authorization": "private-value"},
                    "data": "private-value",
                },
                "testResults": [{"description": "response contract", "status": "pass"}],
            }
            for case in scenarios
        ]
        if duplicate:
            results[-1] = results[0].copy()
        if skipped:
            results[0]["skipped"] = True
        if missing_check:
            scenarios[0]["required_checks"].append("mutation preserved state")
        with tempfile.TemporaryDirectory(dir="/tmp") as directory:
            path = Path(directory) / "report.json"
            path.write_text(json.dumps([{"results": results}]))
            return API.summarize(path, [case["name"] for case in scenarios], scenarios)

    def test_complete_evidence_requires_every_scenario_exactly_once(self):
        self.assertTrue(self.evidence()["passed"])
        self.assertFalse(self.evidence(duplicate=True)["complete"])
        self.assertFalse(self.evidence(skipped=True)["passed"])
        self.assertFalse(self.evidence(missing_check=True)["passed"])

    def test_published_evidence_never_retains_response_values(self):
        self.assertNotIn("private-value", json.dumps(self.evidence()))


if __name__ == "__main__":
    unittest.main()
