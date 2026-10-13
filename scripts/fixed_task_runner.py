"""Drive fixed-task Runs and memory scenarios through the authorized HTTP API (#122).

Both tiers use this driver. The deterministic tier points it at a disposable
local Node whose models are the scripted fixture provider; the live tier points
it at an operator-chosen Node and model under its guard rails. The driver only
observes; fixed_task_evaluation.py grades and aggregates.
"""
import copy
import json
import time
import uuid

from fixed_task_evaluation import canonical_sha256, task_description, task_tools

NO_HUMAN = "No human is available during this evaluation; continue without further input."
TERMINAL = ("COMPLETED", "FAILED", "CANCELLED")
# Seconds a cancellation may take after the Run's own deadline has passed.
CANCEL_GRACE = 30.0
# The harness retries a retryable provider failure up to five times without advancing
# the step (crates/aidash-harness/src/recovery.rs), so one step can make six attempts.
PROVIDER_ATTEMPTS_PER_STEP = 6
# The tool result the harness records when a workbench approval was denied: the call never ran.
APPROVAL_DENIED = "external write approval was denied or expired"
# How the harness words a workbench approval request (crates/aidash-harness/src/agent.rs).
APPROVAL_PROMPT = "Approve this exact external tool action once? Tool: "
# The implicit builtins an Agent gets unless `remove_default` names them, and the ones bound
# Skills need (crates/aidash-domain/src/registry/bindings.rs, exposure.rs `default_tools` and
# `skill_support_tools`). test_fixed_task_evaluation.py checks DEFAULT_TOOLS against the Rust list.
DEFAULT_TOOLS = ("workspace_observe", "workspace_wait", "skill_list", "skill_load", "skill_read", "file_search",
                 "file_read", "task_create", "task_delegate", "agent_discover", "artifact_publish",
                 "workspace_message", "memory_mutate", "memory_recall", "memory_reflect")
SKILL_TOOLS = ("skill_list", "skill_load", "skill_read")
SKILL_ASSET_READ = "skill_asset_read"


class ApiError(RuntimeError):
    def __init__(self, method, path, status, value):
        super().__init__(f"{method} {path} returned HTTP {status}: {str(value)[:400]}")
        self.status = status
        self.value = value


class DeadlineExceeded(RuntimeError):
    """A request could not finish before its caller's deadline."""


class Api:
    """Bounded request log around a transport `call(method, path, body, token, timeout) -> (status, value)`.

    With a deadline (see `until`), every request's timeout, every retry and
    every backoff ends by it, so a stalled request cannot outlive the caller's
    time limit.
    """

    # Gateway and availability failures of a loaded Node. Only reads are retried
    # blindly; a write is retried by its caller after checking its effect.
    TRANSIENT = (502, 503, 504)

    def __init__(self, call, operator_token, retries=4, backoff=2.0, sleep=time.sleep, request_timeout=180.0,
                 clock=time.monotonic):
        self.call = call
        self.operator = operator_token
        self.log = []
        self.retries = retries
        self.backoff = backoff
        self.sleep = sleep
        self.request_timeout = request_timeout
        self.clock = clock
        self.deadline = None

    def until(self, deadline, clock=None, sleep=None):
        """A view of this Api, sharing its log, whose requests all end by `deadline` on `clock`."""
        bounded = copy.copy(self)
        bounded.deadline = deadline
        bounded.clock = clock or self.clock
        bounded.sleep = sleep or self.sleep
        return bounded

    def request(self, method, path, body=None, token=None, accept=(200,)):
        attempts = self.retries + 1 if method == "GET" else 1
        for attempt in range(attempts):
            timeout = self.request_timeout
            if self.deadline is not None:
                timeout = min(timeout, self.deadline - self.clock())
                if timeout <= 0:
                    raise DeadlineExceeded(f"{method} {path}: the deadline passed before the request")
            begin = time.perf_counter()
            status, value = self.call(method, path, body, token or self.operator, timeout)
            self.log.append({"method": method, "path": path, "status": status,
                             "round_trip_ms": round((time.perf_counter() - begin) * 1000, 3)})
            if status in accept:
                return value
            delay = self.backoff * (attempt + 1)
            if status not in self.TRANSIENT or attempt + 1 == attempts or (
                    self.deadline is not None and self.clock() + delay >= self.deadline):
                raise ApiError(method, path, status, value)
            self.sleep(delay)

    def get(self, path, token=None):
        return self.request("GET", path, None, token)

    def post(self, path, body, token=None, accept=(200,)):
        return self.request("POST", path, body, token, accept)


def expand(text):
    """Corpus text macros: `@oversized:N`, `@long:N:text` and `@clip:N:source`."""
    if text.startswith("@oversized:"):
        size = int(text.split(":", 1)[1])
        return ("Confirmed. Keep every constraint. " + "Attached requisition log entry. " * (size // 31 + 1))[:size]
    if text.startswith("@long:"):
        _, size, body = text.split(":", 2)
        return (body + " ") + ("Detail paragraph retained for the full revision. " * (int(size) // 49 + 1))[
            : max(0, int(size) - len(body) - 1)]
    return text


class Experiment:
    """Registry entities and the subject token shared by every Run of one experiment."""

    def __init__(self, api, node, tenant, prefix):
        self.api = api
        self.node = node
        self.tenant = tenant
        self.prefix = prefix
        self.subject = "evaluator"
        self.token = None
        self.catalog_revision = {}

    def ref(self, identifier):
        return {"registry_node": self.node, "id": identifier, "version": "1.0.0"}

    def authorize(self, agent_ids):
        """Create the Tenant policy on a fresh disposable Node (deterministic tier only).

        The allow-all policy is acceptable only because the Node, its database
        and its Tenant exist solely for this invocation. The live tier never
        calls this and uses the operator's existing Tenant policy.
        """
        subjects = {self.subject: {"kind": "user"}}
        for agent in agent_ids:
            subjects[f"{self.node}/agents/{agent}@1.0.0"] = {"kind": "agent"}
        bundle = {"tenant": self.tenant, "subjects": subjects,
                  "policies": [{"id": "evaluation-work", "effect": "allow", "subjects": {"any": True},
                                "actions": ["*"], "resources": {"kinds": ["*"]}}]}
        self.api.post(f"/api/authorization/{self.tenant}", {"expected_revision": 0, "bundle": bundle})
        self.token = self.api.post(f"/api/authorization/{self.tenant}/credentials", {"subject": self.subject})["token"]

    def register(self, kind, identifier, config, schema=None, description="Fixed-task evaluation fixture",
                 name=None):
        entry = {"id": identifier, "version": "1.0.0", "kind": kind, "name": {"en": name or identifier},
                 "description": {"en": description}, "capabilities": [], "tags": ["evaluation"],
                 "languages": ["en", "ja"], "skills": [], "schema": schema if schema is not None else {},
                 "config": config}
        self.api.post("/api/registry", entry)
        self.api.post(f"/api/authorization/{self.tenant}/catalog",
                      {"entry": {"id": identifier, "version": "1.0.0"}, "expected_revision": 0, "enabled": True})
        return {"id": identifier, "version": "1.0.0", "config_sha256": canonical_sha256(config)}


def model_config(model, fixture_url, name):
    config = {"provider": "openrouter", "model_id": model["model_id"], "endpoint": fixture_url + "/v1",
              "credential_env": None, "context_window": model["context_window"],
              "max_output_tokens": model["max_output_tokens"], "modalities": ["text"], "cost": {},
              "projection_versions": ["legacy", "ordered"]}
    if model.get("cache_mode"):
        config["cache_mode"] = model["cache_mode"]
    return config


def fixture_model_ids(corpus, prefix):
    """Deterministic-tier model ids. `anthropic/` is required for explicit caching (ADR 0019)."""
    return {name: (f"anthropic/{prefix}-{name}" if spec.get("cache_mode") == "explicit" else f"fixture/{prefix}-{name}")
            for name, spec in corpus["models"].items()}


def tool_config(experiment, endpoint, alias):
    return {"registry_node": experiment.node, "provider": "integration.http@1", "operation": "invoke",
            "default_alias": alias, "tier": "integration", "narrow": {},
            "transport": {"transport": "http", "endpoint": endpoint, "credential_env": None, "replay": "idempotent"}}


def agent_config(corpus, task, model_ref, tool_refs, fields):
    bindings = [{"kind": "tool", "target": ref, "alias": alias, "narrow": {}} for alias, ref in tool_refs]
    config = {"schema_version": 1, "model": {"id": model_ref["id"], "version": model_ref["version"]},
              "instructions": corpus["instructions"], "bindings": bindings,
              "remove_default": task.get("remove_default", ["file_search", "file_read"]),
              "max_steps": task.get("max_steps", corpus["max_steps"])}
    config.update(json.loads(json.dumps(fields)))
    return config


def minimal_defaults(config):
    """Every implicit builtin the Agent configuration may remove, except the Skill support its bound Skills need.

    The live tier removes them all: delegation (`task_create`, `task_delegate`,
    `agent_discover`) would start child Runs on other Agents' models, and memory
    operations may call memory models, none of which the ledger reserves or the
    evaluator cancels.
    """
    exposure = config.get("exposure")
    deferred = isinstance(exposure, dict) and exposure.get("version") == "deferred@1"
    defaults = [name for name in DEFAULT_TOOLS if not (deferred and name in SKILL_TOOLS)]
    defaults += [SKILL_ASSET_READ] if deferred else []
    support = (SKILL_ASSET_READ,) if deferred else SKILL_TOOLS
    skills = any(binding["kind"] == "skill" for binding in config["bindings"])
    return [name for name in defaults if not (skills and name in support)]


def agent_id(prefix, planned):
    task_index = planned["task"]["_index"]
    return f"{prefix}-t{task_index:02d}-{planned['comparison']['id']}-{planned['arm']}"


def loaded_identity(call_name, result):
    """The Registry id a successful `capability_load` loaded (crates/aidash-domain/src/exposure.rs), else None."""
    if call_name != "capability_load" or not isinstance(result, dict) or result.get("error") is not None or \
            result.get("status") not in ("loaded", "already_loaded"):
        return None
    identity = result.get("identity")
    registry = identity.get("registry") if isinstance(identity, dict) else None
    return registry.get("id") if isinstance(registry, dict) and isinstance(registry.get("id"), str) else None


def tool_calls_from(context):
    calls = []
    for event in context.get("history", []):
        if event.get("kind") != "tool":
            continue
        result = event.get("result")
        error = result.get("error") if isinstance(result, dict) and isinstance(result.get("error"), str) else None
        # Rejected before dispatch: the tool never ran. Any other error may follow a partial effect.
        calls.append({"name": event["call"]["name"], "arguments": event["call"].get("arguments"), "error": error,
                      "dispatch_rejected": bool(error) and (error == APPROVAL_DENIED or error.startswith(
                          "unavailable tool ") or (error.startswith("capability ") and error.endswith(
                              ("use capability_load", "call it after the next model request")))),
                      "loaded": loaded_identity(event["call"]["name"], result)})
    return calls


def approval_tool(prompt):
    """The tool call a harness workbench approval prompt names, else None."""
    if not prompt.startswith(APPROVAL_PROMPT) or "; call: " not in prompt:
        return None
    try:
        call = json.loads(prompt.split("; call: ", 1)[1])
    except ValueError:
        return None
    return call["name"] if isinstance(call, dict) and isinstance(call.get("name"), str) else None


def final_answer(api, token, workspace, run):
    """The published final answer: the Run's output artifact, else its final context text."""
    detail = api.get(f"/api/workspaces/{workspace}", token)
    produced = [artifact for artifact in detail.get("artifacts", [])
                if artifact.get("run_id") == run["id"] or artifact.get("task_id") == run.get("task_id")]
    for artifact in reversed(produced):
        content = artifact.get("content")
        if isinstance(content, dict):
            content = content.get("text") or content.get("content") or json.dumps(content)
        if isinstance(content, str) and content:
            return content
    context = run.get("context") or {}
    for key in ("final", "output", "result"):
        if isinstance(context.get(key), str):
            return context[key]
    return None


def approves(task, prompt, approve_fixture_tools, prefix):
    """Approve only an exact external action of this experiment's own fixture tools.

    The deterministic tier enables this for its local fixture endpoints. The
    live tier never does, and escalation tasks are always denied, so the
    evaluator can never grant an external effect the scenario forbids.
    """
    return (approve_fixture_tools and task["kind"] != "escalation"
            and prompt.startswith(f"{APPROVAL_PROMPT}{prefix}-"))


def blank_observation():
    return {"run_id": None, "workspace": None, "phase": None, "error": None, "human_answers": [], "tool_calls": [],
            "final_answer": None, "steps": None, "compactions": None, "requests": None,
            "usage_records": None, "interrupted": False, "infrastructure_error": None, "cancellation_failed": False,
            "wall_ms": 0.0}


def stop_run(api, token, task_uuid, run_id, clock, sleep, claim_uncertain=False):
    """Cancel a Run the evaluator stops waiting for; return its latest state, or None without one.

    It runs after the Run's own deadline, so its requests get CANCEL_GRACE of their own. When the
    Tenant subject `token` cannot cancel (expired, rejected), the operator token is tried once.
    `claim_uncertain` means a claim was sent without a definite answer: it may have admitted a
    Run that appears later, so absence is polled for through the grace period and, if no Run
    appears, reported as an unresolved cancellation rather than as no Run.
    """
    if task_uuid is None:
        return None
    try:
        return cancel_run(api, token, task_uuid, run_id, clock, sleep, claim_uncertain)
    except Exception as failure:
        if token is None or token == api.operator:
            raise
        try:
            return cancel_run(api, None, task_uuid, run_id, clock, sleep, claim_uncertain)  # None: operator token.
        except Exception as fallback:
            raise fallback from failure


def cancel_run(api, token, task_uuid, run_id, clock, sleep, claim_uncertain=False):
    grace_deadline = clock() + CANCEL_GRACE
    grace = api.until(grace_deadline, clock, sleep)
    while run_id is None:
        run_id = next((run["id"] for run in grace.get("/api/state", token)["runs"] if run["task_id"] == task_uuid),
                      None)
        if run_id is not None:
            break
        if not claim_uncertain:
            return None
        if clock() >= grace_deadline:
            raise RuntimeError(f"a claim of Task {task_uuid} may have admitted a Run that did not appear within "
                               f"{CANCEL_GRACE}s, so it could not be cancelled")
        sleep(min(1.0, max(0.0, grace_deadline - clock())))
    run = grace.get(f"/api/runs/{run_id}", token)["run"]
    if run["phase"] in TERMINAL:
        return run
    grace.post(f"/api/runs/{run_id}/control", {"action": "cancel"}, token)
    return grace.get(f"/api/runs/{run_id}", token)["run"]


def record_final_state(observation, run):
    """Copy a Run's final phase, steps, compactions and tool calls into its observation."""
    context = run.get("context") or {}
    observation.update({"run_id": run["id"], "phase": run["phase"],
                        "error": run.get("error") or run.get("state_error"), "steps": run.get("step"),
                        "compactions": context.get("compactions"), "tool_calls": tool_calls_from(context)})


def run_task(api, experiment, planned, repeat, agent_ref, fixture=None, usage_reader=None, timeout=300.0,
             interval=1.0, clock=time.monotonic, sleep=time.sleep, approve_fixture_tools=False, observation=None,
             after_claim=None):
    """Execute one Task Run end to end and return raw observations; never raises for Run failures.

    Every request ends by the Run's deadline. When the deadline passes, even
    inside a stalled request, when a request fails, and when an interrupt
    (Ctrl-C, SIGTERM) unwinds through here, the Run is cancelled first, so it
    cannot keep doing provider work after the evaluator stops waiting for it;
    an interrupt is then re-raised. `observation`, when given, is filled in
    place, so a caller still holds the Run's record after an interrupt.
    `after_claim(api)`, when given, runs once the claim has admitted the Run;
    an exception from it is an infrastructure failure, so the Run is cancelled.
    """
    task = planned["task"]
    token = experiment.token
    observation = blank_observation() if observation is None else observation
    started = clock()
    deadline = started + timeout
    bounded = api.until(deadline, clock, sleep)
    answers = iter(task.get("human_answers", []))
    task_uuid = None
    # True from the moment a claim is sent until it is definitely accepted or rejected.
    claim_uncertain = False
    try:
        try:
            title = f"{experiment.prefix} {task['id']} {planned['label']} r{repeat}"
            workspace = bounded.post("/api/workspaces", {"title": title, "goal": task["title"]}, token)["id"]
            observation["workspace"] = workspace
            created = bounded.post(f"/api/workspaces/{workspace}/tasks", {
                "title": task["title"], "description": task_description(planned["corpus"], task), "requirements": {},
                "dependencies": [], "parent_id": None}, token)
            task_uuid = created["id"]
            if fixture is not None:
                fixture.register(task_uuid, task, f"{task['id']}/{planned['comparison']['id']}/{planned['arm']}/{repeat}")
            claim = {"revision": created.get("revision", 0),
                     "agent": {"id": agent_ref["id"], "version": agent_ref["version"]}}
            for attempt in range(api.retries + 1):
                claim_uncertain = True
                try:
                    bounded.post(f"/api/tasks/{task_uuid}/claim", claim, token)
                    break
                except ApiError as error:
                    # A timed-out claim may still have committed: retry only when no Run exists.
                    if error.status not in Api.TRANSIENT and error.status < 500:
                        claim_uncertain = False  # Rejected: no Run was admitted.
                    if error.status not in Api.TRANSIENT or attempt == api.retries:
                        raise
                    # A committed claim may already be doing paid work, so the backoff never crosses
                    # the deadline: when it would, stop now and let the deadline path cancel the Run.
                    delay = api.backoff * (attempt + 1)
                    if clock() + delay >= deadline:
                        raise DeadlineExceeded(f"retrying the claim of Task {task_uuid} would pass the deadline")
                    sleep(delay)
                    if any(run["task_id"] == task_uuid for run in bounded.get("/api/state", token)["runs"]):
                        break
            run_id = None
            while run_id is None:
                run_id = next((run["id"] for run in bounded.get("/api/state", token)["runs"]
                               if run["task_id"] == task_uuid), None)
                if run_id is None:
                    if clock() > deadline:
                        raise RuntimeError(f"no Run appeared for Task {task_uuid}")
                    sleep(min(interval, max(0.0, deadline - clock())))
            observation["run_id"] = run_id
            claim_uncertain = False
            if after_claim is not None:
                after_claim(bounded)
            while True:
                run = bounded.get(f"/api/runs/{run_id}", token)["run"]
                if run["phase"] in TERMINAL:
                    break
                if clock() > deadline:
                    raise DeadlineExceeded(f"Run {run_id} passed its {timeout}s time limit")
                if run["phase"] == "WAITING":
                    for request in bounded.get("/api/state", token)["human_requests"]:
                        if request["run_id"] != run_id or request.get("response") is not None:
                            continue
                        if request["kind"] == "APPROVAL_REQUIRED":
                            answer = {"approved": approves(task, request["prompt"], approve_fixture_tools,
                                                           experiment.prefix)}
                        else:
                            text = next(answers, None)
                            answer = {"answer": expand(text) if text is not None else NO_HUMAN}
                        bounded.post(f"/api/human-requests/{request['id']}/answer", answer, token)
                        # The prompt stays in memory for grading (it names the action asked about);
                        # run_result keeps only its digest in the manifest.
                        observation["human_answers"].append({"kind": request["kind"],
                                                             "answer_sha256": canonical_sha256(answer),
                                                             "approved": answer.get("approved"),
                                                             "prompt": request["prompt"],
                                                             "tool": approval_tool(request["prompt"])})
                # Never sleep across the deadline: the next poll then notices it and cancels at once.
                sleep(min(interval, max(0.0, deadline - clock())))
        except Exception as error:
            # Past the deadline, whether a poll noticed it or a request stalled into it.
            if not isinstance(error, DeadlineExceeded) and clock() < deadline:
                raise
            run = stop_run(api, token, task_uuid, observation["run_id"], clock, sleep, claim_uncertain)
            if run is None:
                raise
            observation["interrupted"] = True
        # The Run is finished or cancelled: reading its outcome may take a grace period past the deadline.
        reader = api.until(max(deadline, clock() + CANCEL_GRACE), clock, sleep)
        record_final_state(observation, run)
        if run["phase"] == "COMPLETED":
            observation["final_answer"] = final_answer(reader, token, observation["workspace"], run)
        if fixture is not None:
            observation["requests"] = fixture.observed(task_uuid)
        if usage_reader is not None:
            observation["usage_records"] = usage_reader(observation["run_id"])
    except Exception as error:  # Counted as an infrastructure failure, never dropped.
        observation["infrastructure_error"] = f"{type(error).__name__}: {error}"
        # A failed request says nothing about the Run: it may still be active, so stop it
        # before the caller moves on (stop_run leaves a finished Run alone).
        try:
            run = stop_run(api, token, task_uuid, observation["run_id"], clock, sleep, claim_uncertain)
            # The final state still shows what the Run did, compactions included, for live_runs to check.
            if run is not None:
                record_final_state(observation, run)
        except Exception as cancel_error:
            # The Run may still be running: the caller must not dispatch more work after it.
            observation["cancellation_failed"] = True
            observation["infrastructure_error"] += f"; cancellation failed: {type(cancel_error).__name__}: " \
                                                   f"{cancel_error}"
    except BaseException:  # Ctrl-C or SIGTERM: stop the Run, keep its record, then unwind.
        observation["interrupted"] = True
        try:
            run = stop_run(api, token, task_uuid, observation["run_id"], clock, sleep, claim_uncertain)
            if run is not None:
                record_final_state(observation, run)
        except Exception as error:
            observation["cancellation_failed"] = True
            observation["infrastructure_error"] = f"cancellation after the interrupt failed: {type(error).__name__}: " \
                                                  f"{error}"
        observation["wall_ms"] = round((clock() - started) * 1000, 3)
        raise
    observation["wall_ms"] = round((clock() - started) * 1000, 3)
    return observation


def register_variants(experiment, corpus, planned_runs, model_refs, fixture_url, remove_all_defaults=False):
    """Register tools and one Agent per (task, comparison, arm). Returns variants and per-comparison errors.

    The errors are Agent registrations the Node rejected for arms the corpus
    marks available. Callers decide whether that is a failure; the deterministic
    command treats it as one unless explicitly told the arm is expected to be
    rejected. A comparison is run paired or not at all: when any of its arms is
    rejected, none of its arms stays runnable or is listed as a variant. With
    `remove_all_defaults` (the live tier), each Agent keeps only the builtins it
    cannot remove and the Skill support its bound Skills need.
    """
    tools = {}
    skills = {}
    variants = []
    errors = {}
    corpus_skills = {skill["id"]: skill for skill in corpus.get("skills", [])}
    for planned in planned_runs:
        task = planned["task"]
        if task["id"] not in skills:
            skills[task["id"]] = []
            for name in task.get("skills", []):
                skill = corpus_skills[name]
                identifier = f"{experiment.prefix}-t{task['_index']:02d}-skill-{name}"
                entry = experiment.register("skill", identifier, {"instructions": skill["instructions"]},
                                            {"type": "object"}, skill["description"], skill["name"])
                skills[task["id"]].append(entry)
        if task["id"] not in tools:
            refs = []
            for tool in task_tools(corpus, task):
                identifier = f"{experiment.prefix}-t{task['_index']:02d}-{tool['alias']}".replace("_", "-")
                endpoint = f"{fixture_url}/tools/{task['id']}/{tool['alias']}"
                registration = experiment.register("tool", identifier, tool_config(experiment, endpoint, tool["alias"]),
                                                   json.loads(tool["schema"]), tool["description"])
                refs.append((tool["alias"], registration))
            tools[task["id"]] = refs
        model_ref = model_refs[planned["model"]]
        bindings = [(alias, experiment.ref(ref["id"])) for alias, ref in tools[task["id"]]]
        config = agent_config(corpus, task, model_ref, bindings, planned["agent_fields"])
        config["bindings"] += [{"kind": "skill", "target": experiment.ref(ref["id"]), "narrow": {}}
                               for ref in skills[task["id"]]]
        if remove_all_defaults:
            config["remove_default"] = minimal_defaults(config)
        identifier = agent_id(experiment.prefix, planned)
        try:
            agent = experiment.register("agent", identifier, config)
        except ApiError as error:
            errors[planned["comparison"]["id"]] = f"Agent registration rejected: {str(error)[:300]}"
            continue
        planned["agent_ref"] = agent
        # Grading needs the Registry ids a deferred Skill load must name.
        planned["skill_refs"] = skills[task["id"]]
        variants.append({"task": task["id"], "comparison": planned["comparison"]["id"], "arm": planned["arm"],
                         "label": planned["label"], "agent": agent, "agent_fields": planned["agent_fields"],
                         "model": model_ref, "tools": [ref for _, ref in tools[task["id"]]],
                         "skills": skills[task["id"]]})
    if errors:
        for planned in planned_runs:
            if planned["comparison"]["id"] in errors:
                planned.pop("agent_ref", None)
        variants = [variant for variant in variants if variant["comparison"] not in errors]
    return variants, errors


# ------------------------------------------------------------- memory


def memory_policy(roles, variant):
    zero = {"input_per_million": 0, "output_per_million": 0}
    policy = {"engine": "hindsight_rust", "policy": {
        "extraction": roles["model"], "derivation": roles["model"], "reflection": roles["model"],
        "embedding": roles["embedding"], "reranker": roles["reranker"], "tokenizer": roles["tokenizer"],
        "prices": {name: zero for name in ("extraction", "derivation", "reflection", "embedding", "reranker")},
        "retention": {"unit_max_age_days": None, "candidate_days": 7, "history_days": 30, "history_versions": 16,
                      "model_result_days": 7, "backup_days": 7, "purge_after_seconds": 60, "purge_batch": 32,
                      "max_unit_records": 128, "max_model_operations": 1024},
        "bounds": {"max_unit_bytes": 8192, "max_input_bytes": 8192, "max_units": 16, "max_candidates": 8,
                   "max_entities": 8, "max_evidence": 8, "max_links": 8, "max_graph_hops": 3,
                   "max_graph_visits": 32, "max_results": 4, "max_context_tokens": 8192, "max_model_calls": 4,
                   "max_model_tokens": 8192, "max_cost_micros": 10000, "max_retries": 2, "max_call_seconds": 30},
        "semantic_link_min_similarity_millionths": 700000, "learn_from_runs": False, "maintain_observations": False,
        "refresh_mental_models": False}}
    if variant.get("decay"):
        policy["policy"]["decay"] = dict(variant["decay"], dormancy=None)
    return policy


def unit_content(text):
    return {"text": text, "kind": "world", "learning": "fact", "verification": "unverified", "occurred": None,
            "entities": [], "evidence": [], "links": [], "mental_model": None}


def run_memory_scenario(api, token, workspace, provider, bank, scenario):
    """Apply a scenario's ordered actions to one isolated bank; return checks."""
    units = {source["id"]: str(uuid.uuid4()) for source in scenario.get("sources", [])}
    texts = {source["id"]: expand(source["text"]) for source in scenario.get("sources", [])}
    sent = {}
    checks = []
    path = f"/api/workspaces/{workspace}/memory/units/mutate"
    for index, action in enumerate(scenario["actions"]):
        op = action["op"]
        name = action.get("label") or f"{index}:{op}"
        if op == "query":
            listed = api.post(f"/api/workspaces/{workspace}/memory/units/query", {"provider": provider, "bank": bank},
                              token)
            visible = {}
            for unit in listed:
                symbol = next((key for key, value in units.items() if value == unit["id"]), unit["id"])
                visible[symbol] = unit["content"]["text"]
            expected = {key: expand(value) for key, value in action["visible"].items()}
            checks.append({"action": name, "check": "visible-set", "passed": visible == expected,
                           "observed": sorted(visible), "expected": sorted(expected)})
            continue
        if op == "replay":
            body, original_status, original = sent[action["of"]]
        else:
            source = action["source"]
            text = action.get("text")
            if text and text.startswith("@clip:"):
                _, size, symbol = text.split(":", 2)
                text = texts[symbol][: int(size)]
            elif text:
                text = expand(text)
            if op == "add":
                change = {"operation": "add", "id": units[source], "content": unit_content(texts[source])}
            elif op == "correct":
                change = {"operation": "correct", "id": units[source], "expected_revision": action["observed_revision"],
                          "content": unit_content(text)}
            elif op == "delete":
                change = {"operation": "delete", "id": units[source], "expected_revision": action["observed_revision"]}
            else:
                raise ValueError(f"unknown memory action {op}")
            body = {"operation_id": str(uuid.uuid4()), "provider": provider, "bank": bank, "changes": [change]}
            original_status = original = None
        status, value = api.call("POST", path, body, token, api.request_timeout)
        api.log.append({"method": "POST", "path": path, "status": status, "round_trip_ms": None})
        if op != "replay":
            sent[name] = (body, status, value)
        expect = action["expect"]
        if expect == "ok":
            passed = status == 200
            if passed and "revision" in action:
                revisions = [item.get("revision") for item in value] if isinstance(value, list) else []
                passed = revisions == [action["revision"]]
        elif expect == "rejected":
            passed = 400 <= status < 500
        elif expect == "identical":
            passed = status == 200 and original_status == 200 and value == original
        elif expect == "no-effect":
            passed = (status == 200 and value == original) or 400 <= status < 500
        else:
            raise ValueError(f"unknown memory expectation {expect}")
        checks.append({"action": name, "check": expect, "passed": passed, "status": status})
    return checks, canonical_sha256({"sources": scenario.get("sources", []), "actions": scenario["actions"]})


# ------------------------------------------------------------ live tier


def run_worst_case(task, corpus, model_config, price):
    """Worst-case nanocredits for one Run.

    Every provider attempt of every step, including each harness retry,
    fills the context window at the dearer of the input and cache-write rates
    and spends the whole output reservation.
    """
    steps = task.get("max_steps", corpus["max_steps"])
    prompt_rate = max(price["input_nanocredits_per_token"], price["cache_write_nanocredits_per_token"])
    per_attempt = (model_config["context_window"] * prompt_rate
                   + model_config["max_output_tokens"] * price["output_nanocredits_per_token"])
    return steps * PROVIDER_ATTEMPTS_PER_STEP * per_attempt


def observed_provider_work(api, token, workspace, run_id):
    """Accepted inference attempts the Node reported for a Run, from its Workspace events.

    Informational only. `model.completed` is emitted for accepted attempts, so
    interrupted or retried attempts do not appear, and its payload carries no
    provider-reported model or route. The ledger therefore never prices a Run
    from this list.
    """
    events = api.get(f"/api/workspaces/{workspace}", token).get("events", [])
    attempts = []
    for event in events:
        data = event.get("data") or {}
        if event.get("kind") != "model.completed" or data.get("run_id") != run_id:
            continue
        usage = data.get("context_usage") or {}
        attempts.append({"step": data.get("step"), "input_tokens": usage.get("input_tokens"),
                         "output_tokens": usage.get("output_tokens")})
    return attempts


def live_runs(api, experiment, corpus, planned_runs, repeats, ledger, models, prices, timeout, observations,
              clock=time.monotonic, sleep=time.sleep, binding_check=None):
    """Dispatch live Runs only inside the ledger's spend and time ceilings.

    Each Run is reserved at its worst case before dispatch; a refusal stops all
    further dispatch. Every Run, completed or not, stays charged at its full
    reservation as uncertain: Usage Records have no HTTP read endpoint yet, and
    the Workspace events omit interrupted and retried attempts, so no observed
    cost may release headroom under the hard ceiling.

    `binding_check(api)` raises GuardRefusal unless the Tenant's Provider
    Credential Binding still has the approved credential and revision. It runs
    before each reservation and again once the claim has admitted the Run.
    Admission pins the binding inside the claim and the revision only grows,
    so passing both reads proves the Run pinned the approved credential; a
    failed second read cancels the Run and stops all further dispatch. A Run
    that applied a compaction proves the Node's Jev compactor answered, which
    the approval attests cannot happen, and also stops all further dispatch.

    Appends each finished (planned, repeat, observation) to `observations`,
    which the caller owns so that an exception or interrupt cannot lose the
    record of work already dispatched. Returns the (key, reason) of every Run
    that was planned but never dispatched.
    """
    from fixed_task_evaluation import GuardRefusal
    skipped = []
    refusal = None
    for repeat in range(1, repeats + 1):
        for planned in planned_runs:
            if "agent_ref" not in planned:
                continue
            key = f"{planned['task']['id']}/{planned['comparison']['id']}/{planned['arm']}/{repeat}"
            if refusal is not None:
                skipped.append((key, refusal))
                continue
            model = planned["live_model"]
            if binding_check is not None:
                binding_check(api)
            try:
                attempt = ledger.reserve(key, run_worst_case(planned["task"], corpus, models[model], prices[model]))
            except GuardRefusal as error:
                refusal = f"not dispatched after refusal: {error}"
                skipped.append((key, refusal))
                continue
            observation = blank_observation()
            stops = []

            def after_claim(bounded):
                try:
                    binding_check(bounded)
                except GuardRefusal as error:
                    stops.append(f"Run {key} may have pinned an unapproved credential: {error}")
                    raise
            try:
                # A Run's time limit never reaches past the wall-time deadline or the approval's expiry.
                run_task(api, experiment, planned, repeat, planned["agent_ref"],
                         timeout=min(timeout, ledger.remaining()), clock=clock, sleep=sleep, observation=observation,
                         after_claim=after_claim if binding_check is not None else None)
                if observation.get("cancellation_failed"):
                    stops.append(f"Run {key} may still be active: the evaluator could not cancel it with the subject "
                                 "or the operator token")
                if observation.get("compactions"):
                    stops.append(f"Run {key} applied {observation['compactions']} compaction(s), so the Node's Jev "
                                 "compactor answered although the approval attests external compaction is disabled")
                if observation["run_id"] and observation.get("workspace"):
                    try:
                        attempt["provider_work"] = observed_provider_work(api, experiment.token,
                                                                          observation["workspace"],
                                                                          observation["run_id"])
                    except Exception as error:  # Informational only; the reservation stays charged.
                        attempt["provider_work_error"] = f"{type(error).__name__}: {error}"
            except BaseException:  # Ctrl-C, SIGTERM and the like: the in-flight Run is counted, never dropped.
                observation["interrupted"] = True
                raise
            finally:
                observations.append((planned, repeat, observation))
                ledger.settle(attempt, None)
            if stops:
                raise GuardRefusal("; ".join(stops))
    return skipped


def binding_guard(tenant, provider, expected):
    """A `binding_check` for live_runs: the binding must still name `expected` credential id and revision."""
    from fixed_task_evaluation import GuardRefusal

    def check(api):
        current = api.get(f"/api/tenants/{tenant}/provider-credential-bindings/{provider}")
        observed = (current.get("provider_credential_id"), current.get("revision")) if isinstance(current, dict) \
            else None
        if observed != expected:
            raise GuardRefusal(f"the Tenant's {provider} Provider Credential Binding changed from {expected} to "
                               f"{observed} (credential id, revision)")
    return check
