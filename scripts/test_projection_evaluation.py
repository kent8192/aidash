"""Offline evidence boundary regressions: corrupted imports must not become wins.

The existing evaluator fixture builds and independently grades the manifest. We
exercise the importer/report public boundary, rather than copying its schemas or
asserting helper implementations. No provider, Docker or credentials are used.
"""
import copy
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import projection_evaluation as projection
from test_fixed_task_evaluation import corpus, full_result


def example():
    manifest = full_result(corpus())
    variants = {(v["task"], v["comparison"], v["arm"]): v for v in manifest["experiment"]["variants"]}
    rows, inventory, recovery, compactions = [], [], [], []
    for index, run in enumerate(manifest["runs"]):
        run["run_id"] = f"run-{index}"
        variant = variants[(run["task"], run["comparison"], run["arm"])]
        version = {"legacy": 1, "ordered": 2, "native": 3}[variant["agent_fields"].get("projection_version", "legacy")]
        identifiers = []
        for ordinal, cost in enumerate((10, 20)):
            identifier = f"attempt-{index}-{ordinal}"
            identifiers.append(identifier)
            rows.append({"attempt_id": identifier, "run_id": run["run_id"], "purpose": "main",
                         "projection_version": version, "created_at": f"2026-10-23T00:00:0{ordinal}Z",
                         "input_tokens": 100, "output_tokens": 5, "cache_read_tokens": 10 + ordinal * 10,
                         "cache_write_tokens": 0, "reasoning_tokens": None,
                         "cost_nanocredits": cost, "upstream_cost_nanocredits": 90,
                         "estimated_tokens": 150, "output_reservation": 20, "estimator": "bytes",
                         "estimator_version": 1, "estimate_confidence": "conservative",
                         "cache_identity": None if version == 1 else f"identity-{ordinal}",
                         "requested_route": "groq", "provider_identity": {"reported_model": "reported-m",
                             "reported_provider": "Groq", "reported_route": "groq", "provider_request_id": identifier},
                         "shadow_estimates": [{"snapshot": {"step": ordinal, "estimator": {"id": "calibrated", "version": 2,
                                                                                               "samples": 25, "ratio_ppm": 300000, "margin_permille": 100}},
                             "estimate": {"tokens": 110, "output_reservation": 20, "estimator": "calibrated",
                                          "version": 2, "confidence": "calibrated", "byte_basis": 300,
                                          "ratio_ppm": 300000}}], "outcome": "completed", "rejection_class": None})
        summary_id = f"summary-{index}"
        identifiers.append(summary_id)
        rows.append({"attempt_id": summary_id, "run_id": run["run_id"], "purpose": "summary",
                     "created_at": "2026-10-23T00:00:02Z", "input_tokens": 40, "output_tokens": 10,
                     "cache_read_tokens": None, "cache_write_tokens": None, "cost_nanocredits": 5,
                     "upstream_cost_nanocredits": 4, "outcome": "completed", "requested_route": "groq",
                     "provider_identity": {"reported_route": "groq"}})
        compactions.append({"attempt_id": summary_id, "run_id": run["run_id"], "stage": "summary",
                            "provider": "m", "outcome": "adopted", "before_tokens": 1000, "after_tokens": 200})
        inventory.append({"key": run["key"], "run_id": run["run_id"], "attempt_ids": {
            "inference_usage": identifiers, "decision_attempts": [], "memory_model_attempts": [],
            "context_compaction_attempts": [summary_id], "generation_embedding_usage": []}})
        recovery.append({"run_id": run["run_id"], "overflow_attempts": 1, "local_fitting_failures": 0,
                         "guard_evidence_complete": True, "safety_violation": False, "control_accepted": True})
    export = {"schema_version": 1, "experiment_id": manifest["experiment"]["experiment_id"],
              "manifest_sha256": projection.canonical_sha256(manifest), "source": manifest["experiment"]["aidash"],
              "tenant_id": "evaluation", "planned_run_keys": [r["key"] for r in manifest["runs"]],
              "run_inventory": inventory, "inference_usage": rows, "decision_attempts": [], "memory_model_attempts": [],
              "context_compaction_attempts": compactions, "run_recovery": recovery, "generation_embedding_usage": []}
    return manifest, export


def configuration(manifest, **overrides):
    config = {"candidate": "ordered", "required_routes": ["automatic", "explicit"],
              "routes": {"automatic": {"provider_tag": "groq"}, "explicit": {"provider_tag": "google-vertex/global"}},
              "experiments": {manifest["experiment"]["experiment_id"]: {"route": "automatic", "phase": "pilot", "condition": "primary", "tenant_id": "evaluation"}},
              "independent_blocks": True, "cost_bounds_enforced": True}
    config.update(overrides)
    return config


class EvidenceImportTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.manifest, cls.export = example()

    def test_scope_corruption_missing_joins_private_fields_and_invalid_counts_are_rejected(self):
        # One public-boundary matrix owns import integrity. Each mutation used to
        # be a plausible way to drop expensive/failed work or invent comparability.
        def change(name, value):
            return lambda export: export.__setitem__(name, value)
        cases = {
            "wrong experiment": change("experiment_id", "fte-ffffffffffff"),
            "wrong source": change("source", {**self.export["source"], "commit": "f" * 40}),
            "wrong digest": change("manifest_sha256", "f" * 64),
            "missing planned Run": lambda e: e["planned_run_keys"].pop(),
            "duplicate planned Run": lambda e: e["planned_run_keys"].append(e["planned_run_keys"][0]),
            "missing inventory": lambda e: e["run_inventory"].pop(),
            "missing attempt": lambda e: e["inference_usage"].pop(),
            "duplicate attempt": lambda e: e["inference_usage"].append(e["inference_usage"][0]),
            "wrong Run join": lambda e: e["inference_usage"][0].__setitem__("run_id", "another-run"),
            "cross Tenant": lambda e: e["inference_usage"][0].__setitem__("tenant", "another-tenant"),
            "private prompt": lambda e: e["inference_usage"][0].__setitem__("prompt", "private input"),
            "nonfinite nested estimate": lambda e: e["inference_usage"][0]["shadow_estimates"][0]["estimate"].__setitem__("tokens", float("nan")),
            "negative count": lambda e: e["inference_usage"][0].__setitem__("input_tokens", -1),
            "boolean money": lambda e: e["inference_usage"][0].__setitem__("cost_nanocredits", True),
            "wrong pinned projection": lambda e: e["inference_usage"][0].__setitem__("projection_version", 99),
            "private shadow": lambda e: e["inference_usage"][0]["shadow_estimates"][0]["estimate"].__setitem__("prompt", "private"),
            "missing recovery": lambda e: e["run_recovery"].pop(),
            "unknown purpose": lambda e: e["inference_usage"][0].__setitem__("purpose", "free-work"),
            "conflicting Summary charge": lambda e: e["context_compaction_attempts"][0].__setitem__("cost_nanocredits", 999),
        }
        for name, corrupt in cases.items():
            with self.subTest(name):
                export = copy.deepcopy(self.export)
                corrupt(export)
                with self.assertRaises(projection.EvidenceError):
                    projection.load_evidence(self.manifest, export)

    def test_raw_json_duplicate_keys_and_nonfinite_manifest_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "operator.json"
            path.write_text('{"schema_version":1,"schema_version":1}')
            with self.assertRaisesRegex(projection.EvidenceError, "duplicate JSON"):
                projection.load_evidence(self.manifest, path)
        manifest = copy.deepcopy(self.manifest)
        manifest["runs"][0]["wall_ms"] = float("inf")
        with self.assertRaisesRegex(projection.EvidenceError, "finite"):
            projection.load_evidence(manifest)

    def test_legacy_unowned_memory_record_cannot_claim_complete_run_scope(self):
        export = copy.deepcopy(self.export)
        export["memory_model_attempts"] = [{"attempt_id": "legacy-memory", "source_run_ids": None, "provider_usage": None}]
        with self.assertRaisesRegex(projection.EvidenceError, "missing or out-of-scope"):
            projection.load_evidence(self.manifest, export)


class AccountingReportTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.manifest, cls.export = example()

    def analyze(self, export=None, **config):
        evidence = projection.load_evidence(self.manifest, self.export if export is None else export)
        return projection.analyze_campaign([evidence], configuration(self.manifest, **config))

    def test_summary_upstream_cache_and_failure_accounting_do_not_inflate_cost_or_tokens(self):
        report = self.analyze()
        count = len(self.manifest["runs"])
        total = report["campaign_all_work"]
        self.assertEqual(total["attempted_runs"], count)
        self.assertEqual(total["total_cost_nanocredits"]["total"], 35 * count)
        self.assertEqual(total["cost_by_purpose"]["main"]["nanocredits"]["total"], 30 * count)
        self.assertEqual(total["cost_by_purpose"]["summary"]["nanocredits"]["total"], 5 * count)
        self.assertEqual(total["tokens"]["input_tokens"]["known"], 240 * count)
        self.assertIn("unresolved", total["outcomes"])
        self.assertLess(total["successes"], count)
        self.assertEqual(total["cost_per_success_denominator"], total["successes"])
        self.assertEqual(total["distinct_provider_work"], 3 * count)
        self.assertTrue(all(r["overflow"]["summary_adopted"] == 1 for r in report["runs"]))

    def test_cache_identity_unknown_and_first_hits_remain_distinct_from_cold_or_history(self):
        report = self.analyze()
        legacy = next(r for r in report["runs"] if r["projection"] == "legacy")
        ordered = next(r for r in report["runs"] if r["projection"] == "ordered")
        self.assertEqual(legacy["cache"]["identity_status"], "not-applicable")
        self.assertIsNone(legacy["cache"]["identity_transitions"])
        self.assertEqual(ordered["cache"]["identity_transitions"], 1)
        self.assertIsNone(ordered["cache"]["history_prefix_transitions"])
        self.assertIsNone(ordered["cache"]["first_is_cold"])
        self.assertEqual(ordered["cache"]["first_attempt"]["cache_read_tokens"]["total"], 10)
        self.assertEqual(ordered["cache"]["later_attempts"]["cache_read_tokens"]["total"], 20)
        export = copy.deepcopy(self.export)
        export["inference_usage"][1]["created_at"] = export["inference_usage"][0]["created_at"]
        ambiguous = self.analyze(export)["runs"][0]["cache"]
        self.assertEqual(ambiguous["attempt_order"], "ambiguous-or-unavailable")
        self.assertIsNone(ambiguous["first_attempt"]["cache_read_tokens"]["total"])

    def test_shadow_errors_compare_input_side_and_preserve_estimator_identities(self):
        run = self.analyze()["runs"][0]
        self.assertEqual(run["estimator_errors"]["bytes"][0]["signed_input_error"], 30)
        self.assertEqual(run["estimator_errors"]["calibrated"][0]["signed_input_error"], -10)
        self.assertTrue(run["estimator_errors"]["calibrated"][0]["underestimated"])
        self.assertFalse(run["estimator_errors"]["calibrated"][0]["selected"])
        self.assertEqual(run["estimator_errors"]["calibrated"][0]["version"], 2)

    def test_unknown_auxiliary_charge_and_unreported_route_never_become_zero_or_requested_identity(self):
        export = copy.deepcopy(self.export)
        first = export["run_inventory"][0]
        first["attempt_ids"]["decision_attempts"] = ["decision"]
        export["decision_attempts"] = [{"attempt_id": "decision", "run_id": first["run_id"], "cost_nanocredits": None}]
        export["inference_usage"][0]["provider_identity"] = None
        report = self.analyze(export)
        run = report["runs"][0]
        self.assertIsNone(run["cost"]["total_nanocredits"])
        self.assertEqual(run["cost"]["units"]["nanocredits"]["known"], 35)
        self.assertEqual(run["cost"]["unknown_work"], 1)
        self.assertEqual(run["routes"][0]["status"], "unknown")
        self.assertIsNone(run["routes"][0]["reported_identity"])
        self.assertIsNone(report["campaign_all_work"]["total_cost_nanocredits"]["total"])
        self.assertTrue(report["decision"]["keep_legacy"])

    def test_shared_memory_charge_is_counted_once_without_fabricated_per_run_allocation(self):
        export = copy.deepcopy(self.export)
        owners = [entry["run_id"] for entry in export["run_inventory"][:2]]
        for entry in export["run_inventory"][:2]:
            entry["attempt_ids"]["memory_model_attempts"].append("shared-memory")
        export["memory_model_attempts"] = [{"attempt_id": "shared-memory", "source_run_ids": owners,
            "operation_id": "reflect", "reserved_cost_micros": 1000000, "policy_cost_micros": 1000000,
            "provider_usage": {"cost_nanocredits": 7, "upstream_cost_nanocredits": 6}}]
        report = self.analyze(export)
        self.assertIsNone(report["runs"][0]["cost"]["total_nanocredits"])
        self.assertIsNone(report["runs"][1]["cost"]["total_nanocredits"])
        self.assertEqual(report["campaign_all_work"]["total_cost_nanocredits"]["total"], 35 * len(self.manifest["runs"]) + 7)
        self.assertEqual(report["campaign_all_work"]["cost_by_purpose"]["memory"]["nanocredits"]["total"], 7)

    def test_incompatible_units_are_preserved_and_cannot_pass_the_cost_gate(self):
        export = copy.deepcopy(self.export)
        entry = export["run_inventory"][0]
        entry["attempt_ids"]["decision_attempts"] = ["usd-call"]
        export["decision_attempts"] = [{"attempt_id": "usd-call", "run_id": entry["run_id"], "cost_amount": .5, "cost_unit": "USD"}]
        report = self.analyze(export)
        self.assertEqual(report["campaign_all_work"]["cost_units"]["USD"]["total"], .5)
        self.assertIsNone(report["campaign_all_work"]["total_cost_nanocredits"]["total"])
        self.assertTrue(report["decision"]["keep_legacy"])

    def test_embedding_reservation_is_unpriced_evidence_not_a_provider_charge(self):
        export = copy.deepcopy(self.export)
        entry = export["run_inventory"][0]
        entry["attempt_ids"]["generation_embedding_usage"] = ["embed-request/embedding"]
        export["generation_embedding_usage"] = [{"attempt_id": "embedding", "run_id": entry["run_id"],
            "request_id": "embed-request", "reserved_tokens": 100, "reported_tokens": 20,
            "cost_nanocredits": None, "accounting_kind": "reservation", "provider_work_unpriced": True}]
        report = self.analyze(export)
        self.assertEqual(report["campaign_all_work"]["embedding_reservations"], 1)
        self.assertEqual(report["campaign_all_work"]["unpriced_embedding_work"], 1)
        self.assertEqual(report["campaign_all_work"]["distinct_provider_work"], 3 * len(self.manifest["runs"]))
        self.assertIsNone(report["campaign_all_work"]["total_cost_nanocredits"]["total"])

    def test_fixtures_incomplete_primary_population_and_report_are_reproducible_and_non_adopting(self):
        report = self.analyze()
        self.assertEqual(report["decision"]["status"], "inconclusive")
        self.assertTrue(report["decision"]["keep_legacy"])
        self.assertFalse(report["routes"]["automatic"]["primary_candidate"]["fixed_weight"]["complete_primary_population"])
        self.assertEqual([s["weight"] for s in report["pilot_statistics"]["routes"][0]["strata"]], [.125, .125, .25, .25, .25])
        encoded = json.dumps(report, sort_keys=True, allow_nan=False)
        markdown = projection.render_report(report)
        self.assertEqual(markdown, projection.render_report(json.loads(encoded)))
        self.assertIn("Keep Legacy", markdown)
        self.assertLess(len(markdown), 30000)
        self.assertNotIn("final_answer", encoded)
        self.assertNotIn("PRICING", encoded)

    def test_duplicate_experiments_or_reused_run_identity_cannot_reuse_pilot_as_confirmation(self):
        evidence = projection.load_evidence(self.manifest, self.export)
        with self.assertRaisesRegex(projection.EvidenceError, "duplicate campaign"):
            projection.analyze_campaign([evidence, evidence], configuration(self.manifest))
        other = copy.deepcopy(evidence)
        other["experiment"]["experiment_id"] = "fte-ffffffffffff"
        with self.assertRaisesRegex(projection.EvidenceError, "reused"):
            projection.analyze_campaign([evidence, other], configuration(self.manifest))

    def test_manifest_without_operator_export_retains_unknown_measurements_and_failed_runs(self):
        evidence = projection.load_evidence(self.manifest)
        report = projection.analyze_campaign([evidence], configuration(self.manifest))
        self.assertEqual(report["campaign_all_work"]["attempted_runs"], len(self.manifest["runs"]))
        self.assertIsNone(report["campaign_all_work"]["total_cost_nanocredits"]["total"])
        self.assertIsNone(report["runs"][0]["overflow"]["provider_rejections"])
        self.assertEqual(report["campaign_all_work"]["estimator_errors"]["tokenized"]["status"], "unknown-not-reported")
        self.assertEqual(report["decision"]["status"], "inconclusive")


if __name__ == "__main__":
    unittest.main()
