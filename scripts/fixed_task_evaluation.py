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
import datetime
import hashlib
import io
import json
import math
import os
from pathlib import Path
import re
import random
import uuid
import statistics
import selectors
import time
import subprocess
import sys
import tempfile
import tomllib

from fixed_task_fixture import FIXTURE_PATCHES

ROOT = Path(__file__).resolve().parent.parent
EVALUATION = ROOT / "tests/evaluation"
EVALUATOR_NAME = "aidash-fixed-task-evaluator"
EVALUATOR_VERSION = "2.1.4"
# Files whose bytes define the evaluator, relative to the repository root; their digest is in
# every manifest. The schemas belong here because they decide which evidence `validate` accepts.
EVALUATOR_FILES = ("scripts/fixed_task_evaluation.py", "scripts/fixed_task_fixture.py", "scripts/fixed_task_runner.py",
                   "scripts/tool_call_approvals.py", "scripts/evaluate-fixed-tasks.py",
                   "tests/evaluation/schema/experiment.schema.json", "tests/evaluation/schema/result.schema.json")
OUTCOMES = ("success", "false_completion", "policy_compliant_escalation", "unresolved",
            "overflow_failure", "interrupted", "infrastructure_failure")
LONG_CONTEXT = ("none", "pruning", "summary_recovery", "terminal_overflow")
TERMINAL_PHASES = ("COMPLETED", "FAILED", "CANCELLED")
CONCLUSION_SCOPE = ("Conclusions are limited to the tested tasks, fixture or model configuration and "
                    "repeat counts recorded in this manifest. They are not a general quality, cost or "
                    "superiority claim and approve no default rollout.")
# Comparison statuses that have no runnable arms: the feature is absent, or present but not yet measured.
UNRUNNABLE_STATUSES = ("feature-not-available", "not-measured")
# The only Agent fields a comparison arm may set. Model, step limit, bindings and instructions
# come from the task, so live authorization and spend reservations always price the final Agent.
COMPARISON_AGENT_FIELDS = ("projection_version", "prompt_cache", "exposure")
# The implicit builtins an Agent gets unless `remove_default` names them, the ones bound Skills
# need, and the deferred@1 exposure builtins (crates/aidash-domain/src/registry/bindings.rs,
# exposure.rs `default_tools` and `skill_support_tools`). test_fixed_task_evaluation.py checks
# them against the Rust lists.
DEFAULT_TOOLS = ("workspace_observe", "workspace_wait", "skill_list", "skill_load", "skill_read", "file_search",
                 "file_read", "task_create", "task_delegate", "agent_discover", "artifact_publish",
                 "workspace_message", "memory_mutate", "memory_recall", "memory_reflect")
SKILL_TOOLS = ("skill_list", "skill_load", "skill_read")
SKILL_ASSET_READ = "skill_asset_read"
EXPOSURE_TOOLS = ("capability_search", "capability_describe", "capability_load", "capability_unload")
ORACLE_KINDS = ("python-tests", "json-overdue-totals", "constraint-retention", "tool-selection",
                "escalation", "answer-pattern", "multi-call", "compaction-retention")
# The fields each oracle kind reads, checked when the corpus loads so that no Run is dispatched
# for a task its oracle cannot grade. A `pattern` must compile as a regular expression.
ORACLE_FIELDS = {
    "python-tests": {"target": "string", "visible": "strings", "hidden": "strings"},
    "json-overdue-totals": {"source": "string", "as_of": "string"},
    "constraint-retention": {"budget_usd": "integer", "excluded_vendors": "strings"},
    "tool-selection": {"expected_tool": "string", "answer_pattern": "pattern"},
    "escalation": {"forbidden_tool": "string", "approval_pattern": "pattern", "answer_pattern": "pattern"},
    "answer-pattern": {"answer_pattern": "pattern"},
    "multi-call": {"expected_tools": "strings", "answer_pattern": "pattern"},
    "compaction-retention": {"budget_usd": "integer", "excluded_vendors": "strings", "operation": "string"},
}
OPTIONAL_ORACLE_FIELDS = {"python-tests": {"timeout_seconds": "positive", "cases": "string"},
                          "answer-pattern": {"forbidden_pattern": "pattern"}}
# Messages that identify a failed compaction or an over-window request.
OVERFLOW_PATTERNS = (re.compile(r"(?i)compaction could not fit"), re.compile(r"(?i)context (window|length|overflow)"),
                     re.compile(r"(?i)exceeds? (the )?(model )?(context|window)"), re.compile(r"(?i)too (large|long) for"),
                     re.compile(r"(?i)mandatory context cannot fit the model window"))
SUCCESS_CLAIM = re.compile(r"(?i)\b(all tests pass|tests pass|complete|completed|done|success|successful|finished)\b")
# A negation earlier in the same sentence withdraws that success phrase; another sentence does not.
NEGATION = re.compile(r"(?i)\b(not|no|never|cannot|can't|couldn't|didn't|wasn't|isn't|unable|failed|declined|denied|"
                      r"escalat\w*)\b")
# The python-tests checks whose `detail` is the test runner's output, which includes its timing.
PYTHON_SUITE_CHECKS = ("visible-tests", "hidden-regression")


class CorpusError(ValueError):
    """The corpus or a manifest violates its declared contract."""


# ---------------------------------------------------------------- redaction

SECRET_ASSIGNMENT = re.compile(r"(?i)\b(pgpassword|postgres_password|password)\s*[=:]\s*[^\s'\"&;,]+")
DSN_PASSWORD = re.compile(r"(?i)\b(postgres(?:ql)?://[^:/\s@]+:)[^@\s]+@")


def redact_secrets(text, *literals):
    """Remove database passwords (PGPASSWORD=..., password=..., DSN userinfo) from a message."""
    text = DSN_PASSWORD.sub(r"\1[redacted]@", SECRET_ASSIGNMENT.sub(r"\1=[redacted]", str(text)))
    for literal in literals:
        if literal:
            text = text.replace(literal, "[redacted]")
    return text


# ------------------------------------------------------------------ hashing


def sha256_bytes(data):
    return hashlib.sha256(data).hexdigest()


def sha256_text(text):
    return sha256_bytes(text.encode())


def canonical_sha256(value):
    """Digest of a JSON value with sorted keys; used for configurations."""
    return sha256_text(json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False))


def snapshot(version, base, files, contents=None):
    """Hash every file by relative path; the combined digest covers names and bytes.

    `contents`, when given, maps relative paths to bytes. Bytes already in it are
    hashed as they are, and every file read here is added to it, so a caller can
    keep using exactly the bytes that were hashed instead of rereading the files.
    """
    contents = {} if contents is None else contents
    digests = {}
    for relative in sorted(set(files)):
        if relative not in contents:
            path = (base / relative).resolve()
            if base.resolve() not in path.parents:
                raise CorpusError(f"corpus file escapes its directory: {relative}")
            contents[relative] = path.read_bytes()
        digests[relative] = sha256_bytes(contents[relative])
    combined = sha256_text("".join(f"{name}\0{digest}\n" for name, digest in digests.items()))
    return {"version": version, "sha256": combined, "files": digests}


def evaluator_identity(contents=None):
    """The evaluator digest; `contents`, when given, receives the bytes that were hashed (see `snapshot`)."""
    digest = snapshot(EVALUATOR_VERSION, ROOT, [name for name in EVALUATOR_FILES if (ROOT / name).exists()],
                      contents)
    return {"name": EVALUATOR_NAME, "version": EVALUATOR_VERSION, "sha256": digest["sha256"]}


# Manifest schemas read by pin_evaluator(); while set, validation uses them instead of the files.
PINNED_SCHEMAS = {}


def pin_evaluator():
    """Snapshot the evaluator once, before an experiment does any work, and return its identity.

    The manifest records this identity, and validation uses the schemas from
    the same bytes, so an evaluator file edited during a long run can neither
    rename the evaluator that produced the results nor change which evidence
    is accepted.
    """
    contents = {}
    identity = evaluator_identity(contents)
    for name in ("experiment.schema.json", "result.schema.json"):
        PINNED_SCHEMAS[name] = json.loads(contents[f"tests/evaluation/schema/{name}"])
    return identity


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
    untracked = git("ls-files", "--others", "--exclude-standard", "-z")
    commit = head.decode().strip() if head else None
    dirty = None if status is None else bool(status.strip())
    fingerprint = None
    if dirty and diff is not None and untracked is not None:
        # `git diff` omits untracked files and the status names only their paths, so their bytes
        # are hashed too: changing one during a run must change the recorded source.
        fingerprint = sha256_bytes(diff + status + untracked_contents(root, untracked))
    return {"commit": commit, "dirty": dirty, "dirty_diff_sha256": fingerprint,
            "binary_sha256": sha256_bytes(Path(binary).read_bytes()) if binary else None,
            "server_revision": server_revision}


def untracked_contents(root, listing):
    """Every untracked, unignored file's name and bytes (a symlink's target), from `git ls-files -z`."""
    parts = []
    for name in sorted(item for item in listing.split(b"\0") if item):
        path = Path(root) / os.fsdecode(name)
        try:
            data = os.readlink(path).encode() if path.is_symlink() else path.read_bytes()
        except OSError as error:  # Removed or unreadable since the listing: record that, not nothing.
            data = f"unreadable: {error.strerror}".encode()
        parts.append(name + b"\0" + sha256_bytes(data).encode() + b"\n")
    return b"".join(parts)


# ---------------------------------------------------------- schema validation


def load_schema(name):
    if name in PINNED_SCHEMAS:
        return PINNED_SCHEMAS[name]
    return json.loads((EVALUATION / "schema" / name).read_text())


def validate(instance, schema, root=None, path="$"):
    """Validate the JSON Schema subset used by tests/evaluation/schema.

    Supported keywords: type, const, enum, required, properties,
    additionalProperties, items, minimum, maximum, maxLength, pattern and local $ref.
    Unknown keywords are rejected so a schema cannot silently lose a rule. NaN and
    the infinities are never JSON numbers, although Python's json module parses them.
    """
    root = schema if root is None else root
    if isinstance(instance, float) and not math.isfinite(instance):
        return [f"{path}: {instance} is not a finite JSON number"]
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
                  "number": lambda v: (isinstance(v, int) and not isinstance(v, bool))
                  or (isinstance(v, float) and math.isfinite(v)),
                  "null": lambda v: v is None}
        if not any(checks[name](instance) for name in types):
            return [f"{path}: expected {'/'.join(types)}, got {type(instance).__name__}"]
    if "const" in schema and not json_equal(instance, schema["const"]):
        errors.append(f"{path}: expected {schema['const']!r}")
    if "enum" in schema and not any(json_equal(instance, option) for option in schema["enum"]):
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


def validate_result(result, corpus=None, memory_corpus=None):
    """Validate a result manifest and its embedded experiment; raise on violations.

    `corpus` and `memory_corpus` are the corpora the experiment used (default: the
    repository's); their digests must match the manifest, and its availability
    rows and memory scenarios are checked against them.
    """
    errors = validate(result, load_schema("result.schema.json"))
    if isinstance(result, dict) and isinstance(result.get("experiment"), dict):
        errors += validate(result["experiment"], load_schema("experiment.schema.json"), path="$.experiment")
    # Free-form objects (requests, tool calls, usage records) are not walked by the schema.
    errors += non_finite_numbers(result)
    if not errors and isinstance(result, dict):
        if corpus is None:
            legacy = result["experiment"]["corpus"]["version"] == "fixed-task-corpus-v1"
            corpus = load_corpus(EVALUATION / "corpus-v1.toml" if legacy else None)
        errors += consistency_errors(result, corpus, memory_corpus or load_memory_corpus())
    if errors:
        raise CorpusError("result manifest is invalid:\n" + "\n".join(errors[:50]))
    return result


def non_finite_numbers(value, path="$"):
    """Paths of every NaN or infinity anywhere in a decoded JSON value."""
    if isinstance(value, float) and not math.isfinite(value):
        return [f"{path}: {value} is not a finite JSON number"]
    if isinstance(value, dict):
        return [error for name, item in value.items() for error in non_finite_numbers(item, f"{path}.{name}")]
    if isinstance(value, list):
        return [error for index, item in enumerate(value) for error in non_finite_numbers(item, f"{path}[{index}]")]
    return []


def json_equal(left, right):
    """JSON Schema equality: a boolean never equals a number, although Python's True == 1."""
    if isinstance(left, bool) or isinstance(right, bool):
        return isinstance(left, bool) and isinstance(right, bool) and left == right
    if isinstance(left, dict) and isinstance(right, dict):
        return left.keys() == right.keys() and all(json_equal(left[name], right[name]) for name in left)
    if isinstance(left, list) and isinstance(right, list):
        return len(left) == len(right) and all(map(json_equal, left, right))
    return left == right


def run_key(task, comparison, arm, repeat):
    return f"{task}/{comparison}/{arm}/{repeat}"


def consistency_errors(result, corpus, memory_corpus):
    """Cross-field rules a JSON Schema cannot express."""
    errors = []
    runs = result["runs"]
    experiment = result["experiment"]
    # The scope statement is the manifest's limit on what its measurements may be used to claim.
    if result["conclusion_scope"] != CONCLUSION_SCOPE:
        errors.append("$.conclusion_scope is not the evaluator's conclusion scope")
    keys = [run["key"] for run in runs]
    if len(keys) != len(set(keys)):
        errors.append("$.runs: duplicate run keys")
    for run in runs:
        if run["key"] != run_key(run["task"], run["comparison"], run["arm"], run["repeat"]):
            errors.append(f"$.runs: key {run['key']} does not name its task, comparison, arm and repeat")
        errors += [f"$.runs: {run['key']} {problem}" for problem in classification_errors(run)]
        usage = run["usage"]
        try:
            expected_usage = summarize_usage(None if usage["source"] == "unavailable" else usage["records"])
        except (KeyError, TypeError, AttributeError):
            expected_usage = None
        if not json_equal(usage, expected_usage):
            errors.append(f"$.runs: {run['key']} has a usage summary that differs from its Usage Records")
    errors += availability_errors(result, corpus)
    errors += memory_corpus_errors(result, memory_corpus)
    errors += plan_errors(result, corpus)
    errors += oracle_errors(result, corpus)
    # Every planned (task, comparison, arm, repeat) is recorded or skipped, and nothing else is.
    planned = {run_key(variant["task"], variant["comparison"], variant["arm"], repeat)
               for variant in experiment["variants"] for repeat in range(1, experiment["repeat_count"] + 1)}
    # Manifests written before `skipped_runs` existed have none to list.
    skipped = [item["key"] for item in result.get("skipped_runs", [])]
    if len(skipped) != len(set(skipped)):
        errors.append("$.skipped_runs: duplicate run keys")
    if set(keys) & set(skipped):
        errors.append("$.skipped_runs: a Run is both recorded and skipped")
    unplanned = sorted((set(keys) | set(skipped)) - planned)
    if unplanned:
        errors.append(f"$.runs: {unplanned[:5]} are not planned by the experiment's variants and repeats")
    order = experiment.get("execution_order")
    if order is not None:
        sequence = order.get("planned") if isinstance(order, dict) else None
        if not isinstance(sequence, list) or any(not isinstance(key, str) for key in sequence) or \
                len(sequence) != len(set(sequence)) or not planned <= set(sequence):
            errors.append("$.experiment.execution_order must retain every planned arm/repeat exactly once")
        elif order.get("method") != "seeded-rotating-arm-blocks/1" or \
                not isinstance(order.get("seed"), int) or isinstance(order.get("seed"), bool):
            errors.append("$.experiment.execution_order needs its recorded integer seed and balancing method")
        else:
            try:
                all_items = {run_key(item["task"]["id"], item["comparison"]["id"], item["arm"], repeat): item
                             for item in plan(corpus) for repeat in range(1, experiment["repeat_count"] + 1)}
                selected = list({(all_items[key]["task"]["id"], all_items[key]["comparison"]["id"],
                                  all_items[key]["arm"]): all_items[key] for key in sequence}.values())
                # The actual order starts from corpus order, independent of a manifest editor's order.
                selected_keys = {(item["task"]["id"], item["comparison"]["id"], item["arm"]) for item in selected}
                selected = [item for item in plan(corpus) if (item["task"]["id"], item["comparison"]["id"],
                                                              item["arm"]) in selected_keys]
                expected_order = [run_key(item["task"]["id"], item["comparison"]["id"], item["arm"], repeat)
                                  for item, repeat in execution_schedule(selected, experiment["repeat_count"],
                                                                         order["seed"])]
                if sequence != expected_order:
                    errors.append("$.experiment.execution_order differs from its seed and frozen arm plan")
            except (KeyError, TypeError, CorpusError):
                errors.append("$.experiment.execution_order contains unknown or malformed arm/repeat ids")
    # An error does not show which Runs were never dispatched, so every missing Run is listed as skipped.
    missing = sorted(planned - set(keys) - set(skipped))
    if missing:
        errors.append(f"$.runs: planned Runs {missing[:5]} are neither recorded nor skipped; none may be dropped")
    groups = {}
    for run in runs:
        groups.setdefault((run["task"], run["comparison"], run["arm"]), []).append(run)
    # Rates, metrics and paired comparisons are derived data: each is recomputed from the recorded Runs.
    issues = {item["id"]: item["issue"] for item in corpus["comparisons"]}
    try:
        derived_aggregates = aggregate(runs)
        expected_rows = {(row["task"], row["comparison"], row.get("baseline_arm", "baseline"),
                          row.get("candidate_arm", "candidate")): row for row in compare(issues, derived_aggregates, runs)}
    except (KeyError, TypeError, ValueError, AttributeError) as error:
        errors.append(f"derived summaries cannot be recomputed from the Runs: {type(error).__name__}: {error}")
        derived_aggregates, expected_rows = [], {}
    # Calibration applies to the deterministic tier only, and its failures are recomputed.
    calibration = result["calibration"]
    applies = experiment["tier"] == "deterministic"
    try:
        expected_failures = calibrate(corpus, runs) if applies else []
    except (KeyError, TypeError) as error:
        expected_failures = [f"cannot be recomputed: {type(error).__name__}"]
    if calibration["applied"] is not applies or not json_equal(calibration["failures"], expected_failures):
        errors.append("$.calibration differs from the calibration the Runs produce for this tier")
    derived = {(item["task"], item["comparison"], item["arm"]): item for item in derived_aggregates}
    aggregated = set()
    for recorded in result["aggregates"]:
        group = (recorded["task"], recorded["comparison"], recorded["arm"])
        if group in aggregated:
            errors.append(f"aggregate {'/'.join(group)} is duplicated")
            continue
        aggregated.add(group)
        matching = groups.get(group)
        if matching is None:
            errors.append(f"aggregate {'/'.join(group)} has no recorded Runs")
            continue
        outcomes = {name: sum(run["outcome"] == name for run in matching) for name in OUTCOMES}
        if recorded["attempted"] != len(matching) or not set(recorded["outcomes"]) <= set(OUTCOMES) or \
                {name: recorded["outcomes"].get(name, 0) for name in OUTCOMES} != outcomes:
            errors.append(f"aggregate {'/'.join(group)} does not account for every attempted Run")
        expected = derived.get(group)
        if expected is not None and not all(json_equal(recorded[field], expected[field])
                                            for field in ("label", "rates", "metrics")):
            errors.append(f"aggregate {'/'.join(group)} has a label, rates or metrics that differ from its Runs")
    for group in sorted(groups.keys() - aggregated):
        errors.append(f"Runs of {'/'.join(group)} have no aggregate")
    # No paired comparison may be altered, added or deleted.
    recorded_rows = {}
    for row in result["comparisons"]:
        pair = (row["task"], row["comparison"], row.get("baseline_arm", "baseline"), row.get("candidate_arm", "candidate"))
        if pair in recorded_rows:
            errors.append(f"comparison {'/'.join(pair)} is duplicated")
        recorded_rows[pair] = row
    for pair in sorted(expected_rows.keys() | recorded_rows.keys()):
        if pair not in recorded_rows:
            errors.append(f"comparison {'/'.join(pair)} is missing although both arms have Runs")
        elif pair not in expected_rows:
            errors.append(f"comparison {'/'.join(pair)} has no recorded Runs for both arms")
        elif not json_equal(recorded_rows[pair], expected_rows[pair]):
            errors.append(f"comparison {'/'.join(pair)} differs from the one its Runs produce")
    memory = result["memory_matrix"]
    counted = {}
    for scenario in memory["scenarios"]:
        counted[scenario["status"]] = counted.get(scenario["status"], 0) + 1
        errors += [f"$.memory_matrix: scenario {scenario['id']} {problem}"
                   for problem in memory_status_errors(scenario)]
    if not json_equal(memory["summary"], counted):
        errors.append("$.memory_matrix.summary does not count its scenarios")
    spend = result.get("spend")
    # The live tier reserves spend before every dispatch, so its ledger is mandatory evidence;
    # the deterministic tier makes no paid call and has none.
    if (spend is None) != (experiment["tier"] != "live"):
        errors.append("$.spend must be an object for a live experiment and null otherwise")
    if spend is not None:
        attempts = spend["attempts"]
        if spend["committed_nanocredits"] != sum(attempt["charged"] for attempt in attempts):
            errors.append("$.spend.committed_nanocredits differs from the sum of the attempts' charges")
        if spend["uncertain_attempts"] != len(attempts):
            errors.append("$.spend.uncertain_attempts differs from the number of attempts")
        if spend["ceiling_nanocredits"] != experiment["limits"]["spend_ceiling_nanocredits"]:
            errors.append("$.spend.ceiling_nanocredits differs from the experiment's spend ceiling")
        if spend["committed_nanocredits"] > spend["ceiling_nanocredits"]:
            errors.append("$.spend.committed_nanocredits exceeds the hard spend ceiling")
        for attempt in attempts:
            # The live tier reads no Usage Record (the schema keeps `reported` null), so no attempt
            # is ever charged below its reservation.
            if attempt["charged"] != attempt["reserved"]:
                errors.append(f"$.spend.attempts: {attempt['key']} is not charged at its full reservation")
            if attempt["key"] not in keys:
                errors.append(f"$.spend.attempts: {attempt['key']} reserved spend for a Run that is not recorded")
        # Every dispatched Run was reserved exactly once before dispatch, so paid work cannot vanish.
        reserved = [attempt["key"] for attempt in attempts]
        if len(reserved) != len(set(reserved)):
            errors.append("$.spend.attempts: a Run has more than one spend attempt")
        for key in sorted(set(keys) - set(reserved)):
            errors.append(f"$.spend.attempts: recorded Run {key} has no spend attempt")
    return errors


def plan_errors(result, corpus):
    """Variants and Runs against the corpus plan: each names a planned (task, comparison, arm) exactly.

    The accepted Run keys derive from `experiment.variants`, so a variant must
    carry its pair's label and Agent fields, and a Run its pair's label, the
    task's oracle kind and the digest of the task text sent to the Agent.
    """
    if not json_equal(result["experiment"]["corpus"], corpus["_snapshot"]):
        return []  # availability_errors reports the corpus mismatch.
    pairs = {(item["task"]["id"], item["comparison"]["id"], item["arm"]): item for item in plan(corpus)}
    errors = []
    variants = [(variant["task"], variant["comparison"], variant["arm"])
                for variant in result["experiment"]["variants"]]
    if len(variants) != len(set(variants)):
        errors.append("$.experiment.variants: a (task, comparison, arm) is listed more than once")
    for variant in result["experiment"]["variants"]:
        item = pairs.get((variant["task"], variant["comparison"], variant["arm"]))
        if item is None or variant["label"] != item["label"] or not json_equal(variant["agent_fields"],
                                                                              item["agent_fields"]):
            errors.append(f"$.experiment.variants: {variant['task']}/{variant['comparison']}/{variant['arm']} is not "
                          "a corpus pair with its label and Agent fields")
    for run in result["runs"]:
        item = pairs.get((run["task"], run["comparison"], run["arm"]))
        if item is None or run["label"] != item["label"] or run["oracle"]["kind"] != item["task"]["oracle"]["kind"] \
                or run["evidence"]["input_sha256"] != sha256_text(task_description(corpus, item["task"])):
            errors.append(f"$.runs: {run['key']} does not match its corpus task's label, oracle and input digest")
    return errors


# Regraded oracles by corpus digest, task and recorded evidence: grading is a function of those,
# and a code oracle runs its tests, so repeated validations reuse the result.
REGRADED = {}


def comparable_oracle(oracle):
    """An oracle without the test runner's output in its suite checks, which includes run timings."""
    return dict(oracle, checks=[{name: value for name, value in check.items()
                                 if not (name == "detail" and check.get("check") in PYTHON_SUITE_CHECKS)}
                                for check in oracle["checks"]])


def oracle_errors(result, corpus):
    """Deterministic Runs regraded from their recorded fixture evidence; recorded checks are never the authority.

    A deterministic Run keeps its whole final answer, tool calls and human
    answers, so the task's oracle is applied to them again and must reproduce the
    recorded oracle. A code answer is executed only when it is one of the
    fixture's own patches: validation never runs code read from a manifest. Live
    evidence keeps only digests of restricted content and cannot be regraded.
    """
    experiment = result["experiment"]
    if experiment["tier"] != "deterministic" or not json_equal(experiment["corpus"], corpus["_snapshot"]):
        return []
    tasks = {task["id"]: task for task in corpus["tasks"]}
    variants = {(variant["task"], variant["comparison"], variant["arm"]): variant
                for variant in experiment["variants"]}
    errors = []
    for run in result["runs"]:
        task = tasks.get(run["task"])
        variant = variants.get((run["task"], run["comparison"], run["arm"]))
        if task is None or variant is None:
            continue  # plan_errors and the planned-key check report it.
        evidence = run["evidence"]
        answer = evidence["final_answer"]
        if evidence["final_answer_sha256"] != (None if answer is None else sha256_text(answer)):
            errors.append(f"$.runs: {run['key']} has a final answer that differs from its digest")
            continue
        code = fenced_blocks(answer, "python") if task["oracle"]["kind"] == "python-tests" else []
        if code and code[-1] not in FIXTURE_PATCHES:
            errors.append(f"$.runs: {run['key']} answers with code that is not a fixture patch; validation never "
                          "executes it")
            continue
        observation = {"final_answer": answer, "tool_calls": run["tool_calls"], "human_answers": run["human_answers"],
                       "native_projection": variant["agent_fields"].get("projection_version") == "native",
                       "compactions": run["compactions"], "recovery_operations": run.get("recovery_operations", [])}
        try:
            loads = [ref["id"] for ref in variant["skills"]] if selects_skill(task, variant) else []
            key = (corpus["_snapshot"]["sha256"], task["id"], canonical_sha256([observation, loads]))
            if key not in REGRADED:
                REGRADED[key] = grade(corpus, task, observation, True, loads)
            regraded = REGRADED[key]
        except (KeyError, TypeError, AttributeError, ValueError) as error:
            errors.append(f"$.runs: {run['key']} has evidence its oracle cannot regrade: {type(error).__name__}")
            continue
        if not json_equal(comparable_oracle(run["oracle"]), comparable_oracle(regraded)):
            errors.append(f"$.runs: {run['key']} has an oracle result that its recorded evidence does not reproduce")
    return errors


def availability_errors(result, corpus):
    """Feature availability rows against the corpus, and against the comparisons the manifest measured."""
    if not json_equal(result["experiment"]["corpus"], corpus["_snapshot"]):
        return ["$.experiment.corpus differs from the snapshot of the corpus given to validate (version, digest "
                "or file digests); pass the corpus the experiment used"]
    errors = []
    rows = result["feature_availability"]
    ids = [row["comparison"] for row in rows]
    if len(ids) != len(set(ids)):
        errors.append("$.feature_availability: a comparison has more than one row")
    declared = {item["id"]: item for item in corpus["comparisons"]}
    if set(ids) != set(declared):
        errors.append("$.feature_availability: the rows are not exactly the corpus comparisons")
    measured = {variant["comparison"] for variant in result["experiment"]["variants"]} | \
        {run["comparison"] for run in result["runs"]} | {row["comparison"] for row in result["comparisons"]}
    for row in rows:
        item = declared.get(row["comparison"])
        if item is None:
            continue
        status = item.get("status", "available")
        allowed = (status,) if status != "available" else ("available", "not-selected", "feature-not-available")
        if row["issue"] != item["issue"] or row["status"] not in allowed:
            errors.append(f"$.feature_availability: {row['comparison']} contradicts the corpus")
        if row["comparison"] in measured and row["status"] != "available":
            errors.append(f"$.feature_availability: {row['comparison']} is {row['status']} although it was measured")
    return errors


def memory_corpus_errors(result, memory_corpus):
    """The memory matrix must report every scenario of the memory corpus, once, as the corpus declares it."""
    if not json_equal(result["experiment"]["memory_corpus"], memory_corpus["_snapshot"]):
        return ["$.experiment.memory_corpus differs from the snapshot of the memory corpus given to validate "
                "(version, digest or file digests); pass the one the experiment used"]
    errors = []
    matrix = result["memory_matrix"]
    if matrix["corpus"] != memory_corpus["version"]:
        errors.append("$.memory_matrix.corpus is not the memory corpus version")
    declared = memory_corpus["scenarios"]
    if [scenario["id"] for scenario in matrix["scenarios"]] != [scenario["id"] for scenario in declared]:
        errors.append("$.memory_matrix.scenarios are not exactly the memory corpus scenarios, once each, in order")
        return errors
    for recorded, scenario in zip(matrix["scenarios"], declared, strict=True):
        partial = scenario.get("coverage") == "partial"
        if recorded["issue"] != scenario["issue"] or recorded["evidence"] != scenario["evidence"] or \
                recorded["status"] == ("passed" if partial else "partial"):
            errors.append(f"$.memory_matrix: scenario {scenario['id']} contradicts the memory corpus")
    return errors


def classification_errors(run):
    """Ways a Run's outcome, oracle and derived fields contradict each other (see `classify`).

    The outcome is recomputed exactly as `classify` chose it, from the recorded
    infrastructure error, interruption flag, phase, oracle and phase error.
    """
    problems = []
    oracle, outcome, phase = run["oracle"], run["outcome"], run["phase"]
    checks = oracle["checks"]
    if not json_equal(run["evidence"]["verifier"], checks):
        problems.append("has verifier evidence that differs from its oracle checks")
    if oracle["passed"] is True and (not checks or not all(check.get("passed") is True for check in checks)):
        problems.append("has a passing oracle with a missing or failing check")
    if oracle["passed"] is False and all(check.get("passed") is True for check in checks):
        problems.append("has a failing oracle without a failing check")
    if run["infrastructure_error"] is not None:
        expected = "infrastructure_failure"
        if run["error"] is None:
            problems.append("records an infrastructure error but no error")
    elif run["interrupted"]:
        expected = "interrupted"
    elif phase == "COMPLETED":
        expected = "false_completion" if oracle["passed"] is not True else \
            "policy_compliant_escalation" if oracle["kind"] == "escalation" else "success"
    elif phase in ("FAILED", "CANCELLED"):
        # Without an infrastructure error, `error` is the Run's own error.
        expected = "overflow_failure" if is_overflow(run["error"]) else "unresolved"
    else:
        expected = "interrupted"
    if outcome != expected:
        problems.append(f"has outcome {outcome}, but its phase {phase}, interruption, infrastructure error and "
                        f"oracle produce {expected}")
    terminal = run["long_context"] == "terminal_overflow"
    if terminal != (outcome == "overflow_failure"):
        problems.append("has a long_context that disagrees with its overflow outcome")
    elif not terminal and run["long_context"] != "summary_recovery" and \
            run["long_context"] != ("pruning" if (run["compactions"] or 0) > 0 else "none"):
        problems.append("has a long_context that disagrees with its compactions")
    if run["tool_selection"] is not None and run["tool_selection"].get("correct") is not (oracle["passed"] is True):
        problems.append("has a tool_selection result that disagrees with its oracle")
    retained = [check["passed"] for check in checks if check.get("check") in ("budget", "exclusions")]
    if oracle["kind"] in ("constraint-retention", "compaction-retention"):
        expected = None if oracle["passed"] is None else all(retained)
        if run["constraint_retention"] is not expected:
            problems.append("has a constraint_retention result that disagrees with its oracle")
    elif run["constraint_retention"] is not None:
        problems.append("records constraint_retention without a constraint-retention oracle")
    return problems


def memory_status_errors(scenario):
    """Ways a memory scenario's status contradicts its own checks and variant results."""
    status, checks, variants = scenario["status"], scenario["checks"], scenario["variants"]
    failing = [check for check in checks if check.get("passed") is not True]
    broken = [variant for variant in variants if "error" in variant or variant.get("passed") is False]
    problems = []
    if status in ("passed", "partial") and (not checks or failing or broken):
        problems.append(f"is {status} although a check or variant did not pass")
    if status == "failed" and (not failing or any("error" in variant for variant in variants)):
        problems.append("is failed without a failing check, or with a variant error")
    if status in ("feature-not-available", "requires-live-tier", "not-run") and (checks or variants):
        problems.append(f"is {status} but records checks or variants")
    if status in ("feature-not-available", "requires-live-tier") and scenario["evidence"] != status:
        problems.append(f"is {status} but its evidence is {scenario['evidence']}")
    return problems


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
        if item.get("status", "available") not in ("available", *UNRUNNABLE_STATUSES):
            errors.append(f"comparison {item['id']} has an unknown status")
        if item.get("status") in UNRUNNABLE_STATUSES and not item.get("description"):
            errors.append(f"comparison {item['id']} needs a description that says why it has no arms")
        if item.get("status") not in UNRUNNABLE_STATUSES:
            try:
                arms = comparison_arms(item)
            except CorpusError as error:
                errors.append(str(error))
                arms = []
            for arm, spec in arms:
                fields = spec.get("agent")
                if not isinstance(spec.get("label"), str) or not spec["label"] or not isinstance(fields, dict):
                    errors.append(f"comparison {item['id']} needs a {arm} label and agent fields")
                    continue
                if set(fields) - set(COMPARISON_AGENT_FIELDS):
                    errors.append(f"comparison {item['id']} {arm} may set only the Agent fields "
                                  f"{list(COMPARISON_AGENT_FIELDS)}")
            model_map = item.get("model_by_task_model", {})
            if not isinstance(model_map, dict) or any(not isinstance(name, str) or not isinstance(target, str) or
                                                       name not in corpus["models"] or target not in corpus["models"]
                                                       for name, target in model_map.items()):
                errors.append(f"comparison {item['id']} has an invalid model_by_task_model mapping")
            if item.get("model") and item["model"] not in corpus["models"]:
                errors.append(f"comparison {item['id']} names unknown model {item['model']}")
    tools = {tool["alias"]: tool for tool in corpus["tools"]}
    skills = {skill["id"] for skill in corpus.get("skills", [])}
    corpus_name = "corpus.toml" if path.name == "corpus-v1.toml" else path.name
    files = [corpus_name]
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
            elif comparisons[comparison].get("status") in UNRUNNABLE_STATUSES:
                errors.append(f"task {tid} selects unavailable comparison {comparison}")
        for skill in task.get("skills", []):
            if skill not in skills:
                errors.append(f"task {tid} names unknown Skill {skill}")
        oracle = task.get("oracle", {})
        if oracle.get("kind") in ORACLE_KINDS:
            oracle_problems = oracle_contract_errors(oracle)
            errors += [f"task {tid} {oracle['kind']} oracle {problem}" for problem in oracle_problems]
        else:
            oracle_problems = [f"task {tid} has unknown oracle {oracle.get('kind')}"]
            errors += oracle_problems
        if task.get("expected", {}).get("outcome") not in OUTCOMES:
            errors.append(f"task {tid} declares an unknown expected outcome")
        declared = task.get("tools", [])
        for alias in ([] if declared == "all" else declared) + [task.get("expected_tool")] * ("expected_tool" in task):
            if alias not in tools:
                errors.append(f"task {tid} names unknown tool {alias}")
        if oracle_problems:
            continue  # Its file fields are malformed; the errors above already refuse the corpus.
        for relative in task.get("inputs", []) + oracle.get("visible", []) + oracle.get("hidden", []) + (
                [oracle["source"]] if "source" in oracle else []) + ([oracle["cases"]] if "cases" in oracle else []):
            if not (base / relative).is_file():
                errors.append(f"task {tid} references missing file {relative}")
            files.append(relative)
    if errors:
        raise CorpusError("; ".join(errors))
    # The approved snapshot is what every Run uses: inputs and oracle files are read
    # once here and never reread, so a later change cannot reach a Task or an oracle.
    contents = {corpus_name: raw}
    corpus["_snapshot"] = snapshot(corpus["version"], base, files, contents)
    corpus["_files"] = contents
    corpus["_base"] = base
    return corpus


def oracle_contract_errors(oracle):
    """Problems with the fields an oracle of a known kind reads; empty when it can grade a Run."""
    def problem(value, rule):
        if rule == "string":
            return None if isinstance(value, str) and value.strip() else "a non-empty string"
        if rule == "strings":
            return None if isinstance(value, list) and all(isinstance(item, str) for item in value) else \
                "a list of strings"
        if rule == "integer":
            return None if isinstance(value, int) and not isinstance(value, bool) else "an integer"
        if rule == "positive":
            return None if isinstance(value, (int, float)) and not isinstance(value, bool) and value > 0 and \
                math.isfinite(value) else "a positive number"
        if not isinstance(value, str) or not value:
            return "a non-empty regular expression"
        try:
            re.compile(value)
        except re.error as error:
            return f"a valid regular expression ({error})"
        return None
    problems = []
    kind = oracle["kind"]
    for required, fields in ((True, ORACLE_FIELDS[kind]), (False, OPTIONAL_ORACLE_FIELDS.get(kind, {}))):
        for field, rule in fields.items():
            if field not in oracle:
                if required:
                    problems.append(f"needs {field}")
                continue
            expected = problem(oracle[field], rule)
            if expected:
                problems.append(f"{field} must be {expected}")
    return problems


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
        content = corpus["_files"][relative].decode()
        fence = "csv" if relative.endswith(".csv") else "python" if relative.endswith(".py") else ""
        parts.append(f"{Path(relative).name}:\n```{fence}\n{content.rstrip()}\n```")
    return "\n\n".join(parts)


def comparison_arms(comparison):
    """Normalize versioned arm lists and historical baseline/candidate corpora."""
    if "arms" not in comparison:
        return [(name, comparison.get(name, {})) for name in ("baseline", "candidate")]
    arms = comparison["arms"]
    if not isinstance(arms, list) or len(arms) < 2:
        raise CorpusError(f"comparison {comparison['id']} needs at least two arms")
    ids = [arm.get("id") if isinstance(arm, dict) else None for arm in arms]
    if any(not isinstance(name, str) or not re.fullmatch(r"[a-z][a-z0-9-]*", name) for name in ids) or \
            len(ids) != len(set(ids)):
        raise CorpusError(f"comparison {comparison['id']} arm ids must be distinct lowercase identifiers")
    return [(arm["id"], arm) for arm in arms]


def execution_schedule(planned, repeats, seed):
    """Randomized task blocks with rotating arm positions, reproducible from a seed.

    Each complete cycle of N repeats places every arm of a task at every
    position once. Task-block order and the initial arm permutation are seeded.
    Nothing changes the actual prompts or cross-Run cache condition.
    """
    if not isinstance(seed, int) or isinstance(seed, bool):
        raise CorpusError("execution seed must be an integer")
    groups = {}
    for item in planned:
        groups.setdefault((item["task"]["id"], item["comparison"]["id"]), []).append(item)
    rng = random.Random(seed)
    groups = list(groups.values())
    for arms in groups:
        rng.shuffle(arms)
    schedule = []
    for repeat in range(1, repeats + 1):
        blocks = list(groups)
        rng.shuffle(blocks)
        for arms in blocks:
            rotation = (repeat - 1) % len(arms)
            schedule += [(item, repeat) for item in arms[rotation:] + arms[:rotation]]
    return schedule


def plan(corpus, selected_tasks=None, selected_comparisons=None, selected_arms=None):
    """Every (task, comparison, arm) pair to run; each arm differs only in Agent fields.

    Every explicitly selected task and comparison must contribute at least one
    pair, so a misspelled or inapplicable selector is refused rather than
    silently narrowing the experiment.
    """
    comparisons = {item["id"]: item for item in corpus["comparisons"]}
    task_ids = {task["id"] for task in corpus["tasks"]}
    unknown = sorted(set(selected_tasks or []) - task_ids) + sorted(set(selected_comparisons or []) - set(comparisons))
    if unknown:
        raise CorpusError(f"unknown task or comparison selectors: {unknown}")
    planned = []
    for task in corpus["tasks"]:
        if selected_tasks and task["id"] not in selected_tasks:
            continue
        for comparison_id in task["comparisons"]:
            if selected_comparisons and comparison_id not in selected_comparisons:
                continue
            comparison = comparisons[comparison_id]
            for arm, spec in comparison_arms(comparison):
                fields = dict(spec["agent"])
                if task.get("projection_exposure") and comparison_id in ("projection", "explicit-projection"):
                    fields["exposure"] = {"version": task["projection_exposure"]}
                planned.append({"task": task, "comparison": comparison, "arm": arm,
                                "label": spec["label"], "agent_fields": fields,
                                "model": comparison.get("model_by_task_model", {}).get(task["model"]) or
                                         comparison.get("model") or task["model"],
                                "route_id": comparison.get("route"),
                                "model_configuration": dict(corpus["models"][comparison.get(
                                    "model_by_task_model", {}).get(task["model"]) or comparison.get("model") or
                                    task["model"]])})
    uncovered = sorted(set(selected_tasks or []) - {item["task"]["id"] for item in planned}) + sorted(
        set(selected_comparisons or []) - {item["comparison"]["id"] for item in planned})
    if uncovered:
        raise CorpusError(f"selectors that match no runnable task and comparison pair: {uncovered}")
    if selected_arms:
        requested = set(selected_arms)
        groups = {}
        for item in planned:
            groups.setdefault((item["task"]["id"], item["comparison"]["id"]), set()).add(item["arm"])
        missing = [(task, comparison, sorted(requested - arms)) for (task, comparison), arms in groups.items()
                   if requested - arms]
        if missing:
            raise CorpusError(f"requested arms are absent from selected task/comparison pairs: {missing[:5]}")
        planned = [item for item in planned if item["arm"] in requested]
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
    """The last fenced JSON block, which must be an object; anything else is a malformed answer."""
    blocks = fenced_blocks(text, "json")
    if not blocks:
        raise ValueError("the answer has no ```json block")
    value = json.loads(blocks[-1])
    if not isinstance(value, dict):
        raise ValueError(f"the last ```json block is a {type(value).__name__}, not an object")
    return value


def run_python_tests(directory, pattern, timeout):
    """Run unittest discovery in isolated mode; the start directory is the import root."""
    try:
        completed = subprocess.run(
            [sys.executable, "-I", "-m", "unittest", "discover", "-s", str(directory), "-p", pattern],
            cwd=directory, capture_output=True, text=True, timeout=timeout,
            env={"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "PYTHONDONTWRITEBYTECODE": "1"})
    except subprocess.TimeoutExpired:
        return {"pattern": pattern, "passed": False, "tests": None, "detail": f"timed out after {timeout}s"}
    output = completed.stderr or completed.stdout
    ran = re.findall(r"(?m)^Ran (\d+) tests? in ", output)
    tail = output.strip().splitlines()[-3:]
    return {"pattern": pattern, "passed": completed.returncode == 0, "tests": int(ran[-1]) if ran else None,
            "detail": " | ".join(tail)[:500]}


def bounded_subprocess(command, input_bytes, timeout, max_output_bytes=65536):
    """Collect candidate output without allowing a subprocess to allocate unbounded host memory."""
    deadline = time.monotonic() + timeout
    output = bytearray()
    with subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                          env={"PATH": os.environ.get("PATH", "/usr/bin:/bin")}) as process:
        try:
            process.stdin.write(input_bytes)
            process.stdin.close()
            with selectors.DefaultSelector() as selector:
                selector.register(process.stdout, selectors.EVENT_READ)
                while selector.get_map():
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise subprocess.TimeoutExpired(command, timeout)
                    for key, _ in selector.select(remaining):
                        chunk = os.read(key.fd, 8192)
                        if not chunk:
                            selector.unregister(key.fileobj)
                        elif len(output) + len(chunk) > max_output_bytes:
                            raise ValueError("candidate output exceeds the 65536-byte limit")
                        else:
                            output.extend(chunk)
            process.wait(timeout=max(0.001, deadline - time.monotonic()))
            return subprocess.CompletedProcess(command, process.returncode, output.decode(errors="replace"))
        finally:
            if process.poll() is None:
                process.kill()
                process.wait(timeout=5)


# This child program receives inputs only. Expected answers and the verdict are kept in the
# trusted evaluator process, so candidate imports cannot replace unittest or its assertions.
CANDIDATE_RUNNER = """import importlib.util, json, sys
requests = json.loads(sys.stdin.read())
spec = importlib.util.spec_from_file_location('candidate', sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
function = getattr(module, sys.argv[2])
values = [function(*request['args'], **request['kwargs']) for request in requests]
sys.stdout.write(json.dumps(values, allow_nan=False))
"""


class ContainerCodeOracle:
    """Compare isolated candidate outputs with trusted, frozen expected values."""
    def __init__(self, image):
        if not isinstance(image, str) or not re.fullmatch(r"(?:[a-zA-Z0-9./:_-]+@)?sha256:[0-9a-f]{64}", image):
            raise GuardRefusal("code oracle image must be pinned by a sha256 digest")
        self.image = image

    def __call__(self, directory, cases, timeout, target="pricing.py", entrypoint="discounted_total"):
        name = "aidash-code-oracle-" + uuid.uuid4().hex
        command = ["docker", "run", "--rm", "--pull", "never", "--name", name, "--network", "none",
                   "--read-only", "--pids-limit", "64", "--cpus", "1", "--memory", "128m",
                   "--memory-swap", "128m", "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
                   "--user", "65534:65534", "--tmpfs", "/tmp:rw,nosuid,nodev,size=16m", "--mount",
                   f"type=bind,src={Path(directory).resolve()},dst=/work,readonly", "--workdir", "/work",
                   "--env", "PYTHONDONTWRITEBYTECODE=1", self.image, "python", "-I", "-c", CANDIDATE_RUNNER,
                   "/work/" + target, entrypoint]
        Path(directory).chmod(0o755)
        for path in Path(directory).iterdir():
            path.chmod(0o444)
        requests = [{"args": case["args"], "kwargs": case["kwargs"]} for case in cases]
        expected = [case["expected"] for case in cases]
        try:
            completed = bounded_subprocess(command, json.dumps(requests).encode(), min(timeout, 20))
            try:
                observed = json.loads(completed.stdout)
            except ValueError:
                observed = None
            passed = completed.returncode == 0 and not non_finite_numbers(observed) and json_equal(observed, expected)
            return {"pattern": "frozen-input-vectors", "passed": passed, "tests": len(cases),
                    "detail": "candidate outputs match trusted expected values" if passed else
                              "candidate outputs differ or the isolated process returned invalid output"}
        except (subprocess.TimeoutExpired, ValueError) as error:
            return {"pattern": "frozen-input-vectors", "passed": False, "tests": len(cases),
                    "detail": "container oracle timed out" if isinstance(error, subprocess.TimeoutExpired) else
                              "candidate output exceeds the 65536-byte limit"}
        finally:
            subprocess.run(["docker", "rm", "-f", name], capture_output=True, timeout=20)


def independent_code_vectors(corpus, oracle):
    """Read the trusted vectors from the frozen corpus, never from candidate output."""
    relative = oracle.get("cases")
    if relative not in corpus["_files"]:
        raise GuardRefusal("live code oracle needs frozen independent input/expected vectors in oracle.cases")
    try:
        vectors = json.loads(corpus["_files"][relative])
        if vectors["version"] != 1 or not isinstance(vectors["entrypoint"], str) or not re.fullmatch(
                r"[a-zA-Z_][a-zA-Z0-9_]*", vectors["entrypoint"]):
            raise ValueError("invalid vector contract")
        for suite in ("visible", "hidden"):
            if not isinstance(vectors[suite], list) or not vectors[suite]:
                raise ValueError("empty vector suite")
            for case in vectors[suite]:
                if not isinstance(case, dict) or set(case) != {"args", "kwargs", "expected"} or \
                        not isinstance(case["args"], list) or not isinstance(case["kwargs"], dict) or non_finite_numbers(case):
                    raise ValueError("malformed vector")
        if Path(oracle["target"]).name != oracle["target"]:
            raise ValueError("invalid candidate target")
    except (KeyError, TypeError, ValueError):
        raise GuardRefusal("live code oracle has malformed independent input/expected vectors") from None
    return vectors


def container_code_execution(image):
    return ContainerCodeOracle(image)


def oracle_python_tests(corpus, oracle, answer, allow_code_execution):
    blocks = fenced_blocks(answer, "python")
    if not blocks:
        return False, [{"check": "patch-present", "passed": False}], "the answer contains no ```python block"
    if not allow_code_execution:
        return None, [{"check": "patch-present", "passed": True}], "code execution oracle not approved for this tier"
    files = corpus["_files"]
    checks = [{"check": "patch-present", "passed": True}]
    timeout = oracle.get("timeout_seconds", 20)
    if isinstance(allow_code_execution, ContainerCodeOracle):
        vectors = independent_code_vectors(corpus, oracle)
        for name, label in (("visible", "visible-tests"), ("hidden", "hidden-regression")):
            # No tests or expected values enter the sandbox, including the held-out suite.
            with tempfile.TemporaryDirectory(prefix="aidash-fixed-task-oracle-") as directory:
                Path(directory, oracle["target"]).write_text(blocks[-1])
                result = allow_code_execution(directory, vectors[name], timeout, oracle["target"], vectors["entrypoint"])
            checks.append({"check": label, **result})
        passed = all(check["passed"] for check in checks)
        return passed, checks, None if passed else "independent candidate-output checks failed"

    def suite(tests, prefix, pattern):
        # Each suite gets a fresh directory and a fresh copy of the patch. The held-out
        # tests are written only after the visible suite has exited, so code it ran
        # never saw them; their bytes are compared again after their own run.
        with tempfile.TemporaryDirectory(prefix="aidash-fixed-task-oracle-") as directory:
            work = Path(directory)
            (work / oracle["target"]).write_text(blocks[-1])
            written = {}
            for relative in tests:
                name = prefix + Path(relative).name
                (work / name).write_bytes(files[relative])
                written[name] = files[relative]
            result = (allow_code_execution(work, pattern, timeout) if callable(allow_code_execution)
                      else run_python_tests(work, pattern, timeout))
            changed = sorted(name for name, data in written.items()
                             if not (work / name).is_file() or (work / name).read_bytes() != data)
        if changed:
            return dict(result, passed=False, detail=f"the tested code modified the test files {changed}")
        if not result["tests"]:
            return dict(result, passed=False, detail="no test ran: " + result["detail"])
        return result
    visible = suite(oracle["visible"], "", "test_*.py")
    hidden = suite(oracle["hidden"], "heldout_", "heldout_*.py")
    checks += [{"check": "visible-tests", **visible}, {"check": "hidden-regression", **hidden}]
    passed = visible["passed"] and hidden["passed"]
    return passed, checks, None if passed else "independent tests failed"


def overdue_truth(data, as_of):
    """Overdue totals computed independently from the snapshot's CSV bytes."""
    import csv
    truth = {}
    for row in csv.DictReader(io.StringIO(data.decode(), newline="")):
        if row["status"] == "unpaid" and row["due_date"] < as_of:
            customer = truth.setdefault(row["customer"], {})
            customer[row["currency"]] = customer.get(row["currency"], 0) + int(row["amount_cents"])
    return truth


def oracle_overdue(corpus, oracle, answer):
    try:
        value = last_json_block(answer)
    except ValueError as error:
        return False, [{"check": "json-present", "passed": False}], str(error)
    truth = overdue_truth(corpus["_files"][oracle["source"]], oracle["as_of"])
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
              {"check": "exclusions", "passed": isinstance(value.get("excluded_vendors"), list) and
               all(isinstance(vendor, str) for vendor in value["excluded_vendors"]) and
               sorted(value["excluded_vendors"]) == sorted(oracle["excluded_vendors"]), "expected": sorted(oracle["excluded_vendors"]),
               "observed": value.get("excluded_vendors")}]
    passed = all(check["passed"] for check in checks)
    return passed, checks, None if passed else "a constraint or correction was not retained"


def call_arguments(call):
    """A tool call's arguments as a JSON value; providers may deliver them as a JSON string."""
    arguments = call.get("arguments")
    if isinstance(arguments, str):
        try:
            return json.loads(arguments)
        except ValueError:
            return arguments
    return arguments


def oracle_tool_selection(oracle, answer, tool_calls, expected_arguments=None):
    called = [call for call in tool_calls if not call["name"].startswith(("capability_", "skill_"))]
    first = called[0] if called else None
    checks = [{"check": "first-domain-tool", "passed": bool(first) and first["name"] == oracle["expected_tool"]
               and not first.get("dispatch_rejected"), "observed": first and first["name"]}]
    if expected_arguments is not None:
        observed = call_arguments(first) if first else None
        checks.append({"check": "arguments", "passed": json_equal(observed, expected_arguments),
                       "expected": expected_arguments, "observed": observed})
    checks.append({"check": "answer", "passed": bool(re.search(oracle["answer_pattern"], answer or ""))})
    passed = all(check["passed"] for check in checks)
    return passed, checks, None if passed else ("the expected tool was not selected first with the expected "
                                                "arguments, or the answer is wrong")


def oracle_escalation(oracle, answer, tool_calls, human_answers):
    # The denial must be for the forbidden action: the Tool call approval of a call of that tool,
    # or an approval request the Agent raised itself whose prompt names the action. A request
    # about anything else, or one that was granted, proves nothing about the forbidden effect.
    denied = any(entry.get("approved") is False and (
        (entry.get("kind") == "tool_call" and entry.get("tool") == oracle["forbidden_tool"])
        or (entry.get("kind") == "APPROVAL_REQUIRED"
            and bool(re.search(oracle["approval_pattern"], entry.get("prompt") or "")))) for entry in human_answers)
    # Only a call rejected before dispatch (unavailable, or its approval denied) proves the
    # forbidden effect did not happen; an error after dispatch may follow a partial effect.
    executed = [call for call in tool_calls if call["name"] == oracle["forbidden_tool"]
                and not call.get("dispatch_rejected")]
    checks = [{"check": "forbidden-action-approval-denied", "passed": denied},
              {"check": "forbidden-tool-not-executed", "passed": not executed},
              {"check": "escalation-reported", "passed": bool(re.search(oracle["answer_pattern"], answer or ""))}]
    passed = all(check["passed"] for check in checks)
    return passed, checks, None if passed else "escalation policy was not followed"


def grade(corpus, task, observation, allow_code_execution, required_loads=()):
    """Apply the task's independent oracle to an observed Run. Never reads the Agent's claim.

    `required_loads` lists Registry ids the Run must have loaded with `capability_load`:
    under deferred exposure an answer alone cannot show which Skill reached the model.
    """
    oracle = task["oracle"]
    answer = observation.get("final_answer")
    kind = oracle["kind"]
    if answer is None and kind not in ("escalation",):
        return {"kind": kind, "passed": None, "checks": [], "detail": "the Run produced no final answer"}
    if kind == "python-tests":
        passed, checks, detail = oracle_python_tests(corpus, oracle, answer, allow_code_execution)
    elif kind == "json-overdue-totals":
        passed, checks, detail = oracle_overdue(corpus, oracle, answer)
    elif kind in ("constraint-retention", "compaction-retention"):
        passed, checks, detail = oracle_constraints(oracle, answer)
        if kind == "compaction-retention":
            adopted = observation.get("recovery_operations", [])
            operation = oracle["operation"]
            observed = ((observation.get("compactions") or 0) > 0 if operation == "pruning"
                        else operation in adopted)
            checks.append({"check": "actual-" + operation, "passed": observed})
            passed = passed and observed
            if not observed:
                detail = "no adopted " + operation + " operation was observed"
    elif kind == "multi-call":
        calls = [call for call in observation.get("tool_calls", []) if not call.get("dispatch_rejected")]
        checks = [{"check": "delivered-tools", "passed": sorted(call["name"] for call in calls) ==
                   sorted(oracle["expected_tools"]) and all(not call.get("error") for call in calls)},
                  {"check": "answer", "passed": bool(re.search(oracle["answer_pattern"], answer or ""))}]
        if observation.get("native_projection"):
            ids = [call.get("id") for call in calls]
            turns = [call.get("model_turn") for call in calls]
            checks.append({"check": "native-multi-call-turn", "passed": len(ids) == len(set(ids)) and
                           all(ids) and bool(turns) and None not in turns and len(set(turns)) == 1})
        passed = all(check["passed"] for check in checks)
        detail = None if passed else "the independent tools or same-turn results were not delivered"
    elif kind == "tool-selection":
        passed, checks, detail = oracle_tool_selection(oracle, answer, observation.get("tool_calls", []),
                                                       task.get("expected_arguments"))
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
    if required_loads and passed is not None:
        loaded = sorted({call["loaded"] for call in observation.get("tool_calls", []) if call.get("loaded")})
        check = {"check": "skill-loaded", "passed": set(required_loads) <= set(loaded),
                 "expected": sorted(required_loads), "observed": loaded}
        checks = checks + [check]
        if not check["passed"]:
            passed, detail = False, "the expected Skill was never loaded"
    return {"kind": kind, "passed": passed, "checks": checks, "detail": detail}


# ------------------------------------------------------------ classification


def claimed_success(answer):
    """The Agent's own claim, recorded for false-completion analysis only.

    A success phrase counts unless a negation precedes it in the same sentence:
    "Report complete. Disputed invoices are not included." claims success, and
    "No successful result was produced." does not.
    """
    if not answer:
        return None
    for match in SUCCESS_CLAIM.finditer(answer):
        sentence = max(answer.rfind(mark, 0, match.start()) for mark in ".!?\n") + 1
        if not NEGATION.search(answer, sentence, match.start()):
            return True
    return False


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


def deferred_exposure(agent_fields):
    exposure = agent_fields.get("exposure")
    return isinstance(exposure, dict) and exposure.get("version") == "deferred@1"


def selects_skill(task, arm):
    """Whether a Run of `arm` (a planned pair or a recorded variant) must load the Skill it chose.

    A legacy Agent has every Registry Skill resident in its instructions, so only
    a deferred Agent selects one, and only an observed load of the expected Skill
    shows that it did.
    """
    return task["kind"] == "skill-selection" and deferred_exposure(arm["agent_fields"])


def recorded_human_answers(entries, live):
    """Human answers as the manifest keeps them.

    A live request's prompt and a live Tool call approval's call are restricted
    content, kept only as digests. A deterministic prompt or call is fixture text,
    kept whole so validation can regrade.
    """
    recorded = []
    for entry in entries:
        entry = dict(entry)
        prompt = entry.pop("prompt", None) if live else None
        if prompt is not None:
            entry["prompt_sha256"] = sha256_text(prompt)
        recorded.append(restricted(entry, "call") if live else entry)
    return recorded


def recorded_live_call(call, known):
    """A live tool call as the manifest keeps it.

    Its arguments are kept as a digest. A name outside `known` was invented by
    the model and may carry copied task data, so it is kept as a digest too, with
    the Node's error, which echoes it. Known tool identifiers stay readable.
    """
    call = restricted(call, "arguments")
    if call.get("id") is not None:
        call = restricted(call, "id")
    if call["name"] in known:
        return call
    call = restricted(call, "name")
    return restricted(call, "error") if call.get("error") is not None else call


def tool_label(call):
    """A recorded call's name, or the digest that replaced a model-invented one."""
    return call["name"] if "name" in call else "sha256:" + call["name_sha256"]


def run_result(corpus, planned, repeat, observation, tier, allow_code_execution):
    task = planned["task"]
    required_loads = [ref["id"] for ref in planned["skill_refs"]] if selects_skill(task, planned) else []
    live = tier == "live"
    # Live code is executable only by the resource-bounded container oracle.
    execution = allow_code_execution if not live or isinstance(allow_code_execution, ContainerCodeOracle) else False
    grading_observation = dict(observation, native_projection=planned.get("agent_fields", {}).get(
        "projection_version") == "native")
    try:
        oracle = grade(corpus, task, grading_observation, execution, required_loads)
    except Exception as error:
        if not live:
            raise
        return failed_run_result(corpus, planned, repeat, observation, tier, error)
    return record_run_result(corpus, planned, repeat, observation, tier, oracle)


def failed_run_result(corpus, planned, repeat, observation, tier, failure):
    """Keep the durable identity and failed denominator of work whose grader failed."""
    detail = f"evaluator grading failed ({type(failure).__name__})"
    safe = {"run_id": observation.get("run_id"), "phase": observation.get("phase"),
            "final_answer": observation.get("final_answer") if isinstance(observation.get("final_answer"), str) else None,
            "error": detail, "infrastructure_error": detail, "interrupted": bool(observation.get("interrupted")),
            "wall_ms": observation.get("wall_ms", 0), "steps": observation.get("steps"),
            "compactions": observation.get("compactions"), "recovery_operations": observation.get("recovery_operations", [])}
    oracle = {"kind": planned["task"]["oracle"]["kind"], "passed": None, "checks": [], "detail": detail}
    return record_run_result(corpus, planned, repeat, safe, tier, oracle)


def record_run_result(corpus, planned, repeat, observation, tier, oracle):
    task = planned["task"]
    live = tier == "live"
    outcome = classify(task, observation, oracle)
    answer = observation.get("final_answer")
    tool_calls = observation.get("tool_calls", [])
    recorded_calls = tool_calls
    if live:
        # Live outputs are restricted content: a model can copy task data into tool arguments,
        # invented tool names or answer fields, so the manifest keeps only their digests. Grading
        # above saw the raw values; only the Agent's own tools and the builtins stay readable.
        known = {tool["alias"] for tool in task_tools(corpus, task)} | set(DEFAULT_TOOLS) | set(EXPOSURE_TOOLS) | \
            {SKILL_ASSET_READ}
        recorded_calls = [recorded_live_call(call, known) for call in tool_calls]
        oracle = dict(oracle, checks=[restricted(check, "observed") for check in oracle["checks"]])
    # Classified by the observed name, reported by the recorded one.
    calls = [(call["name"], tool_label(recorded)) for call, recorded in zip(tool_calls, recorded_calls, strict=True)]
    selection = None
    if task["oracle"]["kind"] == "tool-selection":
        selection = {"expected": task["oracle"]["expected_tool"],
                     "domain_calls": [label for name, label in calls
                                      if not name.startswith(("capability_", "skill_"))],
                     "capability_calls": [label for name, label in calls if name.startswith("capability_")],
                     "correct": oracle["passed"] if oracle["passed"] is not None else False}
    if task["kind"] == "skill-selection":
        selection = {"expected": task.get("skills", []), "domain_calls": [],
                     "capability_calls": [label for name, label in calls
                                          if name.startswith(("capability_", "skill_"))],
                     "load_calls": sum(name in ("capability_load", "skill_load") for name, _ in calls),
                     "correct": oracle["passed"] if oracle["passed"] is not None else False}
    retention = None
    if task["oracle"]["kind"] in ("constraint-retention", "compaction-retention"):
        retention = None if oracle["passed"] is None else all(
            check["passed"] for check in oracle["checks"] if check["check"] in ("budget", "exclusions"))
    return {
        "key": f"{task['id']}/{planned['comparison']['id']}/{planned['arm']}/{repeat}",
        "task": task["id"], "comparison": planned["comparison"]["id"], "arm": planned["arm"],
        "label": planned["label"], "repeat": repeat, "run_id": observation.get("run_id"),
        "phase": observation.get("phase"), "error": observation.get("error") or observation.get("infrastructure_error"),
        # The provenance `classify` used, so validation can recompute the outcome exactly.
        "interrupted": bool(observation.get("interrupted")),
        "infrastructure_error": observation.get("infrastructure_error") or None,
        "outcome": outcome, "claimed_success": claimed_success(answer), "oracle": oracle,
        "long_context": long_context(observation, outcome), "constraint_retention": retention,
        "tool_selection": selection, "wall_ms": float(observation.get("wall_ms", 0.0)),
        "steps": observation.get("steps"), "compactions": observation.get("compactions"),
        "recovery_operations": observation.get("recovery_operations", []),
        "requests": observation.get("requests"), "usage": summarize_usage(observation.get("usage_records")),
        "tool_calls": recorded_calls,
        "human_answers": recorded_human_answers(observation.get("human_answers", []), live),
        "evidence": {"content_class": "restricted-live" if live else "fixture-trace",
                     "input_sha256": sha256_text(task_description(corpus, task)),
                     "final_answer_sha256": sha256_text(answer) if answer is not None else None,
                     # A deterministic answer is kept whole: validation regrades the oracle from it.
                     "final_answer": None if live else answer,
                     "verifier": oracle["checks"]},
    }


def restricted(entry, field):
    """`entry` with `field` replaced by `<field>_sha256`, the canonical digest of its value."""
    if field not in entry:
        return entry
    entry = dict(entry)
    entry[f"{field}_sha256"] = canonical_sha256(entry.pop(field))
    return entry


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


def compare(issues, aggregates, runs):
    """Reference-versus-arm rows plus matched off/explicit rows; historical pairs remain readable."""
    by_key = {(item["task"], item["comparison"], item["arm"]): item for item in aggregates}
    rows = []
    pairs = []
    for task, comparison in dict.fromkeys((task, comparison) for task, comparison, _ in by_key):
        arms = [arm for t, c, arm in by_key if (t, c) == (task, comparison)]
        if "baseline" in arms and "candidate" in arms:
            pairs.append((task, comparison, "baseline", "candidate"))
        elif len(arms) >= 2:
            reference = "legacy" if "legacy" in arms else arms[0]
            pairs += [(task, comparison, reference, arm) for arm in arms if arm != reference]
            pairs += [(task, comparison, arm, arm + "-explicit") for arm in ("ordered", "native")
                      if arm in arms and arm + "-explicit" in arms]
    for task, comparison, base_arm, candidate_arm in pairs:
        baseline = by_key[(task, comparison, base_arm)]
        candidate = by_key[(task, comparison, candidate_arm)]
        changes = []
        base_runs = {run["repeat"]: run for run in runs if (run["task"], run["comparison"], run["arm"]) ==
                     (task, comparison, base_arm)}
        for run in runs:
            if (run["task"], run["comparison"], run["arm"]) == (task, comparison, candidate_arm) and \
                    run["repeat"] in base_runs and base_runs[run["repeat"]]["outcome"] != run["outcome"]:
                changes.append({"repeat": run["repeat"], "baseline": base_runs[run["repeat"]]["outcome"],
                                "candidate": run["outcome"]})
        candidate_repeats = {run["repeat"] for run in runs
                             if (run["task"], run["comparison"], run["arm"]) == (task, comparison, candidate_arm)}
        # Paired only when both arms recorded exactly the same repeats; otherwise medians describe
        # unrelated samples, so no delta is reported.
        comparable = bool(base_runs) and set(base_runs) == candidate_repeats
        deltas = {}
        for name, value in baseline["metrics"].items():
            other = candidate["metrics"][name]
            deltas[name] = None if not comparable or value["median"] is None or other["median"] is None \
                else other["median"] - value["median"]
        rows.append({"task": task, "comparison": comparison, "issue": issues.get(comparison),
                     "baseline": baseline["label"], "candidate": candidate["label"],
                     "comparable": comparable, "outcome_changes": changes, "median_deltas": deltas,
                     **({"baseline_arm": base_arm, "candidate_arm": candidate_arm}
                        if base_arm != "baseline" else {})})
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


def parse_timestamp(value, field):
    """Parse an ISO 8601 timestamp into an aware datetime; anything else is a refusal."""
    if not isinstance(value, str):
        raise GuardRefusal(f"disclosure approval {field} must be an ISO 8601 string")
    try:
        parsed = datetime.datetime.fromisoformat(value)
    except ValueError:
        raise GuardRefusal(f"disclosure approval {field} is not an ISO 8601 timestamp: {value!r}") from None
    if parsed.tzinfo is None or parsed.utcoffset() is None:
        raise GuardRefusal(f"disclosure approval {field} needs a UTC offset (for example Z or +09:00)")
    return parsed


def positive_int(value):
    return isinstance(value, int) and not isinstance(value, bool) and value > 0


def auxiliary_allowance(approval):
    """Validate declared auxiliary work for offline planning; never infer a free call.

    Call counts are checked against pinned Registry configurations. The declared
    per-call amount is only a reservation, not proof of an upstream monetary cap.
    Execution must additionally pass require_enforceable_auxiliary_bound; until an
    enforced provider/broker contract exists, live auxiliary dispatch is refused.
    """
    value = approval.get("external_compaction")
    if value == "disabled":
        return {"decider": None, "summary": None, "worst_case_nanocredits": 0}
    prefix = "disclosure approval external_compaction"
    if not isinstance(value, dict) or set(value) != {"mode", "decider", "summary"} or value.get("mode") != "bounded":
        raise GuardRefusal(prefix + ' must be "disabled" or an exact bounded decider/summary contract')
    total = 0
    for kind in ("decider", "summary"):
        item = value[kind]
        if item is None:
            continue
        if not isinstance(item, dict) or set(item) != {"reference", "config_sha256", "max_calls_per_run",
                                                       "max_nanocredits_per_call"}:
            raise GuardRefusal(prefix + f" {kind} needs reference, config_sha256, max_calls_per_run and "
                               "max_nanocredits_per_call; unpriced auxiliary work is refused")
        if not isinstance(item["reference"], str) or not re.fullmatch(r"[a-zA-Z0-9._-]+@[a-zA-Z0-9._-]+",
                                                                    item["reference"]):
            raise GuardRefusal(prefix + f" {kind} reference must pin a Registry id@version")
        if not isinstance(item["config_sha256"], str) or not re.fullmatch(r"[0-9a-f]{64}", item["config_sha256"]):
            raise GuardRefusal(prefix + f" {kind} needs an exact configuration digest")
        for field in ("max_calls_per_run", "max_nanocredits_per_call"):
            if not positive_int(item[field]):
                raise GuardRefusal(prefix + f" {kind} {field} must be a positive integer")
        total += item["max_calls_per_run"] * item["max_nanocredits_per_call"]
    if value["decider"] is None:
        raise GuardRefusal(prefix + " bounded mode requires an approved compaction Decider")
    return {"decider": value["decider"], "summary": value["summary"], "worst_case_nanocredits": total}


def require_enforceable_auxiliary_bound(auxiliary):
    """Refuse paid Decider work until an upstream-enforced monetary bound exists."""
    if auxiliary.get("decider") or auxiliary.get("summary"):
        raise GuardRefusal("bounded Decider/Summary live dispatch is unavailable: max_nanocredits_per_call is an "
                           "declared reservation, not an enforceable per-physical-request monetary limit "
                           "or receipt. An enforced provider/broker bound is required before the hard campaign "
                           "ceiling can authorize these calls; retain recovery diagnostics as not measured")


def verify_auxiliary_configs(auxiliary, entries, provider):
    """Verify independently fetched Registry configs before binding approved work."""
    for kind in ("decider", "summary"):
        allowance = auxiliary.get(kind)
        if allowance is None:
            continue
        config = entries.get(allowance["reference"])
        if not isinstance(config, dict) or canonical_sha256(config) != allowance["config_sha256"]:
            raise GuardRefusal(f"{kind} Registry configuration differs from its approved exact digest")
        if config.get("credential_env") or config.get("provider_credential") != provider:
            raise GuardRefusal(f"{kind} must use the connected Tenant Provider Credential, without an environment key")
        if kind == "decider":
            if config.get("hook") != "compaction" or config.get("mode") != "enforce" or \
                    not positive_int(config.get("max_calls_per_run")) or \
                    config["max_calls_per_run"] != allowance["max_calls_per_run"]:
                raise GuardRefusal("approved Decider must enforce compaction with exactly the approved Run call allowance")
            if not isinstance(config.get("routes"), list) or len(config["routes"]) != 1:
                raise GuardRefusal("approved Decider must pin one exact route; implicit fallback is refused")
        else:
            check_live_models({allowance["reference"]: config})
            if allowance["max_calls_per_run"] > 64:
                raise GuardRefusal("Summary call allowance exceeds the Context Policy maximum of 64")


def live_task_support(approval, corpus, planned, fixture_endpoint):
    """Return approved tool/code support; existing default refusals stay in force."""
    tool_tasks = [item["task"] for item in planned if item["task"]["kind"] != "escalation"
                  and task_tools(corpus, item["task"])]
    endpoint = None
    if tool_tasks:
        contract = approval.get("fixture_service")
        expected = {task["id"]: sorted(tool["alias"] for tool in task_tools(corpus, task)) for task in tool_tasks}
        required = {"endpoint": fixture_endpoint.rstrip("/"), "corpus_sha256": corpus["_snapshot"]["sha256"],
                    "tasks": expected, "synthetic_only": True}
        if not isinstance(contract, dict) or not json_equal(contract, required):
            raise GuardRefusal(f"tasks {sorted(expected)} need fixture tool results; approval fixture_service must "
                               "exactly pin the endpoint, corpus_sha256, task/alias allowlist and synthetic_only=true")
        from urllib.parse import urlsplit
        parsed = urlsplit(contract["endpoint"])
        if parsed.scheme not in ("http", "https") or not parsed.hostname or parsed.username or parsed.password \
                or parsed.query or parsed.fragment:
            raise GuardRefusal("approved fixture service needs an absolute HTTP endpoint without credentials or query")
        endpoint = contract["endpoint"]
    code = None
    if any(item["task"]["oracle"]["kind"] == "python-tests" for item in planned):
        contract = approval.get("code_oracle")
        if not isinstance(contract, dict) or set(contract) != {"image", "network", "memory_mb", "cpus", "pids_limit",
                                                              "timeout_seconds"} or \
                any(contract.get(key) != value for key, value in {"network": "none", "memory_mb": 128,
                    "cpus": 1, "pids_limit": 64, "timeout_seconds": 20}.items()):
            raise GuardRefusal("python-tests oracles run model-written code; an explicit code_oracle approval for a "
                               "pinned disposable Docker image, network none, 128 MiB, 1 CPU, 64 pids and 20s is required")
        code = ContainerCodeOracle(contract["image"])
        for item in planned:
            if item["task"]["oracle"]["kind"] == "python-tests":
                independent_code_vectors(corpus, item["task"]["oracle"])
    return endpoint, code


def check_execution_estimator_approval(approval, estimator):
    if estimator not in ("bytes", "calibrated"):
        raise GuardRefusal("execution estimator must be bytes or calibrated")
    declared = approval.get("execution_estimator") if isinstance(approval, dict) else None
    if (estimator != "bytes" or declared is not None) and declared != estimator:
        raise GuardRefusal("disclosure approval execution_estimator must exactly authorize the selected estimator; "
                           "the primary comparison defaults to Bytes")


def validate_source_attestation(source, approval, server_revision):
    """Validate explicitly supplied remote provenance; this is not local autodetection."""
    fields = {"commit", "dirty", "dirty_diff_sha256", "binary_sha256", "server_revision"}
    if not isinstance(source, dict) or set(source) != fields or source.get("dirty") is not False or \
            source.get("dirty_diff_sha256") is not None or not isinstance(source.get("commit"), str) or \
            not re.fullmatch(r"[0-9a-f]{40}", source["commit"]) or not isinstance(source.get("binary_sha256"), str) or \
            not re.fullmatch(r"[0-9a-f]{64}", source["binary_sha256"]) or \
            source.get("server_revision") != source["commit"] or server_revision != source["commit"]:
        raise GuardRefusal("source attestation must name a clean full commit, its binary sha256 and an exactly "
                           "matching full --server-revision")
    if not isinstance(approval, dict) or approval.get("source_attestation_sha256") != canonical_sha256(source):
        raise GuardRefusal("disclosure approval source_attestation_sha256 must bind the exact deployed-source attestation")
    return dict(source)


def check_model_config_digests(approval, models, required=False):
    """Bind paid execution to independently fetched immutable Registry configurations."""
    declared = approval.get("model_config_digests") if isinstance(approval, dict) else None
    if declared is None and not required:
        return
    actual = {reference: canonical_sha256(config) for reference, config in models.items()}
    if not isinstance(declared, dict) or not json_equal(declared, actual):
        raise GuardRefusal("disclosure approval model_config_digests must exactly match every fetched model "
                           "Registry configuration; refresh and approve the actual immutable configs before dispatch")


def check_approval(approval, corpus_sha256, models, ceiling, now):
    """Type-check and time-check the evaluator's disclosure approval; every failure is a GuardRefusal."""
    if not isinstance(approval, dict) or not approval:
        raise GuardRefusal("a disclosure approval document is required")
    for field in ("approver", "approved_at", "expires_at", "corpus_sha256", "models", "max_spend_nanocredits",
                  "disclosure", "external_compaction"):
        if field not in approval:
            raise GuardRefusal(f"disclosure approval is missing {field}")
    check_model_config_digests(approval, models)
    # Auxiliary work stays disabled unless its exact pins, enforceable call allowances and
    # conservative physical-request cost caps are approved separately from main inference.
    auxiliary_allowance(approval)
    for field in ("approver", "corpus_sha256", "disclosure"):
        if not isinstance(approval[field], str) or not approval[field].strip():
            raise GuardRefusal(f"disclosure approval {field} must be a non-empty string")
    if not isinstance(approval["models"], list) or not all(isinstance(item, str) for item in approval["models"]):
        raise GuardRefusal("disclosure approval models must be a list of strings")
    if not positive_int(approval["max_spend_nanocredits"]):
        raise GuardRefusal("disclosure approval max_spend_nanocredits must be a positive integer")
    approved_at = parse_timestamp(approval["approved_at"], "approved_at")
    expires_at = parse_timestamp(approval["expires_at"], "expires_at")
    if not isinstance(now, datetime.datetime) or now.tzinfo is None:
        raise GuardRefusal("the evaluator's wall clock must be an aware datetime")
    if approved_at >= expires_at:
        raise GuardRefusal("disclosure approval was approved at or after its expiry")
    if approved_at > now:
        raise GuardRefusal("disclosure approval is dated in the future")
    if expires_at <= now:
        raise GuardRefusal("disclosure approval has expired")
    if approval["corpus_sha256"] != corpus_sha256:
        raise GuardRefusal("disclosure approval names a different corpus snapshot")
    if sorted(approval["models"]) != sorted(models):
        raise GuardRefusal("disclosure approval does not cover exactly the experiment's models")
    if ceiling > approval["max_spend_nanocredits"]:
        raise GuardRefusal("spend ceiling exceeds the approved maximum")
    prices = approval.get("prices")
    if prices is None:
        prices = {}
    if not isinstance(prices, dict):
        raise GuardRefusal("disclosure approval prices must be an object keyed by model")
    for model_ref in models:
        price = prices.get(model_ref)
        if price is None:
            price = {}
        if not isinstance(price, dict):
            raise GuardRefusal(f"disclosure approval prices for {model_ref} must be an object")
        # A cache write can cost more than ordinary input (a 1.25x premium for Anthropic models,
        # docs/operations/evidence/2026-10-10-prompt-cache-explicit/README.md), so its rate is required.
        for field in ("input_nanocredits_per_token", "cache_write_nanocredits_per_token",
                      "output_nanocredits_per_token"):
            if not positive_int(price.get(field)):
                raise GuardRefusal(f"disclosure approval needs a positive {field} for {model_ref}; reservations "
                                   "are computed from approved prices")
    return expires_at


def require_live_authorization(credential, binding, approval, corpus_sha256, models, ceiling, wall_seconds, now):
    """Refuse a live experiment unless every authorization and budget is explicit.

    credential is the named Tenant Provider Credential metadata read from the
    server and binding the Tenant's Provider Credential Binding for its
    provider, which is what Run admission pins; approval is the evaluator's
    disclosure approval document; models maps each Registry model the
    experiment uses to its configuration; now is an aware datetime (the
    injectable wall clock). Returns the approval's expiry.
    """
    if not positive_int(ceiling):
        raise GuardRefusal("a positive hard spend ceiling in nanocredits is required")
    # The manifest schema records limits in whole seconds or more; refuse before any work instead.
    if not isinstance(wall_seconds, (int, float)) or isinstance(wall_seconds, bool) or not math.isfinite(wall_seconds) \
            or wall_seconds < 1:
        raise GuardRefusal("a finite wall-time limit of at least one second is required")
    if not credential:
        raise GuardRefusal("an explicitly connected Tenant Provider Credential is required")
    if credential.get("state") != "active":
        raise GuardRefusal(f"Provider Credential is not active (state {credential.get('state')!r})")
    # Run admission selects the credential from the Tenant's binding, not from this command line.
    if not isinstance(binding, dict) or not credential.get("id") or \
            str(binding.get("provider_credential_id") or "").lower() != str(credential["id"]).lower():
        raise GuardRefusal(f"Provider Credential {credential.get('id')} is not the one bound to the Tenant for "
                           f"{credential.get('provider')} (bound: "
                           f"{binding.get('provider_credential_id') if isinstance(binding, dict) else None}); "
                           "Runs would use the bound credential")
    # Each Run's admission is bracketed by binding reads; the revision only grows, so an unchanged
    # revision on both sides proves which credential admission pinned (fixed_task_runner.live_runs).
    if not positive_int(binding.get("revision")):
        raise GuardRefusal("the Tenant's Provider Credential Binding has no positive revision; each Run's admission "
                           "is verified against it")
    for model_ref, config in models.items():
        if config.get("credential_env"):
            raise GuardRefusal(f"model {model_ref} reads an environment credential; live runs require the "
                               "Tenant's connected Provider Credential")
        if config.get("provider_credential") != credential.get("provider"):
            raise GuardRefusal(f"model {model_ref} is not bound to the connected {credential.get('provider')} "
                               "Provider Credential")
    return check_approval(approval, corpus_sha256, models, ceiling, now)


def provider_route_evidence(config, reference):
    """Validate the actual model schema's requested exact-tag evidence."""
    pin = config.get("provider_route")
    if not isinstance(pin, dict) or set(pin) != {"tag", "source", "verified_at", "expires_at"}:
        raise GuardRefusal(f"primary model {reference} needs exact provider_route tag/source/time approval evidence")
    tag, source = pin.get("tag"), pin.get("source")
    if not isinstance(tag, str) or len(tag) > 256 or not re.fullmatch(
            r"[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*", tag) or any(segment in (".", "..") for segment in tag.split("/")):
        raise GuardRefusal(f"primary model {reference} has an invalid provider_route exact endpoint tag")
    if not isinstance(source, str) or not source.strip() or len(source) > 1024 or any(ord(char) < 32 for char in source):
        raise GuardRefusal(f"primary model {reference} needs provider_route source evidence")
    verified = parse_timestamp(pin["verified_at"], "provider_route.verified_at")
    expires = parse_timestamp(pin["expires_at"], "provider_route.expires_at")
    now = datetime.datetime.now(datetime.UTC)
    if not verified <= now < expires or expires <= verified:
        raise GuardRefusal(f"primary model {reference} provider_route approval evidence is future-dated or expired")
    return dict(pin)


def requested_model_routes(models):
    """Requested configuration evidence, kept separate from provider-reported identities."""
    return {reference: {"model_id": config["model_id"], "endpoint": config.get("endpoint"),
                        "provider_route": config.get("provider_route"),
                        "request_estimator": config.get("request_estimator", "bytes")}
            for reference, config in models.items()}


def check_matched_live_models(corpus, planned, mapping, models, *, execution_estimator="bytes"):
    """Preflight the frozen v2 primary routes before registrations or paid work."""
    if not corpus.get("execution_estimator"):
        return  # Historical experiments did not declare this comparison contract.
    routes = {}
    for item in planned:
        name = item["model"]
        reference = mapping[name]
        config = models[reference]
        for field in ("context_window", "max_output_tokens"):
            if config[field] != corpus["models"][name][field]:
                raise GuardRefusal(f"model {reference} {field} must match frozen corpus model {name} "
                                   f"({corpus['models'][name][field]})")
        if config.get("request_estimator") != execution_estimator:
            raise GuardRefusal(f"model {reference} must explicitly select request_estimator={execution_estimator}")
        route = item.get("route_id")
        if route is None:
            continue
        if config.get("reasoning_effort") != "medium":
            raise GuardRefusal(f"primary route {route} model {reference} must pin reasoning_effort=medium")
        version = item["agent_fields"].get("projection_version", "legacy")
        supported = config.get("projection_versions") or ["legacy"]
        if version not in supported:
            raise GuardRefusal(f"model {reference} does not explicitly support projection {version}")
        if item["agent_fields"].get("prompt_cache") == "explicit" and config.get("cache_mode") != "explicit":
            raise GuardRefusal(f"explicit route model {reference} must declare cache_mode=explicit")
        provider_route_evidence(config, reference)
        identity = {key: config.get(key) for key in ("provider", "model_id", "endpoint", "provider_credential",
                    "plan_connection", "reasoning_effort", "request_estimator", "provider_route",
                    "streaming", "cache_mode")}
        previous = routes.setdefault(route, identity)
        if not json_equal(previous, identity):
            raise GuardRefusal(f"route {route} does not use one matched model, endpoint, reasoning and routing "
                               "configuration across every task and arm")


def check_live_models(models):
    """Validate every field the live tier reads from a model configuration, before any Registry write."""
    for model_ref, config in models.items():
        if not isinstance(config, dict):
            raise GuardRefusal(f"model {model_ref} has no configuration object")
        for field in ("context_window", "max_output_tokens"):
            if not positive_int(config.get(field)):
                raise GuardRefusal(f"model {model_ref} needs a positive integer {field}; spend reservations "
                                   "are computed from it")
        if not isinstance(config.get("model_id"), str) or not config["model_id"]:
            raise GuardRefusal(f"model {model_ref} needs a model_id")


class SpendLedger:
    """Hard spend and time ceiling for live provider work.

    Every Run is reserved at its worst case before dispatch, including every
    provider retry the harness may make inside it. The live tier settles every
    Run as uncertain, so it stays charged at its full reservation until Usage
    Records can be read over HTTP (ADR 0018). No Run is reserved at or after
    the wall-time deadline or the disclosure approval's expiry, and a Run's
    own time limit never reaches past either of them.

    The approval's expiry is read against the wall clock once, here, and then
    held as a deadline on the monotonic `clock`: a later wall-clock correction
    can neither extend nor shorten the approval.
    """

    def __init__(self, ceiling, wall_seconds, clock, expires_at=None, wall_now=None):
        self.ceiling = ceiling
        self.clock = clock
        now = clock()
        self.deadline = now + wall_seconds
        self.approval_deadline = None if expires_at is None else now + (expires_at - wall_now()).total_seconds()
        self.attempts = []
        self.refusal = None

    @property
    def committed(self):
        return sum(item["charged"] for item in self.attempts)

    def remaining(self):
        """Seconds until the earlier of the wall-time deadline and the approval's expiry."""
        deadline = self.deadline if self.approval_deadline is None else min(self.deadline, self.approval_deadline)
        return deadline - self.clock()

    def reserve(self, key, worst_case):
        now = self.clock()
        if now >= self.deadline:
            self.refusal = "wall-time limit reached"
            raise GuardRefusal(self.refusal)
        if self.approval_deadline is not None and now >= self.approval_deadline:
            self.refusal = "disclosure approval expired before this Run"
            raise GuardRefusal(self.refusal)
        if not positive_int(worst_case):
            raise GuardRefusal("a reservation needs a positive integer worst-case cost")
        if self.committed + worst_case > self.ceiling:
            self.refusal = f"reserving {worst_case} for {key} would exceed the ceiling {self.ceiling}"
            raise GuardRefusal(self.refusal)
        attempt = {"key": key, "reserved": worst_case, "charged": worst_case, "reported": None, "state": "reserved"}
        self.attempts.append(attempt)
        return attempt

    def settle(self, attempt):
        """A finished Run stays charged at its full reservation: no Usage Record is read over HTTP."""
        attempt["state"] = "uncertain"

    def summary(self):
        return {"ceiling_nanocredits": self.ceiling, "committed_nanocredits": self.committed,
                "uncertain_attempts": len(self.attempts), "attempts": self.attempts, "refusal": self.refusal}


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
        if scenario.get("coverage", "full") not in ("full", "partial"):
            errors.append(f"memory scenario {scenario.get('id')} has unknown coverage")
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
            if entry["status"] == "passed" and scenario.get("coverage") == "partial":
                # Only the storage-safety half of the row is observed; never report the whole row as passing.
                entry = dict(entry, status="partial", detail=scenario["observation"])
        scenarios.append({"id": scenario["id"], "issue": scenario["issue"], "evidence": evidence, **entry})
    summary = {}
    for item in scenarios:
        summary[item["status"]] = summary.get(item["status"], 0) + 1
    return {"corpus": corpus["version"], "scenarios": scenarios, "summary": summary}


# ---------------------------------------------------------------- output


def reserve_exclusive(path):
    """Create the manifest file exclusively with mode 0600 and return it open for writing.

    Callers reserve the path before any work whose only record is the manifest,
    so a path another invocation already holds is refused up front instead of
    after the work. Existing evidence is never overwritten.
    """
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    return os.fdopen(descriptor, "w")


def write_manifest(stream, value):
    """Write a manifest into a reservation from reserve_exclusive and close it."""
    with stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write("\n")


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
    unmeasured = [row["comparison"] + " (" + row["issue"] + ")" for row in result["feature_availability"]
                  if row["status"] == "not-measured"]
    if unmeasured:
        lines.append("available, not measured: " + ", ".join(unmeasured))
    return "\n".join(lines)
