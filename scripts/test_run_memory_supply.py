import importlib.util
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location("evaluate_run_memory_supply",
                                              Path(__file__).with_name("evaluate-run-memory-supply.py"))
supply = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(supply)


def case(name):
    return next(item for item in supply.load_corpus()["cases"] if item["id"] == name)


class GradeTest(unittest.TestCase):
    def test_corpus_loads_with_every_arm_and_known_references(self):
        corpus = supply.load_corpus()
        self.assertEqual(corpus["arms"], ["auto", "on-demand", "none"])
        self.assertRegex(corpus["_sha256"], "^[0-9a-f]{64}$")

    def test_a_correct_answer_without_a_read_is_not_supply(self):
        item = case("exact-fact-among-distractors")
        revisions = {"heron": 1, "falcon": 1, "osprey": 1}
        guessed = supply.grade(item, "auto", "HX-4471-K", {"heron": [], "falcon": [], "osprey": []}, revisions, set())
        self.assertTrue(guessed["answer_correct"])
        self.assertFalse(guessed["passed"])
        read = supply.grade(item, "auto", "HX-4471-K", {"heron": [1], "falcon": [1], "osprey": []}, revisions, set())
        self.assertTrue(read["passed"])
        distracted = supply.grade(item, "auto", "HX-4471-K or FX-2208-M", {"heron": [1]}, revisions, set())
        self.assertFalse(distracted["answer_correct"])

    def test_superseded_revisions_and_deleted_units_are_stale(self):
        corrected = case("corrected-fact")
        stale = supply.grade(corrected, "auto", "2027-02-17", {"kestrel": [1, 2]}, {"kestrel": 2}, set())
        self.assertEqual(stale["stale_reads"], ["kestrel"])
        self.assertFalse(stale["passed"])
        self.assertTrue(supply.grade(corrected, "on-demand", "2027-02-17", {"kestrel": [2]}, {"kestrel": 2},
                                     set())["passed"])
        deleted = case("deleted-fact")
        leaked = supply.grade(deleted, "auto", "UNKNOWN", {"merlin": [1]}, {"merlin": 1, "merlin-owner": 1},
                              {"merlin"})
        self.assertEqual(leaked["stale_reads"], ["merlin"])
        self.assertFalse(leaked["passed"])
        unread = supply.grade(deleted, "auto", "UNKNOWN", {"merlin": [], "merlin-owner": [1]},
                              {"merlin": 1, "merlin-owner": 1}, {"merlin"})
        self.assertEqual(unread["stale_reads"], [])
        self.assertTrue(unread["passed"])

    def test_the_control_passes_only_without_any_read(self):
        item = case("deleted-fact")
        revisions = {"merlin": 1, "merlin-owner": 1}
        self.assertTrue(supply.grade(item, "none", "UNKNOWN", {}, revisions, {"merlin"})["passed"])
        self.assertFalse(supply.grade(item, "none", "UNKNOWN", {"merlin-owner": [1]}, revisions,
                                      {"merlin"})["passed"])

    def test_summary_counts_errors_separately(self):
        runs = [{"case": "c", "arm": "auto", "grade": {"passed": True, "answer_correct": True, "supplied": True,
                                                      "stale_reads": []}, "memory_recall_calls": 0,
                 "input_tokens": 10, "steps": 1},
                {"case": "c", "arm": "auto", "error": "RuntimeError: x"}]
        self.assertEqual(supply.summarize(runs)["arms"]["auto"],
                         {"n": 2, "passed": 1, "answer_correct": 1, "supplied": 1, "stale": 0, "errors": 1})


if __name__ == "__main__":
    unittest.main()
