"""Offline tests for journal probes and the shared panel metric."""
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import context_recovery_probes as probes
import optimize_context_recovery as optimizer


class ProbeTest(unittest.TestCase):
    def test_corrections_identifiers_human_and_side_effect_results(self):
        journal = [
            {"seq": 1, "event": {"kind": "human", "response": "BUDGET-USD=500; EXCLUDE-VENDOR=Northwind"}},
            {"seq": 2, "event": {"kind": "tool", "call": {"id": "c1", "name": "lookup", "arguments": {"invoice": "REQ-2026-0720"}},
                                  "result": {"content": "Correction: BUDGET-USD=300. EXCLUDE-VENDOR=Contoso. ID INV-2026-0105"}}},
            {"seq": 3, "event": {"kind": "tool", "call": {"id": "c2", "name": "record_note", "arguments": {"page": 2}},
                                  "result": {"recorded": True, "page": 2}}},
        ]
        actual = probes.generate(journal, ["record_note"])
        constraints = {(p["question"], p["answer"]) for p in actual if p["type"] == "constraint"}
        self.assertIn(("What is the current value of BUDGET-USD?", "300"), constraints)
        self.assertNotIn(("What is the current value of BUDGET-USD?", "500"), constraints)
        self.assertEqual({p["answer"] for p in actual if "excluded value" in p["question"]}, {"Northwind", "Contoso"})
        self.assertIn("INV-2026-0105", [p["answer"] for p in actual if p["type"] == "identifier"])
        self.assertIn("REQ-2026-0720", [p["answer"] for p in actual if p["type"] == "identifier"])
        side_effect = next(p for p in actual if p["type"] == "side_effect_outcome")
        self.assertEqual((side_effect["seq"], side_effect["answer"]), (3, '{"page":2,"recorded":true}'))
        self.assertTrue(any(p["type"] == "person_instruction" and p["seq"] == 1 for p in actual))
        self.assertEqual(actual, probes.generate(list(reversed(journal)), ["record_note"]))
        self.assertFalse(any(p["type"] == "side_effect_outcome" for p in probes.generate(journal)))
        with self.assertRaisesRegex(ValueError, "duplicate"):
            probes.generate([journal[0], journal[0]])

    def test_panel_score_and_feedback_use_exact_answers_and_worst_model(self):
        example = optimizer.prepare({"source": "synthetic", "journal": [
            {"seq": 1, "event": {"kind": "human", "response": "BUDGET-USD=300"}}]})
        answers = {p["id"]: p["answer"] for p in example["probes"]}

        class Model:
            def __init__(self, answers):
                self.answers = answers
            def __call__(self, prompt):
                return json.dumps(self.answers) if "Return a JSON object mapping" in prompt else "summary"

        poor = dict(answers)
        poor[example["probes"][0]["id"]] += " "
        score, feedback = optimizer.evaluate_episode(example, optimizer.SEED,
                                                     {"good": Model(answers), "poor": Model(poor)})
        self.assertEqual(score, 0.5)
        self.assertEqual(feedback["models"]["good"]["score"], 1.0)
        self.assertEqual(len(feedback["models"]["poor"]["missed"]), 1)
        score, feedback = optimizer.evaluate_episode(example, {**optimizer.SEED, "marker": "range 1-1"},
                                                     {"good": Model(answers)})
        self.assertEqual(score, 0)
        self.assertIn("marker", feedback["error"])
        with self.assertRaises(ValueError):
            optimizer.prepare({"source": "other", "journal": []})

    def test_dry_run_invokes_gepa_with_fake_lms(self):
        try:
            import gepa  # noqa: F401
        except ImportError:
            self.skipTest("install gepa with uv to exercise the optimizer")
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "result.json"
            command = [sys.executable, str(Path(optimizer.__file__)), "--dry-run", "--max-metric-calls", "4", "--output", str(output)]
            subprocess.run(command, check=True, capture_output=True, text=True, timeout=30)
            result = json.loads(output.read_text())
        self.assertEqual(result["panel"], ["fake-a", "fake-b"])
        self.assertEqual(result["validation_mean_worst_model_score"], 0.0)
        self.assertEqual(result["held_out_probe_comparison"], "insufficient-data")
        self.assertTrue(result["validation"][0]["feedback"]["models"]["fake-a"]["missed"])
        self.assertEqual(set(result["candidate"]), {"summary_prompt", "marker"})


if __name__ == "__main__":
    unittest.main()
