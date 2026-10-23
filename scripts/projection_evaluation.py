"""Offline matched-projection evidence, accounting and switching decisions (#181).

The fixed-task manifest remains the oracle owner. Operator JSON is an additional,
Tenant-scoped, prompt-free export, never a replacement for outcomes or a transport
API. No function in this module dispatches a Run or performs network I/O.

Public contract:
  load_evidence(result, operator_export=None, *, corpus=None, memory_corpus=None)
      accepts mappings or local JSON paths. Optional corpus arguments are the
      immutable corpus mappings/paths belonging to a historical result.
  analyze_campaign(imports, configuration) -> JSON-finite decision artifact
  render_report(artifact) -> bounded, reproducible Markdown

The operator envelope contains schema_version=1, experiment_id, canonical whole
result manifest_sha256, exact source=experiment.aidash, tenant_id, the complete
planned_run_keys, run_inventory, and all six diagnostic tables (see TABLES plus
run_recovery). Each inventory row is {key, run_id, attempt_ids:{table:[id,...]}}.
Inference, decision, memory and compaction identifiers are raw attempt_id values;
embedding reservation identifiers are request_id + '/' + attempt_id because an
ancestor reservation can repeat the same physical attempt. Shared memory rows
list all source_run_ids and occur in each owner's inventory, but only once in
the table. Null historical ownership cannot establish a complete import.

Configuration registers each experiment_id in `experiments` with route,
phase ('pilot', 'confirmation' or 'deployment'), condition ('primary' or an
explicit diagnostic/warm-cache label), and optional tenant_id. `routes` maps
'automatic'/'explicit' to {provider_tag:exact_endpoint_tag, upstream_model:...};
requested_route is also accepted as the endpoint-tag key. `candidate` is null,
'ordered' or 'native'; `prompt_cache_policies` selects off/explicit per route.
Confirmation additionally needs frozen confirmation_repetitions,
confirmation_frozen, independent_blocks, cost_bounds_enforced,
cost_bounds[route][task][projection_or_arm]. Adoption requires complete imported
pilot and deployment-mode phases on both routes; deployment_mode_confirmed is
not evidence. Freeze deployment_repetitions and routes[route].deployment_estimator
as {id,version,confidence,encoding?,encoding_version?}, then verify its actual
attempts. Each route declares tokenized_applicable:false or a supported
token_encoding, and may name reported_model_aliases/canonical_model_ids explicitly.
No alias equivalence or estimator applicability is inferred from model names.
These assertions do not replace missing provider, oracle, route or guard facts.
Optional safety annotations on run_recovery are independently verified
guard_evidence_complete, safety_violation, false_completion, constraint_loss,
and control_accepted; the SQL export cannot manufacture these oracle facts.

Memory policy_cost_micros and reserved_cost_micros are tariffs/reservations,
not provider-reported money. Embedding budget rows are reservation evidence,
not extra charges or an extra count of provider attempts. Shared-memory money
has a unique campaign charge but no invented allocation to individual Runs.
"""
from collections import Counter, defaultdict
import copy
import hashlib
import json
from pathlib import Path
import statistics

import fixed_task_evaluation as evaluation

VERSION = "1.0.0"
PRIMARY_WEIGHTS = {
    "code-change-hidden-regression": .125,
    "code-change-verified-fix": .125,
    "structured-data-overdue-totals": .25,
    "long-conversation-corrections": .25,
    "correction-lost-in-pruned-tool-result": .25,
}
CONTROLS = ("terminal-context-overflow", "approval-escalation", "step-limit-unresolved")
TABLES = ("inference_usage", "decision_attempts", "memory_model_attempts", "context_compaction_attempts", "generation_embedding_usage")
TOKENS = ("input_tokens", "output_tokens", "cache_read_tokens", "cache_write_tokens", "reasoning_tokens")
MAX_BYTES = 128 * 1024 * 1024
MAX_ROWS = 100000
MAX_TEXT = 4096

# These are metadata fields selected by `manage diagnostics evaluation`, plus
# optional operator-owned accounting and independent guard annotations. Reject
# unknown fields rather than accidentally importing prompts, secrets or context.
COMMON = {"attempt_id", "id", "run_id", "tenant_id", "tenant", "created_at", "completed_at",
          "finished_at", "settled_at", "cost_nanocredits", "upstream_cost_nanocredits", "cost_amount",
          "cost_unit", "provider_work_id", "reserved_nanocredits", "purpose", "status", "outcome"}
USAGE_FIELDS = COMMON | {
    "response_epoch", "model_id", "model_version", "projection_version", "requested_route",
    "provider_identity", "shadow_estimates", "cache_identity", "input_tokens", "output_tokens",
    "cache_read_tokens", "cache_write_tokens", "reasoning_tokens", "estimated_tokens", "estimator",
    "estimator_version", "estimate_confidence", "upstream_model", "byte_basis", "output_reservation",
    "estimate_ratio_ppm", "calibration_sample", "rejection_class", "encoding", "calibration_state",
    "calibration_samples", "overflow_demoted", "history_prefix_sha256",
}
TABLE_FIELDS = {
    "inference_usage": USAGE_FIELDS,
    "decision_attempts": COMMON | {"decision_id", "step", "cycle", "retry", "decider", "mode", "route",
                                    "kind", "verdict", "model_id", "model_version"},
    "memory_model_attempts": COMMON | {"operation_id", "operation", "operation_kind", "model_id",
                                       "model_version", "input_tokens", "output_tokens", "upstream_model",
                                       "retry", "mode", "route", "owner_run_id", "semantic_retry_cycle",
                                       "source_run_ids", "ordinal", "cycle", "state", "reserved_tokens",
                                       "reserved_cost_micros", "provider_usage", "policy_tokens", "policy_cost_micros",
                                       "provider_id", "provider_version"},
    "context_compaction_attempts": COMMON | {"stage", "policy_version", "provider", "source_from_seq",
                                             "source_through_seq", "base_revision", "observed_input_seq",
                                             "before_tokens", "after_tokens", "reason", "candidate_digest", "provider_dispatched"},
    "generation_embedding_usage": COMMON | {"request_id", "workspace_id", "entry_id", "provider_id",
                                             "provider_version", "request_bytes", "reserved_tokens", "reported_tokens",
                                             "accounting_kind", "provider_work_unpriced"},
}
RECOVERY_FIELDS = {"run_id", "id", "tenant_id", "tenant", "phase", "projection_version", "step",
                   "revision", "compactions", "compaction_count", "overflow_recovery_attempts",
                   "overflow_retry_count", "local_fitting_failures", "provider_overflow_rejections",
                   "pruning_adopted", "summary_adopted", "safety_violation", "false_completion",
                   "constraint_loss", "guard_evidence_complete", "updated_at", "recovery_attempts",
                   "compaction_attempts", "adopted_compactions", "overflow_recovery", "task_id", "control",
                   "semantic_retry_cycle", "overflow_attempts", "overflow_request_tokens", "window_permille",
                   "estimator_demoted", "context_reason", "control_accepted"}
IDENTITY_FIELDS = {"reported_model", "reported_provider", "reported_route", "provider_request_id"}
ESTIMATE_FIELDS = {"tokens", "estimator", "version", "confidence", "byte_basis", "output_reservation",
                   "ratio_ppm", "encoding", "calibration_state", "calibration_samples", "applicable",
                   "overflow_demoted"}


class EvidenceError(ValueError):
    """Corrupt, incomplete-scope or incompatible evidence cannot be imported."""


def canonical_sha256(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":"),
                                     ensure_ascii=False, allow_nan=False).encode()).hexdigest()


def _json(value):
    if isinstance(value, (str, Path)):
        path = Path(value)
        if path.stat().st_size > MAX_BYTES:
            raise EvidenceError("evidence exceeds the bounded import size")
        try:
            value = json.loads(path.read_text(), object_pairs_hook=_unique_object)
        except (ValueError, OSError) as error:
            raise EvidenceError(f"invalid JSON evidence: {error}") from error
    if not isinstance(value, dict):
        raise EvidenceError("evidence must be a JSON object")
    errors = evaluation.non_finite_numbers(value)
    if errors:
        raise EvidenceError(errors[0])
    return copy.deepcopy(value)


def _unique_object(items):
    result = {}
    for key, value in items:
        if key in result:
            raise EvidenceError(f"duplicate JSON object key: {key}")
        result[key] = value
    return result


def _text(value, name, nullable=False):
    if value is None and nullable:
        return
    if not isinstance(value, str) or not value or len(value) > MAX_TEXT:
        raise EvidenceError(f"{name} must be a bounded nonempty string")


def _count(value, name, nullable=True):
    if value is None and nullable:
        return
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise EvidenceError(f"{name} must be a nonnegative integer or unknown")
    if value > 2 ** 64 - 1:
        raise EvidenceError(f"{name} exceeds the exported counter range")


def _exact_set(values, expected, name):
    if not isinstance(values, list) or any(not isinstance(item, str) for item in values):
        raise EvidenceError(f"{name} must list identifiers")
    if len(values) != len(set(values)):
        raise EvidenceError(f"{name} contains duplicates")
    if set(values) != set(expected):
        raise EvidenceError(f"{name} differs from the complete planned set (missing or unplanned records)")


def _attempt_id(row):
    value = row.get("attempt_id", row.get("id"))
    _text(value, "attempt_id")
    if "id" in row and "attempt_id" in row and row["id"] != row["attempt_id"]:
        raise EvidenceError("id and attempt_id disagree")
    return value


def _record_id(row, table):
    if table == "generation_embedding_usage":
        _text(row.get("request_id"), "request_id")
        return row["request_id"] + "/" + _attempt_id(row)
    return _attempt_id(row)


def _metadata(row, allowed, tenant, table):
    if not isinstance(row, dict) or set(row) - allowed:
        raise EvidenceError(f"{table} has unexpected metadata fields")
    for field in ("tenant_id", "tenant"):
        if field in row and row[field] != tenant:
            raise EvidenceError(f"{table} row escapes the authorized Tenant scope")
    for field, value in row.items():
        if isinstance(value, str) and len(value) > MAX_TEXT:
            raise EvidenceError(f"{table}.{field} exceeds the bounded metadata length")
    for field in TOKENS + ("estimated_tokens", "output_reservation", "cost_nanocredits",
                            "upstream_cost_nanocredits", "reserved_nanocredits", "byte_basis", "request_bytes",
                            "reserved_tokens", "reported_tokens", "policy_tokens", "reserved_cost_micros", "policy_cost_micros",
                            "overflow_attempts", "local_fitting_failures", "provider_overflow_rejections",
                            "overflow_recovery_attempts", "overflow_retry_count", "compactions", "compaction_count"):
        if field in row:
            _count(row[field], f"{table}.{field}")
    if "cost_amount" in row and row["cost_amount"] is not None:
        amount = row["cost_amount"]
        if isinstance(amount, bool) or not isinstance(amount, (int, float)) or amount < 0:
            raise EvidenceError("cost_amount must be a nonnegative finite number or unknown")
        _text(row.get("cost_unit"), "cost_unit")
    if "cost_unit" in row:
        _text(row["cost_unit"], "cost_unit", nullable=True)
    identity = row.get("provider_identity")
    if identity is not None:
        if not isinstance(identity, dict) or set(identity) - IDENTITY_FIELDS:
            raise EvidenceError("provider_identity has unexpected fields")
        for name, value in identity.items():
            _text(value, f"provider_identity.{name}", nullable=True)
    shadows = row.get("shadow_estimates", [])
    if shadows is None:
        shadows = []
    if not isinstance(shadows, list) or len(shadows) > 16:
        raise EvidenceError("shadow_estimates must be a bounded list")
    for shadow in shadows:
        if not isinstance(shadow, dict):
            raise EvidenceError("shadow estimate must be an object")
        if "estimate" in shadow:
            if set(shadow) != {"snapshot", "estimate"} or not isinstance(shadow["snapshot"], dict) or set(shadow["snapshot"]) - {"step", "estimator"}:
                raise EvidenceError("shadow snapshot has unexpected fields")
            snapshot = shadow["snapshot"]
            _count(snapshot.get("step"), "snapshot.step", nullable=False)
            settings = snapshot.get("estimator")
            if not isinstance(settings, dict) or set(settings) - {"id", "version", "ratio_ppm", "margin_permille", "samples", "encoding", "encoding_version"}:
                raise EvidenceError("shadow estimator snapshot has unexpected fields")
            if settings.get("id") not in ("bytes", "calibrated", "tokenized"):
                raise EvidenceError("shadow snapshot estimator is unknown")
            for name in ("version", "ratio_ppm", "margin_permille", "samples", "encoding_version"):
                if name in settings:
                    _count(settings[name], "snapshot." + name)
            if "encoding" in settings:
                _text(settings["encoding"], "snapshot.encoding", nullable=True)
            estimate = shadow["estimate"]
        else:
            estimate = shadow
        if not isinstance(estimate, dict) or set(estimate) - ESTIMATE_FIELDS:
            raise EvidenceError("shadow estimate has unexpected fields")
        for field in ("tokens", "output_reservation", "byte_basis", "version", "calibration_samples"):
            if field in estimate:
                _count(estimate[field], f"shadow_estimates.{field}")
        for field in ("estimator", "confidence", "encoding", "calibration_state"):
            if field in estimate:
                _text(estimate[field], f"shadow_estimates.{field}", nullable=True)
        if estimate.get("tokens") is not None and estimate.get("output_reservation") is not None and estimate["tokens"] < estimate["output_reservation"]:
            raise EvidenceError("shadow estimate is smaller than its output reservation")
        if "applicable" in estimate and not isinstance(estimate["applicable"], bool):
            raise EvidenceError("estimate applicability must be boolean")
    if row.get("provider_usage") is not None:
        usage = row["provider_usage"]
        if not isinstance(usage, dict) or set(usage) - (set(TOKENS) | {"cost_nanocredits", "upstream_cost_nanocredits", "provider_identity"}):
            raise EvidenceError("memory provider_usage has unexpected fields")
        _metadata(usage, USAGE_FIELDS, tenant, "memory.provider_usage")
    if table == "inference_usage":
        if "projection_version" in row:
            _count(row["projection_version"], "projection_version")
        if row.get("purpose", "main") not in ("main", "summary"):
            raise EvidenceError("inference purpose must be main or summary")
        if row.get("estimated_tokens") is not None and row.get("output_reservation") is not None and row["estimated_tokens"] < row["output_reservation"]:
            raise EvidenceError("request estimate is smaller than its output reservation")
    if table == "generation_embedding_usage" and (row.get("accounting_kind") != "reservation" or row.get("provider_work_unpriced") is not True or row.get("cost_nanocredits") is not None or row.get("cost_amount") is not None):
        raise EvidenceError("embedding reservation evidence cannot assert a provider charge")
    if "provider_dispatched" in row and row["provider_dispatched"] is not None and not isinstance(row["provider_dispatched"], bool):
        raise EvidenceError("provider_dispatched must be boolean or unknown")
    for field in ("safety_violation", "false_completion", "constraint_loss", "guard_evidence_complete", "control_accepted"):
        if field in row and row[field] is not None and not isinstance(row[field], bool):
            raise EvidenceError(f"{field} must be boolean or unknown")


def _variant_projection(variant):
    value = variant.get("agent_fields", {}).get("projection_version")
    return {1: "legacy", 2: "ordered", 3: "native", "1": "legacy", "2": "ordered", "3": "native"}.get(
        value, value or (variant.get("label") if variant.get("label") in ("legacy", "ordered", "native") else None))


def load_evidence(manifest, export=None, *, corpus=None, memory_corpus=None):
    """Associate an existing fixed-task result with a complete operator export.

    Both arguments accept JSON mappings or local paths. The export envelope is
    defined by projection-evidence.schema.json. Every planned Run must be either
    recorded or explicitly skipped by the fixed-task owner. Every recorded live
    Run, attempt inventory and recovery row must join exactly; an unknown amount
    is a valid observation, but a missing row is corruption. `None` deliberately
    retains unavailable measurements for fixtures and preflight diagnostics.
    """
    manifest = _json(manifest)
    try:
        if isinstance(corpus, (str, Path)):
            corpus = evaluation.load_corpus(Path(corpus))
        if isinstance(memory_corpus, (str, Path)):
            memory_corpus = evaluation.load_memory_corpus(Path(memory_corpus))
        evaluation.validate_result(manifest, corpus=corpus, memory_corpus=memory_corpus)
    except (evaluation.CorpusError, KeyError, TypeError, AttributeError) as error:
        raise EvidenceError(f"invalid fixed-task manifest: {error}") from error
    experiment = manifest["experiment"]
    variants = {(v["task"], v["comparison"], v["arm"]): v for v in experiment["variants"]}
    normalized = {"schema_version": 1, "manifest_sha256": canonical_sha256(manifest),
                  "export_sha256": None, "experiment": experiment, "skipped_runs": manifest.get("skipped_runs", []),
                  "limitations": list(manifest.get("limitations", [])), "manifest_error": manifest.get("error"),
                  "spend": manifest.get("spend"), "tenant_id": None, "complete_export": False, "runs": []}
    for run in manifest["runs"]:
        variant = variants[(run["task"], run["comparison"], run["arm"])]
        projection = _variant_projection(variant)
        normalized["runs"].append({
            "key": run["key"], "run_id": run["run_id"], "task": run["task"], "comparison": run["comparison"],
            "arm": run["arm"], "repeat": run["repeat"], "projection": projection,
            "prompt_cache": variant.get("agent_fields", {}).get("prompt_cache", "off"),
            "matched_configuration_sha256": canonical_sha256({"model": variant["model"], "tools": variant["tools"],
                "skills": variant["skills"], "agent_fields": {k: v for k, v in variant.get("agent_fields", {}).items()
                                                            if k not in ("projection_version", "prompt_cache")}}),
            "outcome": run["outcome"], "oracle_passed": run["oracle"]["passed"],
            "constraint_retention": run["constraint_retention"], "wall_ms": run["wall_ms"],
            "long_context": run["long_context"], "phase": run["phase"],
            "inference_usage": list(run["usage"]["records"]), "usage_available": run["usage"]["source"] == "usage-records",
            "decision_attempts": [], "memory_model_attempts": [], "context_compaction_attempts": [], "recovery": None,
            "generation_embedding_usage": [],
        })
    if export is None:
        normalized["limitations"].append("No complete Tenant-scoped operator export; auxiliary work, routes and recovery may be unknown.")
        return normalized
    export = _json(export)
    schema = json.loads((evaluation.EVALUATION / "schema/projection-evidence.schema.json").read_text())
    errors = evaluation.validate(export, schema)
    if errors:
        raise EvidenceError("invalid operator export: " + "; ".join(errors[:8]))
    if export["experiment_id"] != experiment["experiment_id"]:
        raise EvidenceError("operator export experiment_id differs from the manifest")
    if not evaluation.json_equal(export["source"], experiment["aidash"]):
        raise EvidenceError("operator export source identity differs from the manifest")
    if export["manifest_sha256"] != normalized["manifest_sha256"]:
        raise EvidenceError("operator export manifest digest differs from the fixed result")
    tenant = export["tenant_id"]
    _text(tenant, "tenant_id")
    planned = {evaluation.run_key(v["task"], v["comparison"], v["arm"], repeat)
               for v in experiment["variants"] for repeat in range(1, experiment["repeat_count"] + 1)}
    _exact_set(export["planned_run_keys"], planned, "planned_run_keys")
    by_key = {r["key"]: r for r in normalized["runs"]}
    run_ids = [r["run_id"] for r in normalized["runs"]]
    if any(not isinstance(value, str) or not value for value in run_ids) or len(set(run_ids)) != len(run_ids):
        raise EvidenceError("operator import requires unique durable run_id for every attempted Run")
    inventory = export["run_inventory"]
    _exact_set([r["key"] for r in inventory], by_key, "run_inventory")
    expected = {table: defaultdict(set) for table in TABLES}
    for entry in inventory:
        run = by_key[entry["key"]]
        if entry["run_id"] != run["run_id"]:
            raise EvidenceError("run_inventory durable Run reference differs from manifest")
        for table in TABLES:
            identifiers = entry["attempt_ids"][table]
            _exact_set(identifiers, set(identifiers), f"{table} attempt inventory")
            for identifier in identifiers:
                if identifier in expected[table] and table != "memory_model_attempts":
                    raise EvidenceError(f"duplicate {table} attempt inventory across Runs")
                expected[table][identifier].add(run["run_id"])
    by_id = {r["run_id"]: r for r in normalized["runs"]}
    for table in TABLES:
        rows = export[table]
        if len(rows) > MAX_ROWS:
            raise EvidenceError(f"{table} exceeds the bounded row count")
        found = {}
        for row in rows:
            _metadata(row, TABLE_FIELDS[table], tenant, table)
            identifier = _record_id(row, table)
            if identifier in found:
                raise EvidenceError(f"duplicate {table} attempt {identifier}")
            owners = row.get("source_run_ids") if table == "memory_model_attempts" else [row.get("run_id")]
            if not isinstance(owners, list) or not owners or any(r not in by_id for r in owners) or len(set(owners)) != len(owners) or expected[table].get(identifier) != set(owners):
                raise EvidenceError(f"{table} attempt has a missing or out-of-scope Run join")
            if table == "memory_model_attempts" and "run_id" in row and row["run_id"] not in owners:
                raise EvidenceError("memory run_id differs from source_run_ids")
            found[identifier] = row
        if set(found) != set(expected[table]):
            raise EvidenceError(f"{table} is missing records from its complete attempt inventory")
        for run in normalized["runs"]:
            run[table] = sorted((r for r in rows if run["run_id"] in (r.get("source_run_ids") if table == "memory_model_attempts" else [r["run_id"]])),
                                key=lambda row: (row.get("created_at") or "", _attempt_id(row)))
            if table == "inference_usage":
                run["usage_available"] = True
    recoveries = export["run_recovery"]
    _exact_set([r.get("run_id", r.get("id")) for r in recoveries], by_id, "run_recovery")
    for row in recoveries:
        _metadata(row, RECOVERY_FIELDS, tenant, "run_recovery")
        if "run_id" in row and "id" in row and row["run_id"] != row["id"]:
            raise EvidenceError("run_recovery id and run_id disagree")
        by_id[row.get("run_id", row.get("id"))]["recovery"] = row
    # A Summary Usage Record is the cost owner of the corresponding compaction
    # attempt. Require both sides of that durable association.
    for run in normalized["runs"]:
        inference = {_attempt_id(r): r for r in run["inference_usage"]}
        compactions = {_attempt_id(r): r for r in run["context_compaction_attempts"]}
        for identifier, row in inference.items():
            expected_projection = {"legacy": 1, "ordered": 2, "native": 3}.get(run["projection"])
            if row.get("purpose", "main") == "main" and row.get("projection_version") is not None and row["projection_version"] != expected_projection:
                raise EvidenceError("Usage Record Projection Version differs from the pinned Run arm")
            if row.get("purpose", "main") == "summary" and identifier not in compactions:
                raise EvidenceError("Summary Usage Record is missing its compaction attempt join")
        for identifier, row in compactions.items():
            if row.get("stage") in ("summary", "summary_recovery"):
                if row.get("provider_dispatched") is False and identifier in inference:
                    raise EvidenceError("Summary nondispatch contradicts its inference Usage Record")
                if row.get("provider_dispatched") is not False and identifier not in inference:
                    raise EvidenceError("Summary compaction is missing its inference Usage Record")
        _cost(run, True)  # Contradictory charge ownership is import corruption.
    normalized.update(export_sha256=canonical_sha256(export), tenant_id=tenant, complete_export=True)
    return normalized


def _summary(values):
    known = [v for v in values if v is not None]
    return {"known": sum(known), "known_count": len(known), "unknown_count": len(values) - len(known),
            "total": sum(known) if len(known) == len(values) else None}


def _distribution(values):
    known = [v for v in values if v is not None]
    return {"n": len(known), "unknown": len(values) - len(known),
            "mean": statistics.mean(known) if known else None,
            "median": statistics.median(known) if known else None,
            "sample_variance": statistics.variance(known) if len(known) > 1 else None,
            "minimum": min(known) if known else None, "maximum": max(known) if known else None}


def _estimator_summary(rows):
    identities = defaultdict(list)
    for row in rows:
        identity = {k: row[k] for k in ("version", "confidence", "encoding", "encoding_version", "calibration_state",
                                        "calibration_samples", "selected", "applicable", "actual_estimator", "snapshot")}
        identities[json.dumps(identity, sort_keys=True)].append(row)
    return {"status": "reported" if rows else "unknown-not-reported",
            "signed_input_error": _distribution([e["signed_input_error"] for e in rows]),
            "absolute_input_error": _distribution([e["absolute_input_error"] for e in rows]),
            "underestimated": sum(e["underestimated"] is True for e in rows),
            "overflow_demoted": sum(e["overflow_demoted"] is True for e in rows),
            "identities": [{"identity": json.loads(identity), "signed_input_error": _distribution([e["signed_input_error"] for e in values]),
                             "absolute_input_error": _distribution([e["absolute_input_error"] for e in values])}
                            for identity, values in sorted(identities.items())]}


def _charge(row):
    # Upstream cost is a breakdown of billed cost, never an additional charge.
    if "provider_usage" in row:
        return _charge(row.get("provider_usage") or {})
    billed = row.get("cost_nanocredits")
    if billed is not None:
        if row.get("cost_amount") is not None and (row.get("cost_unit") != "nanocredits" or row["cost_amount"] != billed):
            raise EvidenceError("conflicting amounts or units for one provider charge")
        return "nanocredits", billed
    if "cost_amount" in row or "cost_unit" in row:
        return row.get("cost_unit") or "unknown", row.get("cost_amount")
    return "nanocredits", None


def _cost(run, complete):
    work = {}
    for table, default_purpose in (("inference_usage", "main"), ("decision_attempts", "decider"),
                                   ("memory_model_attempts", "memory"), ("context_compaction_attempts", "summary")):
        for row in run[table]:
            # Pruning is local work; summary provider work is already in usage.
            if table == "context_compaction_attempts" and row.get("stage") in ("pruning", "prune"):
                continue
            if table == "context_compaction_attempts" and row.get("provider_dispatched") is False:
                continue
            identifier = row.get("provider_work_id") or _attempt_id(row)
            unit, amount = _charge(row)
            purpose = row.get("purpose") or default_purpose
            if identifier in work:
                previous = work[identifier]
                if table != "context_compaction_attempts" or previous["purpose"] != "summary":
                    raise EvidenceError("provider-work identifier collides across independent accounting owners")
                if previous["unit"] != unit or (amount is not None and previous["amount"] is not None and amount != previous["amount"]):
                    raise EvidenceError("incompatible duplicate provider-work charge")
                if previous["amount"] is None:
                    previous["amount"] = amount
                previous["sources"].append(table)
            else:
                work[identifier] = {"provider_work_id": identifier, "unit": unit, "amount": amount, "purpose": purpose,
                                    "sources": [table], "shared_run_ids": row.get("source_run_ids", [run["run_id"]])}
    memory_ids = {_attempt_id(row) for row in run["memory_model_attempts"]}
    unpriced_embeddings = {_attempt_id(row) for row in run["generation_embedding_usage"] if _attempt_id(row) not in memory_ids}
    by_unit = defaultdict(list)
    purposes = defaultdict(lambda: defaultdict(list))
    for item in work.values():
        by_unit[item["unit"]].append(item["amount"])
        purposes[item["purpose"]][item["unit"]].append(item["amount"])
    # Even an empty main ledger does not prove a free Run without full export.
    if not complete or not run["usage_available"]:
        by_unit["nanocredits"].append(None)
    by_unit["nanocredits"].extend([None] * len(unpriced_embeddings))
    units = {unit: _summary(values) for unit, values in sorted(by_unit.items())}
    nanocredits = units.get("nanocredits", {"total": 0})["total"]
    if len(units) > 1 or any(unit != "nanocredits" for unit in units) or any(len(w["shared_run_ids"]) > 1 for w in work.values()):
        nanocredits = None
    return {"units": units, "purposes": {p: {u: _summary(v) for u, v in sorted(values.items())}
                                           for p, values in sorted(purposes.items())},
            "total_nanocredits": nanocredits, "provider_work_count": len(work),
            "unknown_work": sum(item["amount"] is None for item in work.values()),
            "auxiliary_evidence_complete": complete, "provider_work": list(work.values()),
            "embedding_reservations": len(run["generation_embedding_usage"]),
            "unpriced_embedding_work": len(unpriced_embeddings),
            "reservations_nanocredits": _summary([row.get("reserved_nanocredits") for table in TABLES for row in run[table]
                                                   if "reserved_nanocredits" in row])}


def _estimates(row):
    selected = {"tokens": row.get("estimated_tokens"), "estimator": row.get("estimator"),
                "version": row.get("estimator_version"), "confidence": row.get("estimate_confidence"),
                "output_reservation": row.get("output_reservation"), "encoding": row.get("encoding"),
                "calibration_state": row.get("calibration_state"), "calibration_samples": row.get("calibration_samples"),
                "overflow_demoted": row.get("overflow_demoted"), "byte_basis": row.get("byte_basis"), "selected": True}
    result = [selected]
    for item in row.get("shadow_estimates") or []:
        estimate = dict(item.get("estimate", item), selected=False)
        settings = item.get("snapshot", {}).get("estimator", {})
        estimate.update(requested_estimator=settings.get("id"), snapshot=item.get("snapshot"),
                        encoding=settings.get("encoding", estimate.get("encoding")),
                        encoding_version=settings.get("encoding_version"),
                        calibration_samples=settings.get("samples", estimate.get("calibration_samples")))
        if settings.get("id") == "calibrated" and estimate.get("estimator") == "bytes":
            estimate["calibration_state"] = "bytes-fallback"
        result.append(estimate)
    return result


def _model_identity(row, settings):
    """Compare two separately recorded facts, using only declared equivalences."""
    identity = row.get("provider_identity") or {}
    requested = settings.get("requested_route", settings.get("provider_tag"))
    actual = identity.get("reported_route")
    requested_row = row.get("requested_route")
    route_status = "unknown" if not requested or not actual else "matched" if actual == requested and (requested_row is None or requested_row == requested) else "mismatch"
    expected_model = settings.get("upstream_model")
    aliases = []
    for key in ("reported_model_aliases", "canonical_model_ids"):
        values = settings.get(key, [])
        if not isinstance(values, list) or any(not isinstance(alias, str) or not alias for alias in values):
            raise EvidenceError("reported model aliases/canonical IDs must be explicit nonempty strings")
        aliases.extend(values)
    reported = identity.get("reported_model")
    model_status = "unknown" if not expected_model or not reported else "matched" if reported in [expected_model, *aliases] else "mismatch"
    dispatched_model = row.get("upstream_model")
    requested_model_status = "unknown" if not expected_model or not dispatched_model else "matched" if dispatched_model == expected_model else "mismatch"
    statuses = (route_status, model_status, requested_model_status)
    status = "matched" if all(status == "matched" for status in statuses) else "mismatch" if "mismatch" in statuses else "unknown"
    return {"attempt_id": row.get("attempt_id", row.get("id")), "requested_route": requested_row,
            "expected_route": requested, "expected_model": expected_model,
            "dispatched_model": dispatched_model, "requested_model_status": requested_model_status,
            "reported_identity": identity or None, "route_status": route_status,
            "model_status": model_status, "status": status}


def _estimate_gaps(estimate, name, expected=None):
    gaps = []
    for field in ("tokens", "output_reservation", "version", "byte_basis"):
        value = estimate.get(field)
        if type(value) is not int or value < 0 or field == "version" and value == 0:
            gaps.append(f"{name}: {field} is unknown or invalid")
    if estimate.get("estimator") not in ("bytes", "calibrated", "tokenized"):
        gaps.append(f"{name}: estimator identity is unknown")
    if estimate.get("confidence") not in ("conservative", "calibrated", "tokenized"):
        gaps.append(f"{name}: estimator confidence is unknown")
    if expected:
        for declaration, recorded in (("id", "estimator"), ("version", "version"), ("confidence", "confidence"),
                                       ("encoding", "encoding"), ("encoding_version", "encoding_version")):
            if declaration in expected and estimate.get(recorded) != expected[declaration]:
                gaps.append(f"{name}: actual {declaration} differs from the frozen estimator")
    return gaps


def _measurement_gaps(rows, settings):
    gaps = []
    applicable = settings.get("tokenized_applicable")
    encoding = settings.get("token_encoding")
    if encoding is not None:
        applicable = True
    if type(applicable) is not bool:
        gaps.append("Tokenized estimator applicability was not declared")
    required_shadows = {"calibrated"} | ({"tokenized"} if applicable is True else set())
    for row in rows:
        identifier = row.get("attempt_id", row.get("id", "unknown"))
        for field in TOKENS[:4]:
            if row.get(field) is None:
                gaps.append(f"{identifier}: required {field} is unknown")
        estimates = _estimates(row)
        selected = estimates[0]
        selected["byte_basis"] = row.get("byte_basis")
        gaps.extend(_estimate_gaps(selected, f"{identifier} selected estimate"))
        if row.get("purpose", "main") != "main":
            continue
        shadows = defaultdict(list)
        for estimate in estimates[1:]:
            shadows[estimate.get("requested_estimator") or estimate.get("estimator")].append(estimate)
        for name in required_shadows:
            values = shadows[name]
            if len(values) != 1:
                gaps.append(f"{identifier}: required {name} shadow is missing or duplicated")
                continue
            estimate = values[0]
            if estimate.get("applicable") is False:
                gaps.append(f"{identifier}: required {name} shadow cannot be declared inapplicable")
            gaps.extend(_estimate_gaps(estimate, f"{identifier} {name} shadow"))
            snapshot = estimate.get("snapshot")
            pin = snapshot.get("estimator", {}) if isinstance(snapshot, dict) else {}
            if pin.get("id") != name or type(pin.get("version")) is not int or pin["version"] < 1:
                gaps.append(f"{identifier}: {name} shadow snapshot identity/version is unknown")
            if name == "calibrated" and (type(pin.get("samples")) is not int or pin.get("ratio_ppm") is None):
                gaps.append(f"{identifier}: frozen calibration state is unknown")
            if name == "tokenized" and (not pin.get("encoding") or type(pin.get("encoding_version")) is not int or pin["encoding_version"] < 1 or encoding is not None and pin["encoding"] != encoding):
                gaps.append(f"{identifier}: Tokenized encoding identity/version is missing or mismatched")
    return sorted(set(gaps))


def _run_metrics(run, complete, settings):
    main = [r for r in run["inference_usage"] if r.get("purpose", "main") == "main"]
    all_usage = run["inference_usage"]
    tokens = {field: _summary([row.get(field) for row in all_usage] + ([] if run["usage_available"] else [None]))
              for field in TOKENS}
    timestamps = [r.get("created_at") for r in main]
    order_known = run["usage_available"] and (len(main) <= 1 or all(timestamps) and len(set(timestamps)) == len(timestamps))
    first = main[:1] if order_known else []
    later = main[1:] if order_known else []
    cache = {"first_attempt": {f: _summary([r.get(f) for r in first]) for f in ("cache_read_tokens", "cache_write_tokens")},
             "later_attempts": {f: _summary([r.get(f) for r in later]) for f in ("cache_read_tokens", "cache_write_tokens")},
             "first_attempts": len(first), "later_attempt_count": len(later),
             "attempt_order": "observed-time" if order_known else "ambiguous-or-unavailable",
             "first_is_cold": None, "identity_status": "not-applicable" if run["projection"] == "legacy" else "observed",
             "identity_transitions": None, "identity_unknown_attempts": 0,
             "history_prefix_transitions": None}
    if not order_known:
        cache["first_attempt"] = {f: _summary([None]) for f in ("cache_read_tokens", "cache_write_tokens")}
        cache["later_attempts"] = {f: _summary([None] * max(1, len(main) - 1)) for f in ("cache_read_tokens", "cache_write_tokens")}
    if run["projection"] != "legacy":
        identities = [r.get("cache_identity") for r in main]
        cache["identity_unknown_attempts"] = sum(value is None for value in identities)
        cache["identity_transitions"] = sum(left != right for left, right in zip(identities, identities[1:])) if order_known and identities and all(identities) else None
        if cache["identity_transitions"] is None:
            cache["identity_status"] = "unknown"
    prefixes = [r.get("history_prefix_sha256") for r in main]
    if order_known and all(prefixes):
        cache["history_prefix_transitions"] = sum(left != right for left, right in zip(prefixes, prefixes[1:]))
    route_rows = []
    for row in all_usage:
        purpose_settings = settings.get("auxiliary_models", {}).get(row.get("purpose", "main"), settings)
        route_rows.append(_model_identity(row, purpose_settings))
    estimates = defaultdict(list)
    for row in all_usage:
        for estimate in _estimates(row):
            name = estimate.get("requested_estimator") or estimate.get("estimator") or "unknown"
            tokens_estimate, output, reported = estimate.get("tokens"), estimate.get("output_reservation"), row.get("input_tokens")
            applicable = estimate.get("applicable", True)
            signed = tokens_estimate - output - reported if applicable and all(v is not None for v in (tokens_estimate, output, reported)) else None
            estimates[name].append({"attempt_id": row.get("attempt_id", row.get("id")), "version": estimate.get("version"),
                                    "confidence": estimate.get("confidence"), "encoding": estimate.get("encoding"),
                                    "calibration_state": estimate.get("calibration_state"), "calibration_samples": estimate.get("calibration_samples"),
                                    "actual_estimator": estimate.get("estimator"), "encoding_version": estimate.get("encoding_version"),
                                    "snapshot": estimate.get("snapshot"),
                                    "selected": estimate["selected"], "applicable": applicable, "signed_input_error": signed,
                                    "absolute_input_error": abs(signed) if signed is not None else None,
                                    "underestimated": signed < 0 if signed is not None else None,
                                    "overflow_demoted": estimate.get("overflow_demoted")})
    recovery = run["recovery"] or {}
    compactions = run["context_compaction_attempts"]
    adopted = [r for r in compactions if r.get("outcome") in ("adopted", "applied")]
    overflow = {"provider_rejections": sum(r.get("outcome") == "rejected" and r.get("rejection_class") in
                                              ("context_overflow", "context_length_exceeded", "overflow") for r in all_usage) if run["usage_available"] else None,
                "local_fitting_failures": recovery.get("local_fitting_failures"),
                "current_local_fitting_reason": recovery.get("context_reason"),
                "recovery_attempts": recovery.get("overflow_recovery_attempts", recovery.get("overflow_retry_count")),
                "current_overflow_episode_attempts": recovery.get("overflow_attempts"),
                "current_estimator_demoted": recovery.get("estimator_demoted"),
                "compaction_attempts": len(compactions) if complete else None,
                "pruning_adopted": sum(r.get("stage") in ("pruning", "prune") for r in adopted) if complete else None,
                "summary_adopted": sum(r.get("stage") in ("summary", "summary_recovery") for r in adopted) if complete else None,
                "compaction_unknown_outcomes": sum(r.get("outcome") is None for r in compactions) if complete else None,
                "terminal_overflow_outcome": run["outcome"] == "overflow_failure"}
    guards = {"safety_violation": recovery.get("safety_violation"),
              "false_completion": run["outcome"] == "false_completion" or recovery.get("false_completion") is True,
              "constraint_loss": run["constraint_retention"] is False or recovery.get("constraint_loss") is True,
              "guard_evidence_complete": recovery.get("guard_evidence_complete") is True,
              "control_accepted": recovery.get("control_accepted")}
    return {**run, "inference_usage": None, "decision_attempts": None, "memory_model_attempts": None,
            "context_compaction_attempts": None, "recovery": None,
            "generation_embedding_usage": None,
            "success": run["outcome"] == "success" and run["oracle_passed"] is True,
            "tokens": tokens, "cost": _cost(run, complete), "cache": cache, "routes": route_rows,
            "route_complete": bool(route_rows) and all(row["status"] == "matched" for row in route_rows),
            "selected_estimators": sorted({str(row.get("estimator") or "unknown") for row in main}),
            "selected_estimate_identities": [_estimates(row)[0] for row in main],
            "measurement_gaps": _measurement_gaps(all_usage, settings),
            "estimator_errors": dict(estimates), "overflow": overflow, "guards": guards}


def _group(runs):
    outcomes = dict(sorted(Counter(r["outcome"] for r in runs).items()))
    successes = sum(r["success"] for r in runs)
    purposes = defaultdict(lambda: defaultdict(list))
    units = defaultdict(list)
    work = {}
    for run in runs:
        for item in run["cost"]["provider_work"]:
            identifier = item["provider_work_id"]
            if identifier in work and work[identifier] != item:
                raise EvidenceError("shared provider work has contradictory metadata")
            work[identifier] = item
    for item in work.values():
        units[item["unit"]].append(item["amount"])
        purposes[item["purpose"]][item["unit"]].append(item["amount"])
    incomplete = sum(not r["cost"]["auxiliary_evidence_complete"] or not r["usage_available"] for r in runs)
    if incomplete:
        units["nanocredits"].extend([None] * incomplete)
    embedding_unpriced = sum(r["cost"]["unpriced_embedding_work"] for r in runs)
    if embedding_unpriced:
        units["nanocredits"].extend([None] * embedding_unpriced)
    costs = _summary(units.get("nanocredits", []))
    if any(u != "nanocredits" for u in units):
        costs["total"] = None
    costs["unknown_runs"] = sum(r["cost"]["total_nanocredits"] is None for r in runs)
    # A subset containing only some of a shared operation's sources cannot claim
    # the whole charge belongs to that subset. Full campaign accounting can.
    scope = {r["run_id"] for r in runs}
    if any(not set(item["shared_run_ids"]) <= scope for item in work.values()):
        costs["total"] = None
    estimator_rows = defaultdict(list)
    for run in runs:
        for name, rows in run["estimator_errors"].items():
            estimator_rows[name].extend(rows)
    return {"attempted_runs": len(runs), "successes": successes, "success_rate": successes / len(runs) if runs else None,
            "outcomes": outcomes, "total_cost_nanocredits": costs,
            "cost_per_success_nanocredits": costs["total"] / successes if costs["total"] is not None and successes else None,
            "cost_per_success_denominator": successes, "distinct_provider_work": len(work),
            "embedding_reservations": sum(r["cost"]["embedding_reservations"] for r in runs),
            "unpriced_embedding_work": embedding_unpriced,
            "cost_units": {u: _summary(v) for u, v in sorted(units.items())},
            "cost_by_purpose": {p: {u: _summary(v) for u, v in sorted(amounts.items())} for p, amounts in sorted(purposes.items())},
            "tokens": {field: {"known": sum(r["tokens"][field]["known"] for r in runs),
                                "unknown_count": sum(r["tokens"][field]["unknown_count"] for r in runs),
                                "known_count": sum(r["tokens"][field]["known_count"] for r in runs)} for field in TOKENS},
            "wall_ms": _distribution([r["wall_ms"] for r in runs]),
            "cost_per_run_nanocredits": _distribution([r["cost"]["total_nanocredits"] for r in runs]),
            "estimator_errors": {name: _estimator_summary(estimator_rows[name])
                                 for name in sorted(set(estimator_rows) | {"bytes", "calibrated", "tokenized"})},
            "overflow": {name: _summary([r["overflow"][name] for r in runs]) for name in
                         ("provider_rejections", "local_fitting_failures", "recovery_attempts", "compaction_attempts", "pruning_adopted", "summary_adopted")},
            "cache": {"legacy_identity_not_applicable_runs": sum(r["cache"]["identity_status"] == "not-applicable" for r in runs),
                      "identity_unknown_runs": sum(r["cache"]["identity_status"] == "unknown" for r in runs),
                      "identity_transitions": _summary([r["cache"]["identity_transitions"] for r in runs if r["cache"]["identity_status"] != "not-applicable"]),
                      "first_attempt": {field: {"known": sum(r["cache"]["first_attempt"][field]["known"] for r in runs),
                                                 "unknown_count": sum(r["cache"]["first_attempt"][field]["unknown_count"] for r in runs)} for field in ("cache_read_tokens", "cache_write_tokens")},
                      "later_attempts": {field: {"known": sum(r["cache"]["later_attempts"][field]["known"] for r in runs),
                                                  "unknown_count": sum(r["cache"]["later_attempts"][field]["unknown_count"] for r in runs)} for field in ("cache_read_tokens", "cache_write_tokens")}}}


def _weighted(runs):
    strata = {task: [r for r in runs if r["task"] == task] for task in PRIMARY_WEIGHTS}
    if any(not rows for rows in strata.values()):
        return {"success_probability": None, "expected_cost_nanocredits": None, "cost_per_success_nanocredits": None,
                "weights": PRIMARY_WEIGHTS, "complete_primary_population": False}
    success = sum(PRIMARY_WEIGHTS[task] * sum(r["success"] for r in rows) / len(rows) for task, rows in strata.items())
    means = {task: _summary([r["cost"]["total_nanocredits"] for r in rows])["total"] for task, rows in strata.items()}
    cost = None if any(value is None for value in means.values()) else sum(PRIMARY_WEIGHTS[task] * means[task] / len(strata[task]) for task in strata)
    return {"success_probability": success, "expected_cost_nanocredits": cost,
            "cost_per_success_nanocredits": cost / success if cost is not None and success else None,
            "weights": PRIMARY_WEIGHTS, "complete_primary_population": True}


def _inconclusive(reason):
    return {"status": "inconclusive", "reasons": [reason], "bounds": {}, "denominators": {}}


def _candidate_run(run, candidate, selected, policy):
    return run["projection"] == candidate and run["prompt_cache"] == policy and (
        run["arm"] == selected or run["arm"] == "candidate")


def _paired_strata(baseline, treatment, route, selected, config, expected_n=None):
    strata, reasons = [], []
    for task, weight in PRIMARY_WEIGHTS.items():
        arms = []
        for arm_rows in (baseline, treatment):
            matches = {}
            for row in arm_rows:
                if row["task"] != task:
                    continue
                block = row["repeat"]
                if block in matches:
                    reasons.append(f"{task}: duplicate repetition block {block}")
                matches[block] = row
            arms.append(matches)
        left, right = arms
        if set(left) != set(right) or not left:
            reasons.append(f"{task}: missing matched repetition blocks")
        if expected_n is not None and (type(expected_n) is not int or expected_n <= 0 or len(left) != expected_n or len(right) != expected_n):
            reasons.append(f"{task}: confirmation does not match the predeclared repetition count")
        bounds = config.get("cost_bounds", {}).get(route, {}).get(task, {})
        strata.append({"task": task, "weight": weight, "independent_blocks": config.get("independent_blocks") is True,
                       "baseline_cost_bound": bounds.get("legacy"), "candidate_cost_bound": bounds.get(selected, bounds.get(config.get("candidate"))),
                       "cost_bounds_enforced": config.get("cost_bounds_enforced") is True,
                       "pairs": [{"baseline_success": left[b]["success"], "candidate_success": right[b]["success"],
                                  "baseline_cost": left[b]["cost"]["total_nanocredits"], "candidate_cost": right[b]["cost"]["total_nanocredits"]}
                                 for b in sorted(set(left) & set(right))]})
    return strata, reasons


def _phase_key(phase):
    return "deployment-mode" if phase == "deployment" else phase


def _arm_id(run):
    return str(run["projection"]) + ("-explicit" if run["prompt_cache"] == "explicit" else "")


def _phase_matrix(rows, phase, arms, repeats, required_tasks):
    reasons = []
    if type(repeats) is not int or repeats < 1:
        return [f"{phase}: the frozen repetition count is missing or invalid"]
    if not rows:
        return [f"{phase}: complete actual registered phase imports are missing"]
    blocks = set(range(1, repeats + 1))
    expected_tasks = set(required_tasks) | {row["task"] for row in rows}
    unexpected = {_arm_id(row) for row in rows} - set(arms)
    if unexpected:
        reasons.append(f"{phase}: unexpected evaluated arms: " + ", ".join(sorted(unexpected)))
    for arm in arms:
        for task in sorted(expected_tasks):
            values = [r["repeat"] for r in rows if _arm_id(r) == arm and r["task"] == task]
            if set(values) != blocks or len(values) != repeats:
                population = "control" if task in CONTROLS else "task"
                reasons.append(f"{phase}: {arm}/{task} {population} has missing, duplicated or unfrozen repetition blocks")
    return reasons


def _deployment_estimator(settings, config):
    expected = settings.get("deployment_estimator", config.get("deployment_estimator"))
    if not isinstance(expected, dict) or expected.get("id") not in ("bytes", "calibrated", "tokenized") or type(expected.get("version")) is not int or expected["version"] < 1 or expected.get("confidence") not in ("conservative", "calibrated", "tokenized"):
        return None
    if expected["id"] == "tokenized" and (not expected.get("encoding") or type(expected.get("encoding_version")) is not int or expected["encoding_version"] < 1):
        return None
    return expected


def _retained_guard_failures(rows, candidate):
    failures, unknown = [], []
    baselines = defaultdict(list)
    for row in rows:
        if row["projection"] == "legacy" and row["prompt_cache"] == "off":
            baselines[(row["campaign_phase"], row["condition"], row["task"], row["repeat"])].append(row)
    for row in rows:
        if row["projection"] != candidate:
            continue
        context = f"{row['campaign_phase']}/{row['condition']} {row['task']} repetition {row['repeat']}"
        if row["guards"]["safety_violation"] is True:
            failures.append("Candidate safety violation observed: " + context + ".")
        matches = baselines.get((row["campaign_phase"], row["condition"], row["task"], row["repeat"]), [])
        for guard in ("false_completion", "constraint_loss"):
            if row["guards"][guard] is True and matches and all(other["guards"][guard] is False for other in matches):
                failures.append(f"Candidate-only {guard} regression: {context}.")
            elif row["guards"][guard] is True and (not matches or len(matches) != 1):
                unknown.append(f"Candidate {guard} has no unambiguous matched Legacy observation: {context}.")
    return sorted(set(failures)), sorted(set(unknown))


def analyze_campaign(experiments, config):
    """Return a JSON-safe reproducible artifact, with separate route decisions.

    `experiments` contains load_evidence() results. `config.experiments` maps
    experiment_id to {route, phase, condition, tenant_id}; requested endpoint tags
    live in config.routes[route].requested_route. Unregistered experiments are
    retained as diagnostics, never silently assigned a route or phase. Statistical
    gates consume only frozen, independent confirmation blocks, while all pilot,
    control, warm-cache and failed work remains in campaign accounting.
    """
    config = _json(config)
    if not isinstance(experiments, (list, tuple)) or not experiments:
        raise EvidenceError("a campaign requires at least one imported experiment")
    identifiers = [e["experiment"]["experiment_id"] for e in experiments]
    if len(identifiers) != len(set(identifiers)):
        raise EvidenceError("duplicate campaign experiment_id")
    durable_runs = [r["run_id"] for e in experiments if e["complete_export"] for r in e["runs"]]
    if len(durable_runs) != len(set(durable_runs)):
        raise EvidenceError("a durable Run was reused across campaign experiments or phases")
    candidate = config.get("candidate")
    if candidate is not None and candidate not in ("ordered", "native"):
        raise EvidenceError("candidate must be ordered, native or null")
    routes_config = config.get("routes", {})
    required = config.get("required_routes", ["automatic", "explicit"])
    if not isinstance(required, list) or any(not isinstance(r, str) for r in required) or len(required) != len(set(required)) or set(required) != {"automatic", "explicit"}:
        raise EvidenceError("required_routes must preserve automatic and explicit routes")
    all_runs, audits, limitations = [], [], []
    registry = config.get("experiments", {})
    if not isinstance(routes_config, dict) or not isinstance(registry, dict):
        raise EvidenceError("routes and experiments must be configuration mappings")
    policies = config.get("prompt_cache_policies", {})
    if not isinstance(policies, dict) or any(p not in ("off", "explicit") for p in policies.values()) or policies.get("automatic", "off") != "off":
        raise EvidenceError("prompt-cache policies must preserve the automatic route's off condition")
    for evidence in experiments:
        exp = evidence["experiment"]
        registration = registry.get(exp["experiment_id"], {})
        route = registration.get("route", "unassigned")
        route_config = routes_config.get(route, {})
        if not isinstance(route_config, dict):
            raise EvidenceError("routes entries must be configuration objects")
        requested = route_config.get("requested_route", route_config.get("provider_tag"))
        if requested is not None:
            _text(requested, "requested_route")
        if registration.get("tenant_id") is not None and evidence["tenant_id"] is not None and registration["tenant_id"] != evidence["tenant_id"]:
            raise EvidenceError("campaign registration Tenant differs from the imported export")
        live = exp["tier"] == "live" and exp["model"]["kind"] == "live-provider"
        audit = {"experiment_id": exp["experiment_id"], "route": route, "phase": _phase_key(registration.get("phase", "unknown")),
                 "condition": registration.get("condition", "unknown"), "live": live,
                 "manifest_sha256": evidence["manifest_sha256"], "export_sha256": evidence["export_sha256"],
                 "source": exp["aidash"], "corpus": exp["corpus"], "evaluator": exp["evaluator"],
                 "tenant_id": evidence["tenant_id"], "complete_export": evidence["complete_export"],
                 "planned_runs": len(exp["variants"]) * exp["repeat_count"], "attempted_runs": len(evidence["runs"]),
                 "skipped_runs": evidence["skipped_runs"], "error": evidence["manifest_error"], "spend_reservations": evidence["spend"]}
        audits.append(audit)
        limitations.extend(evidence["limitations"])
        for row in evidence["runs"]:
            run = _run_metrics(row, evidence["complete_export"], route_config)
            run.update(experiment_id=exp["experiment_id"], route=route, campaign_phase=audit["phase"],
                       condition=audit["condition"], live=live, complete_export=evidence["complete_export"])
            all_runs.append(run)
    route_reports, pilot_statistics = {}, {"routes": []}
    for route in sorted(set(required) | {r["route"] for r in all_runs}):
        rows = [r for r in all_runs if r["route"] == route]
        grouped = defaultdict(list)
        for row in rows:
            grouped[(row["task"], row["arm"], row["campaign_phase"], row["condition"])].append(row)
        table = [{"task": key[0], "arm": key[1], "phase": key[2], "condition": key[3],
                  "population": "primary" if key[0] in PRIMARY_WEIGHTS else "control" if key[0] in CONTROLS else "diagnostic",
                  "weight": PRIMARY_WEIGHTS.get(key[0], 0), **_group(group)} for key, group in sorted(grouped.items())]
        route_audits = [a for a in audits if a["route"] == route]
        reasons = []
        if not rows:
            reasons.append("Required route has no attempted Runs.")
        if any(not a["live"] for a in route_audits):
            reasons.append("Scripted fixtures measure accounting only and cannot authorize a default change.")
        if any(not a["complete_export"] for a in route_audits):
            reasons.append("Complete operator evidence is unavailable.")
        if any(a["skipped_runs"] or a["error"] for a in route_audits):
            reasons.append("The planned phase is incomplete or stopped; skipped and failed work is retained.")
        if any(a["source"].get("dirty") is not False or not a["source"].get("commit") or not a["source"].get("binary_sha256") or not a["source"].get("server_revision") for a in route_audits if a["phase"] == "confirmation"):
            reasons.append("The confirmation source, clean state, binary and deployed revision are not fully identified.")
        if len({canonical_sha256({k: a[k] for k in ("source", "corpus", "evaluator")}) for a in route_audits}) > 1:
            reasons.append("Source, corpus or evaluator identity changed across campaign phases.")
        if candidate not in ("ordered", "native"):
            reasons.append("No candidate Projection Version was frozen.")
        if config.get("confirmation_frozen") is not True:
            reasons.append("Candidate and confirmation sample size were not frozen before confirmation.")
        if config.get("independent_blocks") is not True:
            reasons.append("Independent repetition blocks are not established.")
        if not routes_config.get(route, {}).get("requested_route", routes_config.get(route, {}).get("provider_tag")):
            reasons.append("An exact requested upstream route was not declared.")
        cache_policy = config.get("prompt_cache_policies", {}).get(route, "off")
        selected = config.get("candidate_arms", {}).get(route, candidate + "-explicit" if candidate and cache_policy == "explicit" else candidate)
        primary = [r for r in rows if r["campaign_phase"] == "confirmation" and r["condition"] == "primary"
                   and (r["projection"] == "legacy" or _candidate_run(r, candidate, selected, cache_policy))]
        # Explicit caching is a separate, combined treatment, never described as
        # the isolated effect of a Projection Version.
        baseline = [r for r in primary if r["projection"] == "legacy" and r["prompt_cache"] == "off"]
        treatment = [r for r in primary if _candidate_run(r, candidate, selected, cache_policy)]
        pilot = [r for r in rows if r["campaign_phase"] == "pilot" and r["condition"] == "primary"]
        deployment = [r for r in rows if r["campaign_phase"] == "deployment-mode" and r["condition"] == "primary"]
        settings = routes_config.get(route, {})
        original_tasks = set(PRIMARY_WEIGHTS) | set(CONTROLS) | set(settings.get("task_ids", []))
        pilot_arms = ["legacy", "ordered", "native"] + (["ordered-explicit", "native-explicit"] if route == "explicit" else [])
        if "arms" in settings and settings["arms"] != pilot_arms:
            reasons.append("Pilot arm registration differs from the accepted three/five-arm comparison.")
        confirmation_n = config.get("confirmation_repetitions", config.get("confirmation_repeats", "undeclared"))
        deployment_n = settings.get("deployment_repetitions", config.get("deployment_repetitions", config.get("deployment_repeats")))
        deployment_arms = settings.get("deployment_arms", [selected])
        if not isinstance(deployment_arms, list) or selected not in deployment_arms or any(a not in ("legacy", selected) for a in deployment_arms):
            deployment_arms = [selected]
            reasons.append("Deployment-mode arm registration is missing the selected candidate or has unsupported arms.")
        phase_matrices = {
            "pilot": _phase_matrix(pilot, "pilot", pilot_arms, config.get("pilot_repeats", 10), original_tasks),
            "confirmation": _phase_matrix(primary, "confirmation", ["legacy", selected], confirmation_n, original_tasks),
            "deployment-mode": _phase_matrix(deployment, "deployment-mode", deployment_arms, deployment_n, original_tasks),
        }
        for phase in phase_matrices:
            phase_audits = [a for a in route_audits if a["phase"] == phase and a["condition"] == "primary"]
            if not phase_audits or any(not a["live"] or not a["complete_export"] or a["skipped_runs"] or a["error"] or a["attempted_runs"] != a["planned_runs"] for a in phase_audits):
                phase_matrices[phase].append(f"{phase}: complete actual registered live imports are missing or incomplete")
            absent = [identifier for identifier, registration in registry.items() if registration.get("route") == route and _phase_key(registration.get("phase")) == phase and registration.get("condition") == "primary" and identifier not in identifiers]
            if absent:
                phase_matrices[phase].append(f"{phase}: registered experiment imports are missing")
            reasons.extend(phase_matrices[phase])
        expected_deployment = _deployment_estimator(settings, config)
        if expected_deployment is None:
            reasons.append("The intended deployment estimator identity/version/confidence was not frozen.")
        for phase_rows, phase in ((pilot, "pilot"), (primary, "confirmation"), (deployment, "deployment-mode")):
            if any(not r["route_complete"] for r in phase_rows):
                reasons.append(f"{phase}: actual provider endpoint/model identity is unknown or mismatched.")
            if any(r["measurement_gaps"] for r in phase_rows):
                reasons.append(f"{phase}: required applicable token counters, estimate identities or shadow measurements are unknown.")
            if any(r["cache"]["identity_status"] == "unknown" for r in phase_rows):
                reasons.append(f"{phase}: applicable Stable Prefix cache-identity transitions are unknown.")
            if any(r["cost"]["total_nanocredits"] is None for r in phase_rows):
                reasons.append(f"{phase}: total provider cost is unknown or incompatible.")
            if any(not r["guards"]["guard_evidence_complete"] or r["guards"]["safety_violation"] is None for r in phase_rows):
                reasons.append(f"{phase}: independent guard evidence is incomplete.")
            if any(r["task"] in CONTROLS and r["guards"]["control_accepted"] is not True for r in phase_rows):
                reasons.append(f"{phase}: independent control acceptance is missing or failed.")
            if phase in ("pilot", "confirmation"):
                if any(not r["selected_estimators"] or any(e != "bytes" for e in r["selected_estimators"]) for r in phase_rows):
                    reasons.append(f"{phase}: the primary execution estimator is not the frozen Bytes mode.")
            elif expected_deployment is not None:
                if any(not r["selected_estimate_identities"] or any(_estimate_gaps(e, "deployment", expected_deployment) for e in r["selected_estimate_identities"]) for r in phase_rows):
                    reasons.append("Deployment-mode attempts do not verify the frozen deployment estimator.")
        pilot_strata, pilot_reasons = _paired_strata([r for r in pilot if r["projection"] == "legacy" and r["prompt_cache"] == "off"],
                                                    [r for r in pilot if _candidate_run(r, candidate, selected, cache_policy)],
                                                    route, selected, config)
        if pilot_reasons or phase_matrices["pilot"] or any(not r["live"] or not r["route_complete"] or not r["complete_export"] or r["measurement_gaps"] for r in pilot):
            for entry in pilot_strata:
                entry["independent_blocks"] = False
            pilot_reasons.append("Pilot evidence is incomplete, non-live, unpaired or route-incomparable.")
        if route in required:
            pilot_statistics["routes"].append({"route": route, "strata": pilot_strata, "reasons": pilot_reasons})
        if not primary or not baseline or not treatment:
            reasons.append("Independent matched confirmation Runs are missing.")
        if any(not r["route_complete"] for r in primary):
            reasons.append("Some provider attempts have missing or mismatched actual route evidence.")
        if any(not r["selected_estimators"] or any(e.lower() != "bytes" for e in r["selected_estimators"]) for r in primary):
            reasons.append("Primary execution does not consistently record the frozen Bytes estimator.")
        for task in set(r["task"] for r in primary):
            if len({r["matched_configuration_sha256"] for r in primary if r["task"] == task}) > 1:
                reasons.append(f"{task}: model, tools, skills or other harness settings differ across arms.")
        if any(not r["guards"]["guard_evidence_complete"] or r["guards"]["safety_violation"] is None for r in primary):
            reasons.append("Independent safety and constraint guard evidence is incomplete.")
        if any(r["task"] in CONTROLS and r["guards"]["control_accepted"] is not True for r in primary):
            reasons.append("Independent live control acceptance criteria were not satisfied or are unknown.")
        if any(r["cost"]["total_nanocredits"] is None for r in primary):
            reasons.append("Total provider cost has unknown amounts, incompatible units or incomplete auxiliary accounting.")
        missing = (set(PRIMARY_WEIGHTS) | set(CONTROLS)) - {r["task"] for r in primary}
        if missing:
            reasons.append("Confirmation omits original task/control rows: " + ", ".join(sorted(missing)))
        strata, pairing_reasons = _paired_strata(baseline, treatment, route, selected, config,
                                                 confirmation_n)
        reasons.extend(pairing_reasons)
        try:
            from projection_statistics import quality_bound, cost_ratio_bound
            quality, economic = quality_bound(strata), cost_ratio_bound(strata)
        except ImportError:
            quality = _inconclusive("The pinned statistical implementation is unavailable.")
            economic = _inconclusive("The pinned statistical implementation is unavailable.")
        guard_failures, guard_unknowns = _retained_guard_failures(rows, candidate)
        reasons.extend(guard_unknowns)
        status = "inconclusive" if reasons else "fail" if guard_failures else "pass" if quality["status"] == economic["status"] == "pass" else "fail" if "fail" in (quality["status"], economic["status"]) else "inconclusive"
        route_reports[route] = {"status": status, "reasons": sorted(set(reasons + guard_failures)),
                                "quality_gate": quality, "economic_gate": economic, "guard_failures": sorted(set(guard_failures)),
                                "phase_completeness": phase_matrices, "deployment_estimator": expected_deployment,
                                "treatment": "combined-projection-and-explicit-cache" if any(r["prompt_cache"] == "explicit" for r in treatment) else "projection-with-prompt-cache-off",
                                "candidate_arm": selected, "requested_route": routes_config.get(route, {}).get("requested_route", routes_config.get(route, {}).get("provider_tag")),
                                "tasks": table, "all_work": _group(rows),
                                "primary_legacy": {**_group([r for r in baseline if r["task"] in PRIMARY_WEIGHTS]), "fixed_weight": _weighted(baseline)},
                                "primary_candidate": {**_group([r for r in treatment if r["task"] in PRIMARY_WEIGHTS]), "fixed_weight": _weighted(treatment)},
                                "control_work": _group([r for r in rows if r["task"] in CONTROLS]),
                                "strata": strata}
    statuses = [route_reports[r]["status"] for r in required]
    status = "pass" if all(s == "pass" for s in statuses) else "fail" if "fail" in statuses else "inconclusive"
    artifact = {"schema_version": 1, "evaluator": {"name": "aidash-projection-evidence", "version": VERSION},
                "config_sha256": canonical_sha256(config), "configuration": config, "experiments": audits,
                "decision": {"status": status, "keep_legacy": status != "pass", "candidate": candidate,
                             "scope": "Only the frozen task population and supported model configurations on both required routes; no rollout is enabled by this report."},
                "routes": route_reports, "pilot_statistics": pilot_statistics, "campaign_all_work": _group(all_runs), "runs": all_runs,
                "limitations": sorted(set(limitations + ["First dispatched attempts are not asserted cold; cache identities are not provider prefix or hit evidence.",
                                                        "Provider-reported identities are independent metadata; unreported revisions and physical cache placement remain unknown.",
                                                        "Provider costs are in their recorded units; reservations and upstream-cost breakdowns are not added to billed charges.",
                                                        "Statistical bounds describe only the frozen task population and do not prove unseen failures impossible."]))}
    # Reject overflow from arithmetic as well as non-finite imported numbers.
    errors = evaluation.non_finite_numbers(artifact)
    if errors:
        raise EvidenceError("non-finite derived report: " + errors[0])
    return artifact


def _cell(value):
    return str(value).replace("|", "\\|").replace("\r", " ").replace("\n", " ")[:240]


def render_report(analysis):
    """Render bounded reproducible Markdown; `analysis` is the JSON artifact.

    Detailed per-Run and estimator identity observations live in the JSON artifact,
    rather than unbounded Markdown tables. Rendering has no clock-dependent data.
    """
    decision = analysis["decision"]
    rows = ["# Matched Projection Version comparison", "",
            f"Decision: **{decision['status']}**. " + ("Keep Legacy." if decision["keep_legacy"] else "The measured gate passed; rollout requires its separately scoped implementation."), "",
            decision["scope"], "", f"Configuration SHA-256: `{analysis['config_sha256']}`", "",
            "Fixtures verify accounting and request semantics; they provide no provider quality, pricing or rollout evidence.", "",
            "## Evidence identity", "", "| Experiment | Route | Phase | Source | Result digest | Export digest |",
            "| --- | --- | --- | --- | --- | --- |"]
    for item in analysis["experiments"][:100]:
        rows.append("| " + " | ".join(_cell(v) for v in (item["experiment_id"], item["route"], item["phase"],
                    item["source"].get("commit"), item["manifest_sha256"], item["export_sha256"] or "unavailable")) + " |")
    for name, route in sorted(analysis["routes"].items()):
        rows += ["", f"## Route: {_cell(name)}", "", f"Decision: **{route['status']}**. Treatment: {_cell(route['treatment'])}.", "",
                 f"Requested upstream route: `{_cell(route['requested_route'] or 'unknown')}`. Actual route evidence is retained per attempt.", ""]
        for reason in route["reasons"][:50]:
            rows.append("- " + _cell(reason))
        rows += ["", "| Gate | Status | Bound |", "| --- | --- | --- |"]
        for gate in ("quality_gate", "economic_gate"):
            item = route[gate]
            rows.append(f"| {gate} | {item['status']} | {_cell(json.dumps(item.get('bounds', {}), sort_keys=True))} |")
        rows += ["", "Primary weights: coding rows 1/8 each; overdue totals and both correction-retention rows 1/4 each. Controls and diagnostics have no primary completion weight.", "",
                 "| Task | Arm | Phase / condition | Population | Runs | Successes | Known cost / unknown charges (nanocredits) |",
                 "| --- | --- | --- | --- | ---: | ---: | --- |"]
        for item in route["tasks"][:500]:
            cost = item["total_cost_nanocredits"]
            rows.append("| " + " | ".join(_cell(v) for v in (item["task"], item["arm"], item["phase"] + " / " + item["condition"],
                        item["population"], item["attempted_runs"], item["successes"], f"{cost['known']} / {cost['unknown_count']}")) + " |")
        rows += ["", "All outcomes, failed work, control costs, purpose-separated charges, token known/unknown counts, estimator errors, cache transitions, first/later cache usage, overflow and adopted compaction counts are retained in the companion JSON artifact.", ""]
        for label, key in (("Legacy fixed-weight estimate", "primary_legacy"), ("Candidate fixed-weight estimate", "primary_candidate")):
            rows.append(label + ": `" + _cell(json.dumps(route[key]["fixed_weight"], sort_keys=True)) + "`.")
    total = analysis["campaign_all_work"]
    rows += ["", "## Campaign accounting", "", f"Attempted Runs: {total['attempted_runs']}; independently successful tasks: {total['successes']}.",
             f"Cost per successful task denominator: {total['cost_per_success_denominator']}; all unsuccessful work remains in the numerator.",
             "Recorded cost units: `" + _cell(json.dumps(total["cost_units"], sort_keys=True)) + "`.", "", "## Limits", ""]
    rows.extend("- " + _cell(item) for item in analysis["limitations"][:100])
    return "\n".join(rows) + "\n"
