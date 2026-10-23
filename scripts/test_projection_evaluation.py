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
import types
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import projection_evaluation as projection
from test_fixed_task_evaluation import cli, corpus, full_result, observation


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


def zero_dispatch_control():
    """A real fixed-task manifest with local overflow and an empty physical ledger."""
    value = corpus()
    seed = full_result(value)
    planned = projection.evaluation.plan(value, ["terminal-context-overflow"], ["projection"])
    experiment = copy.deepcopy(seed["experiment"])
    experiment["repeat_count"] = 1
    experiment["variants"] = []
    runs = []
    for index, item in enumerate(planned):
        variant = dict(seed["experiment"]["variants"][0], task=item["task"]["id"], comparison=item["comparison"]["id"],
                       arm=item["arm"], label=item["label"], agent_fields=item["agent_fields"])
        experiment["variants"].append(variant)
        runs.append(projection.evaluation.run_result(value, item, 1,
            observation(run_id=f"zero-control-{index}", phase="FAILED", error="Context window overflow during local fitting",
                        usage_records=[], steps=0, compactions=0), "deterministic", False))
    manifest = cli.assemble(value, experiment, runs, projection.evaluation.feature_availability(value),
                            seed["memory_matrix"], True, None, [], None)
    export = {"schema_version": 1, "experiment_id": experiment["experiment_id"],
              "manifest_sha256": projection.canonical_sha256(manifest), "source": experiment["aidash"],
              "tenant_id": "evaluation", "planned_run_keys": [r["key"] for r in runs],
              "run_inventory": [{"key": r["key"], "run_id": r["run_id"],
                                  "attempt_ids": {table: [] for table in projection.TABLES}} for r in runs],
              "run_recovery": [{"run_id": r["run_id"], "guard_evidence_complete": True, "safety_violation": False,
                                 "control_accepted": True, "local_fitting_failures": 1, "context_reason": "context_unreducible"}
                                for r in runs], **{table: [] for table in projection.TABLES}}
    return manifest, export


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
            "numeric source boolean": change("source", {**self.export["source"], "dirty": 0}),
            "empty Tenant": change("tenant_id", ""),
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
            "boolean projection": lambda e: e["inference_usage"][0].__setitem__("projection_version", True),
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

    def test_summary_without_dispatch_needs_durable_false_journal_evidence(self):
        export = copy.deepcopy(self.export)
        summary = export["context_compaction_attempts"][0]
        identifier = summary["attempt_id"]
        summary.update(provider_dispatched=False, outcome="unauthorized")
        export["inference_usage"] = [r for r in export["inference_usage"] if r["attempt_id"] != identifier]
        export["run_inventory"][0]["attempt_ids"]["inference_usage"].remove(identifier)
        imported = projection.load_evidence(self.manifest, export)
        report = projection.analyze_campaign([imported], configuration(self.manifest))
        self.assertEqual(report["runs"][0]["cost"]["total_nanocredits"], 30)
        self.assertEqual(report["runs"][0]["cost"]["provider_work_count"], 2)
        for state in (None, True, "missing"):
            with self.subTest(state=state):
                corrupt = copy.deepcopy(export)
                row = corrupt["context_compaction_attempts"][0]
                if state == "missing":
                    row.pop("provider_dispatched")
                else:
                    row["provider_dispatched"] = state
                with self.assertRaisesRegex(projection.EvidenceError, "missing its inference"):
                    projection.load_evidence(self.manifest, corrupt)
        contradictory = copy.deepcopy(self.export)
        contradictory["context_compaction_attempts"][0]["provider_dispatched"] = False
        with self.assertRaisesRegex(projection.EvidenceError, "nondispatch contradicts"):
            projection.load_evidence(self.manifest, contradictory)


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


def gate_campaign():
    """Construct normalized imports for the report's own prerequisite boundary.

    Import integrity has the owner-boundary matrix above. Here the statistics
    engine is independently covered and supplies passing bounds, so removing
    one report prerequisite cannot be masked by low-power statistical results.
    These are synthetic test records, never provider performance evidence.
    """
    manifest, export = example()
    seed = projection.load_evidence(manifest, export)
    config = {"candidate": "native", "confirmation_frozen": True, "confirmation_repetitions": 2,
              "pilot_repeats": 10, "deployment_repetitions": 1, "deployment_mode_confirmed": True,
              "independent_blocks": True, "cost_bounds_enforced": True, "experiments": {}, "routes": {},
              "cost_bounds": {}}
    imports = []
    tasks = list(projection.PRIMARY_WEIGHTS) + list(projection.CONTROLS)
    for route in ("automatic", "explicit"):
        model, endpoint = "model-" + route, "endpoint-" + route
        arms = ["legacy", "ordered", "native"] + (["ordered-explicit", "native-explicit"] if route == "explicit" else [])
        config["routes"][route] = {"provider_tag": endpoint, "upstream_model": model, "arms": arms,
            "deployment_estimator": {"id": "calibrated", "version": 2, "confidence": "calibrated"},
            "tokenized_applicable": route == "automatic"}
        if route == "automatic":
            config["routes"][route]["token_encoding"] = "o200k_base"
        config["cost_bounds"][route] = {task: {"legacy": 100, "native": 100} for task in projection.PRIMARY_WEIGHTS}
        for phase, repeats, selected in (("pilot", 10, arms), ("confirmation", 2, ["legacy", "native"]),
                                         ("deployment-mode", 1, ["native"])):
            imported = copy.deepcopy(seed)
            exp = imported["experiment"]
            exp.update(experiment_id=f"fte-{len(imports) + 1:012x}", tier="live", repeat_count=repeats)
            exp["model"]["kind"] = "live-provider"
            exp["aidash"].update(binary_sha256="b" * 64, server_revision="a" * 40)
            exp["variants"] = [{"task": task, "arm": arm} for task in tasks for arm in selected]
            imported["runs"] = []
            config["experiments"][exp["experiment_id"]] = {"route": route, "phase": phase, "condition": "primary"}
            for task in tasks:
                for arm in selected:
                    for repeat in range(1, repeats + 1):
                        run = copy.deepcopy(seed["runs"][0])
                        identifier = f"{route}/{phase}/{task}/{arm}/{repeat}"
                        run.update(key=identifier, run_id=identifier, task=task, arm=arm, repeat=repeat,
                                   projection=arm.split("-")[0], prompt_cache="explicit" if arm.endswith("explicit") else "off",
                                   outcome="success", oracle_passed=True, constraint_retention=True,
                                   context_compaction_attempts=[], decision_attempts=[], memory_model_attempts=[],
                                   generation_embedding_usage=[], recovery={"guard_evidence_complete": True,
                                       "safety_violation": False, "control_accepted": True})
                        row = copy.deepcopy(seed["runs"][0]["inference_usage"][0])
                        row.update(attempt_id=identifier, run_id=identifier, cost_nanocredits=100 if arm == "legacy" else 50,
                                   upstream_cost_nanocredits=None, byte_basis=300, requested_route=endpoint,
                                   upstream_model=model, projection_version={"legacy": 1, "ordered": 2, "native": 3}[arm.split("-")[0]],
                                   cache_identity=None if arm == "legacy" else "stable-identity",
                                   provider_identity={"reported_model": model, "reported_provider": "provider", "reported_route": endpoint})
                        if phase == "deployment-mode":
                            row.update(estimator="calibrated", estimator_version=2, estimate_confidence="calibrated")
                        if route == "automatic":
                            row["shadow_estimates"].append({"snapshot": {"step": 0, "estimator": {
                                "id": "tokenized", "version": 1, "encoding": "o200k_base", "encoding_version": 1}},
                                "estimate": {"tokens": 110, "output_reservation": 20, "byte_basis": 300,
                                             "estimator": "tokenized", "version": 1, "confidence": "tokenized"}})
                        run["inference_usage"] = [row]
                        imported["runs"].append(run)
            imports.append(imported)
    return imports, config


class AdoptionPrerequisiteTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.imports, cls.config = gate_campaign()

    def analyze(self, imports=None, config=None):
        engine = types.SimpleNamespace(quality_bound=lambda rows: {"status": "pass", "bounds": {}},
                                       cost_ratio_bound=lambda rows: {"status": "pass", "bounds": {}})
        with mock.patch.dict(sys.modules, {"projection_statistics": engine}):
            return projection.analyze_campaign(self.imports if imports is None else imports,
                                               self.config if config is None else config)

    def test_complete_registered_phases_and_known_measurements_can_reach_the_gate(self):
        report = self.analyze()
        self.assertEqual(report["decision"]["status"], "pass", report["routes"]["automatic"]["reasons"])
        self.assertFalse(report["decision"]["keep_legacy"])

    def test_complete_zero_dispatch_control_is_not_applicable_without_exempting_unknown_or_primary_work(self):
        manifest, export = zero_dispatch_control()
        evidence = projection.load_evidence(manifest, export)
        report = projection.analyze_campaign([evidence], configuration(manifest))
        for row in report["runs"]:
            self.assertTrue(row["route_complete"])
            self.assertTrue(row["no_provider_dispatch_control"])
            self.assertEqual(row["route_evidence_status"], "not-applicable")
            self.assertEqual(row["measurement_status"], "not-applicable")
            self.assertEqual(row["cache"]["identity_status"], "not-applicable")
            self.assertIsNone(row["cache"]["identity_transitions"])
            self.assertEqual(row["routes"], [])
            self.assertEqual(row["selected_estimators"], [])
            self.assertEqual(row["cost"]["total_nanocredits"], 0)
            self.assertEqual(row["tokens"]["input_tokens"]["known_count"], 0)
            self.assertIsNone(row["tokens"]["input_tokens"]["total"])
        unknown = projection.load_evidence(manifest)
        unknown_report = projection.analyze_campaign([unknown], configuration(manifest))
        self.assertTrue(unknown_report["decision"]["keep_legacy"])
        self.assertTrue(all(not r["no_provider_dispatch_control"] for r in unknown_report["runs"]))
        # Exercise the same public-import control through the full prerequisite
        # gate. Remaining synthetic observations supply the independently tested
        # statistical bounds, not an assertion of live provider performance.
        imports = copy.deepcopy(self.imports)
        ordered = next(r for r in evidence["runs"] if r["projection"] == "ordered")
        index = next(i for i, r in enumerate(imports[0]["runs"]) if r["arm"] == "ordered" and r["task"] == "terminal-context-overflow" and r["repeat"] == 1)
        original = imports[0]["runs"][index]
        replacement = copy.deepcopy(ordered)
        replacement.update({k: original[k] for k in ("key", "run_id", "arm", "repeat", "comparison", "matched_configuration_sha256")})
        imports[0]["runs"][index] = replacement
        self.assertEqual(self.analyze(imports)["decision"]["status"], "pass")
        for phase_import in imports[1:3]:
            # Native controls share the same no-I/O applicability contract in
            # confirmation and deployment, with their independently pinned arm.
            target = next(r for r in phase_import["runs"] if r["projection"] == "native" and r["task"] == "terminal-context-overflow")
            target.update(inference_usage=[], outcome="overflow_failure", oracle_passed=None)
        self.assertEqual(self.analyze(imports)["decision"]["status"], "pass")
        for invalid in ("incomplete export", "control not accepted", "primary completion"):
            with self.subTest(invalid=invalid):
                changed = copy.deepcopy(imports)
                if invalid == "incomplete export":
                    changed[0]["complete_export"] = False
                elif invalid == "control not accepted":
                    changed[0]["runs"][index]["recovery"]["control_accepted"] = None
                else:
                    primary = next(r for r in changed[1]["runs"] if r["task"] in projection.PRIMARY_WEIGHTS)
                    primary["inference_usage"] = []
                self.assertTrue(self.analyze(changed)["decision"]["keep_legacy"])

    def test_confirmation_boolean_cannot_replace_missing_pilot_or_deployment_imports(self):
        for phase in ("pilot", "deployment-mode"):
            with self.subTest(phase=phase):
                imports = [e for e in self.imports if self.config["experiments"][e["experiment"]["experiment_id"]]["phase"] != phase]
                report = self.analyze(imports)
                self.assertEqual(report["decision"]["status"], "inconclusive")
                self.assertTrue(report["decision"]["keep_legacy"])
                self.assertTrue(report["routes"]["automatic"]["phase_completeness"][phase])

    def test_deployment_attempts_must_verify_the_frozen_estimator(self):
        imports = copy.deepcopy(self.imports)
        deployment = next(e for e in imports if self.config["experiments"][e["experiment"]["experiment_id"]]["phase"] == "deployment-mode")
        deployment["runs"][0]["inference_usage"][0].update(estimator="bytes", estimator_version=1, estimate_confidence="conservative")
        report = self.analyze(imports)
        self.assertEqual(report["decision"]["status"], "inconclusive")
        self.assertIn("Deployment-mode attempts do not verify the frozen deployment estimator.", report["routes"]["automatic"]["reasons"])

    def test_controls_must_cover_each_arm_and_each_frozen_repetition(self):
        for missing_arm in ("legacy", "native"):
            with self.subTest(arm=missing_arm):
                imports = copy.deepcopy(self.imports)
                confirmation = imports[1]
                control = "approval-escalation"
                confirmation["runs"] = [r for r in confirmation["runs"] if not (r["arm"] == missing_arm and r["task"] == control)]
                # A self-consistent narrower manifest still cannot erase a frozen control.
                confirmation["experiment"]["variants"] = [v for v in confirmation["experiment"]["variants"] if not (v["arm"] == missing_arm and v["task"] == control)]
                report = self.analyze(imports)
                self.assertEqual(report["decision"]["status"], "inconclusive")
                self.assertTrue(any(f"{missing_arm}/{control} control" in r for r in report["routes"]["automatic"]["phase_completeness"]["confirmation"]))
        imports = copy.deepcopy(self.imports)
        row = next(r for r in imports[1]["runs"] if r["arm"] == "native" and r["task"] in projection.CONTROLS and r["repeat"] == 2)
        row["repeat"] = 3
        self.assertEqual(self.analyze(imports)["decision"]["status"], "inconclusive")

    def test_required_token_estimate_and_shadow_unknowns_block_adoption(self):
        mutations = {"input": lambda r: r.__setitem__("input_tokens", None),
                     "output": lambda r: r.__setitem__("output_tokens", None),
                     "cache-read": lambda r: r.__setitem__("cache_read_tokens", None),
                     "cache-write": lambda r: r.__setitem__("cache_write_tokens", None),
                     "version": lambda r: r.__setitem__("estimator_version", None),
                     "confidence": lambda r: r.__setitem__("estimate_confidence", None),
                     "estimate": lambda r: r.__setitem__("estimated_tokens", None),
                     "shadows": lambda r: r.__setitem__("shadow_estimates", []),
                     "calibrated-shadow": lambda r: r["shadow_estimates"][0]["estimate"].__setitem__("tokens", None),
                     "tokenized-shadow": lambda r: r["shadow_estimates"].pop()}
        for name, mutate in mutations.items():
            with self.subTest(name=name):
                imports = copy.deepcopy(self.imports)
                mutate(imports[1]["runs"][0]["inference_usage"][0])
                report = self.analyze(imports)
                self.assertEqual(report["decision"]["status"], "inconclusive")
                self.assertTrue(report["runs"][len(imports[0]["runs"])]["measurement_gaps"])

    def test_model_identity_needs_exact_or_explicitly_declared_alias_evidence(self):
        for value in (None, "wrong-model", "model-automatic-20261023"):
            with self.subTest(model=value):
                imports = copy.deepcopy(self.imports)
                imports[1]["runs"][0]["inference_usage"][0]["provider_identity"]["reported_model"] = value
                report = self.analyze(imports)
                self.assertEqual(report["decision"]["status"], "inconclusive")
                observed = report["runs"][len(imports[0]["runs"])]["routes"][0]
                self.assertEqual(observed["model_status"], "unknown" if value is None else "mismatch")
        imports, config = copy.deepcopy(self.imports), copy.deepcopy(self.config)
        imports[1]["runs"][0]["inference_usage"][0]["provider_identity"]["reported_model"] = "explicit-canonical-model"
        config["routes"]["automatic"]["canonical_model_ids"] = ["explicit-canonical-model"]
        self.assertEqual(self.analyze(imports, config)["decision"]["status"], "pass")

    def test_candidate_only_pilot_guard_regressions_veto_passing_confirmation(self):
        for guard in ("false_completion", "constraint_loss"):
            with self.subTest(guard=guard):
                imports = copy.deepcopy(self.imports)
                row = next(r for r in imports[0]["runs"] if r["arm"] == "native" and r["task"] == "long-conversation-corrections")
                if guard == "false_completion":
                    row.update(outcome="false_completion", oracle_passed=False)
                else:
                    row["constraint_retention"] = False
                report = self.analyze(imports)
                self.assertEqual(report["decision"]["status"], "fail")
                self.assertTrue(any(guard in reason and "pilot/primary" in reason for reason in report["routes"]["automatic"]["guard_failures"]))

    def test_retained_warm_cache_regression_and_unpaired_deployment_loss_cannot_be_ignored(self):
        imports, config = copy.deepcopy(self.imports), copy.deepcopy(self.config)
        warm = copy.deepcopy(imports[0])
        warm["experiment"].update(experiment_id="fte-000000000099", repeat_count=1)
        warm["experiment"]["variants"] = [{"task": "long-conversation-corrections", "arm": arm} for arm in ("legacy", "native")]
        warm["runs"] = [r for r in warm["runs"] if r["task"] == "long-conversation-corrections" and r["arm"] in ("legacy", "native") and r["repeat"] == 1]
        for row in warm["runs"]:
            row["run_id"] = "warm/" + row["run_id"]
            row["inference_usage"][0]["attempt_id"] = "warm/" + row["inference_usage"][0]["attempt_id"]
            row["inference_usage"][0]["run_id"] = row["run_id"]
            if row["arm"] == "native":
                row.update(outcome="false_completion", oracle_passed=False)
        config["experiments"][warm["experiment"]["experiment_id"]] = {"route": "automatic", "phase": "pilot", "condition": "cross-run-warm"}
        report = self.analyze(imports + [warm], config)
        self.assertEqual(report["decision"]["status"], "fail")
        self.assertTrue(any("pilot/cross-run-warm" in reason for reason in report["routes"]["automatic"]["guard_failures"]))
        imports = copy.deepcopy(self.imports)
        row = next(r for r in imports[2]["runs"] if r["task"] == "long-conversation-corrections")
        row["constraint_retention"] = False
        report = self.analyze(imports)
        self.assertTrue(report["decision"]["keep_legacy"])
        self.assertTrue(any("no unambiguous matched Legacy" in reason for reason in report["routes"]["automatic"]["reasons"]))


if __name__ == "__main__":
    unittest.main()
