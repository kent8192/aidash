import importlib.util
import json
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location("evaluate_native_memory", ROOT / "scripts/evaluate-native-memory.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)

AGENT = {"id": "research", "version": "1"}
PROVIDER = {"id": "memory", "version": "1"}
DIVERSE_AGENT = {"id": "research-diverse", "version": "1"}
DIVERSE_PROVIDER = {"id": "memory-diverse", "version": "1"}


def sample(pattern):
    """The first alternative of every group satisfies a frozen regex label."""
    return " ".join(re.findall(r"\(\?i\)\(([^|)]+)", pattern))


class Home:
    """Minimal in-memory Home: extracts one Unit per label and recalls `limit` of them per provider."""

    def __init__(self, fixture, limit_by_provider):
        self.labels = {case["text"]: case["labels"] for case in fixture["cases"]}
        self.limit = limit_by_provider
        self.banks, self.extracted, self.operations = 0, {}, {}

    def __call__(self, path, body):
        if path.endswith("/memory/participants"):
            self.banks += 1
            return 200, {"bank": {"participant": self.banks}}
        if path.endswith("/memory/units/mutate"):
            change = body["changes"][0]
            return 200, [{"revision": 1}] if change["operation"] == "add" else []
        if path.endswith("/memory/units/query"):
            return 200, []
        if path.endswith("/semantic/entries"):
            return 200, [{"id": unit["id"], "state": "READY"} for units in self.extracted.values() for unit in units]
        action = body["action"]
        bank = body["bank"]["participant"]
        if action["action"] == "retain":
            evidence = action["evidence"][0]
            units = [{"id": f"{bank}-{label['id']}", "content": {
                "kind": label["kind"], "learning": label["learning"], "evidence": [evidence],
                "text": " ".join(sample(pattern) for pattern in label["patterns"])}} for label in self.labels[action["text"]]]
            self.extracted[bank] = units
            self.operations[body["operation_id"]] = bank
            return 200, {"value": units}
        if action["action"] == "usage":
            rows = [{"id": operation, "calls": 1, "tokens": 10, "cost_micros": 1}
                    for operation, owner in self.operations.items() if owner == bank]
            return 200, {"value": {"items": rows, "next": None}}
        if action["action"] == "recall":
            units = self.extracted[bank][: self.limit[body["provider"]["id"]]]
            return 200, {"value": {"status": "ready", "units": units}}
        raise AssertionError(action)


class EvaluateNativeMemoryTest(unittest.TestCase):
    def evaluate(self, **options):
        fixture = __import__("tomllib").loads((ROOT / "tests/fixtures/native_memory_evaluation.toml").read_text())
        home = Home(fixture, {"memory": 4, "memory-diverse": 2})
        return MODULE.evaluate(home, "workspace", AGENT, PROVIDER, {"kind": "test"}, **options)

    def test_baseline_report_keeps_its_shape_and_gains_delivery_metrics(self):
        report = self.evaluate()
        self.assertNotIn("diversification", report)
        self.assertEqual(sorted(report["latency"]), ["correction", "extraction", "indexing", "recall"])
        for case in report["cases"]:
            metrics = case["metrics"]
            self.assertEqual(metrics["useful_recall"], 1)
            self.assertEqual(metrics["delivered_units"], 4)
            envelope = {"status": "ready", "units": case["raw_recall"]["units"]}
            self.assertEqual(metrics["delivered_tokens"], len(json.dumps(envelope, ensure_ascii=False, separators=(",", ":")).encode()))

    def test_paired_arm_reports_recall_and_delivered_tokens_with_and_without_diversity_per_language(self):
        report = self.evaluate(diversity={"agent": DIVERSE_AGENT, "provider": DIVERSE_PROVIDER, "profile": {"kind": "diverse"}})
        paired = report["diversification"]
        self.assertEqual(paired["profile"], {"kind": "diverse"})
        self.assertEqual(len(paired["cases"]), len(report["cases"]))
        self.assertTrue(all(row["path"] for row in paired["requests"]))
        comparison = paired["comparison"]
        self.assertEqual(sorted(comparison["by_language"]), ["en-US", "ja-JP"])
        for row in comparison["cases"]:
            self.assertEqual(row["delivered_units_without"], 4)
            self.assertEqual(row["delivered_units_with"], 2)
            self.assertEqual(row["useful_recall_without"], 1)
            self.assertEqual(row["useful_recall_with"], 0.5)
            self.assertLess(row["delivered_tokens_with"], row["delivered_tokens_without"])
        for total in comparison["by_language"].values():
            self.assertEqual((total["useful_recall_without"], total["useful_recall_with"]), (1, 0.5))
            self.assertLess(total["delivered_tokens_with"], total["delivered_tokens_without"])

    def test_pairing_rejects_arms_that_evaluated_different_cases(self):
        row = {"id": "a", "language": "en-US", "metrics": {"useful_recall": 1, "delivered_units": 1, "delivered_tokens": 1}}
        with self.assertRaises(AssertionError):
            MODULE.compare_diversification([row], [{**row, "id": "b"}])


if __name__ == "__main__":
    unittest.main()
