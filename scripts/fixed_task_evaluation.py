"""Fixed-task quality and cost evaluation: corpus, oracles, scoring and manifests (#122).

This module holds the parts shared by both execution tiers: loading and hashing
the versioned corpus, independent oracles, outcome classification, aggregation
with explicit denominators, manifest validation, and the live tier's spend and
time guard rails. It performs no network I/O. The Run drivers live in
fixed_task_runner.py and the scripted fixture provider in fixed_task_fixture.py.

An Agent's own success claim is recorded but never used as the oracle. Every
attempted Run is reported; failed, overflowing and interrupted Runs stay in the
denominators.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parent.parent
EVALUATION = ROOT / "tests/evaluation"
EVALUATOR_NAME = "aidash-fixed-task-evaluator"
EVALUATOR_VERSION = "1.0.0"
# Files whose bytes define the evaluator; their digest is in every manifest.
EVALUATOR_FILES = ("fixed_task_evaluation.py", "fixed_task_fixture.py", "fixed_task_runner.py",
                   "evaluate-fixed-tasks.py")
OUTCOMES = ("success", "false_completion", "policy_compliant_escalation", "unresolved",
            "overflow_failure", "interrupted", "infrastructure_failure")
LONG_CONTEXT = ("none", "pruning", "summary_recovery", "terminal_overflow")
TERMINAL_PHASES = ("COMPLETED", "FAILED", "CANCELLED")
CONCLUSION_SCOPE = ("Conclusions are limited to the tested tasks, fixture or model configuration and "
                    "repeat counts recorded in this manifest. They are not a general quality, cost or "
                    "superiority claim and approve no default rollout.")
ORACLE_KINDS = ("python-tests", "json-overdue-totals", "constraint-retention", "tool-selection",
                "escalation", "answer-pattern")
# Messages that identify a failed compaction or an over-window request.
OVERFLOW_PATTERNS = (re.compile(r"(?i)compaction could not fit"), re.compile(r"(?i)context (window|length|overflow)"),
                     re.compile(r"(?i)exceeds? (the )?(model )?(context|window)"), re.compile(r"(?i)too (large|long) for"))
SUCCESS_CLAIM = re.compile(r"(?i)\b(all tests pass|tests pass|task (is )?complete|completed|done|success|finished)\b")
NEGATED_CLAIM = re.compile(r"(?i)\b(not|cannot|can't|unable|declined|denied|escalat\w*)\b")
EVIDENCE_LIMIT = 4096


class CorpusError(ValueError):
    """The corpus or a manifest violates its declared contract."""


# ------------------------------------------------------------------ hashing


def sha256_bytes(data):
    return hashlib.sha256(data).hexdigest()


def sha256_text(text):
    return sha256_bytes(text.encode())


def canonical_sha256(value):
    """Digest of a JSON value with sorted keys; used for configurations."""
    return sha256_text(json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False))


def snapshot(version, base, files):
    """Hash every file by relative path; the combined digest covers names and bytes."""
    digests = {}
    for relative in sorted(set(files)):
        path = (base / relative).resolve()
        if base.resolve() not in path.parents:
            raise CorpusError(f"corpus file escapes its directory: {relative}")
        digests[relative] = sha256_bytes(path.read_bytes())
    combined = sha256_text("".join(f"{name}\0{digest}\n" for name, digest in digests.items()))
    return {"version": version, "sha256": combined, "files": digests}


def evaluator_identity():
    scripts = Path(__file__).resolve().parent
    digest = snapshot(EVALUATOR_VERSION, scripts, [name for name in EVALUATOR_FILES if (scripts / name).exists()])
    return {"name": EVALUATOR_NAME, "version": EVALUATOR_VERSION, "sha256": digest["sha256"]}


def source_identity(binary=None, server_revision=None, root=ROOT):
    """Record the tested source. Unknown values stay null rather than guessed."""
    def git(*args):
        try:
            return subprocess.run(["git", *args], cwd=root, check=True, capture_output=True).stdout
        except (OSError, subprocess.CalledProcessError):
            return None
    head = git("rev-parse", "HEAD")
    status = git("status", "--porcelain")
    diff = git("diff", "HEAD", "--binary")
    commit = head.decode().strip() if head else None
    dirty = None if status is None else bool(status.strip())
    return {"commit": commit, "dirty": dirty,
            "dirty_diff_sha256": sha256_bytes(diff + (status or b"")) if dirty and diff is not None else None,
            "binary_sha256": sha256_bytes(Path(binary).read_bytes()) if binary else None,
            "server_revision": server_revision}


# ---------------------------------------------------------- schema validation


def load_schema(name):
    return json.loads((EVALUATION / "schema" / name).read_text())


def validate(instance, schema, root=None, path="$"):
    """Validate the JSON Schema subset used by tests/evaluation/schema.

    Supported keywords: type, const, enum, required, properties,
    additionalProperties, items, minimum, maximum, maxLength, pattern and local $ref.
    Unknown keywords are rejected so a schema cannot silently lose a rule.
    """
    root = schema if root is None else root
    errors = []
    known = {"$schema", "$id", "title", "description", "$defs", "type", "const", "enum", "required",
             "properties", "additionalProperties", "items", "minimum", "maximum", "maxLength", "pattern", "$ref"}
    unknown = set(schema) - known
    if unknown:
        raise CorpusError(f"unsupported schema keywords at {path}: {sorted(unknown)}")
    if "$ref" in schema:
        reference = schema["$ref"]
        if not reference.startswith("#/"):
            raise CorpusError(f"only local schema references are supported: {reference}")
        target = root
        for part in reference[2:].split("/"):
            target = target[part]
        return validate(instance, target, root, path)
    if "type" in schema:
        types = schema["type"] if isinstance(schema["type"], list) else [schema["type"]]
        checks = {"object": lambda v: isinstance(v, dict), "array": lambda v: isinstance(v, list),
                  "string": lambda v: isinstance(v, str), "boolean": lambda v: isinstance(v, bool),
                  "integer": lambda v: isinstance(v, int) and not isinstance(v, bool),
                  "number": lambda v: isinstance(v, (int, float)) and not isinstance(v, bool),
                  "null": lambda v: v is None}
        if not any(checks[name](instance) for name in types):
            return [f"{path}: expected {'/'.join(types)}, got {type(instance).__name__}"]
    if "const" in schema and instance != schema["const"]:
        errors.append(f"{path}: expected {schema['const']!r}")
    if "enum" in schema and instance not in schema["enum"]:
        errors.append(f"{path}: {instance!r} is not one of {schema['enum']}")
    if isinstance(instance, (int, float)) and not isinstance(instance, bool) and "minimum" in schema:
        if instance < schema["minimum"]:
            errors.append(f"{path}: {instance} is below {schema['minimum']}")
    if isinstance(instance, (int, float)) and not isinstance(instance, bool) and "maximum" in schema:
        if instance > schema["maximum"]:
            errors.append(f"{path}: {instance} is above {schema['maximum']}")
    if isinstance(instance, str):
        if "maxLength" in schema and len(instance) > schema["maxLength"]:
            errors.append(f"{path}: longer than {schema['maxLength']}")
        if "pattern" in schema and not re.search(schema["pattern"], instance):
            errors.append(f"{path}: does not match {schema['pattern']}")
    if isinstance(instance, dict):
        for name in schema.get("required", []):
            if name not in instance:
                errors.append(f"{path}: missing {name}")
        properties = schema.get("properties", {})
        extra = schema.get("additionalProperties", True)
        for name, value in instance.items():
            if name in properties:
                errors += validate(value, properties[name], root, f"{path}.{name}")
            elif extra is False:
                errors.append(f"{path}: unexpected property {name}")
            elif isinstance(extra, dict):
                errors += validate(value, extra, root, f"{path}.{name}")
    if isinstance(instance, list) and "items" in schema:
        for index, value in enumerate(instance):
            errors += validate(value, schema["items"], root, f"{path}[{index}]")
    return errors


def validate_result(result):
    """Validate a result manifest and its embedded experiment; raise on violations."""
    errors = validate(result, load_schema("result.schema.json"))
    if isinstance(result, dict) and isinstance(result.get("experiment"), dict):
        errors += validate(result["experiment"], load_schema("experiment.schema.json"), path="$.experiment")
    if not errors and isinstance(result, dict):
        errors += consistency_errors(result)
    if errors:
        raise CorpusError("result manifest is invalid:\n" + "\n".join(errors[:50]))
    return result


def consistency_errors(result):
    """Cross-field rules a JSON Schema cannot express."""
    errors = []
    runs = result["runs"]
    keys = [run["key"] for run in runs]
    if len(keys) != len(set(keys)):
        errors.append("$.runs: duplicate run keys")
    expected = len(result["experiment"]["variants"]) * result["experiment"]["repeat_count"]
    if result["error"] is None and result["spend"] is None and len(runs) != expected:
        errors.append(f"$.runs: {len(runs)} runs recorded for {expected} planned; none may be dropped")
    for aggregate in result["aggregates"]:
        matching = [run for run in runs if (run["task"], run["comparison"], run["arm"]) ==
                    (aggregate["task"], aggregate["comparison"], aggregate["arm"])]
        if aggregate["attempted"] != len(matching) or sum(aggregate["outcomes"].values()) != len(matching):
            errors.append(f"aggregate {aggregate['task']}/{aggregate['comparison']}/{aggregate['arm']} "
                          "does not account for every attempted Run")
    return errors


# ------------------------------------------------------------------- corpus


def load_corpus(path=None):
    path = Path(path or EVALUATION / "corpus.toml")
    raw = path.read_bytes()
    corpus = tomllib.loads(raw.decode())
    base = path.parent
    errors = []
    for key in ("version", "instructions", "max_steps", "models", "comparisons", "tools", "tasks"):
        if key not in corpus:
            errors.append(f"corpus is missing {key}")
    if errors:
        raise CorpusError("; ".join(errors))
    comparisons = {item["id"]: item for item in corpus["comparisons"]}
    if len(comparisons) != len(corpus["comparisons"]):
        errors.append("duplicate comparison ids")
    for item in corpus["comparisons"]:
        if item.get("status", "available") not in ("available", "feature-not-available"):
            errors.append(f"comparison {item['id']} has an unknown status")
        if item.get("status") != "feature-not-available":
            for arm in ("baseline", "candidate"):
                if not isinstance(item.get(arm), dict) or "label" not in item[arm] or "agent" not in item[arm]:
                    errors.append(f"comparison {item['id']} needs a {arm} label and agent fields")
            if item.get("model") and item["model"] not in corpus["models"]:
                errors.append(f"comparison {item['id']} names unknown model {item['model']}")
    tools = {tool["alias"]: tool for tool in corpus["tools"]}
    skills = {skill["id"] for skill in corpus.get("skills", [])}
    files = [path.name]
    ids = set()
    for task in corpus["tasks"]:
        tid = task.get("id", "?")
        if tid in ids:
            errors.append(f"duplicate task {tid}")
        ids.add(tid)
        for key in ("kind", "language", "title", "prompt", "model", "comparisons", "fixture_agent", "oracle", "expected"):
            if key not in task:
                errors.append(f"task {tid} is missing {key}")
        if task.get("model") not in corpus["models"]:
            errors.append(f"task {tid} names unknown model {task.get('model')}")
        for comparison in task.get("comparisons", []):
            if comparison not in comparisons:
                errors.append(f"task {tid} names unknown comparison {comparison}")
            elif comparisons[comparison].get("status") == "feature-not-available":
                errors.append(f"task {tid} selects unavailable comparison {comparison}")
        for skill in task.get("skills", []):
            if skill not in skills:
                errors.append(f"task {tid} names unknown Skill {skill}")
        oracle = task.get("oracle", {})
        if oracle.get("kind") not in ORACLE_KINDS:
            errors.append(f"task {tid} has unknown oracle {oracle.get('kind')}")
        if task.get("expected", {}).get("outcome") not in OUTCOMES:
            errors.append(f"task {tid} declares an unknown expected outcome")
        declared = task.get("tools", [])
        for alias in ([] if declared == "all" else declared) + [task.get("expected_tool")] * ("expected_tool" in task):
            if alias not in tools:
                errors.append(f"task {tid} names unknown tool {alias}")
        for relative in task.get("inputs", []) + oracle.get("visible", []) + oracle.get("hidden", []) + (
                [oracle["source"]] if "source" in oracle else []):
            if not (base / relative).is_file():
                errors.append(f"task {tid} references missing file {relative}")
            files.append(relative)
    if errors:
        raise CorpusError("; ".join(errors))
    corpus["_snapshot"] = snapshot(corpus["version"], base, files)
    corpus["_base"] = base
    return corpus


def task_tools(corpus, task):
    """Fixture tools bound to a task's Agent, in corpus order."""
    declared = task.get("tools")
    if declared is None:
        declared = ["reference_lookup"] if task["kind"] in ("long-context", "unresolved") else []
    aliases = [tool["alias"] for tool in corpus["tools"]] if declared == "all" else list(declared)
    return [tool for tool in corpus["tools"] if tool["alias"] in aliases]


def task_description(corpus, task):
    """The exact Task text sent to the Agent: the prompt plus its controlled inputs."""
    parts = [task["prompt"].strip()]
    for relative in task.get("inputs", []):
        content = (corpus["_base"] / relative).read_text()
        fence = "csv" if relative.endswith(".csv") else "python" if relative.endswith(".py") else ""
        parts.append(f"{Path(relative).name}:\n```{fence}\n{content.rstrip()}\n```")
    return "\n\n".join(parts)


def plan(corpus, selected_tasks=None, selected_comparisons=None):
    """Every (task, comparison, arm) pair to run; each arm differs only in Agent fields."""
    comparisons = {item["id"]: item for item in corpus["comparisons"]}
    planned = []
    for task in corpus["tasks"]:
        if selected_tasks and task["id"] not in selected_tasks:
            continue
        for comparison_id in task["comparisons"]:
            if selected_comparisons and comparison_id not in selected_comparisons:
                continue
            comparison = comparisons[comparison_id]
            for arm in ("baseline", "candidate"):
                planned.append({"task": task, "comparison": comparison, "arm": arm,
                                "label": comparison[arm]["label"], "agent_fields": comparison[arm]["agent"],
                                "model": comparison.get("model") or task["model"]})
    return planned


def feature_availability(corpus, selected_comparisons=None, registration_errors=None):
    registration_errors = registration_errors or {}
    rows = []
    for item in corpus["comparisons"]:
        status = item.get("status", "available")
        detail = item.get("description")
        if status == "available" and item["id"] in registration_errors:
            status, detail = "feature-not-available", registration_errors[item["id"]]
        elif status == "available" and selected_comparisons and item["id"] not in selected_comparisons:
            status = "not-selected"
        rows.append({"comparison": item["id"], "issue": item["issue"], "status": status, "detail": detail})
    return rows


# ------------------------------------------------------------------ oracles


def fenced_blocks(text, language):
    return re.findall(r"```" + re.escape(language) + r"[ \t]*\n(.*?)```", text or "", re.S)


def last_json_block(text):
    blocks = fenced_blocks(text, "json")
    if not blocks:
        raise ValueError("the answer has no ```json block")
    return json.loads(blocks[-1])


def run_python_tests(directory, pattern, timeout):
    """Run unittest discovery in isolated mode; the start directory is the import root."""
    try:
        completed = subprocess.run(
            [sys.executable, "-I", "-m", "unittest", "discover", "-s", str(directory), "-p", pattern],
            cwd=directory, capture_output=True, text=True, timeout=timeout,
            env={"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "PYTHONDONTWRITEBYTECODE": "1"})
    except subprocess.TimeoutExpired:
        return {"pattern": pattern, "passed": False, "detail": f"timed out after {timeout}s"}
    tail = (completed.stderr or completed.stdout).strip().splitlines()[-3:]
    return {"pattern": pattern, "passed": completed.returncode == 0, "detail": " | ".join(tail)[:500]}


def oracle_python_tests(corpus, oracle, answer, allow_code_execution):
    blocks = fenced_blocks(answer, "python")
    if not blocks:
        return False, [{"check": "patch-present", "passed": False}], "the answer contains no ```python block"
    if not allow_code_execution:
        return None, [{"check": "patch-present", "passed": True}], "code execution oracle not approved for this tier"
    base = corpus["_base"]
    checks = [{"check": "patch-present", "passed": True}]
    with tempfile.TemporaryDirectory(prefix="aidash-fixed-task-oracle-") as directory:
        work = Path(directory)
        (work / oracle["target"]).write_text(blocks[-1])
        for relative in oracle["visible"]:
            shutil.copyfile(base / relative, work / Path(relative).name)
        for relative in oracle["hidden"]:
            shutil.copyfile(base / relative, work / ("heldout_" + Path(relative).name))
        timeout = oracle.get("timeout_seconds", 20)
        visible = run_python_tests(work, "test_*.py", timeout)
        hidden = run_python_tests(work, "heldout_*.py", timeout)
    checks += [{"check": "visible-tests", **visible}, {"check": "hidden-regression", **hidden}]
    passed = visible["passed"] and hidden["passed"]
    return passed, checks, None if passed else "independent tests failed"


def overdue_truth(path, as_of):
    import csv
    truth = {}
    with open(path, newline="") as stream:
        for row in csv.DictReader(stream):
            if row["status"] == "unpaid" and row["due_date"] < as_of:
                customer = truth.setdefault(row["customer"], {})
                customer[row["currency"]] = customer.get(row["currency"], 0) + int(row["amount_cents"])
    return truth


def oracle_overdue(corpus, oracle, answer):
    try:
        value = last_json_block(answer)
    except ValueError as error:
        return False, [{"check": "json-present", "passed": False}], str(error)
    truth = overdue_truth(corpus["_base"] / oracle["source"], oracle["as_of"])
    checks = [{"check": "json-present", "passed": True},
              {"check": "as-of", "passed": value.get("as_of") == oracle["as_of"]},
              {"check": "overdue-totals", "passed": value.get("overdue") == truth}]
    passed = all(check["passed"] for check in checks)
    return passed, checks, None if passed else "overdue totals differ from the independently computed snapshot"


def oracle_constraints(oracle, answer):
    try:
        value = last_json_block(answer)
    except ValueError as error:
        return False, [{"check": "json-present", "passed": False}], str(error)
    checks = [{"check": "json-present", "passed": True},
              {"check": "budget", "passed": value.get("budget_usd") == oracle["budget_usd"],
               "expected": oracle["budget_usd"], "observed": value.get("budget_usd")},
              {"check": "exclusions", "passed": sorted(value.get("excluded_vendors") or []) ==
               sorted(oracle["excluded_vendors"]), "expected": sorted(oracle["excluded_vendors"]),
               "observed": value.get("excluded_vendors")}]
    passed = all(check["passed"] for check in checks)
    return passed, checks, None if passed else "a constraint or correction was not retained"


def oracle_tool_selection(oracle, answer, tool_calls):
    called = [call for call in tool_calls if not call["name"].startswith(("capability_", "skill_"))]
    first = called[0] if called else None
    checks = [{"check": "first-domain-tool", "passed": bool(first) and first["name"] == oracle["expected_tool"]
               and not first.get("dispatch_rejected"), "observed": first and first["name"]},
              {"check": "answer", "passed": bool(re.search(oracle["answer_pattern"], answer or ""))}]
    passed = all(check["passed"] for check in checks)
    return passed, checks, None if passed else "the expected tool was not selected first or the answer is wrong"


def oracle_escalation(oracle, answer, tool_calls, human_answers):
    asked = any(entry["kind"] == "APPROVAL_REQUIRED" for entry in human_answers)
    executed = [call for call in tool_calls if call["name"] == oracle["forbidden_tool"] and not call.get("error")]
    checks = [{"check": "approval-requested", "passed": asked},
              {"check": "forbidden-tool-not-executed", "passed": not executed},
              {"check": "escalation-reported", "passed": bool(re.search(oracle["answer_pattern"], answer or ""))}]
    passed = all(check["passed"] for check in checks)
    return passed, checks, None if passed else "escalation policy was not followed"


def grade(corpus, task, observation, allow_code_execution):
    """Apply the task's independent oracle to an observed Run. Never reads the Agent's claim."""
    oracle = task["oracle"]
    answer = observation.get("final_answer")
    kind = oracle["kind"]
    if answer is None and kind not in ("escalation",):
        return {"kind": kind, "passed": None, "checks": [], "detail": "the Run produced no final answer"}
    if kind == "python-tests":
        passed, checks, detail = oracle_python_tests(corpus, oracle, answer, allow_code_execution)
    elif kind == "json-overdue-totals":
        passed, checks, detail = oracle_overdue(corpus, oracle, answer)
    elif kind == "constraint-retention":
        passed, checks, detail = oracle_constraints(oracle, answer)
    elif kind == "tool-selection":
        passed, checks, detail = oracle_tool_selection(oracle, answer, observation.get("tool_calls", []))
    elif kind == "escalation":
        passed, checks, detail = oracle_escalation(oracle, answer, observation.get("tool_calls", []),
                                                   observation.get("human_answers", []))
    else:
        checks = [{"check": "answer", "passed": bool(re.search(oracle["answer_pattern"], answer or ""))}]
        if "forbidden_pattern" in oracle:
            checks.append({"check": "forbidden-content-absent",
                           "passed": not re.search(oracle["forbidden_pattern"], answer or "")})
        passed = all(check["passed"] for check in checks)
        detail = None if passed else "the answer misses required content or contains forbidden content"
    return {"kind": kind, "passed": passed, "checks": checks, "detail": detail}


# ------------------------------------------------------------ classification


def claimed_success(answer):
    """The Agent's own claim, recorded for false-completion analysis only."""
    if not answer:
        return None
    return bool(SUCCESS_CLAIM.search(answer)) and not NEGATED_CLAIM.search(answer)


def is_overflow(error):
    return bool(error) and any(pattern.search(error) for pattern in OVERFLOW_PATTERNS)


def classify(task, observation, oracle):
    """Map one observed Run to exactly one outcome; nothing is excluded."""
    if observation.get("infrastructure_error"):
        return "infrastructure_failure"
    if observation.get("interrupted"):
        return "interrupted"
    phase = observation.get("phase")
    if phase == "COMPLETED":
        if oracle["passed"]:
            return "policy_compliant_escalation" if task["oracle"]["kind"] == "escalation" else "success"
        return "false_completion"
    if phase in ("FAILED", "CANCELLED"):
        return "overflow_failure" if is_overflow(observation.get("error")) else "unresolved"
    return "interrupted"


def long_context(observation, outcome):
    if outcome == "overflow_failure":
        return "terminal_overflow"
    if observation.get("summary_recovered"):
        return "summary_recovery"
    return "pruning" if (observation.get("compactions") or 0) > 0 else "none"


# --------------------------------------------------------------- usage


def summarize_usage(records):
    """Summarize Usage Records without turning unknown values into zero (ADR 0018)."""
    if records is None:
        empty = {"known": 0, "unknown": 0}
        return {"source": "unavailable", "attempts": 0, "outcomes": {}, "input_tokens": dict(empty),
                "output_tokens": dict(empty), "estimated_tokens": dict(empty), "cache_read_tokens": dict(empty),
                "cache_write_tokens": dict(empty), "cost_known_nanocredits": 0, "cost_uncertain_attempts": 0,
                "records": []}

    def known_sum(field):
        values = [record.get(field) for record in records]
        return {"known": sum(value for value in values if value is not None),
                "unknown": sum(value is None for value in values)}
    outcomes = {}
    for record in records:
        outcomes[record["outcome"]] = outcomes.get(record["outcome"], 0) + 1
    return {"source": "usage-records", "attempts": len(records), "outcomes": outcomes,
            "input_tokens": known_sum("input_tokens"), "output_tokens": known_sum("output_tokens"),
            "estimated_tokens": known_sum("estimated_tokens"), "cache_read_tokens": known_sum("cache_read_tokens"),
            "cache_write_tokens": known_sum("cache_write_tokens"),
            "cost_known_nanocredits": sum(record.get("cost_nanocredits") or 0 for record in records),
            "cost_uncertain_attempts": sum(record.get("cost_nanocredits") is None for record in records),
            "records": records}


# ------------------------------------------------------------- run results


def run_result(corpus, planned, repeat, observation, tier, allow_code_execution):
    task = planned["task"]
    oracle = grade(corpus, task, observation, allow_code_execution)
    outcome = classify(task, observation, oracle)
    answer = observation.get("final_answer")
    tool_calls = observation.get("tool_calls", [])
    selection = None
    if task["oracle"]["kind"] == "tool-selection":
        domain = [call["name"] for call in tool_calls if not call["name"].startswith(("capability_", "skill_"))]
        selection = {"expected": task["oracle"]["expected_tool"], "domain_calls": domain,
                     "capability_calls": [call["name"] for call in tool_calls if call["name"].startswith("capability_")],
                     "correct": oracle["passed"] if oracle["passed"] is not None else False}
    if task["kind"] == "skill-selection":
        loads = [call for call in tool_calls if call["name"] in ("capability_load", "skill_load")]
        selection = {"expected": task.get("skills", []), "domain_calls": [],
                     "capability_calls": [call["name"] for call in tool_calls if call["name"].startswith(
                         ("capability_", "skill_"))], "load_calls": len(loads),
                     "correct": oracle["passed"] if oracle["passed"] is not None else False}
    retention = None
    if task["oracle"]["kind"] == "constraint-retention":
        retention = None if oracle["passed"] is None else all(
            check["passed"] for check in oracle["checks"] if check["check"] in ("budget", "exclusions"))
    live = tier == "live"
    return {
        "key": f"{task['id']}/{planned['comparison']['id']}/{planned['arm']}/{repeat}",
        "task": task["id"], "comparison": planned["comparison"]["id"], "arm": planned["arm"],
        "label": planned["label"], "repeat": repeat, "run_id": observation.get("run_id"),
        "phase": observation.get("phase"), "error": observation.get("error") or observation.get("infrastructure_error"),
        "outcome": outcome, "claimed_success": claimed_success(answer), "oracle": oracle,
        "long_context": long_context(observation, outcome), "constraint_retention": retention,
        "tool_selection": selection, "wall_ms": float(observation.get("wall_ms", 0.0)),
        "steps": observation.get("steps"), "compactions": observation.get("compactions"),
        "requests": observation.get("requests"), "usage": summarize_usage(observation.get("usage_records")),
        "tool_calls": tool_calls, "human_answers": observation.get("human_answers", []),
        "evidence": {"content_class": "restricted-live" if live else "fixture-trace",
                     "input_sha256": sha256_text(task_description(corpus, task)),
                     "final_answer_sha256": sha256_text(answer) if answer is not None else None,
                     "final_answer": None if live or answer is None else answer[:EVIDENCE_LIMIT],
                     "verifier": oracle["checks"]},
    }


def distribution(values):
    known = [value for value in values if value is not None]
    if not known:
        return {"n": 0, "unknown": len(values), "min": None, "median": None, "max": None, "total": None}
    return {"n": len(known), "unknown": len(values) - len(known), "min": min(known),
            "median": statistics.median(known), "max": max(known), "total": sum(known)}


def run_metrics(run):
    usage = run["usage"]
    requests = run["requests"] or {}

    def usage_total(field):
        values = usage[field]
        # A Run with any unknown attempt has an unknown total, never a partial one.
        return None if usage["source"] != "usage-records" or values["unknown"] else values["known"]
    return {
        "wall_ms": run["wall_ms"], "steps": run["steps"], "compactions": run["compactions"],
        "inference_attempts": usage["attempts"] if usage["source"] == "usage-records" else None,
        "input_tokens": usage_total("input_tokens"), "output_tokens": usage_total("output_tokens"),
        "estimated_tokens": usage_total("estimated_tokens"), "cache_read_tokens": usage_total("cache_read_tokens"),
        "cache_write_tokens": usage_total("cache_write_tokens"),
        "cost_known_nanocredits": usage["cost_known_nanocredits"] if usage["source"] == "usage-records" else None,
        "cost_uncertain_attempts": usage["cost_uncertain_attempts"] if usage["source"] == "usage-records" else None,
        "first_request_bytes": (requests.get("bytes") or [None])[0],
        "max_request_bytes": max(requests["bytes"]) if requests.get("bytes") else None,
        "first_request_tool_definitions": (requests.get("tool_definitions") or [None])[0],
    }


def aggregate(runs):
    groups = {}
    for run in runs:
        groups.setdefault((run["task"], run["comparison"], run["arm"]), []).append(run)
    result = []
    for (task, comparison, arm), members in groups.items():
        outcomes = {name: sum(run["outcome"] == name for run in members) for name in OUTCOMES}
        total = len(members)
        rates = {name: {"numerator": count, "denominator": total} for name, count in outcomes.items()}
        retention = [run["constraint_retention"] for run in members if run["constraint_retention"] is not None]
        rates["constraint_retention"] = {"numerator": sum(retention), "denominator": len(retention)}
        selection = [run["tool_selection"]["correct"] for run in members if run["tool_selection"]]
        rates["tool_selection_correct"] = {"numerator": sum(selection), "denominator": len(selection)}
        claimed = [run for run in members if run["claimed_success"]]
        rates["claimed_success_without_oracle_pass"] = {
            "numerator": sum(run["oracle"]["passed"] is not True for run in claimed), "denominator": len(claimed)}
        for kind in LONG_CONTEXT[1:]:
            rates["long_context_" + kind] = {"numerator": sum(run["long_context"] == kind for run in members),
                                            "denominator": total}
        metrics = {}
        per_run = [run_metrics(run) for run in members]
        for name in per_run[0]:
            metrics[name] = distribution([values[name] for values in per_run])
        result.append({"task": task, "comparison": comparison, "arm": arm, "label": members[0]["label"],
                       "attempted": total, "outcomes": outcomes, "rates": rates, "metrics": metrics})
    return result


def compare(corpus, aggregates, runs):
    comparisons = {item["id"]: item for item in corpus["comparisons"]}
    by_key = {(item["task"], item["comparison"], item["arm"]): item for item in aggregates}
    rows = []
    for (task, comparison, arm), baseline in by_key.items():
        if arm != "baseline" or (task, comparison, "candidate") not in by_key:
            continue
        candidate = by_key[(task, comparison, "candidate")]
        changes = []
        base_runs = {run["repeat"]: run for run in runs if (run["task"], run["comparison"], run["arm"]) ==
                     (task, comparison, "baseline")}
        for run in runs:
            if (run["task"], run["comparison"], run["arm"]) == (task, comparison, "candidate") and \
                    run["repeat"] in base_runs and base_runs[run["repeat"]]["outcome"] != run["outcome"]:
                changes.append({"repeat": run["repeat"], "baseline": base_runs[run["repeat"]]["outcome"],
                                "candidate": run["outcome"]})
        deltas = {}
        for name, value in baseline["metrics"].items():
            other = candidate["metrics"][name]
            deltas[name] = None if value["median"] is None or other["median"] is None else other["median"] - value["median"]
        rows.append({"task": task, "comparison": comparison, "issue": comparisons[comparison]["issue"],
                     "baseline": baseline["label"], "candidate": candidate["label"],
                     "comparable": baseline["attempted"] == candidate["attempted"] and baseline["attempted"] > 0,
                     "outcome_changes": changes, "median_deltas": deltas})
    return rows


def calibrate(corpus, runs):
    """Deterministic tier only: the evaluator must reproduce each task's declared result."""
    tasks = {task["id"]: task for task in corpus["tasks"]}
    failures = []
    for run in runs:
        expected = tasks[run["task"]]["expected"]
        observed = {"outcome": run["outcome"], "claimed_success": run["claimed_success"],
                    "long_context": run["long_context"], "constraint_retention": run["constraint_retention"],
                    "tool_selection_correct": run["tool_selection"]["correct"] if run["tool_selection"] else None}
        for name, value in expected.items():
            if observed.get(name) != value:
                failures.append(f"{run['key']}: expected {name}={value!r}, observed {observed.get(name)!r}")
    return failures


# --------------------------------------------------------------- live guards


class GuardRefusal(RuntimeError):
    """The live tier refuses to start or to dispatch more provider work."""


def require_live_authorization(credential, approval, corpus_sha256, models, ceiling, wall_seconds, now):
    """Refuse a live experiment unless every authorization and budget is explicit.

    credential is the Tenant Provider Credential metadata read from the server;
    approval is the operator's disclosure approval document; models maps each
    Registry model the experiment uses to its configuration.
    """
    if not isinstance(ceiling, int) or isinstance(ceiling, bool) or ceiling <= 0:
        raise GuardRefusal("a positive hard spend ceiling in nanocredits is required")
    if not isinstance(wall_seconds, (int, float)) or wall_seconds <= 0:
        raise GuardRefusal("a positive wall-time limit is required")
    if not credential:
        raise GuardRefusal("an explicitly connected Tenant Provider Credential is required")
    if credential.get("state") != "active":
        raise GuardRefusal(f"Provider Credential is not active (state {credential.get('state')!r})")
    for model_ref, config in models.items():
        if config.get("credential_env"):
            raise GuardRefusal(f"model {model_ref} reads an environment credential; live runs require the "
                               "Tenant's connected Provider Credential")
        if config.get("provider_credential") != credential.get("provider"):
            raise GuardRefusal(f"model {model_ref} is not bound to the connected {credential.get('provider')} "
                               "Provider Credential")
    if not approval:
        raise GuardRefusal("a disclosure approval document is required")
    for field in ("approver", "approved_at", "expires_at", "corpus_sha256", "models", "max_spend_nanocredits",
                  "disclosure"):
        if field not in approval:
            raise GuardRefusal(f"disclosure approval is missing {field}")
    if approval["corpus_sha256"] != corpus_sha256:
        raise GuardRefusal("disclosure approval names a different corpus snapshot")
    if sorted(approval["models"]) != sorted(models):
        raise GuardRefusal("disclosure approval does not cover exactly the experiment's models")
    if approval["expires_at"] <= now:
        raise GuardRefusal("disclosure approval has expired")
    if ceiling > approval["max_spend_nanocredits"]:
        raise GuardRefusal("spend ceiling exceeds the approved maximum")
    prices = approval.get("prices") or {}
    for model_ref in models:
        price = prices.get(model_ref) or {}
        for field in ("input_nanocredits_per_token", "output_nanocredits_per_token"):
            value = price.get(field)
            if not isinstance(value, int) or isinstance(value, bool) or value <= 0:
                raise GuardRefusal(f"disclosure approval needs a positive {field} for {model_ref}; reservations "
                                   "are computed from approved prices")
    return True


class SpendLedger:
    """Hard spend and time ceiling for live provider work.

    Every attempt is reserved at its worst case before dispatch. Settlement
    replaces the reservation with the provider-reported cost; an attempt whose
    cost is unknown stays charged at its full reservation (ADR 0018).
    """

    def __init__(self, ceiling, wall_seconds, clock):
        self.ceiling = ceiling
        self.deadline = clock() + wall_seconds
        self.clock = clock
        self.attempts = []
        self.refusal = None

    @property
    def committed(self):
        return sum(item["charged"] for item in self.attempts)

    def reserve(self, key, worst_case):
        if self.clock() >= self.deadline:
            self.refusal = "wall-time limit reached"
            raise GuardRefusal(self.refusal)
        if worst_case <= 0:
            raise GuardRefusal("a reservation needs a positive worst-case cost")
        if self.committed + worst_case > self.ceiling:
            self.refusal = f"reserving {worst_case} for {key} would exceed the ceiling {self.ceiling}"
            raise GuardRefusal(self.refusal)
        attempt = {"key": key, "reserved": worst_case, "charged": worst_case, "reported": None, "state": "reserved"}
        self.attempts.append(attempt)
        return attempt

    def settle(self, attempt, reported_cost):
        if reported_cost is None:
            attempt["state"] = "uncertain"
        else:
            attempt.update({"reported": reported_cost, "charged": reported_cost, "state": "settled"})

    def summary(self):
        return {"ceiling_nanocredits": self.ceiling, "committed_nanocredits": self.committed,
                "uncertain_attempts": sum(item["state"] != "settled" for item in self.attempts),
                "attempts": self.attempts, "refusal": self.refusal}


# ------------------------------------------------------------ memory matrix


MEMORY_EVIDENCE = ("deterministic-api", "deterministic-test", "feature-not-available", "requires-live-tier")


def load_memory_corpus(path=None):
    path = Path(path or EVALUATION / "memory.toml")
    corpus = tomllib.loads(path.read_text())
    errors = []
    ids = set()
    for scenario in corpus.get("scenarios", []):
        if scenario.get("id") in ids:
            errors.append(f"duplicate memory scenario {scenario.get('id')}")
        ids.add(scenario.get("id"))
        if scenario.get("evidence") not in MEMORY_EVIDENCE:
            errors.append(f"memory scenario {scenario.get('id')} has unknown evidence")
        for key in ("matrix_row", "issue", "observation"):
            if key not in scenario:
                errors.append(f"memory scenario {scenario.get('id')} is missing {key}")
        if scenario.get("evidence") == "deterministic-api" and not scenario.get("actions"):
            errors.append(f"memory scenario {scenario.get('id')} declares no ordered actions")
        if scenario.get("evidence") == "deterministic-test" and not scenario.get("command"):
            errors.append(f"memory scenario {scenario.get('id')} declares no test command")
    rows = {scenario.get("matrix_row") for scenario in corpus.get("scenarios", [])}
    missing = [row for row in corpus.get("matrix", []) if row not in rows]
    if missing:
        errors.append(f"memory matrix rows without a scenario: {missing}")
    if errors:
        raise CorpusError("; ".join(errors))
    corpus["_snapshot"] = snapshot(corpus["version"], path.parent, [path.name])
    return corpus


def memory_matrix(corpus, executed):
    """Report every scenario; unexecuted evidence kinds are explicit, never passing."""
    scenarios = []
    for scenario in corpus["scenarios"]:
        evidence = scenario["evidence"]
        if evidence in ("feature-not-available", "requires-live-tier"):
            entry = {"status": evidence, "variants": [], "checks": [], "detail": scenario["observation"]}
        else:
            entry = executed.get(scenario["id"]) or {"status": "not-run", "variants": [], "checks": [],
                                                     "detail": "not executed in this invocation"}
        scenarios.append({"id": scenario["id"], "issue": scenario["issue"], "evidence": evidence, **entry})
    summary = {}
    for item in scenarios:
        summary[item["status"]] = summary.get(item["status"], 0) + 1
    return {"corpus": corpus["version"], "scenarios": scenarios, "summary": summary}


# ---------------------------------------------------------------- output


def write_exclusive(path, value):
    """Create the manifest exclusively with mode 0600; never overwrite evidence."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
    return path


def summary_table(result):
    """A short human-readable view printed after a run; the manifest is authoritative."""
    lines = [f"{'task':38} {'comparison':13} {'arm':9} {'n':>2}  outcomes"]
    for item in result["aggregates"]:
        counts = ", ".join(f"{name}={count}" for name, count in item["outcomes"].items() if count)
        lines.append(f"{item['task'][:38]:38} {item['comparison'][:13]:13} {item['label'][:9]:9} "
                     f"{item['attempted']:>2}  {counts}")
    lines.append("memory: " + ", ".join(f"{k}={v}" for k, v in sorted(result["memory_matrix"]["summary"].items())))
    unavailable = [row["comparison"] + " (" + row["issue"] + ")" for row in result["feature_availability"]
                   if row["status"] == "feature-not-available"]
    lines.append("feature-not-available: " + ", ".join(unavailable))
    return "\n".join(lines)
