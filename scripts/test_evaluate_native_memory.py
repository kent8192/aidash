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
    """Minimal in-memory Home: extracts one Unit per label; a diversifying provider recalls the distinct near-duplicate."""

    def __init__(self, fixture):
        self.labels = {case["text"]: case["labels"] for case in fixture["cases"]}
        self.banks, self.extracted, self.operations, self.groups, self.admitted = 0, {}, {}, {}, []

    def __call__(self, path, body):
        if path.endswith("/memory/participants"):
            self.banks += 1
            return 200, {"bank": {"participant": self.banks}}
        if path.endswith("/memory/units/mutate"):
            if body["changes"][0]["operation"] != "add":
                return 200, []
            units = [{"id": change["id"], "revision": 1, "content": change["content"]} for change in body["changes"]]
            if len(units) > 1:
                self.groups[body["bank"]["participant"]] = units
                self.admitted.append((body["provider"]["id"], [unit["id"] for unit in units]))
            return 200, units
        if path.endswith("/memory/units/query"):
            return 200, []
        if path.endswith("/semantic/entries"):
            ready = [unit for units in (*self.extracted.values(), *self.groups.values()) for unit in units]
            return 200, [{"id": unit["id"], "state": "READY"} for unit in ready]
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
            if bank in self.groups:
                group = self.groups[bank]
                diverse = body["provider"]["id"] == "memory-diverse"
                units = [group[0], group[3]] if diverse else group[:2]
            else:
                units = self.extracted[bank]
            return 200, {"value": {"status": "ready", "units": units}}
        raise AssertionError(action)


class EvaluateNativeMemoryTest(unittest.TestCase):
    def evaluate(self, **options):
        fixture = __import__("tomllib").loads((ROOT / "tests/fixtures/native_memory_evaluation.toml").read_text())
        home = Home(fixture)
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

    def test_paired_arm_runs_the_near_duplicate_groups_and_reports_recall_and_tokens_per_language(self):
        report = self.evaluate(diversity={"agent": DIVERSE_AGENT, "provider": DIVERSE_PROVIDER, "profile": {"kind": "diverse"}})
        paired = report["diversification"]
        self.assertEqual(paired["profile"], {"kind": "diverse"})
        self.assertEqual([case["id"] for case in paired["cases"]], [case["id"] for case in paired["baseline_cases"]])
        self.assertEqual({case["language"] for case in paired["cases"]}, {"en-US", "ja-JP"})
        self.assertEqual(len(report["cases"]), 2, "the generic labelled cases run once, on the baseline Agent")
        comparison = paired["comparison"]
        self.assertEqual(sorted(comparison["by_language"]), ["en-US", "ja-JP"])
        for row in comparison["cases"]:
            self.assertEqual((row["useful_recall_without"], row["useful_recall_with"]), (0.5, 1))
            self.assertEqual((row["delivered_units_without"], row["delivered_units_with"]), (2, 2))
            self.assertGreater(row["delivered_tokens_with"], 0)
        for arm in (paired["baseline_cases"], paired["cases"]):
            for case in arm:
                self.assertLessEqual(case["metrics"]["delivered_tokens"], case["metrics"]["budget_tokens"])
        for total in comparison["by_language"].values():
            self.assertEqual((total["useful_recall_without"], total["useful_recall_with"]), (0.5, 1))
            self.assertEqual((total["delivered_units_without"], total["delivered_units_with"]),
                             (2 * total["cases"], 2 * total["cases"]))
            self.assertGreater(total["delivered_tokens_with"], 0)

    def test_paired_arms_admit_ids_with_the_same_relative_order_so_score_ties_break_identically(self):
        fixture = __import__("tomllib").loads((ROOT / "tests/fixtures/native_memory_evaluation.toml").read_text())
        home = Home(fixture)
        MODULE.evaluate(home, "workspace", AGENT, PROVIDER, {"kind": "test"},
                        diversity={"agent": DIVERSE_AGENT, "provider": DIVERSE_PROVIDER, "profile": {"kind": "diverse"}})
        arms = {provider: [ids for owner, ids in home.admitted if owner == provider] for provider in ("memory", "memory-diverse")}
        self.assertEqual(len(arms["memory"]), len(fixture["diversity_cases"]))
        self.assertEqual(len(arms["memory-diverse"]), len(fixture["diversity_cases"]))
        for ids in (*arms["memory"], *arms["memory-diverse"]):
            self.assertEqual(ids, sorted(ids), "Unit IDs must ascend in fixture order in every arm")
            self.assertEqual(len(set(ids)), len(ids))

    def test_pairing_rejects_arms_that_evaluated_different_cases(self):
        row = {"id": "a", "language": "en-US", "metrics": {"useful_recall": 1, "delivered_units": 1, "delivered_tokens": 1}}
        with self.assertRaises(AssertionError):
            MODULE.compare_diversification([row], [{**row, "id": "b"}])
        with self.assertRaises(ValueError):
            MODULE.compare_diversification([row, row], [row])


if __name__ == "__main__":
    unittest.main()
