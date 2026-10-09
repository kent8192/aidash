#!/usr/bin/env python3
"""Measure legacy and deferred@1 capability exposure through the real HTTP API.

For each frozen fixture case, registers the same synthetic HTTP tools and
optional Registry Skills under a legacy Agent and a deferred@1 Agent, runs one
Task with each, and reads the Run inspection. External tool calls are denied by
default, so fixture endpoints are never contacted. The report contains raw
measurements only: exposure bytes, provider token counts, the model's tool
calls and whether the expected tool was selected. It makes no accuracy or cost
claim and approves nothing.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import time
import tomllib
import urllib.error
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parent.parent
TERMINAL = ("COMPLETED", "FAILED", "CANCELLED")
EVENT_WINDOW = 100  # GET /api/workspaces/{id} returns at most this many recent events.
NO_HUMAN = "No human is available during this evaluation; continue without further input."
DISPATCH_REJECTIONS = ("unavailable tool ", "capability ")
LIMITATIONS = [
    "context.usage holds only the latest provider response; per-request token counts come from "
    "model.completed events in the Run's Workspace, which GET /api/workspaces/{id} caps at 100.",
    "Legacy requests record no tool-definition or Skill-body bytes; context.usage.exposure exists "
    "only for deferred@1 requests.",
    "The tool definitions and instructions sent in each request are not exposed by the API.",
    "Compaction can remove older tool events from context.history; see each run's compactions.",
]


def evaluate(call, node, model, profile, fixture=None, report=None, endpoint=None, approve=False,
             timeout=600.0, interval=1.0):
    fixture = fixture or ROOT / "tests/fixtures/capability_exposure_evaluation.toml"
    raw = fixture.read_bytes()
    spec = tomllib.loads(raw.decode())
    endpoint = endpoint or "http://127.0.0.1:9/aidash-capability-exposure-evaluation"
    report = {} if report is None else report
    report.update({"fixture": spec["version"], "fixture_sha256": hashlib.sha256(raw).hexdigest(),
                   "profile": profile, "grade": "measurement-only",
                   "external_calls": "approved" if approve else "denied",
                   "accuracy_claim_approved": False, "cost_claim_approved": False,
                   "production_quality_approved": False, "limitations": LIMITATIONS,
                   "cases": [], "requests": []})
    tools = {tool["alias"]: tool for tool in spec["tools"]}
    skills = {skill["id"]: skill for skill in spec.get("skills", [])}
    prefix = "cxe-" + uuid.uuid4().hex[:8]
    phase = "initialization"

    def api(path, body=None):
        begin = time.perf_counter()
        status, value = call(path, body)
        report["requests"].append({"phase": phase, "path": path, "status": status,
                                   "round_trip_ms": (time.perf_counter() - begin) * 1000})
        if status != 200:
            raise RuntimeError(f"evaluation request {path} returned HTTP {status}: {value}")
        return value

    def target(identifier):
        return {"registry_node": node, "id": identifier, "version": "1.0.0"}

    def entry(kind, identifier, name, description, schema, config, languages=("en",)):
        return {"id": identifier, "version": "1.0.0", "kind": kind, "name": {"en": name},
                "description": {"en": description}, "capabilities": [], "tags": ["evaluation"],
                "languages": list(languages), "skills": [], "schema": schema, "config": config}

    def register_tool(case_id, alias):
        tool = tools[alias]
        identifier = f"{prefix}-{case_id}-{alias}".replace("_", "-")
        api("/api/registry", entry("tool", identifier, alias, tool["description"], json.loads(tool["schema"]), {
            "registry_node": node, "provider": "integration.http@1", "operation": "invoke",
            "default_alias": alias, "tier": "integration", "narrow": {},
            "transport": {"transport": "http", "endpoint": endpoint, "credential_env": None,
                          "replay": "idempotent"}}))
        return identifier

    def register_skill(case_id, skill_id):
        skill = skills[skill_id]
        identifier = f"{prefix}-{case_id}-{skill_id}"
        config = {"instructions": skill["instructions"]}
        if skill.get("files"):
            config["files"] = [{"path": f["path"], "content": f["content"]} for f in skill["files"]]
        api("/api/registry", entry("skill", identifier, skill["name"], skill["description"],
                                   {"type": "object"}, config))
        return identifier

    def human_requests(run_id):
        state = api("/api/state")
        for request in state["human_requests"]:
            if request["run_id"] == run_id and request.get("response") is None:
                yield request

    def run_task(case, variant, agent_id):
        nonlocal phase
        phase = f"{case['id']}:{variant}:admission"
        workspace = api("/api/workspaces", {"title": f"{prefix} {case['id']} {variant}",
                                            "goal": "Capability exposure measurement"})["id"]
        task = api(f"/api/workspaces/{workspace}/tasks", {
            "title": f"{case['id']} ({variant})", "description": case["task"], "requirements": {},
            "dependencies": [], "parent_id": None})["id"]
        api(f"/api/tasks/{task}/delegate", {"node_id": node, "agent": {"id": agent_id, "version": "1.0.0"}})
        phase = f"{case['id']}:{variant}:execution"
        deadline = time.monotonic() + timeout
        run_id = None
        while run_id is None:
            run_id = next((run["id"] for run in api("/api/state")["runs"] if run["task_id"] == task), None)
            if run_id is None:
                if time.monotonic() > deadline:
                    raise RuntimeError(f"no Run appeared for task {task}")
                time.sleep(interval)
        answers, timed_out = [], False
        while True:
            run = api(f"/api/runs/{run_id}")["run"]
            if run["phase"] in TERMINAL:
                break
            if time.monotonic() > deadline:
                api(f"/api/runs/{run_id}/control", {"action": "cancel"})
                timed_out = True
                run = api(f"/api/runs/{run_id}")["run"]
                break
            if run["phase"] == "WAITING":
                for request in human_requests(run_id):
                    approval = request["kind"] == "APPROVAL_REQUIRED"
                    answer = {"approved": approve} if approval else {"answer": NO_HUMAN}
                    api(f"/api/human-requests/{request['id']}/answer", answer)
                    answers.append({"kind": request["kind"], "prompt": request["prompt"], "answer": answer})
            time.sleep(interval)
        phase = f"{case['id']}:{variant}:inspection"
        events = api(f"/api/workspaces/{workspace}")["events"]
        return workspace, task, run, events, answers, timed_out

    def measure(case, variant, agent_id, policy, workspace, task, run, events, answers, timed_out):
        context = run.get("context") or {}
        usage = context.get("usage") or {}
        completions = [event["data"] for event in events
                       if event["kind"] == "model.completed" and event["data"].get("run_id") == run["id"]]
        per_request = []
        for data in completions:
            observed = data.get("context_usage") or {}
            exposure = observed.get("exposure")
            per_request.append({
                "step": data.get("step"), "input_tokens": observed.get("input_tokens"),
                "output_tokens": observed.get("output_tokens"),
                "exposure": None if exposure is None else {
                    "metadata_bytes": exposure["metadata_bytes"], "schema_bytes": exposure["schema_bytes"],
                    "skill_bytes": exposure["skill_bytes"], "exposed": exposure["exposed"]}})
        calls = []
        for event in context.get("history", []):
            if event.get("kind") != "tool":
                continue
            result = event.get("result")
            error = result.get("error") if isinstance(result, dict) and isinstance(result.get("error"), str) else None
            calls.append({"name": event["call"]["name"], "arguments": event["call"].get("arguments"),
                          "error": error,
                          "dispatch_rejected": error is not None and (
                              error.startswith(DISPATCH_REJECTIONS[0])
                              or (error.startswith(DISPATCH_REJECTIONS[1]) and error.endswith("use capability_load")))})
        expected = [index for index, item in enumerate(calls) if item["name"] == case["expected_tool"]]
        selected = [index for index in expected if not calls[index]["dispatch_rejected"]]
        final_exposure = usage.get("exposure")
        return {
            "variant": variant, "agent": {"id": agent_id, "version": "1.0.0"}, "exposure_policy": policy,
            "workspace": workspace, "task": task, "run_id": run["id"], "phase": run["phase"],
            "error": run.get("error"), "state_error": run.get("state_error"), "step": run.get("step"),
            "timed_out": timed_out, "compactions": context.get("compactions"),
            "provider_usage": {
                "observed_requests": len(per_request),
                "input_tokens": sum(item["input_tokens"] or 0 for item in per_request),
                "output_tokens": sum(item["output_tokens"] or 0 for item in per_request),
                "event_window_complete": len(events) < EVENT_WINDOW,
                "latest": {key: usage.get(key) for key in ("input_tokens", "output_tokens", "context_window")},
            },
            "exposure_bytes": None if policy is None else {
                "schema_bytes": [item["exposure"]["schema_bytes"] for item in per_request if item["exposure"]],
                "skill_bytes": [item["exposure"]["skill_bytes"] for item in per_request if item["exposure"]],
                "metadata_bytes": [item["exposure"]["metadata_bytes"] for item in per_request if item["exposure"]],
                "latest": final_exposure,
                "state": context.get("exposure"),
            },
            "requests": per_request,
            "tool_calls": calls,
            "capability_calls": {name: sum(item["name"] == name for item in calls) for name in (
                "capability_search", "capability_describe", "capability_load", "capability_unload",
                "skill_asset_read")},
            "selection": {"expected_tool": case["expected_tool"], "called": bool(expected),
                          "selected": bool(selected), "first_call_index": expected[0] if expected else None,
                          "first_selected_index": selected[0] if selected else None},
            "human_answers": answers,
        }

    for case in spec["cases"]:
        phase = case["id"] + ":registration"
        aliases = list(tools) if case.get("tools", "all") == "all" else case["tools"]
        unknown = [alias for alias in aliases + [case["expected_tool"]] + case.get("eager", []) if alias not in tools]
        if unknown:
            raise ValueError(f"case {case['id']} names unknown tools {unknown}")
        tool_ids = {alias: register_tool(case["id"], alias) for alias in aliases}
        skill_ids = [register_skill(case["id"], skill_id) for skill_id in case.get("skills", [])]
        deferred_policy = {"version": "deferred@1", **case.get("deferred", {})}
        item = {"id": case["id"], "language": case["language"], "task": case["task"],
                "expected_tool": case["expected_tool"], "tool_count": len(aliases),
                "skill_count": len(skill_ids), "registry": {"tools": tool_ids, "skills": skill_ids},
                "variants": {}}
        for variant, policy in (("legacy", None), ("deferred", deferred_policy)):
            phase = f"{case['id']}:{variant}:registration"
            bindings = []
            for alias, identifier in tool_ids.items():
                binding = {"kind": "tool", "target": target(identifier), "alias": alias, "narrow": {}}
                if policy is not None and alias in case.get("eager", []):
                    binding["exposure"] = "eager"
                bindings.append(binding)
            bindings += [{"kind": "skill", "target": target(identifier), "narrow": {}} for identifier in skill_ids]
            config = {"schema_version": 1, "model": model, "instructions": spec["instructions"],
                      "bindings": bindings, "remove_default": case.get("remove_default", []),
                      "max_steps": case.get("max_steps", spec.get("max_steps", 16))}
            if policy is not None:
                config["exposure"] = policy
            agent_id = f"{prefix}-{case['id']}-{variant}"
            api("/api/registry", entry("agent", agent_id, agent_id, "Capability exposure measurement Agent",
                                       {}, config, languages=("en", "ja")))
            item["variants"][variant] = measure(case, variant, agent_id, policy,
                                                *run_task(case, variant, agent_id))
        report["cases"].append(item)
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    for name in ("base-url", "model-id", "model-version", "server-revision", "output"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--token-env", default="AIDASH_EVALUATION_TOKEN",
                        help="environment variable holding the operator bearer token")
    parser.add_argument("--fixture", type=Path, default=ROOT / "tests/fixtures/capability_exposure_evaluation.toml")
    parser.add_argument("--tool-endpoint", default=None,
                        help="HTTP endpoint declared by the fixture tools (contacted only with --approve-external-calls)")
    parser.add_argument("--approve-external-calls", action="store_true",
                        help="approve fixture tool calls instead of denying them")
    parser.add_argument("--run-timeout", type=float, default=600.0, help="seconds before a Run is cancelled")
    parser.add_argument("--poll-interval", type=float, default=1.0)
    args = parser.parse_args()
    token = os.environ[args.token_env]
    base = args.base_url.rstrip("/")

    def call(path, body):
        request = urllib.request.Request(base + path,
            data=None if body is None else json.dumps(body).encode(),
            headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                return response.status, json.load(response)
        except urllib.error.HTTPError as error:
            return error.code, error.read().decode(errors="replace")[:2000]

    report, failure = {}, None
    try:
        status, identity = call("/.well-known/aidash", None)
        if status != 200:
            raise RuntimeError(f"node identity returned HTTP {status}")
        model = {"id": args.model_id, "version": args.model_version}
        profile = {"server_revision": args.server_revision, "node": identity["id"], "model": model,
                   "model_acceptance": "unapproved"}
        evaluate(call, identity["id"], model, profile, args.fixture, report, args.tool_endpoint,
                 args.approve_external_calls, args.run_timeout, args.poll_interval)
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
        failure = error
    output = Path(args.output)
    descriptor = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as stream:
        json.dump(report, stream, ensure_ascii=False, indent=2)
    print(f"Evaluation evidence: {output}")
    if failure is not None:
        raise failure


if __name__ == "__main__":
    main()
