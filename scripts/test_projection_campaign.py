"""Campaign admission and durable spend boundaries, without credentials or providers."""
import datetime
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import projection_campaign as campaign


class SharedBudgetTest(unittest.TestCase):
    def setUp(self):
        self.now = datetime.datetime(2026, 10, 23, tzinfo=datetime.timezone.utc)
        self.budget = campaign.SharedBudget.create("a" * 64, 100, 60, now=self.now)

    def test_all_phases_share_reserved_and_uncertain_spend(self):
        self.budget.reserve("pilot", "automatic", 70, 10, "private/first.json", now=self.now)
        self.budget.finish("pilot", "automatic", exit_code=1, now=self.now)
        self.assertFalse(self.budget.can_fit_phase(31, 1, now=self.now))
        self.budget.reserve("pilot", "explicit", 30, 10, "private/second.json", now=self.now)
        self.assertEqual(self.budget.remaining_nanocredits, 0)
        with self.assertRaisesRegex(campaign.CampaignRefusal, "budget"):
            self.budget.reserve("confirmation", "automatic", 1, 1, "private/third.json", now=self.now)

    def test_resume_keeps_earlier_reservations_and_refuses_a_repeat(self):
        self.budget.reserve("pilot", "automatic", 60, 10, "private/result.json", now=self.now)
        restored = campaign.SharedBudget.from_document(self.budget.document(), "a" * 64, now=self.now)
        self.assertEqual(restored.remaining_nanocredits, 40)
        with self.assertRaisesRegex(campaign.CampaignRefusal, "already reserved"):
            restored.reserve("pilot", "automatic", 1, 1, "private/another.json", now=self.now)
        with self.assertRaisesRegex(campaign.CampaignRefusal, "fingerprint"):
            campaign.SharedBudget.from_document(self.budget.document(), "b" * 64, now=self.now)

    def test_deadline_and_backwards_clock_never_release_headroom(self):
        late = self.now + datetime.timedelta(seconds=61)
        self.assertFalse(self.budget.can_fit_phase(1, 1, now=late))
        with self.assertRaisesRegex(campaign.CampaignRefusal, "clock"):
            self.budget.can_fit_phase(1, 1, now=self.now - datetime.timedelta(seconds=1))

    def test_corrupt_or_nonfinite_accounting_is_refused(self):
        for amount in (True, -1, float("nan"), float("inf")):
            with self.subTest(amount=amount), self.assertRaises(campaign.CampaignRefusal):
                self.budget.reserve("pilot", "automatic", amount, 1, "private/result.json", now=self.now)
        document = self.budget.document()
        document["reservations"] = [{"phase": "pilot", "route": "automatic", "nanocredits": 101,
                                      "seconds": 1, "output": "x", "status": "reserved", "exit_code": None}]
        with self.assertRaisesRegex(campaign.CampaignRefusal, "ceiling"):
            campaign.SharedBudget.from_document(document, "a" * 64, now=self.now)

    def test_exclusive_state_lock_and_private_output(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "state.json"
            with campaign.state_lock(path):
                campaign.write_json(path, self.budget.document(), exclusive=True)
                with self.assertRaisesRegex(campaign.CampaignRefusal, "locked"):
                    with campaign.state_lock(path):
                        self.fail("second writer acquired the campaign")
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            self.assertEqual(json.loads(path.read_text())["plan_sha256"], "a" * 64)
            with self.assertRaises(FileExistsError):
                campaign.write_json(path, {}, exclusive=True)


class ConfigurationTest(unittest.TestCase):
    def test_frozen_config_has_exact_three_and_five_arms_and_shared_caps(self):
        config = campaign.load_config()
        self.assertEqual(config["routes"]["automatic"]["arms"], ["legacy", "ordered", "native"])
        self.assertEqual(len(config["routes"]["explicit"]["arms"]), 5)
        self.assertEqual(config["limits"]["max_spend_nanocredits"], 50_000_000_000)
        self.assertEqual(config["limits"]["max_wall_seconds"], 14400)

    def test_config_rejects_limit_changes_and_unmatched_arms(self):
        config = campaign.load_config()
        for update in ({"max_wall_seconds": float("nan")}, {"max_spend_nanocredits": 50_000_000_001}):
            value = json.loads(json.dumps(config))
            value["limits"].update(update)
            with self.assertRaises(campaign.CampaignRefusal):
                campaign.validate_config(value)
        config["routes"]["automatic"]["arms"].append("native-explicit")
        with self.assertRaisesRegex(campaign.CampaignRefusal, "arms"):
            campaign.validate_config(config)


class CampaignBoundaryTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.corpus = campaign.evaluation.load_corpus()
        cls.plan = campaign.make_plan(campaign.load_config(), cls.corpus,
            source={"commit": "a" * 40, "dirty": False, "dirty_diff_sha256": None,
                    "binary_sha256": None, "server_revision": None})

    def test_complete_phase_is_reserved_before_first_dispatch_and_failure_survives_restart(self):
        now = datetime.datetime(2026, 10, 23, tzinfo=datetime.timezone.utc)
        costs = {route: {"nanocredits": 25, "seconds": 10} for route in campaign.ARMS}
        calls = []
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory) / "state.json"
            def child(command, timeout):
                durable = campaign.read_json(state)
                self.assertEqual(len(durable["reservations"]), 2)
                self.assertTrue(all(x["status"] == "reserved" for x in durable["reservations"]))
                calls.append(command)
                return 7
            with mock.patch.object(campaign, "check_execution", return_value=costs), \
                    mock.patch.object(campaign, "execution_command", return_value=["unused"]):
                with self.assertRaisesRegex(campaign.CampaignRefusal, "stopped with exit 7"):
                    campaign.run_phase(self.plan, {}, state_path=state,
                        corpus_path=campaign.evaluation.EVALUATION / "corpus.toml",
                        output_dir=directory, invoke_child=child, now=lambda: now)
                durable = campaign.read_json(state)
                self.assertEqual([x["status"] for x in durable["reservations"]], ["uncertain", "reserved"])
                with self.assertRaisesRegex(campaign.CampaignRefusal, "unresolved"):
                    campaign.run_phase(self.plan, {}, state_path=state,
                        corpus_path=campaign.evaluation.EVALUATION / "corpus.toml",
                        output_dir=directory, invoke_child=child, now=lambda: now)
            self.assertEqual(len(calls), 1)

    def test_complete_phase_over_budget_or_deadline_never_starts_child(self):
        for costs in ({route: {"nanocredits": campaign.MAX_NANOCREDITS, "seconds": 1}
                       for route in campaign.ARMS},
                      {route: {"nanocredits": 1, "seconds": campaign.MAX_SECONDS}
                       for route in campaign.ARMS}):
            with self.subTest(costs=costs), tempfile.TemporaryDirectory() as directory, \
                    mock.patch.object(campaign, "check_execution", return_value=costs):
                child = mock.Mock(side_effect=AssertionError("provider dispatch must not start"))
                with self.assertRaisesRegex(campaign.CampaignRefusal, "complete phase cannot fit"):
                    campaign.run_phase(self.plan, {}, state_path=Path(directory) / "state.json",
                        corpus_path=campaign.evaluation.EVALUATION / "corpus.toml",
                        output_dir=directory, invoke_child=child)
                child.assert_not_called()

    def test_source_and_complete_trial_population_cannot_be_relabelled(self):
        plan = self.plan
        route = "automatic"
        variants = [dict(task=t["task"], comparison=plan["routes"][route]["comparison"], arm=t["arm"])
                    for t in plan["routes"][route]["trials"] if t["repeat"] == 1]
        manifest = {"experiment": {"corpus": plan["corpus"], "evaluator": plan["evaluator"],
                    "aidash": dict(plan["source"], server_revision="a" * 40), "tier": "live",
                    "repeat_count": 10, "variants": variants}}
        campaign.validate_result_plan(manifest, plan, route)
        for mutate, message in ((lambda e: e["aidash"].update(commit="b" * 40), "source"),
                                (lambda e: e["variants"].pop(), "inventory"),
                                (lambda e: e.update(repeat_count=9), "repeats"),
                                (lambda e: e["aidash"].update(server_revision="b" * 40), "deployed")):
            value = copy.deepcopy(manifest)
            mutate(value["experiment"])
            with self.subTest(message=message), self.assertRaisesRegex(campaign.CampaignRefusal, message):
                campaign.validate_result_plan(value, plan, route)

    def test_source_drift_refuses_before_manifest_admission(self):
        with mock.patch.object(campaign.evaluation, "source_identity",
                               return_value={"commit": "b" * 40, "dirty": False}):
            with self.assertRaisesRegex(campaign.CampaignRefusal, "clean source"):
                campaign.check_execution(self.plan, self.corpus, {})

    def test_terminal_reservation_outcome_cannot_be_rewritten(self):
        now = datetime.datetime(2026, 10, 23, tzinfo=datetime.timezone.utc)
        budget = campaign.SharedBudget.create("a" * 64, 100, 60, now=now)
        budget.reserve("pilot", "automatic", 20, 10, "output", now=now)
        budget.finish("pilot", "automatic", exit_code=1, now=now)
        with self.assertRaisesRegex(campaign.CampaignRefusal, "already terminal"):
            budget.finish("pilot", "automatic", exit_code=0, now=now)

    def test_elapsed_deadline_between_routes_never_starts_second_child(self):
        current = datetime.datetime(2026, 10, 23, tzinfo=datetime.timezone.utc)
        costs = {route: {"nanocredits": 1, "seconds": 10} for route in campaign.ARMS}
        calls = []
        def child(command, timeout):
            nonlocal current
            calls.append(command)
            current += datetime.timedelta(seconds=campaign.MAX_SECONDS + 1)
            return 0
        with tempfile.TemporaryDirectory() as directory, \
                mock.patch.object(campaign, "check_execution", return_value=costs), \
                mock.patch.object(campaign, "execution_command", return_value=["unused"]):
            with self.assertRaisesRegex(campaign.CampaignRefusal, "deadline"):
                campaign.run_phase(self.plan, {}, state_path=Path(directory) / "state.json",
                    corpus_path=campaign.evaluation.EVALUATION / "corpus.toml",
                    output_dir=directory, invoke_child=child, now=lambda: current)
        self.assertEqual(calls, [["unused"]])

    def test_actual_approval_and_ledger_bind_cost_bounds_before_analysis(self):
        with tempfile.TemporaryDirectory() as directory:
            execution = {"plan_sha256": self.plan["plan_sha256"], "routes": {}}
            for route, settings in self.plan["routes"].items():
                mapping, models, prices = {}, {}, {}
                for name in settings["corpus_models"]:
                    ref = f"{route}-{name}@1.0.0"
                    mapping[name] = ref
                    models[ref] = dict(self.corpus["models"][name], provider="openrouter",
                        provider_credential="openrouter", provider_route={"tag": settings["provider_tag"]},
                        model_id=settings["upstream_model"], reasoning_effort="medium",
                        request_estimator="bytes", cache_mode=settings["cache_mode"])
                    prices[ref] = {"input_nanocredits_per_token": 1, "output_nanocredits_per_token": 1,
                                   "cache_write_nanocredits_per_token": 1}
                source = dict(self.plan["source"], binary_sha256="b" * 64, server_revision="a" * 40)
                approval = {"external_compaction": "disabled", "prices": prices,
                    "campaign_plan_sha256": self.plan["plan_sha256"],
                    "source_attestation_sha256": campaign.digest(source),
                    "model_config_digests": {ref: campaign.digest(model) for ref, model in models.items()}}
                path = Path(directory) / f"{route}.json"
                campaign.write_json(path, approval, exclusive=True)
                execution["routes"][route] = {"models": models, "model_refs": mapping, "prices": prices,
                    "provider_credential": "pc-concrete-id", "disclosure_approval": str(path),
                    "upper_wall_seconds": len(settings["trials"]) * 180}
            costs = campaign.phase_reservations(self.plan, self.corpus, execution)
            route, entry = "automatic", execution["routes"]["automatic"]
            manifest = {"experiment": {"aidash": source,
                "harness": {"disclosure_approval": campaign.digest(campaign.read_json(entry["disclosure_approval"]))},
                "model": {"registrations": {name: {"config_sha256": campaign.digest(entry["models"][ref])}
                                               for name, ref in entry["model_refs"].items()}},
                "limits": {"run_timeout_seconds": 180, "wall_seconds": entry["upper_wall_seconds"]}},
                "spend": {"ceiling_nanocredits": costs[route]["nanocredits"], "attempts": [
                    {"key": key, "reserved": bound} for key, bound in costs[route]["run_bounds"].items()]}}
            bounds = campaign.analysis_reservation_evidence(manifest, self.plan, route, execution, self.corpus)
            self.assertEqual(set(bounds), set(self.plan["routes"][route]["task_ids"]))
            corrupt = copy.deepcopy(manifest)
            corrupt["spend"]["attempts"][0]["reserved"] = 1
            with self.assertRaisesRegex(campaign.CampaignRefusal, "statistical cost bound"):
                campaign.analysis_reservation_evidence(corrupt, self.plan, route, execution, self.corpus)
            # The wall-time bound is derived from this sequential runner. An
            # operator cannot declare a one-second phase to bypass admission.
            execution["routes"][route]["upper_wall_seconds"] = 1
            with self.assertRaisesRegex(campaign.CampaignRefusal, "sequential"):
                campaign.phase_reservations(self.plan, self.corpus, execution)

    def test_deployment_trials_preserve_frozen_candidate_and_campaign(self):
        plan = copy.deepcopy(self.plan)
        plan.update(phase="confirmation", candidate="native", repeats=125,
                    candidate_arms={"automatic": "native", "explicit": "native-explicit"},
                    campaign_plan_sha256=self.plan["plan_sha256"])
        plan.pop("plan_sha256")
        plan["plan_sha256"] = campaign.digest(plan)
        deployment = campaign.deployment_plan(plan, {"id": "calibrated", "version": 2,
                                                     "confidence": "calibrated"})
        self.assertEqual(deployment["phase"], "deployment-mode")
        self.assertEqual(deployment["candidate_arms"], plan["candidate_arms"])
        self.assertEqual(deployment["campaign_plan_sha256"], self.plan["plan_sha256"])
        self.assertEqual(deployment["confirmation_plan_sha256"], plan["plan_sha256"])
        self.assertEqual(deployment["repeats"], 125)
        self.assertNotEqual(deployment["plan_sha256"], plan["plan_sha256"])
        campaign.validate_plan(deployment)


if __name__ == "__main__":
    unittest.main()
