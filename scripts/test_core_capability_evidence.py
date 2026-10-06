"""Regression checks for acceptance identifiers and fail-closed gate evidence."""

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location(
    "core_evidence", Path(__file__).with_name("core-capability-evidence.py")
)
EVIDENCE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(EVIDENCE)


class CoreEvidenceTests(unittest.TestCase):
    mapping = {
        "acceptance": {
            "AT01": {
                "tests": [{"name": "apps::execution::limit", "source": "limits.rs"}]
            }
        },
        "requirements": {"R01": {"acceptance": ["AT01"]}},
        "commands": {"runtime": "scripts/test-capability-cluster.sh"},
    }

    def test_every_parameterized_case_must_pass(self):
        outcomes = {
            "apps::execution::limit::case_1": "ok",
            "apps::execution::limit::case_2": "ok",
        }
        _, errors = EVIDENCE.acceptance_results(self.mapping, outcomes)
        self.assertEqual(errors, [])
        for status in ("FAILED", "ignored"):
            with self.subTest(status=status):
                outcomes["apps::execution::limit::case_2"] = status
                results, errors = EVIDENCE.acceptance_results(self.mapping, outcomes)
                self.assertFalse(results["AT01"]["passed"])
                self.assertEqual(len(errors), 1)

    def test_similar_or_obsolete_names_cannot_satisfy_a_required_assertion(self):
        for name in (
            "execution::limit",
            "apps::execution::limit_extra",
            "other::apps::execution::limit",
        ):
            with self.subTest(name=name):
                results, errors = EVIDENCE.acceptance_results(self.mapping, {name: "ok"})
                self.assertFalse(results["AT01"]["passed"])
                self.assertEqual(len(errors), 1)

    def test_inventory_accepts_cargo_cases_but_rejects_an_obsolete_identifier(self):
        with tempfile.TemporaryDirectory(dir="/tmp") as temporary:
            directory = Path(temporary)
            mapping = directory / "mapping.json"
            mapping.write_text(json.dumps(self.mapping))
            listing = directory / "tests.list"
            listing.write_text(
                "apps::execution::limit::case_1: test\n"
                "\x1b[32mapps::execution::limit::case_2: test\x1b[0m\n"
                "2 tests, 0 benchmarks\n"
            )
            with patch.object(EVIDENCE, "MAPPING", mapping):
                self.assertEqual(EVIDENCE.check_inventory(directory), 0)
                listing.write_text("execution::limit: test\n")
                self.assertEqual(EVIDENCE.check_inventory(directory), 1)
            report = json.loads((directory / "inventory-result.json").read_text())
            self.assertFalse(report["valid"])
            self.assertEqual(len(report["errors"]), 1)

    def test_successful_tests_do_not_replace_source_isolation_or_recovery_evidence(self):
        with tempfile.TemporaryDirectory(dir="/tmp") as temporary:
            directory = Path(temporary)
            mapping = directory / "mapping.json"
            mapping.write_text(json.dumps(self.mapping))
            (directory / "source.json").write_text(json.dumps({"source_sha256": "before"}))
            (directory / "runtime.log").write_text("test apps::execution::limit ... ok\n")
            with patch.object(EVIDENCE, "MAPPING", mapping), patch.object(
                EVIDENCE, "revision", return_value={"source_sha256": "after"}
            ):
                self.assertEqual(EVIDENCE.finish(directory, 0), 1)
            report = json.loads((directory / "result.json").read_text())
            self.assertTrue(report["acceptance"]["AT01"]["passed"])
            self.assertFalse(report["passed"])
            self.assertEqual(len(report["errors"]), 3)


if __name__ == "__main__":
    unittest.main()
