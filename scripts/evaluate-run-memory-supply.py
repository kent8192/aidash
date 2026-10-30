#!/usr/bin/env python3
r"""Measure whether native memory reaches a live Run's context (#122).

For each case in tests/evaluation/memory-supply.toml and each arm, the
evaluator creates a Workspace with a semantic index, a fresh memory
participant, seeds its bank with the case's Units (then applies corrections
and deletions), waits for the index, assigns the participant to a new Task and
claims the Task with that arm's Agent:

- `auto` binds the memory provider with its default context source, recalled
  before every inference step;
- `on-demand` binds the same policy with `pin: on_demand`, reached only through
  the `memory_recall` tool;
- `none` binds no memory and is the control.

A Run is graded on its answer and on supply: `/memory/units/impact` must list
the Run against every required Unit at its current revision, and never against
a superseded revision or a deleted Unit. The embedding is the fixture's 8-dim
hash, so retrieval is keyword-driven; this measures supply, not semantic recall.

  AIDASH_EVALUATION_TOKEN=... AIDASH_EVALUATION_SUBJECT_TOKEN=... \
  python3 scripts/evaluate-run-memory-supply.py --base-url http://127.0.0.1:18083 \
    --tenant TENANT --model MODEL_ID@VERSION \
    --embedding-endpoint http://host.docker.internal:18777/v1 --repeats 3 \
    --output private/evaluation/memory-supply.json
"""
import argparse
import datetime
import importlib.util
import json
import os
import re
import secrets
import sys
import time
import tomllib
import uuid
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS))
import fixed_task_runner as runner  # noqa: E402
from fixed_task_evaluation import canonical_sha256  # noqa: E402

ROOT = SCRIPTS.parent
CORPUS = ROOT / "tests/evaluation/memory-supply.toml"
ARMS = ("auto", "on-demand", "none")
# The memory slot's default context source (server/src/apps/execution/repositories/agent/sources.rs),
# with automatic recall turned off.
ON_DEMAND_SOURCE = {"provider": "aidash.native-context/1", "operation": "semantic_memory",
                    "tools": {"read": [], "write": []}, "pin": {"policy": "on_demand"}}


def load_corpus(path=CORPUS):
    raw = Path(path).read_bytes()
    corpus = tomllib.loads(raw.decode())
    unknown = set(corpus["arms"]) - set(ARMS)
    if unknown:
        raise ValueError(f"unknown arms {sorted(unknown)}")
    for case in corpus["cases"]:
        names = {unit["id"] for unit in case["units"]}
        referenced = set(case.get("required", [])) | set(case.get("deletions", [])) | \
            {item["id"] for item in case.get("corrections", [])}
        if referenced - names:
            raise ValueError(f"case {case['id']} references unknown Units {sorted(referenced - names)}")
        re.compile(case["expect"])
        for pattern in case.get("forbid", []):
            re.compile(pattern)
    corpus["_sha256"] = __import__("hashlib").sha256(raw).hexdigest()
    return corpus


def grade(case, arm, answer, reads, revisions, deleted):
    """Grade one Run.

    `reads` maps Unit symbol -> list of revisions this Run read (from impact);
    `revisions` maps Unit symbol -> its current revision; `deleted` is the set of
    deleted symbols.
    """
    text = answer or ""
    answered = bool(re.search(case["expect"], text)) and not any(re.search(p, text) for p in case.get("forbid", []))
    required = case.get("required", [])
    supplied = all(revisions[name] in reads.get(name, []) for name in required)
    stale = sorted(name for name, seen in reads.items()
                   if seen and (name in deleted or any(revision != revisions.get(name) for revision in seen)))
    any_read = any(reads.values())
    if arm == "none":
        # The control has no memory: a read would be a leak, and only a case answerable without memory may pass.
        supply_ok = not any_read
    else:
        supply_ok = supplied and not stale
    return {"answer_correct": answered, "supplied": supplied if arm != "none" else None, "stale_reads": stale,
            "read_units": sorted(name for name, seen in reads.items() if seen), "supply_ok": supply_ok,
            "passed": answered and supply_ok}


def summarize(runs):
    table = {}
    for run in runs:
        key = (run["case"], run["arm"])
        row = table.setdefault(key, {"case": run["case"], "arm": run["arm"], "n": 0, "passed": 0, "answer_correct": 0,
                                     "supplied": 0, "stale": 0, "errors": 0, "memory_recall_calls": 0,
                                     "input_tokens": 0, "steps": 0})
        row["n"] += 1
        if run.get("error"):
            row["errors"] += 1
            continue
        grade_ = run["grade"]
        row["passed"] += grade_["passed"]
        row["answer_correct"] += grade_["answer_correct"]
        row["supplied"] += bool(grade_["supplied"])
        row["stale"] += bool(grade_["stale_reads"])
        row["memory_recall_calls"] += run["memory_recall_calls"]
        row["input_tokens"] += run.get("input_tokens") or 0
        row["steps"] += run.get("steps") or 0
    by_arm = {}
    for row in table.values():
        arm = by_arm.setdefault(row["arm"], {"n": 0, "passed": 0, "answer_correct": 0, "supplied": 0, "stale": 0,
                                             "errors": 0})
        for field in arm:
            arm[field] += row[field]
    return {"cells": list(table.values()), "arms": by_arm}


def http_transport(base):
    spec = importlib.util.spec_from_file_location("evaluate_fixed_tasks", SCRIPTS / "evaluate-fixed-tasks.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.require_https(base)
    return module.http_transport(base)


class Campaign:
    def __init__(self, api, experiment, corpus, model, embedding_endpoint, run_timeout, projection_version="native"):
        self.api = api
        self.projection_version = projection_version
        self.experiment = experiment
        self.corpus = corpus
        self.model = model
        self.embedding = {"provider": "openai", "endpoint": embedding_endpoint,
                          "model": f"{experiment.prefix}-embedding", "model_version": "1", "dimensions": 8}
        self.run_timeout = run_timeout
        self.agents = {}
        self.providers = {}

    @property
    def token(self):
        return self.experiment.token

    def register(self):
        prefix = self.experiment.prefix
        roles = {"model": self.model}
        embedding = self.experiment.register("embedding", f"{prefix}-embedding", self.embedding)
        roles["embedding"] = {"id": embedding["id"], "version": "1.0.0"}
        for kind, config in (("reranker", {"provider": "rrf"}), ("tokenizer", {"provider": "utf8_upper_bound"})):
            entry = self.experiment.register(kind, f"{prefix}-{kind}", config)
            roles[kind] = {"id": entry["id"], "version": "1.0.0"}
        policy = runner.memory_policy(roles, {})
        for arm in self.corpus["arms"]:
            bindings = []
            if arm != "none":
                config = dict(policy, context_source=ON_DEMAND_SOURCE) if arm == "on-demand" else policy
                provider = self.experiment.register("memory", f"{prefix}-memory-{arm}", config)
                self.providers[arm] = {"id": provider["id"], "version": "1.0.0"}
                bindings.append({"kind": "memory", "target": self.experiment.ref(provider["id"]), "narrow": {}})
            agent = self.experiment.register("agent", f"{prefix}-agent-{arm}", {
                "schema_version": 1, "model": self.model, "instructions": self.corpus["instructions"],
                "bindings": bindings,
                # Supply is measured without context pruning, so no compaction Decider is involved. Every
                # Agent is offered the memory builtins by default; the control must not see them (a call
                # without a memory binding is an authority pause), and no arm writes memory.
                "remove_default": ["file_search", "file_read", runner.COMPACTION_DEFAULT, "memory_mutate"] +
                                  (["memory_recall", "memory_reflect"] if arm == "none" else []),
                "max_steps": self.corpus["max_steps"], "projection_version": self.projection_version})
            self.agents[arm] = {"id": agent["id"], "version": "1.0.0"}

    def wait_ready(self, workspace, ids, timeout=120):
        deadline = time.monotonic() + timeout
        while True:
            entries = self.api.get(f"/api/workspaces/{workspace}/semantic/entries", self.token)
            states = {entry["id"]: entry["state"] for entry in entries if entry["id"] in ids}
            if len(states) == len(ids) and all(value == "READY" for value in states.values()):
                return
            if any(value in ("ERROR", "REVOKED") for value in states.values()) or time.monotonic() > deadline:
                raise RuntimeError(f"semantic index did not become ready: {states}")
            time.sleep(0.5)

    def seed(self, workspace, provider, bank, case):
        """Admit, correct and delete the case's Units; return ids, current revisions and deleted symbols."""
        ids = {unit["id"]: str(uuid.uuid4()) for unit in case["units"]}
        path = f"/api/workspaces/{workspace}/memory/units/mutate"
        admitted = self.api.post(path, {"operation_id": str(uuid.uuid4()), "provider": provider, "bank": bank,
                                        "changes": [{"operation": "add", "id": ids[unit["id"]],
                                                     "content": runner.unit_content(unit["text"])}
                                                    for unit in case["units"]]}, self.token)
        revisions = {name: next(item["revision"] for item in admitted if item["id"] == ids[name]) for name in ids}
        changes = [{"operation": "correct", "id": ids[item["id"]], "expected_revision": revisions[item["id"]],
                    "content": runner.unit_content(item["text"])} for item in case.get("corrections", [])]
        changes += [{"operation": "delete", "id": ids[name], "expected_revision": revisions[name]}
                    for name in case.get("deletions", [])]
        if changes:
            changed = self.api.post(path, {"operation_id": str(uuid.uuid4()), "provider": provider, "bank": bank,
                                           "changes": changes}, self.token)
            for item in changed:
                name = next(symbol for symbol, value in ids.items() if value == item["id"])
                if item.get("revision") is not None:
                    revisions[name] = item["revision"]
        deleted = set(case.get("deletions", []))
        self.wait_ready(workspace, [ids[name] for name in ids if name not in deleted])
        return ids, revisions, deleted

    def run(self, case, arm, repeat):
        record = {"case": case["id"], "arm": arm, "repeat": repeat, "language": case["language"]}
        started = time.monotonic()
        try:
            token = self.token
            workspace = self.api.post("/api/workspaces", {"title": f"{self.experiment.prefix} {case['id']} {arm} "
                                                                   f"r{repeat}", "goal": "Memory supply evaluation"},
                                      token)["id"]
            record["workspace"] = workspace
            self.api.post(f"/api/workspaces/{workspace}/semantic/index", {"expected_revision": 0, "spec": {
                "embedding": self.embedding, "vector": {"provider": "postgres", "endpoint": "local",
                                                        "credential_env": None},
                "enabled": True, "auto_context": False, "max_sources": 100, "max_results": 10,
                "max_result_tokens": 32768, "max_input_bytes": 32768}}, token)
            # The control seeds the same Units through the `auto` provider, so a leak would be visible.
            provider = self.providers.get(arm) or self.providers.get("auto") or next(iter(self.providers.values()))
            participant = self.api.post(f"/api/workspaces/{workspace}/memory/participants",
                                        {"agent": self.agents[arm] if arm != "none" else self.agents.get(
                                            "auto", next(iter(self.agents.values())))}, token)
            bank = participant["bank"]
            ids, revisions, deleted = self.seed(workspace, provider, bank, case)
            created = self.api.post(f"/api/workspaces/{workspace}/tasks", {
                "title": f"Memory supply: {case['id']}", "description": case["prompt"], "requirements": {},
                "dependencies": [], "parent_id": None}, token)
            task = created["id"]
            revision = created.get("revision", 0)
            if arm != "none":
                assigned = self.api.post(f"/api/workspaces/{workspace}/tasks/{task}/memory-participant", {
                    "task_revision": revision, "expected": None,
                    "target": {"participant_id": participant["id"], "participant_revision": participant["revision"]}},
                    token)
                revision = assigned.get("task_revision", revision) if isinstance(assigned, dict) else revision
            self.api.post(f"/api/tasks/{task}/claim", {"revision": revision, "agent": self.agents[arm]}, token)
            run = self.wait_run(task)
            record.update({"run_id": run["id"], "phase": run["phase"], "steps": run.get("step"),
                           "error_text": run.get("error")})
            context = run.get("context") or {}
            record["input_tokens"] = (context.get("usage") or {}).get("input_tokens")
            calls = runner.tool_calls_from(context)
            record["memory_recall_calls"] = sum(call["name"] == "memory_recall" for call in calls)
            record["tool_calls"] = [call["name"] for call in calls]
            answer = runner.final_answer(self.api, token, workspace, run) if run["phase"] == "COMPLETED" else None
            record["answer"] = answer
            live = [ids[name] for name in ids]
            impact = self.api.post(f"/api/workspaces/{workspace}/memory/units/impact",
                                   {"provider": provider, "bank": bank, "units": live}, token)
            reads = {name: [] for name in ids}
            for item in impact if isinstance(impact, list) else impact.get("units", []):
                name = next((symbol for symbol, value in ids.items() if value == item.get("id")), None)
                if name is None:
                    continue
                reads[name] = [entry["read_revision"] for entry in item.get("runs", []) if entry["id"] == run["id"]]
            record["impact"] = impact
            record["reads"] = reads
            record["grade"] = grade(case, arm, answer, reads, revisions, deleted)
        except Exception as error:  # Recorded per Run; one failure never hides the others.
            record["error"] = f"{type(error).__name__}: {error}"[:600]
        record["wall_ms"] = round((time.monotonic() - started) * 1000)
        return record

    def wait_run(self, task):
        """Wait for the Task's Run; a paused, timed-out or interrupted Run is cancelled, never left behind."""
        deadline = time.monotonic() + self.run_timeout
        run_id = None
        finished = False
        try:
            while time.monotonic() < deadline:
                if run_id is None:
                    run_id = next((run["id"] for run in self.api.get("/api/state", self.token)["runs"]
                                   if run["task_id"] == task), None)
                if run_id is not None:
                    run = self.api.get(f"/api/runs/{run_id}", self.token)["run"]
                    if run["phase"] in runner.TERMINAL:
                        finished = True
                        return run
                    if run.get("control") == "PAUSED":
                        raise RuntimeError(f"Run {run_id} paused: {run.get('error') or run.get('state_error')}")
                    if run["phase"] == "WAITING":
                        for request in self.api.get("/api/state", self.token)["human_requests"]:
                            if request["run_id"] == run_id and request.get("response") is None:
                                answer = {"approved": False} if request["kind"] == "APPROVAL_REQUIRED" else \
                                    {"answer": runner.NO_HUMAN}
                                self.api.post(f"/api/human-requests/{request['id']}/answer", answer, self.token)
                time.sleep(1.0)
            raise RuntimeError(f"Run for Task {task} did not finish within {self.run_timeout}s")
        finally:
            if run_id is not None and not finished:
                try:
                    self.api.post(f"/api/runs/{run_id}/control", {"action": "cancel"}, self.token)
                except Exception as error:  # Reported; the caller records the Run as an error anyway.
                    print(f"warning: could not cancel Run {run_id}: {error}", file=sys.stderr, flush=True)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--tenant", required=True)
    parser.add_argument("--model", required=True, help="Registry id@version of the Agents' (and memory roles') model")
    parser.add_argument("--embedding-endpoint", required=True,
                        help="OpenAI-compatible /v1 endpoint the Node may reach (the fixture service)")
    parser.add_argument("--corpus", default=str(CORPUS))
    parser.add_argument("--case", action="append", help="Only these case ids")
    parser.add_argument("--arm", action="append", choices=ARMS, help="Only these arms")
    parser.add_argument("--projection-version", default="native", choices=("legacy", "ordered", "native"))
    parser.add_argument("--repeats", type=int, default=1)
    parser.add_argument("--run-timeout", type=float, default=300.0)
    parser.add_argument("--output", required=True)
    args = parser.parse_args(argv)
    deployment_administrator = os.environ.get("AIDASH_EVALUATION_TOKEN")
    subject = os.environ.get("AIDASH_EVALUATION_SUBJECT_TOKEN")
    if not deployment_administrator or not subject:
        sys.exit("AIDASH_EVALUATION_TOKEN and AIDASH_EVALUATION_SUBJECT_TOKEN must both be set")
    if args.repeats < 1:
        sys.exit("--repeats must be at least 1")
    output = Path(args.output)
    output.parent.mkdir(parents=True, exist_ok=True)
    with open(output, "x") as reserved:  # Never overwrite an earlier result.
        reserved.write("{}\n")
    corpus = load_corpus(args.corpus)
    if args.arm:
        corpus["arms"] = [arm for arm in corpus["arms"] if arm in args.arm]
    cases = [case for case in corpus["cases"] if not args.case or case["id"] in args.case]
    identifier, _, version = args.model.partition("@")
    api = runner.Api(http_transport(args.base_url), deployment_administrator)
    node = api.get("/.well-known/aidash")["id"]
    experiment = runner.Experiment(api, node, args.tenant, "mse" + secrets.token_hex(3))
    experiment.token = subject
    campaign = Campaign(api, experiment, corpus, {"id": identifier, "version": version}, args.embedding_endpoint,
                        args.run_timeout, args.projection_version)
    result = {"corpus": {"version": corpus["version"], "sha256": corpus["_sha256"]}, "prefix": experiment.prefix,
              "model": args.model, "embedding": "fixture 8-dim hash (keyword-driven retrieval)",
              "started_at": datetime.datetime.now(datetime.UTC).isoformat(), "runs": [], "error": None}
    try:
        campaign.register()
        for repeat in range(1, args.repeats + 1):
            for case in cases:
                for arm in corpus["arms"]:
                    record = campaign.run(case, arm, repeat)
                    result["runs"].append(record)
                    verdict = record.get("error") or ("pass" if record["grade"]["passed"] else
                                                      f"fail {json.dumps(record['grade'], ensure_ascii=False)}")
                    print(f"{case['id']:32} {arm:10} r{repeat} {verdict}", flush=True)
    except BaseException as error:  # The partial record is still written.
        result["error"] = f"{type(error).__name__}: {error}"[:600]
        if not isinstance(error, Exception):
            raise
    finally:
        result["finished_at"] = datetime.datetime.now(datetime.UTC).isoformat()
        result["summary"] = summarize(result["runs"])
        result["digest"] = canonical_sha256({"corpus": result["corpus"], "runs": [
            {key: run.get(key) for key in ("case", "arm", "repeat", "grade")} for run in result["runs"]]})
        output.write_text(json.dumps(result, ensure_ascii=False, indent=1) + "\n")
        os.chmod(output, 0o600)
    for arm, row in result["summary"]["arms"].items():
        print(f"{arm:10} passed {row['passed']}/{row['n']} answer {row['answer_correct']} supplied {row['supplied']} "
              f"stale {row['stale']} errors {row['errors']}")
    print(f"Result: {output}")
    return 1 if result["error"] else 0


if __name__ == "__main__":
    sys.exit(main())
