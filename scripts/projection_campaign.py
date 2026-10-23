"""Campaign planning and durable shared limits for matched projection experiments.

Planning is offline. A reservation is never refunded from an accepted-response
event: it may omit retried or interrupted work. An interrupted phase therefore
keeps its full reservation across process restarts.
"""
import contextlib
import copy
import datetime
import hashlib
import json
import math
import os
from pathlib import Path
import re
import secrets
import subprocess
import sys
import tomllib

import fixed_task_evaluation as evaluation
import fixed_task_runner as runner

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_CONFIG = ROOT / "tests/evaluation/projection-campaign.toml"
MAX_NANOCREDITS = 50_000_000_000
MAX_SECONDS = 14_400
CONFIG_VERSION = 1
STATE_SCHEMA = "aidash-projection-campaign-state-v1"
PLAN_SCHEMA = "aidash-projection-campaign-plan-v1"
FINGERPRINT = re.compile(r"[0-9a-f]{64}")
PRIMARY_WEIGHTS = {
    "code-change-hidden-regression": 0.125,
    "code-change-verified-fix": 0.125,
    "structured-data-overdue-totals": 0.25,
    "long-conversation-corrections": 0.25,
    "correction-lost-in-pruned-tool-result": 0.25,
}
CONTROL_TASKS = ("terminal-context-overflow", "approval-escalation", "step-limit-unresolved")
ARMS = {"automatic": ["legacy", "ordered", "native"],
        "explicit": ["legacy", "ordered", "native", "ordered-explicit", "native-explicit"]}
CAMPAIGN_FILES = ("scripts/projection_campaign.py", "scripts/evaluate-projections.py",
                  "scripts/projection_statistics.py", "scripts/projection_evaluation.py",
                  "tests/evaluation/projection-campaign.toml")


class CampaignRefusal(ValueError):
    """A campaign cannot safely or reproducibly perform the requested step."""


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":"),
                                     ensure_ascii=False, allow_nan=False).encode()).hexdigest()


def number(value, label, *, integer=False, minimum=0):
    if isinstance(value, bool) or not isinstance(value, int if integer else (int, float)) \
            or not math.isfinite(value) or value < minimum:
        raise CampaignRefusal(f"{label} must be a finite {'integer' if integer else 'number'} >= {minimum}")
    return value


def utc_now():
    return datetime.datetime.now(datetime.timezone.utc)


def instant(value):
    try:
        result = datetime.datetime.fromisoformat(value.replace("Z", "+00:00")) if isinstance(value, str) else value
        if not isinstance(result, datetime.datetime) or result.tzinfo is None or result.utcoffset() is None:
            raise ValueError("timezone absent")
        return result.astimezone(datetime.timezone.utc)
    except (TypeError, ValueError, OverflowError) as error:
        raise CampaignRefusal("campaign timestamps require an explicit UTC offset") from error


def read_json(path, maximum_bytes=32 * 1024 * 1024):
    path = Path(path)
    with path.open("rb") as stream:
        data = stream.read(maximum_bytes + 1)
    if len(data) > maximum_bytes:
        raise CampaignRefusal(f"JSON input exceeds {maximum_bytes} bytes")

    def invalid(value):
        raise CampaignRefusal(f"non-finite JSON number {value}")

    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise CampaignRefusal(f"duplicate JSON key {key}")
            result[key] = value
        return result

    return json.loads(data, parse_constant=invalid, object_pairs_hook=unique)


def write_json(path, value, *, exclusive=False):
    """Write private JSON atomically; immutable evidence uses exclusive creation."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    data = (json.dumps(value, indent=2, ensure_ascii=False, allow_nan=False) + "\n").encode()
    if exclusive:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "wb") as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        return
    temporary = path.with_name(f".{path.name}.{secrets.token_hex(8)}.tmp")
    try:
        write_json(temporary, value, exclusive=True)
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


@contextlib.contextmanager
def state_lock(path):
    """One writer per campaign; a crashed writer requires explicit reconciliation."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    lock = path.with_name(path.name + ".lock")
    try:
        descriptor = os.open(lock, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError as error:
        raise CampaignRefusal("campaign state is locked; inspect the prior process and reconcile its work") from error
    try:
        with os.fdopen(descriptor, "w") as stream:
            json.dump({"pid": os.getpid(), "created_at": utc_now().isoformat()}, stream)
        yield
    finally:
        lock.unlink(missing_ok=True)


class SharedBudget:
    """A persisted upper-bound ledger shared by every paid campaign phase."""

    def __init__(self, value):
        self.value = copy.deepcopy(value)

    @classmethod
    def create(cls, plan_sha256, nanocredits=MAX_NANOCREDITS, seconds=MAX_SECONDS, *, now=None):
        number(nanocredits, "spend ceiling", integer=True, minimum=1)
        number(seconds, "time ceiling", minimum=1)
        if nanocredits > MAX_NANOCREDITS or seconds > MAX_SECONDS:
            raise CampaignRefusal("campaign ceiling exceeds the agreed USD 50 / four hours")
        if not isinstance(plan_sha256, str) or not FINGERPRINT.fullmatch(plan_sha256):
            raise CampaignRefusal("campaign needs a full plan fingerprint")
        timestamp = instant(now or utc_now()).isoformat()
        return cls({"schema": STATE_SCHEMA, "plan_sha256": plan_sha256,
                    "ceiling_nanocredits": nanocredits, "ceiling_seconds": seconds,
                    "started_at": timestamp, "last_observed_at": timestamp, "reservations": []})

    @classmethod
    def from_document(cls, value, plan_sha256, *, now=None):
        if not isinstance(value, dict) or value.get("schema") != STATE_SCHEMA:
            raise CampaignRefusal("invalid campaign state schema")
        if value.get("plan_sha256") != plan_sha256:
            raise CampaignRefusal("campaign state plan fingerprint differs")
        restored = cls.create(plan_sha256, value.get("ceiling_nanocredits"), value.get("ceiling_seconds"),
                              now=instant(value.get("started_at")))
        if not isinstance(value.get("reservations"), list):
            raise CampaignRefusal("campaign reservations must be a list")
        keys = set()
        for item in value["reservations"]:
            if not isinstance(item, dict):
                raise CampaignRefusal("invalid campaign reservation")
            key = (item.get("phase"), item.get("route"))
            if key in keys or key[0] not in ("preflight", "pilot", "confirmation", "deployment-mode") \
                    or key[1] not in ARMS:
                raise CampaignRefusal("invalid or duplicate campaign reservation")
            keys.add(key)
            number(item.get("nanocredits"), "reserved spend", integer=True)
            number(item.get("seconds"), "reserved duration", minimum=1)
            if item.get("status") not in ("reserved", "finished", "uncertain") \
                    or not isinstance(item.get("output"), str) or not item["output"]:
                raise CampaignRefusal("invalid campaign reservation state")
            code = item.get("exit_code")
            if code is not None and (isinstance(code, bool) or not isinstance(code, int)):
                raise CampaignRefusal("invalid campaign exit code")
        restored.value = copy.deepcopy(value)
        if restored.remaining_nanocredits < 0:
            raise CampaignRefusal("campaign reservations exceed the spend ceiling")
        restored.remaining_seconds(now=now)
        return restored

    @property
    def remaining_nanocredits(self):
        return self.value["ceiling_nanocredits"] - sum(x["nanocredits"] for x in self.value["reservations"])

    def remaining_seconds(self, *, now=None):
        current = instant(now or utc_now())
        last = instant(self.value["last_observed_at"])
        start = instant(self.value["started_at"])
        if last < start or current < last:
            raise CampaignRefusal("campaign clock moved backwards; elapsed budget cannot be released")
        self.value["last_observed_at"] = current.isoformat()
        return max(0, self.value["ceiling_seconds"] - (current - start).total_seconds())

    def can_fit_phase(self, total_worst_case_nanocredits, upper_wall_seconds, *, now=None):
        number(total_worst_case_nanocredits, "phase budget", integer=True)
        number(upper_wall_seconds, "phase duration", minimum=1)
        return total_worst_case_nanocredits <= self.remaining_nanocredits \
            and upper_wall_seconds <= self.remaining_seconds(now=now)

    def reserve(self, phase, route, nanocredits, seconds, output, *, now=None):
        if phase not in ("preflight", "pilot", "confirmation", "deployment-mode") or route not in ARMS:
            raise CampaignRefusal("invalid campaign phase or route")
        if any(x["phase"] == phase and x["route"] == route for x in self.value["reservations"]):
            raise CampaignRefusal("phase/route already reserved; reconcile it instead of sampling again")
        if not self.can_fit_phase(nanocredits, seconds, now=now):
            raise CampaignRefusal("complete phase does not fit the remaining shared budget or deadline")
        if not isinstance(output, str) or not output:
            raise CampaignRefusal("phase output path is required")
        self.value["reservations"].append({"phase": phase, "route": route, "nanocredits": nanocredits,
                                           "seconds": seconds, "output": output, "status": "reserved",
                                           "exit_code": None})

    def finish(self, phase, route, *, exit_code, now=None):
        self.remaining_seconds(now=now)
        item = next((x for x in self.value["reservations"] if x["phase"] == phase and x["route"] == route), None)
        if item is None:
            raise CampaignRefusal("phase was never reserved")
        if item["status"] != "reserved":
            raise CampaignRefusal("phase is already terminal; reconciliation must not rewrite its outcome")
        if isinstance(exit_code, bool) or not isinstance(exit_code, int):
            raise CampaignRefusal("phase needs an integer exit code")
        item["status"] = "finished" if exit_code == 0 else "uncertain"
        item["exit_code"] = exit_code
        # Even success does not prove that every internal provider attempt reported usage.

    def document(self):
        return copy.deepcopy(self.value)


def validate_config(config):
    if not isinstance(config, dict) or config.get("schema_version") != CONFIG_VERSION:
        raise CampaignRefusal("unsupported campaign configuration")
    if config.get("pilot_repeats") != 10 or isinstance(config.get("pilot_repeats"), bool) \
            or config.get("primary_estimator") != "bytes":
        raise CampaignRefusal("campaign requires ten pilot repetitions and Bytes primary execution")
    number(config.get("execution_seed"), "execution seed", integer=True)
    if not isinstance(config.get("independent_blocks", False), bool) or not isinstance(
            config.get("independence_evidence", ""), str):
        raise CampaignRefusal("independence needs a boolean assessment and a textual evidence declaration")
    if config.get("independent_blocks") is True and not config.get("independence_evidence", "").strip():
        raise CampaignRefusal("independence cannot be asserted without predeclared supporting evidence")
    if config.get("required_routes") != ["automatic", "explicit"] or config.get("primary_weights") != PRIMARY_WEIGHTS:
        raise CampaignRefusal("required routes and primary weights differ from the accepted protocol")
    limits = config.get("limits", {})
    spend = number(limits.get("max_spend_nanocredits"), "spend ceiling", integer=True, minimum=1)
    seconds = number(limits.get("max_wall_seconds"), "wall ceiling", minimum=1)
    number(limits.get("run_timeout_seconds"), "Run timeout", minimum=1)
    if spend > MAX_NANOCREDITS or seconds > MAX_SECONDS:
        raise CampaignRefusal("configured limits exceed the shared campaign ceilings")
    if set(config.get("routes", {})) != set(ARMS):
        raise CampaignRefusal("campaign requires exactly the automatic and explicit routes")
    for route, expected_arms in ARMS.items():
        settings = config["routes"][route]
        if settings.get("arms") != expected_arms:
            raise CampaignRefusal(f"{route} arms differ from the matched design")
        if settings.get("cache_mode") != route or settings.get("reasoning_effort") != "medium":
            raise CampaignRefusal(f"{route} must pin caching mode and reasoning_effort medium")
        for field in ("upstream_model", "provider_tag", "comparison"):
            if not isinstance(settings.get(field), str) or not settings[field]:
                raise CampaignRefusal(f"{route} needs {field}")
    return config


def load_config(path=DEFAULT_CONFIG):
    with Path(path).open("rb") as stream:
        return validate_config(tomllib.load(stream))


def campaign_identity():
    files = {}
    for relative in CAMPAIGN_FILES:
        path = ROOT / relative
        if not path.is_file():
            raise CampaignRefusal(f"campaign implementation is incomplete: {relative}")
        files[relative] = hashlib.sha256(path.read_bytes()).hexdigest()
    return {"version": "1.0.0", "sha256": digest(files), "files": files}


def _arms(comparison):
    if "arms" in comparison:
        return [(x["id"], x) for x in comparison["arms"]]
    return [(name, comparison[name]) for name in ("baseline", "candidate")]


def make_plan(config, corpus, *, source=None, evaluator=None):
    """Freeze all pilot trial keys without connecting to a Node or provider."""
    validate_config(config)
    source = source or evaluation.source_identity()
    evaluator = evaluator or evaluation.pin_evaluator()
    tasks = {x["id"]: x for x in corpus["tasks"]}
    required = set(PRIMARY_WEIGHTS) | set(CONTROL_TASKS)
    if not required <= tasks.keys():
        raise CampaignRefusal("corpus omits required projection tasks")
    comparisons = {x["id"]: x for x in corpus["comparisons"]}
    routes = {}
    for route, settings in config["routes"].items():
        comparison = comparisons.get(settings["comparison"])
        if comparison is None or [x[1]["label"] for x in _arms(comparison)] != settings["arms"]:
            raise CampaignRefusal(f"corpus has no matching {route} three/five-arm comparison")
        selected = [task for task in corpus["tasks"] if settings["comparison"] in task["comparisons"]]
        if not required <= {x["id"] for x in selected}:
            raise CampaignRefusal(f"{route} comparison omits required tasks")
        planned = []
        for repeat in range(1, config["pilot_repeats"] + 1):
            for task in selected:
                for arm, variant in _arms(comparison):
                    planned.append({"task": task["id"], "arm": arm, "label": variant["label"],
                                    "repeat": repeat,
                                    "key": evaluation.run_key(task["id"], settings["comparison"], arm, repeat)})
        routes[route] = dict(copy.deepcopy(settings), model=comparison.get("model"),
                             model_by_task_model=copy.deepcopy(comparison.get("model_by_task_model", {})),
                             task_ids=[x["id"] for x in selected],
                             trials=planned, planned_runs=len(planned))
    plan = {"schema": PLAN_SCHEMA, "created_at": utc_now().isoformat(), "source": source,
            "corpus": corpus["_snapshot"], "evaluator": evaluator, "config": copy.deepcopy(config),
            "campaign_evaluator": campaign_identity(),
            "config_sha256": digest(config), "routes": routes, "phase": "pilot",
            "repeats": config["pilot_repeats"], "candidate": None,
            "paid_execution_ready": False, "default_change": False,
            "execution_prerequisites": ["clean deployed source and immutable model registrations",
                                        "Tenant and connected Provider Credential",
                                        "approved exact disclosure manifest and price ceilings",
                                        "complete phase worst-case spend/time fit",
                                        "prompt-free operator evidence export"]}
    plan["plan_sha256"] = digest(plan)
    return plan


def validate_plan(plan):
    if not isinstance(plan, dict) or plan.get("schema") != PLAN_SCHEMA:
        raise CampaignRefusal("unsupported campaign plan")
    expected = plan.get("plan_sha256")
    payload = {k: v for k, v in plan.items() if k != "plan_sha256"}
    if not isinstance(expected, str) or digest(payload) != expected:
        raise CampaignRefusal("campaign plan fingerprint differs")
    validate_config(plan.get("config"))
    if plan.get("config_sha256") != digest(plan["config"]):
        raise CampaignRefusal("campaign configuration fingerprint differs")
    if plan.get("phase") not in ("pilot", "confirmation", "deployment-mode"):
        raise CampaignRefusal("invalid campaign phase")
    return plan


def phase_reservations(plan, corpus, execution):
    """Price a complete phase from pinned model definitions, not accepted-response events."""
    validate_plan(plan)
    if not isinstance(execution, dict) or execution.get("plan_sha256") != plan["plan_sha256"]:
        raise CampaignRefusal("execution manifest must name this exact plan fingerprint")
    routes = execution.get("routes")
    if not isinstance(routes, dict) or set(routes) != set(ARMS):
        raise CampaignRefusal("execution manifest must configure both required routes")
    tasks = {x["id"]: x for x in corpus["tasks"]}
    result = {}
    for route, settings in plan["routes"].items():
        entry = routes[route]
        approval = read_json(entry["disclosure_approval"])
        auxiliary = evaluation.auxiliary_allowance(approval)
        evaluation.verify_auxiliary_configs(auxiliary, entry.get("auxiliary_models", {}), "openrouter")
        mapping, models, prices = entry.get("model_refs", {}), entry.get("models", {}), entry.get("prices", {})
        if not isinstance(mapping, dict) or not mapping:
            raise CampaignRefusal(f"{route} needs pinned Registry model_refs")
        evaluation.check_live_models(models)
        reserve = 0
        run_bounds, cost_bounds = {}, {}
        for trial in settings["trials"]:
            task = tasks.get(trial["task"])
            if task is None:
                raise CampaignRefusal("planned task is absent from the frozen corpus")
            model_name = settings.get("model_by_task_model", {}).get(
                task["model"], settings.get("model") or task["model"])
            reference = mapping.get(model_name)
            # A comparison-wide model overrides each task's model (explicit route).
            if reference is None and route == "explicit" and task["model"] != "small-window":
                reference = mapping.get("explicit-cache")
            model, price = models.get(reference), prices.get(reference)
            if not isinstance(model, dict) or not isinstance(price, dict):
                raise CampaignRefusal(f"{route} is missing the model or approved price of {task['id']}")
            pin = model.get("provider_route")
            if not isinstance(pin, dict) or pin.get("tag") != settings["provider_tag"] \
                    or model.get("model_id") != settings["upstream_model"] \
                    or model.get("reasoning_effort") != settings["reasoning_effort"] \
                    or model.get("request_estimator", "bytes") != plan.get("execution_estimator", "bytes") \
                    or model.get("cache_mode") != settings["cache_mode"]:
                raise CampaignRefusal(f"{route} model differs from the pinned primary experiment")
            if model.get("provider") != "openrouter" or model.get("provider_credential") != "openrouter" \
                    or model.get("credential_env") or model.get("plan_connection"):
                raise CampaignRefusal("campaign needs connected Tenant credentials, not environment keys or Plans")
            expected_caps = corpus["models"][model_name]
            if any(model.get(field) != expected_caps[field] for field in ("context_window", "max_output_tokens")):
                raise CampaignRefusal(f"{route} model changed the frozen experimental window/output caps")
            for field in ("input_nanocredits_per_token", "output_nanocredits_per_token",
                          "cache_write_nanocredits_per_token"):
                number(price.get(field), f"approved {field}", integer=True, minimum=1)
            bound = runner.run_worst_case(task, corpus, model, price, auxiliary)
            reserve += bound
            run_bounds[trial["key"]] = bound
            cost_bounds.setdefault(trial["task"], {})[trial["label"]] = bound
        # The contract applies to every Run, including unsuccessful work. A
        # free-form phase allowance cannot replace this derived reservation.
        if "auxiliary_ceiling_nanocredits" in entry:
            raise CampaignRefusal("use the pinned per-Run auxiliary contract, not a free-form phase ceiling")
        seconds = number(entry.get("upper_wall_seconds"), "complete phase upper duration", minimum=1)
        if seconds < len(settings["trials"]) * plan["config"]["limits"]["run_timeout_seconds"]:
            raise CampaignRefusal("phase duration cannot understate the sequential Runs' enforced timeout bounds")
        result[route] = {"nanocredits": reserve, "seconds": seconds,
                         "planned_runs": len(settings["trials"]),
                         "run_bounds": run_bounds, "cost_bounds": cost_bounds}
    return result


def analysis_reservation_evidence(manifest, plan, route, execution, corpus):
    """Derive statistical bounds from the approval recorded before paid dispatch."""
    costs = phase_reservations(plan, corpus, execution)[route]
    entry = execution["routes"][route]
    approval = read_json(entry["disclosure_approval"])
    experiment = manifest["experiment"]
    if approval.get("campaign_plan_sha256") != plan["plan_sha256"] \
            or approval.get("prices") != entry["prices"] \
            or approval.get("model_config_digests") != {ref: digest(model) for ref, model in entry["models"].items()} \
            or experiment.get("harness", {}).get("disclosure_approval") != digest(approval):
        raise CampaignRefusal("result does not bind the predispatch approval, prices and model configurations")
    if approval.get("source_attestation_sha256") != digest(experiment["aidash"]):
        raise CampaignRefusal("result source differs from the approved operator attestation")
    actual_models = experiment.get("model", {}).get("registrations", {})
    if set(actual_models) != set(entry["model_refs"]) or any(
            actual_models[name].get("config_sha256") != digest(entry["models"][ref])
            for name, ref in entry["model_refs"].items()):
        raise CampaignRefusal("actual registered models differ from the approved reservation configurations")
    spend = manifest.get("spend") or {}
    if spend.get("ceiling_nanocredits") != costs["nanocredits"]:
        raise CampaignRefusal("actual phase spend ceiling differs from the approved complete-phase bound")
    if experiment.get("limits", {}).get("run_timeout_seconds") != plan["config"]["limits"]["run_timeout_seconds"] \
            or experiment.get("limits", {}).get("wall_seconds") != costs["seconds"]:
        raise CampaignRefusal("actual Run/phase deadline differs from the frozen enforced reservation limits")
    for attempt in spend.get("attempts", []):
        if attempt.get("reserved") != costs["run_bounds"].get(attempt.get("key")):
            raise CampaignRefusal("actual Run reservation differs from its predispatch statistical cost bound")
    return costs["cost_bounds"]


def validate_result_plan(manifest, plan, route):
    """The complete frozen trial inventory must survive failure and interruption."""
    validate_plan(plan)
    experiment = manifest.get("experiment", {})
    if experiment.get("corpus") != plan["corpus"] or experiment.get("evaluator") != plan["evaluator"]:
        raise CampaignRefusal("result corpus/evaluator differs from the frozen plan")
    source = experiment.get("aidash", {})
    if any(source.get(key) != plan["source"].get(key)
           for key in ("commit", "dirty", "dirty_diff_sha256")):
        raise CampaignRefusal("result source differs from the frozen plan")
    if experiment.get("tier") == "live" and (source.get("dirty") is not False or
                                             source.get("server_revision") != source.get("commit")):
        raise CampaignRefusal("live result must identify the exact frozen clean deployed source")
    if experiment.get("repeat_count") != plan["repeats"]:
        raise CampaignRefusal("result repeats differ from the frozen plan")
    planned = {item["key"] for item in plan["routes"][route]["trials"]}
    actual = {evaluation.run_key(variant["task"], variant["comparison"], variant["arm"], repeat)
              for variant in experiment.get("variants", [])
              for repeat in range(1, experiment.get("repeat_count", 0) + 1)}
    if actual != planned:
        raise CampaignRefusal("result variants differ from the complete frozen trial inventory")


def deployment_plan(plan, estimator):
    """Freeze fresh deployment-estimator trials without rerunning candidate selection."""
    validate_plan(plan)
    if plan["phase"] != "confirmation":
        raise CampaignRefusal("deployment confirmation requires a frozen confirmation plan")
    if not isinstance(estimator, dict) or estimator.get("id") not in ("bytes", "calibrated"):
        raise CampaignRefusal("deployment estimator needs an explicit supported snapshot id")
    number(estimator.get("version"), "deployment estimator version", integer=True, minimum=1)
    if not isinstance(estimator.get("confidence"), str) or not estimator["confidence"]:
        raise CampaignRefusal("deployment estimator snapshot needs confidence")
    frozen = copy.deepcopy(plan)
    frozen.update(phase="deployment-mode", execution_estimator=estimator["id"],
                  deployment_estimator=copy.deepcopy(estimator),
                  confirmation_plan_sha256=plan["plan_sha256"], created_at=utc_now().isoformat())
    frozen.pop("plan_sha256")
    frozen["plan_sha256"] = digest(frozen)
    return frozen


def execution_command(plan, execution, route, corpus_path, output, ceiling, seconds):
    """Construct argv; no shell expansion and no bearer token in process arguments."""
    settings = plan["routes"][route]
    entry = execution["routes"][route]
    for field in ("base_url", "tenant", "provider_credential", "disclosure_approval"):
        if not isinstance(entry.get(field), str) or not entry[field]:
            raise CampaignRefusal(f"{route} execution needs {field}")
    command = [sys.executable, str(ROOT / "scripts/evaluate-fixed-tasks.py"), "live",
               "--base-url", entry["base_url"], "--tenant", entry["tenant"],
               "--provider-credential", entry["provider_credential"],
               "--server-revision", execution["server_revision"],
               "--corpus", str(corpus_path), "--comparison", settings["comparison"],
               "--execution-seed", str(plan["config"]["execution_seed"]),
               "--repeats", str(plan["repeats"]),
               "--run-timeout", str(plan["config"]["limits"]["run_timeout_seconds"]),
               "--max-spend-nanocredits", str(ceiling), "--max-wall-seconds", str(seconds),
               "--disclosure-approval", entry["disclosure_approval"], "--output", str(output)]
    for model_name, reference in sorted(entry["model_refs"].items()):
        if not isinstance(reference, str) or reference.count("@") != 1:
            raise CampaignRefusal("each model mapping needs an immutable id@version reference")
        command.extend(["--model", f"{model_name}={reference}"])
    if entry.get("tool_endpoint"):
        command.extend(["--tool-endpoint", entry["tool_endpoint"]])
    if entry.get("source_attestation"):
        command.extend(["--source-attestation", entry["source_attestation"]])
    if plan["phase"] != "pilot":
        command.extend(["--arm", "legacy", "--arm", plan["candidate_arms"][route]])
    command.extend(["--execution-estimator", plan.get("execution_estimator", "bytes")])
    return command


def check_execution(plan, corpus, execution):
    """Read-only admission of the exact frozen experiment before state reservation."""
    validate_plan(plan)
    if corpus["_snapshot"] != plan["corpus"]:
        raise CampaignRefusal("current corpus differs from the frozen campaign")
    if campaign_identity() != plan.get("campaign_evaluator"):
        raise CampaignRefusal("campaign evaluator changed after the plan was frozen")
    if evaluation.pin_evaluator() != plan["evaluator"]:
        raise CampaignRefusal("fixed-task evaluator changed after the plan was frozen")
    source = evaluation.source_identity()
    if source.get("dirty") is not False or source.get("commit") != plan["source"].get("commit"):
        raise CampaignRefusal("paid execution requires the frozen clean source commit")
    if execution.get("server_revision") != source["commit"]:
        raise CampaignRefusal("the deployed source revision must equal the exact frozen commit")
    if execution.get("approved_plan_sha256") != plan["plan_sha256"]:
        raise CampaignRefusal("the approved execution manifest does not name this plan")
    for name in ("AIDASH_EVALUATION_TOKEN", "AIDASH_EVALUATION_SUBJECT_TOKEN"):
        if not os.environ.get(name):
            raise CampaignRefusal(f"{name} is required; no provider environment keys are accepted")
    costs = phase_reservations(plan, corpus, execution)
    for route, entry in execution["routes"].items():
        approval = read_json(entry["disclosure_approval"])
        evaluation.require_enforceable_auxiliary_bound(evaluation.auxiliary_allowance(approval))
        evaluation.check_execution_estimator_approval(approval, plan.get("execution_estimator", "bytes"))
        if not entry.get("source_attestation"):
            raise CampaignRefusal(f"{route} needs an operator attestation of the deployed source and binary")
        attestation = read_json(entry["source_attestation"])
        evaluation.validate_source_attestation(attestation, approval, execution["server_revision"])
        if set(attestation) != set(plan["source"]) or any(attestation.get(field) != plan["source"].get(field)
                for field in ("commit", "dirty", "dirty_diff_sha256")) \
                or attestation.get("server_revision") != source["commit"] \
                or not isinstance(attestation.get("binary_sha256"), str) \
                or not FINGERPRINT.fullmatch(attestation["binary_sha256"]):
            raise CampaignRefusal(f"{route} deployed source attestation differs from the frozen clean commit")
        if approval.get("source_attestation_sha256") != digest(attestation):
            raise CampaignRefusal(f"{route} source attestation is not bound to its disclosure approval")
        if approval.get("campaign_plan_sha256") != plan["plan_sha256"]:
            raise CampaignRefusal(f"{route} disclosure approval does not bind this exact campaign plan")
        if approval.get("prices") != entry["prices"]:
            raise CampaignRefusal(f"{route} prices differ from the approved reservation rates")
        if approval.get("model_config_digests") != {reference: digest(model)
                for reference, model in entry["models"].items()}:
            raise CampaignRefusal(f"{route} approval does not pin the exact model configurations")
    return costs


def invoke(command, timeout):
    """Delegate cancellation to the existing live runner; timeout is the shared remainder."""
    import signal
    process = subprocess.Popen(command, start_new_session=True)
    try:
        return process.wait(timeout=timeout)
    except (KeyboardInterrupt, subprocess.TimeoutExpired) as error:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=45)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
        raise CampaignRefusal("campaign interrupted; reservations remain charged and cancellation needs reconciliation") from error


def run_phase(plan, execution, *, state_path, corpus_path, output_dir, invoke_child=invoke, now=utc_now):
    """Reserve the complete phase across both routes before any provider can be dispatched."""
    corpus = evaluation.load_corpus(corpus_path)
    costs = check_execution(plan, corpus, execution)
    state_path, output_dir = Path(state_path), Path(output_dir)
    with state_lock(state_path):
        limits = plan["config"]["limits"]
        identity = plan.get("campaign_plan_sha256", plan["plan_sha256"])
        budget = SharedBudget.from_document(read_json(state_path), identity, now=now()) if state_path.exists() \
            else SharedBudget.create(identity, limits["max_spend_nanocredits"], limits["max_wall_seconds"], now=now())
        if any(x["status"] in ("reserved", "uncertain") for x in budget.value["reservations"]):
            raise CampaignRefusal("prior campaign work is unresolved; reconcile cancellation and charges before dispatch")
        phase = plan["phase"]
        total_cost = sum(x["nanocredits"] for x in costs.values())
        total_seconds = sum(x["seconds"] for x in costs.values())
        if not budget.can_fit_phase(total_cost, total_seconds, now=now()):
            write_json(state_path, budget.document(), exclusive=not state_path.exists())
            raise CampaignRefusal("complete phase cannot fit the remaining shared USD 50 / four-hour budget; keep Legacy")
        commands = []
        for route in plan["config"]["required_routes"]:
            output = output_dir / f"{phase}-{route}.json"
            if output.exists():
                raise CampaignRefusal("phase output exists; import the prior evidence instead of repeating trials")
            commands.append((route, output, execution_command(plan, execution, route, corpus_path, output,
                                                              costs[route]["nanocredits"], costs[route]["seconds"])))
        for route, output, _ in commands:
            budget.reserve(phase, route, costs[route]["nanocredits"], costs[route]["seconds"], str(output), now=now())
        # Both reservations are durable before the first Registry write/provider call.
        write_json(state_path, budget.document(), exclusive=not state_path.exists())
        for route, _, command in commands:
            code = 1
            try:
                remaining = min(costs[route]["seconds"], budget.remaining_seconds(now=now()))
                if remaining <= 0:
                    raise CampaignRefusal("shared campaign deadline reached before starting the next route")
                code = invoke_child(command, remaining)
            finally:
                budget.finish(phase, route, exit_code=code, now=now())
                write_json(state_path, budget.document())
            if code != 0:
                raise CampaignRefusal(f"{phase}/{route} stopped with exit {code}; later routes remain undispatched and reserved")
        return budget.document()
