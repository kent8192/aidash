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


class FakeExperiment:
    prefix = "mse"
    token = "subject"

    def __init__(self):
        self.entries = {}

    def register(self, kind, identifier, config):
        self.entries[identifier] = (kind, config)
        return {"id": identifier, "version": "1.0.0"}

    def ref(self, identifier):
        return {"registry_deployment": "node", "id": identifier, "version": "1.0.0"}


class FakeClock:
    def __init__(self):
        self.now = 0.0

    def __call__(self):
        return self.now

    def sleep(self, seconds):
        self.now += max(seconds, 0.01)


def campaign(arms, call=None, clock=None):
    corpus = dict(supply.load_corpus(), arms=arms)
    clock = clock or FakeClock()
    api = supply.runner.Api(call or (lambda *args: (500, None)), "administrator", sleep=clock.sleep, clock=clock)
    return supply.Campaign(api, FakeExperiment(), corpus, {"id": "model", "version": "1"}, "http://embedding",
                           run_timeout=10.0), clock


class RegisterTest(unittest.TestCase):
    def test_a_control_only_campaign_registers_a_seed_provider_and_participant_agent(self):
        selected, _ = campaign(["none"])
        selected.register()
        self.assertEqual(selected.providers, {})
        self.assertEqual(selected.seed["provider"]["id"], "mse-memory-seed")
        kind, config = selected.experiment.entries[selected.seed["agent"]["id"]]
        self.assertEqual((kind, [binding["kind"] for binding in config["bindings"]]), ("agent", ["memory"]))
        self.assertNotIn("mse-agent-seed", [agent["id"] for agent in selected.agents.values()])

    def test_the_control_seeds_through_an_evaluated_arm_when_one_is_selected(self):
        selected, _ = campaign(["on-demand", "none"])
        selected.register()
        self.assertEqual(selected.seed, {"provider": selected.providers["on-demand"],
                                         "agent": selected.agents["on-demand"]})
        self.assertNotIn("mse-memory-seed", selected.experiment.entries)

    def test_no_arm_offers_reflection_and_only_the_control_loses_recall(self):
        selected, _ = campaign(["auto", "on-demand", "none"])
        selected.register()
        removed = {arm: selected.experiment.entries[agent["id"]][1]["remove_default"]
                   for arm, agent in selected.agents.items()}
        for arm, names in removed.items():
            self.assertIn("memory_reflect", names, arm)
            self.assertEqual("memory_recall" in names, arm == "none", arm)


class ClaimTest(unittest.TestCase):
    def test_a_lost_claim_response_recovers_and_cancels_the_admitted_run(self):
        state = {"phase": "RUNNING", "cancelled": False, "runs": []}

        def call(method, path, body, token, timeout):
            if path == "/api/tasks/task/claim":
                state["runs"] = [{"id": "run", "task_id": "task"}]
                return 504, "gateway timeout"  # The claim committed, but its answer was lost.
            if path == "/api/state":
                return 200, {"runs": state["runs"], "human_requests": []}
            if path == "/api/runs/run":
                return 200, {"run": {"id": "run", "phase": state["phase"]}}
            if path == "/api/runs/run/control":
                state["phase"] = "CANCELLED"
                return 200, {}
            raise AssertionError(path)

        selected, clock = campaign(["auto"], call)
        record = {}
        with self.assertRaises(supply.runner.ApiError):
            selected.claim_and_wait("task", {"revision": 0}, record, clock, clock.sleep)
        self.assertEqual((state["phase"], record["run_id"], record["stopped_phase"]), ("CANCELLED", "run", "CANCELLED"))
        self.assertNotIn("cancellation_failed", record)

    def test_slow_polls_never_outlive_the_run_deadline(self):
        clock = FakeClock()
        requests = []
        state = {"phase": "RUNNING"}

        def call(method, path, body, token, timeout):
            requests.append((path, clock.now + timeout))
            clock.now += min(timeout, 4.0)  # A slow Node: every request takes up to 4 s.
            if path == "/api/state":
                return 200, {"runs": [{"id": "run", "task_id": "task"}], "human_requests": []}
            if path == "/api/runs/run":
                return 200, {"run": {"id": "run", "phase": state["phase"]}}
            if path == "/api/runs/run/control":
                state["phase"] = "CANCELLED"
            return 200, {}

        selected, _ = campaign(["auto"], call, clock)
        record = {}
        with self.assertRaises(supply.runner.DeadlineExceeded):
            selected.claim_and_wait("task", {"revision": 0}, record, clock, clock.sleep)
        cancel = next(index for index, (path, _) in enumerate(requests) if path == "/api/runs/run/control")
        # Every request of the wait ends by the 10 s run timeout; stopping (its read of the Run, then the
        # cancel) gets its own grace period.
        self.assertTrue(all(ends <= 10.0 for _, ends in requests[:cancel - 1]), requests)
        self.assertEqual((record["run_id"], record["stopped_phase"]), ("run", "CANCELLED"))

if __name__ == "__main__":
    unittest.main()
