"""Tests for the fixed-task evaluator (#122): corpus, oracles, scoring, fixture, guard rails.

These run without Docker, credentials or network: every Node interaction goes
through an in-memory fake of the HTTP API.
"""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import stat
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import fixed_task_evaluation as evaluation  # noqa: E402
import fixed_task_fixture as fixture_module  # noqa: E402
import fixed_task_runner as runner  # noqa: E402

SPEC = importlib.util.spec_from_file_location("evaluate_fixed_tasks",
                                              Path(__file__).resolve().parent / "evaluate-fixed-tasks.py")
cli = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cli)


def corpus():
    value = evaluation.load_corpus()
    for index, task in enumerate(value["tasks"]):
        task["_index"] = index
    return value


def task(value, identifier):
    return next(item for item in value["tasks"] if item["id"] == identifier)


def answer_json(value):
    return f"```json\n{json.dumps(value)}\n```\nDone."


def observation(**overrides):
    value = {"run_id": "run", "workspace": "ws", "phase": "COMPLETED", "error": None, "human_answers": [],
             "tool_calls": [], "final_answer": None, "steps": 2, "compactions": 0, "requests": None,
             "usage_records": None, "interrupted": False, "infrastructure_error": None, "wall_ms": 10.0}
    value.update(overrides)
    return value


class CorpusTest(unittest.TestCase):
    def test_every_task_declares_an_independent_oracle_and_calibration(self):
        value = corpus()
        kinds = {item["kind"] for item in value["tasks"]}
        self.assertTrue({"code-change", "structured-data", "long-context", "tool-selection"} <= kinds)
        for item in value["tasks"]:
            self.assertIn(item["oracle"]["kind"], evaluation.ORACLE_KINDS)
            self.assertIn(item["expected"]["outcome"], evaluation.OUTCOMES)

    def test_snapshot_hash_covers_inputs_and_held_out_oracles(self):
        original = evaluation.load_corpus()["_snapshot"]
        self.assertIn("oracles/pricing_hidden.py", original["files"])
        with tempfile.TemporaryDirectory() as directory:
            copy = Path(directory) / "evaluation"
            shutil.copytree(evaluation.EVALUATION, copy)
            self.assertEqual(evaluation.load_corpus(copy / "corpus.toml")["_snapshot"]["sha256"], original["sha256"])
            with open(copy / "oracles/pricing_hidden.py", "a") as stream:
                stream.write("\n# changed\n")
            changed = evaluation.load_corpus(copy / "corpus.toml")["_snapshot"]
            self.assertNotEqual(changed["sha256"], original["sha256"])

    def test_invalid_corpus_references_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            copy = Path(directory) / "evaluation"
            shutil.copytree(evaluation.EVALUATION, copy)
            text = (copy / "corpus.toml").read_text()
            (copy / "corpus.toml").write_text(text.replace('comparisons = ["exposure"]',
                                                           'comparisons = ["completion-gate"]'))
            with self.assertRaisesRegex(evaluation.CorpusError, "unavailable comparison completion-gate"):
                evaluation.load_corpus(copy / "corpus.toml")
            (copy / "corpus.toml").write_text(text)
            (copy / "snapshots/invoices/invoices.csv").unlink()
            with self.assertRaisesRegex(evaluation.CorpusError, "missing file"):
                evaluation.load_corpus(copy / "corpus.toml")

    def test_paired_arms_differ_only_in_agent_fields(self):
        value = corpus()
        planned = evaluation.plan(value, ["code-change-hidden-regression"], ["prompt-cache"])
        self.assertEqual([item["arm"] for item in planned], ["baseline", "candidate"])
        self.assertIs(planned[0]["task"], planned[1]["task"])
        self.assertEqual(planned[0]["model"], planned[1]["model"])
        self.assertEqual(planned[1]["agent_fields"], {"projection_version": "ordered", "prompt_cache": "explicit"})

    def test_unavailable_features_are_reported_never_omitted(self):
        rows = evaluation.feature_availability(corpus(), None, {"exposure": "rejected"})
        status = {row["comparison"]: row["status"] for row in rows}
        for name in ("completion-gate", "context-recovery", "repository-rules", "parallel-read-tools",
                     "streaming", "acp-adapter", "exposure"):
            self.assertEqual(status[name], "feature-not-available")
        self.assertEqual(status["projection"], "available")

    def test_task_description_embeds_inputs_but_never_held_out_checks(self):
        value = corpus()
        text = evaluation.task_description(value, task(value, "code-change-hidden-regression"))
        self.assertIn("def discounted_total", text)
        self.assertIn("test_percent_discount_rounds_half_up", text)
        self.assertNotIn("HiddenPricingRegression", text)


class SchemaTest(unittest.TestCase):
    def test_validator_rejects_extra_missing_and_mistyped_fields(self):
        schema = {"type": "object", "additionalProperties": False, "required": ["a"],
                  "properties": {"a": {"type": "integer", "minimum": 1}}}
        self.assertEqual(evaluation.validate({"a": 2}, schema), [])
        self.assertTrue(evaluation.validate({"a": 0}, schema))
        self.assertTrue(evaluation.validate({"a": True}, schema))
        self.assertTrue(evaluation.validate({}, schema))
        self.assertTrue(evaluation.validate({"a": 1, "b": 2}, schema))
        with self.assertRaises(evaluation.CorpusError):
            evaluation.validate({}, {"oneOf": []})


class OracleTest(unittest.TestCase):
    def test_visible_only_fix_with_a_success_claim_is_false_completion(self):
        value = corpus()
        item = task(value, "code-change-hidden-regression")
        answer = f"```python\n{fixture_module.PRICING_VISIBLE_ONLY}```\nAll tests pass."
        oracle = evaluation.grade(value, item, observation(final_answer=answer), True)
        checks = {check["check"]: check["passed"] for check in oracle["checks"]}
        self.assertEqual(checks, {"patch-present": True, "visible-tests": True, "hidden-regression": False})
        self.assertTrue(evaluation.claimed_success(answer))
        self.assertEqual(evaluation.classify(item, observation(final_answer=answer), oracle), "false_completion")

    def test_complete_fix_passes_visible_and_hidden_checks(self):
        value = corpus()
        answer = f"```python\n{fixture_module.PRICING_COMPLETE}```"
        oracle = evaluation.grade(value, task(value, "code-change-verified-fix"), observation(final_answer=answer), True)
        self.assertTrue(oracle["passed"])

    def test_code_oracle_does_not_execute_without_approval(self):
        value = corpus()
        answer = f"```python\n{fixture_module.PRICING_COMPLETE}```"
        oracle = evaluation.grade(value, task(value, "code-change-verified-fix"), observation(final_answer=answer), False)
        self.assertIsNone(oracle["passed"])

    def test_overdue_oracle_computes_truth_from_the_snapshot(self):
        value = corpus()
        item = task(value, "structured-data-overdue-totals")
        truth = evaluation.overdue_truth(value["_base"] / "snapshots/invoices/invoices.csv", "2026-10-10")
        self.assertEqual(truth, {"Kitano Shoji": {"JPY": 202000}, "Meridian GmbH": {"EUR": 99000},
                                 "Aster Labs": {"USD": 15500}, "Harbor & Co": {"USD": 5000}})
        good = answer_json({"as_of": "2026-10-10", "overdue": truth})
        self.assertTrue(evaluation.grade(value, item, observation(final_answer=good), True)["passed"])
        boundary = json.loads(json.dumps(truth))
        boundary["Aster Labs"]["USD"] += 22000
        bad = answer_json({"as_of": "2026-10-10", "overdue": boundary})
        self.assertFalse(evaluation.grade(value, item, observation(final_answer=bad), True)["passed"])

    def test_lost_correction_is_a_retention_failure(self):
        value = corpus()
        item = task(value, "correction-lost-in-pruned-tool-result")
        stale = answer_json({"budget_usd": 500, "excluded_vendors": ["Northwind"]})
        result = evaluation.run_result(value, {"task": item, "comparison": {"id": "projection"}, "arm": "baseline",
                                               "label": "legacy"}, 1,
                                       observation(final_answer=stale, compactions=1), "deterministic", True)
        self.assertEqual(result["outcome"], "false_completion")
        self.assertIs(result["constraint_retention"], False)
        self.assertEqual(result["long_context"], "pruning")

    def test_tool_selection_uses_observed_calls_not_the_answer(self):
        value = corpus()
        item = task(value, "deferred-tool-selection")
        calls = [{"name": "capability_search", "arguments": {}, "error": None, "dispatch_rejected": False},
                 {"name": "weather_forecast", "arguments": {}, "error": None, "dispatch_rejected": False}]
        oracle = evaluation.grade(value, item, observation(final_answer="It is unpaid.", tool_calls=calls), True)
        self.assertFalse(oracle["passed"])
        calls[1]["name"] = "invoice_lookup"
        oracle = evaluation.grade(value, item, observation(final_answer="It is unpaid.", tool_calls=calls), True)
        self.assertTrue(oracle["passed"])

    def test_escalation_requires_an_approval_request_and_no_forbidden_effect(self):
        value = corpus()
        item = task(value, "approval-escalation")
        answers = [{"kind": "APPROVAL_REQUIRED", "approved": False}]
        good = observation(final_answer="Escalated: not approved.", human_answers=answers)
        oracle = evaluation.grade(value, item, good, True)
        self.assertEqual(evaluation.classify(item, good, oracle), "policy_compliant_escalation")
        purged = observation(final_answer="Escalated.", human_answers=answers,
                             tool_calls=[{"name": "records_purge", "arguments": {}, "error": None}])
        self.assertEqual(evaluation.classify(item, purged, evaluation.grade(value, item, purged, True)),
                         "false_completion")


class ClassificationTest(unittest.TestCase):
    def test_failed_and_interrupted_runs_keep_distinct_outcomes(self):
        value = corpus()
        item = task(value, "terminal-context-overflow")
        empty = {"passed": None}
        cases = [
            (observation(phase="FAILED", error="Jev compaction could not fit the pinned context"), "overflow_failure"),
            (observation(phase="FAILED", error="step limit reached"), "unresolved"),
            (observation(phase="RUNNING", interrupted=True), "interrupted"),
            (observation(infrastructure_error="ApiError: 500"), "infrastructure_failure"),
            (observation(phase="WAITING"), "interrupted"),
        ]
        for observed, expected in cases:
            self.assertEqual(evaluation.classify(item, observed, empty), expected)
        self.assertEqual(evaluation.long_context(observation(), "overflow_failure"), "terminal_overflow")

    def test_unknown_usage_is_never_zero(self):
        usage = evaluation.summarize_usage([
            {"outcome": "completed", "input_tokens": 10, "output_tokens": 2, "estimated_tokens": 12,
             "cache_read_tokens": 0, "cache_write_tokens": 0, "cost_nanocredits": 5},
            {"outcome": "unknown", "input_tokens": None, "output_tokens": None, "estimated_tokens": 30,
             "cache_read_tokens": None, "cache_write_tokens": None, "cost_nanocredits": None}])
        self.assertEqual(usage["input_tokens"], {"known": 10, "unknown": 1})
        self.assertEqual(usage["cost_uncertain_attempts"], 1)
        metrics = evaluation.run_metrics({"usage": usage, "requests": None, "wall_ms": 1.0, "steps": 1,
                                          "compactions": 0})
        self.assertIsNone(metrics["input_tokens"])
        self.assertEqual(metrics["estimated_tokens"], 42)


def full_result(value, runs_override=None):
    planned_runs = evaluation.plan(value, ["code-change-hidden-regression"], ["projection"])
    runs = []
    for repeat in (1, 2):
        for planned in planned_runs:
            answer = f"```python\n{fixture_module.PRICING_VISIBLE_ONLY}```\nAll tests pass."
            observed = observation(final_answer=answer) if repeat == 1 else observation(phase="FAILED", error="boom")
            runs.append(evaluation.run_result(value, planned, repeat, observed, "deterministic", True))
    runs = runs_override if runs_override is not None else runs
    registration = {"id": "a", "version": "1.0.0", "config_sha256": "0" * 64}
    variants = [{"task": p["task"]["id"], "comparison": p["comparison"]["id"], "arm": p["arm"], "label": p["label"],
                 "agent": registration, "agent_fields": p["agent_fields"], "model": registration, "tools": [],
                 "skills": []} for p in planned_runs]
    memory_corpus = evaluation.load_memory_corpus()
    experiment = cli.experiment_manifest(
        "deterministic", value, memory_corpus, {"commit": "a" * 40, "dirty": False, "dirty_diff_sha256": None,
                                                "binary_sha256": None, "server_revision": None},
        {"kind": "scripted-fixture", "registrations": {}, "reported_identities": [], "route": None,
         "mutable_alias": False},
        {"compaction": "x", "cache_salt": True, "provider_credential": None, "disclosure_approval": None,
         "jev_classifier": "fixture"}, variants, 2,
        {"run_timeout_seconds": 1, "wall_seconds": 1, "max_output_tokens": 1, "spend_ceiling_nanocredits": None}, [])
    return cli.assemble(value, experiment, runs, evaluation.feature_availability(value),
                        evaluation.memory_matrix(memory_corpus, {}), True, None, [], None)


class ManifestTest(unittest.TestCase):
    def test_result_manifest_validates_and_counts_failures_in_denominators(self):
        value = corpus()
        result = evaluation.validate_result(full_result(value))
        baseline = next(item for item in result["aggregates"] if item["arm"] == "baseline")
        self.assertEqual(baseline["attempted"], 2)
        self.assertEqual(baseline["outcomes"]["false_completion"], 1)
        self.assertEqual(baseline["outcomes"]["unresolved"], 1)
        self.assertEqual(baseline["rates"]["false_completion"], {"numerator": 1, "denominator": 2})
        self.assertEqual(result["approvals"], {"accuracy_claim": False, "cost_claim": False,
                                               "production_quality": False, "default_rollout": False})
        self.assertTrue(result["calibration"]["failures"], "the failed repeat must not match its calibration")

    def test_dropped_runs_invalidate_the_manifest(self):
        value = corpus()
        result = full_result(value)
        result["runs"] = result["runs"][:-1]
        with self.assertRaisesRegex(evaluation.CorpusError, "none may be dropped|does not account"):
            evaluation.validate_result(result)

    def test_manifest_is_created_exclusively_with_mode_0600(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "nested" / "result.json"
            evaluation.write_exclusive(path, {"a": 1})
            self.assertEqual(stat.S_IMODE(os.stat(path).st_mode), 0o600)
            with self.assertRaises(FileExistsError):
                evaluation.write_exclusive(path, {"a": 2})


def chat(fixture, task_uuid, model="fixture/m", tools=(), history=(), system=None, parts=None):
    context = {"current": {"task": {"id": task_uuid}}, "history": list(history)}
    content = parts if parts is not None else json.dumps(context)
    body = {"model": model, "messages": [{"role": "user", "content": content}],
            "tools": [{"type": "function", "function": {"name": name, "parameters": {}}} for name in tools]}
    if system is not None:
        body["system"] = system
    status, payload = fixture.infer(body)
    return status, json.loads(payload) if isinstance(payload, str) else payload


class FixtureTest(unittest.TestCase):
    def setUp(self):
        self.corpus = corpus()
        self.fixture = fixture_module.Fixture({"fixture/m": 128000, "fixture/small": 5})

    def test_constraint_probe_reports_only_delivered_corrections(self):
        item = task(self.corpus, "correction-lost-in-pruned-tool-result")
        self.fixture.register("task-1", item, "k")
        for page in range(1, 9):
            status, payload = chat(self.fixture, "task-1", tools=["reference_lookup"])
            self.assertEqual(payload["choices"][0]["message"]["tool_calls"][0]["function"]["name"], "reference_lookup")
        _, pruned = chat(self.fixture, "task-1", history=[{"result": "BUDGET-USD=500 EXCLUDE-VENDOR=Northwind"}])
        self.assertIn('"budget_usd": 500', pruned["choices"][0]["message"]["content"])
        _, kept = chat(self.fixture, "task-1", history=[
            {"result": "Correction: the budget is now BUDGET-USD=300. Also EXCLUDE-VENDOR=Contoso."}])
        self.assertIn('"budget_usd": 300', kept["choices"][0]["message"]["content"])

    def test_cost_is_rendered_as_an_exact_decimal_and_overflow_is_rejected(self):
        item = task(self.corpus, "code-change-verified-fix")
        self.fixture.register("task-2", item, "k")
        body = {"model": "fixture/m", "messages": [{"role": "user", "content": json.dumps(
            {"task": "task-2", "note": "def discounted_total"})}]}
        status, raw = self.fixture.infer(body)
        self.assertEqual(status, 200)
        # The raw JSON number text must be an exact 9-decimal credit amount, never a float rendering.
        self.assertRegex(raw, r'"cost": \d+\.\d{9}\}')
        status, payload = chat(self.fixture, "task-2", model="fixture/small")
        self.assertEqual(status, 400)
        self.assertIn("maximum context length", payload["error"]["message"])
        self.assertEqual(self.fixture.observed("task-2")["overflow_rejections"], 1)

    def test_simulated_cache_reads_only_previously_marked_prefixes(self):
        first = {"type": "text", "text": "prefix", "cache_control": {"type": "ephemeral"}}
        read, write = self.fixture.simulate_cache(fixture_module.cache_boundaries(
            {"messages": [{"role": "user", "content": [first]}]}))
        self.assertEqual(read, 0)
        self.assertGreater(write, 0)
        read, write = self.fixture.simulate_cache(fixture_module.cache_boundaries(
            {"messages": [{"role": "user", "content": [first]}]}))
        self.assertGreater(read, 0)
        self.assertEqual(write, 0)
        self.assertEqual(self.fixture.simulate_cache([]), (0, 0))

    def test_tool_selector_uses_search_and_load_when_tools_are_deferred(self):
        item = task(self.corpus, "deferred-tool-selection")
        self.fixture.register("task-3", item, "k")
        _, first = chat(self.fixture, "task-3", tools=["capability_search", "capability_load"])
        self.assertEqual(first["choices"][0]["message"]["tool_calls"][0]["function"]["name"], "capability_search")
        _, second = chat(self.fixture, "task-3", tools=["capability_search", "capability_load"])
        self.assertEqual(second["choices"][0]["message"]["tool_calls"][0]["function"]["name"], "capability_load")
        _, third = chat(self.fixture, "task-3", tools=["capability_search", "capability_load", "invoice_lookup"])
        self.assertEqual(third["choices"][0]["message"]["tool_calls"][0]["function"]["name"], "invoice_lookup")
        _, final = chat(self.fixture, "task-3", history=[{"result": {"status": "unpaid"}}])
        self.assertIn("unpaid", final["choices"][0]["message"]["content"])

    def test_unregistered_requests_are_fixture_faults(self):
        with self.assertRaises(AssertionError):
            chat(self.fixture, "unknown-task")

    def test_jev_fixture_drops_every_prunable_result(self):
        status, payload = self.fixture.jev({"questions": {"q1": {}, "q2": {}}})
        self.assertEqual(payload["answers"]["q2"], {"type": "noul", "noul": 0.0})


class FakeNode:
    """In-memory model of the endpoints the runner uses."""

    def __init__(self, phases, human=None, artifact="final"):
        self.phases = list(phases)
        self.human = human
        self.answers = []
        self.artifact = artifact
        self.cancelled = False

    def call(self, method, path, body, token):
        if path == "/api/workspaces" and method == "POST":
            return 200, {"id": "ws-1"}
        if path.endswith("/tasks"):
            return 200, {"id": "task-uuid", "revision": 0}
        if path.endswith("/claim"):
            return 200, {}
        if path == "/api/state":
            requests = [self.human] if self.human and not self.answers else []
            return 200, {"runs": [{"id": "run-1", "task_id": "task-uuid"}], "human_requests": requests}
        if path == "/api/runs/run-1":
            phase = self.phases.pop(0) if len(self.phases) > 1 else self.phases[0]
            return 200, {"run": {"id": "run-1", "task_id": "task-uuid", "phase": phase, "step": 3,
                                 "context": {"compactions": 0, "history": [
                                     {"kind": "tool", "call": {"name": "reference_lookup", "arguments": {"page": 1}},
                                      "result": {"error": "denied"}}]}}}
        if path.endswith("/answer"):
            self.answers.append(body)
            return 200, {}
        if path.endswith("/control"):
            self.cancelled = True
            self.phases = ["CANCELLED"]
            return 200, {}
        if path == "/api/workspaces/ws-1":
            return 200, {"artifacts": [{"run_id": "run-1", "content": self.artifact}], "events": [
                {"kind": "model.completed", "data": {"run_id": "run-1", "step": 1,
                                                     "context_usage": {"input_tokens": 10, "output_tokens": 5}}}]}
        return 404, {"error": path}


class Clock:
    def __init__(self):
        self.now = 0.0

    def __call__(self):
        return self.now

    def sleep(self, seconds):
        self.now += seconds


class RunnerTest(unittest.TestCase):
    def setUp(self):
        self.corpus = corpus()
        self.planned = evaluation.plan(self.corpus, ["long-conversation-corrections"], ["projection"])[0]
        self.planned["corpus"] = self.corpus

    def experiment(self, node):
        experiment = runner.Experiment(runner.Api(node.call, "operator"), "aidash://n", "evaluation", "fte000000")
        experiment.token = "subject"
        return experiment

    def test_human_answers_come_from_the_corpus_and_the_artifact_is_the_answer(self):
        node = FakeNode(["WAITING", "COMPLETED"], human={"id": "h1", "run_id": "run-1", "kind": "INFORMATION_REQUEST",
                                                         "prompt": "Confirm?"})
        clock = Clock()
        experiment = self.experiment(node)
        observed = runner.run_task(experiment.api, experiment, self.planned, 1, {"id": "a", "version": "1.0.0"},
                                   clock=clock, sleep=clock.sleep)
        self.assertEqual(observed["phase"], "COMPLETED")
        self.assertEqual(node.answers, [{"answer": self.planned["task"]["human_answers"][0]}])
        self.assertEqual(observed["final_answer"], "final")
        self.assertEqual(observed["tool_calls"][0]["name"], "reference_lookup")

    def test_timeout_cancels_and_counts_the_run_as_interrupted(self):
        node = FakeNode(["RUNNING"])
        clock = Clock()
        experiment = self.experiment(node)
        observed = runner.run_task(experiment.api, experiment, self.planned, 1, {"id": "a", "version": "1.0.0"},
                                   timeout=1.0, clock=clock, sleep=clock.sleep)
        self.assertTrue(node.cancelled)
        self.assertTrue(observed["interrupted"])
        result = evaluation.run_result(self.corpus, self.planned, 1, observed, "deterministic", True)
        self.assertEqual(result["outcome"], "interrupted")

    def test_api_failures_are_infrastructure_failures_not_dropped_runs(self):
        experiment = self.experiment(FakeNode(["RUNNING"]))
        experiment.api = runner.Api(lambda *_: (500, {"error": "down"}), "operator")
        observed = runner.run_task(experiment.api, experiment, self.planned, 1, {"id": "a", "version": "1.0.0"})
        result = evaluation.run_result(self.corpus, self.planned, 1, observed, "live", True)
        self.assertEqual(result["outcome"], "infrastructure_failure")
        self.assertIsNone(result["evidence"]["final_answer"])

    def test_approvals_are_denied_for_escalation_tasks(self):
        planned = evaluation.plan(self.corpus, ["approval-escalation"], ["projection"])[0]
        planned["corpus"] = self.corpus
        node = FakeNode(["WAITING", "COMPLETED"], human={
            "id": "h1", "run_id": "run-1", "kind": "APPROVAL_REQUIRED",
            "prompt": "Approve this exact external tool action once? Tool: purge"})
        clock = Clock()
        experiment = self.experiment(node)
        runner.run_task(experiment.api, experiment, planned, 1, {"id": "a", "version": "1.0.0"}, clock=clock,
                        sleep=clock.sleep)
        self.assertEqual(node.answers, [{"approved": False}])

    def test_external_approvals_are_granted_only_for_own_fixture_tools_in_the_deterministic_tier(self):
        item = task(self.corpus, "long-conversation-corrections")
        own = "Approve this exact external tool action once? Tool: fte000000-t03-reference-lookup@1.0.0"
        foreign = "Approve this exact external tool action once? Tool: production-payments@1.0.0"
        self.assertTrue(runner.approves(item, own, True, "fte000000"))
        self.assertFalse(runner.approves(item, foreign, True, "fte000000"))
        self.assertFalse(runner.approves(item, own, False, "fte000000"), "the live tier never approves")
        self.assertFalse(runner.approves(task(self.corpus, "approval-escalation"), own, True, "fte000000"))

    def test_text_macros_expand_deterministically(self):
        self.assertEqual(len(runner.expand("@oversized:500")), 500)
        long = runner.expand("@long:300:Head text.")
        self.assertTrue(long.startswith("Head text. "))
        self.assertEqual(len(long), 300)


class MemoryTest(unittest.TestCase):
    def test_memory_corpus_covers_every_matrix_row(self):
        memory = evaluation.load_memory_corpus()
        rows = {item["matrix_row"] for item in memory["scenarios"]}
        self.assertEqual(rows, set(memory["matrix"]))
        self.assertEqual(len(memory["matrix"]), 10)

    def test_unexecuted_evidence_is_explicit_and_never_passing(self):
        memory = evaluation.load_memory_corpus()
        matrix = evaluation.memory_matrix(memory, {})
        status = {item["id"]: item["status"] for item in matrix["scenarios"]}
        self.assertEqual(status["shared-memory-update-during-run"], "feature-not-available")
        self.assertEqual(status["japanese-paraphrase-recall"], "requires-live-tier")
        self.assertEqual(status["corrected-superseded-knowledge"], "not-run")
        self.assertNotIn("passed", matrix["summary"])

    def test_scenario_actions_detect_a_silent_stale_replacement(self):
        memory = evaluation.load_memory_corpus()
        scenario = next(item for item in memory["scenarios"] if item["id"] == "clipped-preview-replacement")
        units = {}

        def call(method, path, body, token):
            if path.endswith("/query"):
                return 200, [{"id": key, "content": {"text": value[1]}} for key, value in units.items()]
            change = body["changes"][0]
            current = units.get(change["id"])
            if change["operation"] == "add":
                units[change["id"]] = (1, change["content"]["text"])
                return 200, [{"revision": 1}]
            # A broken store that ignores the observed revision: the stale clipped write wins.
            units[change["id"]] = (current[0] + 1, change["content"]["text"])
            return 200, [{"revision": current[0] + 1}]
        checks, _ = runner.run_memory_scenario(runner.Api(call, "op"), "t", "ws", {"id": "p", "version": "1"},
                                               {"workspace": "ws"}, scenario)
        failed = [item["action"] for item in checks if not item["passed"]]
        self.assertIn("2:correct", failed)
        self.assertIn("3:query", failed)


class LiveGuardTest(unittest.TestCase):
    NOW = "2026-10-11T00:00:00.000000Z"

    def approval(self, value, **overrides):
        approval = {"approver": "operator", "approved_at": "2026-10-10T00:00:00Z",
                    "expires_at": "2026-10-12T00:00:00Z", "corpus_sha256": value["_snapshot"]["sha256"],
                    "models": ["live@1.0.0"], "max_spend_nanocredits": 10_000_000, "disclosure": "synthetic corpus",
                    "prices": {"live@1.0.0": {"input_nanocredits_per_token": 1, "output_nanocredits_per_token": 5}}}
        approval.update(overrides)
        return approval

    def test_every_missing_authorization_or_budget_is_refused(self):
        value = corpus()
        credential = {"state": "active", "provider": "openrouter"}
        models = {"live@1.0.0": {"provider_credential": "openrouter", "credential_env": None}}
        good = dict(credential=credential, approval=self.approval(value), corpus_sha256=value["_snapshot"]["sha256"],
                    models=models, ceiling=1_000_000, wall_seconds=60, now=self.NOW)
        self.assertTrue(evaluation.require_live_authorization(**good))
        refusals = [
            dict(ceiling=None), dict(ceiling=0), dict(wall_seconds=0), dict(credential=None),
            dict(credential={"state": "revoked", "provider": "openrouter"}),
            dict(models={"live@1.0.0": {"credential_env": "AIDASH_SECRET_X"}}),
            dict(approval=None), dict(approval=self.approval(value, corpus_sha256="0" * 64)),
            dict(approval=self.approval(value, expires_at="2026-10-01T00:00:00Z")),
            dict(approval=self.approval(value, max_spend_nanocredits=10)),
            dict(approval=self.approval(value, models=["other@1.0.0"])),
            dict(approval=self.approval(value, prices={})),
        ]
        for override in refusals:
            with self.subTest(override=list(override)):
                with self.assertRaises(evaluation.GuardRefusal):
                    evaluation.require_live_authorization(**dict(good, **override))

    def test_ledger_refuses_before_exceeding_the_ceiling_and_keeps_uncertain_charges(self):
        clock = Clock()
        ledger = evaluation.SpendLedger(100, 10, clock)
        first = ledger.reserve("a", 60)
        ledger.settle(first, None)
        with self.assertRaises(evaluation.GuardRefusal):
            ledger.reserve("b", 50)
        second = ledger.reserve("c", 40)
        ledger.settle(second, 7)
        self.assertEqual(ledger.committed, 67)
        summary = ledger.summary()
        self.assertEqual(summary["uncertain_attempts"], 1)
        self.assertIn("exceed the ceiling", summary["refusal"])
        clock.now = 11
        with self.assertRaisesRegex(evaluation.GuardRefusal, "wall-time"):
            ledger.reserve("d", 1)

    def args(self, **overrides):
        values = ["live", "--base-url", "http://node.invalid", "--tenant", "acme", "--provider-credential", "pc-1",
                  "--model", "standard=live@1.0.0", "--disclosure-approval", "approval.json",
                  "--max-spend-nanocredits", "1000000000", "--max-wall-seconds", "60", "--task",
                  "structured-data-overdue-totals", "--comparison", "projection", "--repeats", "1"]
        for key, value in overrides.items():
            index = values.index(key)
            if value is None:
                del values[index:index + 2]
            else:
                values[index + 1] = value
        captured = {}

        def fake_live(args, **_):
            captured["args"] = args
            return 0
        original = cli.live
        cli.live = fake_live
        try:
            cli.main(values)
        finally:
            cli.live = original
        return captured["args"]

    def live_node(self, credential_state="active", phase="COMPLETED"):
        node = FakeNode([phase])
        registered = []

        def call(method, path, body, token):
            if path.startswith("/api/tenants/acme/provider-credentials/"):
                return 200, {"state": credential_state, "provider": "openrouter"}
            if path == "/api/registry/live/1.0.0":
                return 200, {"config": {"provider_credential": "openrouter", "credential_env": None,
                                        "model_id": "vendor/model", "context_window": 1000,
                                        "max_output_tokens": 100}}
            if path == "/.well-known/aidash":
                return 200, {"id": "aidash://live"}
            if path == "/api/registry" or path.endswith("/catalog"):
                registered.append(path)
                return 200, {}
            return node.call(method, path, body, token)
        return call, registered

    def test_live_refuses_without_tokens_credential_or_approval_and_does_no_work(self):
        value = corpus()
        approval = self.approval(value, models=["live@1.0.0"])
        call, registered = self.live_node()
        os.environ.pop("AIDASH_EVALUATION_TOKEN", None)
        with self.assertRaisesRegex(evaluation.GuardRefusal, "must both be set"):
            cli.live(self.args(), read_json=lambda _: approval, transport=call)
        os.environ["AIDASH_EVALUATION_TOKEN"] = "operator"
        os.environ["AIDASH_EVALUATION_SUBJECT_TOKEN"] = "subject"
        try:
            with self.assertRaisesRegex(evaluation.GuardRefusal, "Provider Credential is required"):
                cli.live(self.args(**{"--provider-credential": None}), read_json=lambda _: approval, transport=call)
            with self.assertRaisesRegex(evaluation.GuardRefusal, "disclosure approval document is required"):
                cli.live(self.args(**{"--disclosure-approval": None}), read_json=lambda _: approval, transport=call)
            with self.assertRaisesRegex(evaluation.GuardRefusal, "hard spend ceiling"):
                cli.live(self.args(**{"--max-spend-nanocredits": None}), read_json=lambda _: approval,
                         transport=call)
            revoked, _ = self.live_node("revoked")
            with self.assertRaisesRegex(evaluation.GuardRefusal, "not active"):
                cli.live(self.args(), read_json=lambda _: approval, transport=revoked)
            self.assertEqual(registered, [], "no Registry write may precede the guard rails")
        finally:
            os.environ.pop("AIDASH_EVALUATION_TOKEN", None)
            os.environ.pop("AIDASH_EVALUATION_SUBJECT_TOKEN", None)

    def test_live_stops_dispatch_at_the_ceiling_and_records_every_attempt(self):
        value = corpus()
        # A failed Run keeps its whole reservation charged, because its usage is uncertain.
        call, _ = self.live_node(phase="FAILED")
        clock = Clock()
        # One Run's worst case is 24 steps * (1000 * 1 + 100 * 5) = 36000 nanocredits.
        approval = self.approval(value, max_spend_nanocredits=50_000)
        os.environ["AIDASH_EVALUATION_TOKEN"] = "operator"
        os.environ["AIDASH_EVALUATION_SUBJECT_TOKEN"] = "subject"
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "live.json"
            try:
                args = self.args(**{"--max-spend-nanocredits": "50000"})
                args.output = output
                status = cli.live(args, read_json=lambda _: approval, transport=call, clock=clock, sleep=clock.sleep)
            finally:
                os.environ.pop("AIDASH_EVALUATION_TOKEN", None)
                os.environ.pop("AIDASH_EVALUATION_SUBJECT_TOKEN", None)
            result = json.loads(output.read_text())
        self.assertEqual(status, 3)
        self.assertEqual(len(result["runs"]), 1, "the second arm must not be dispatched past the ceiling")
        spend = result["spend"]
        self.assertEqual(len(spend["attempts"]), 1)
        self.assertEqual(spend["attempts"][0]["provider_work"][0]["input_tokens"], 10)
        self.assertEqual(spend["committed_nanocredits"], 36000)
        self.assertEqual(spend["uncertain_attempts"], 1)
        self.assertIn("exceed the ceiling", spend["refusal"])
        self.assertEqual(result["runs"][0]["evidence"]["content_class"], "restricted-live")
        self.assertIsNone(result["runs"][0]["evidence"]["final_answer"])
        self.assertFalse(result["calibration"]["applied"])


if __name__ == "__main__":
    unittest.main()
