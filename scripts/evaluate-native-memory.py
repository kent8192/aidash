#!/usr/bin/env python3
"""Evaluate frozen bilingual labels through the real authorized native memory API.

Creates fresh participants in an explicitly selected fixture Workspace. Input is
synthetic; no existing participant is changed. Results contain raw claims, exact
profile references, request timings and durable charged usage. Regex labels are
an auditable contract grade, not a human semantic-quality or release approval.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import time
import tomllib
import urllib.error
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parent.parent


def delivered_tokens(recalled):
    """UTF-8 bytes of the compact delivered envelope, which the pinned `utf8_upper_bound` tokenizer charges."""
    return len(json.dumps(recalled, ensure_ascii=False, separators=(",", ":")).encode())


def latencies(requests):
    return {phase: sorted(row["round_trip_ms"] for row in requests if row["phase"].endswith(":" + phase))
            for phase in ("extraction", "recall", "indexing", "correction")}


def compare_diversification(baseline, diversified):
    """Pair each case without and with diversification; both arms use the same frozen cases."""
    rows = []
    for plain, diverse in zip(baseline, diversified, strict=True):
        assert plain["id"] == diverse["id"], "paired arms must evaluate the same case"
        rows.append({"id": plain["id"], "language": plain["language"],
                     **{name + suffix: arm["metrics"][name] for suffix, arm in (("_without", plain), ("_with", diverse))
                        for name in ("useful_recall", "delivered_units", "delivered_tokens")}})
    languages = {}
    for row in rows:
        total = languages.setdefault(row["language"], {"cases": 0, "useful_recall_without": 0.0, "useful_recall_with": 0.0,
                                                       "delivered_tokens_without": 0, "delivered_tokens_with": 0})
        total["cases"] += 1
        for name in ("useful_recall_without", "useful_recall_with", "delivered_tokens_without", "delivered_tokens_with"):
            total[name] += row[name]
    for total in languages.values():
        for name in ("useful_recall_without", "useful_recall_with"):
            total[name] /= total["cases"]
    return {"cases": rows, "by_language": languages}


def evaluate(call, workspace, agent, provider, profile, fixture=None, report=None, diversity=None):
    """Evaluate the frozen labels. `diversity` ({"agent", "provider", "profile"}) names an Agent whose memory policy is
    identical except for `diversity_lambda_millionths`; the fixture's near-duplicate `diversity_cases` then run on both
    Agents under a two-Unit budget and the report gains a paired `diversification.comparison`."""
    fixture = fixture or ROOT / "tests/fixtures/native_memory_evaluation.toml"
    raw = fixture.read_bytes()
    labels = tomllib.loads(raw.decode())
    report = {} if report is None else report
    report.update({"fixture": labels["version"], "fixture_sha256": hashlib.sha256(raw).hexdigest(),
              "profile": profile, "grade": "fixed-regex-contract-labels", "production_quality_approved": False,
              "numeric_service_targets_approved": False, "cases": [], "requests": []})

    def helpers(provider, requests):
        state = {"phase": "initialization"}

        def api(path, body=None):
            begin = time.perf_counter()
            status, value = call(path, body)
            requests.append({"phase": state["phase"], "path": path, "status": status,
                             "round_trip_ms": (time.perf_counter() - begin) * 1000})
            if status != 200:
                raise RuntimeError(f"evaluation request {path} returned HTTP {status}")
            return value

        def operate(bank, action, operation_id=None):
            return api(f"/api/workspaces/{workspace}/memory/operate", {
                "operation_id": operation_id or str(uuid.uuid4()), "provider": provider, "bank": bank, "action": action})

        def usage(bank):
            rows, cursor = [], None
            for _ in range(16):
                page = operate(bank, {"action": "usage", "after": cursor})["value"]
                rows.extend(page["items"])
                cursor = page["next"]
                if cursor is None:
                    return rows
            raise RuntimeError("evaluation usage exceeds its finite page budget")

        return state, api, operate, usage

    def await_index(api, state, case, identities):
        state["phase"] = case["id"] + ":indexing"
        deadline = time.monotonic() + 90
        while True:
            entries = api(f"/api/workspaces/{workspace}/semantic/entries")
            states = {entry["id"]: entry["state"] for entry in entries if entry["id"] in identities}
            if len(states) == len(identities) and all(value == "READY" for value in states.values()):
                return
            if any(value in ("ERROR", "REVOKED") for value in states.values()) or time.monotonic() > deadline:
                raise RuntimeError(f"evaluation index did not become ready: {states}")
            time.sleep(0.25)

    def run_near_duplicates(agent, provider, cases, requests):
        """Admit each near-duplicate group as separate Units and recall under a budget that fits exactly two of them."""
        state, api, operate, _ = helpers(provider, requests)
        for case in labels["diversity_cases"]:
            state["phase"] = case["id"] + ":admission"
            bank = api(f"/api/workspaces/{workspace}/memory/participants", {"agent": agent})["bank"]
            identities = [str(uuid.uuid4()) for _ in case["units"]]
            changes = [{"operation": "add", "id": identity, "content": {
                "text": row["text"], "kind": "world", "learning": "fact", "verification": "unverified",
                "occurred": None, "entities": [], "evidence": [], "links": []}}
                for identity, row in zip(identities, case["units"], strict=True)]
            admitted = api(f"/api/workspaces/{workspace}/memory/units/mutate", {
                "operation_id": str(uuid.uuid4()), "provider": provider, "bank": bank, "changes": changes})
            await_index(api, state, case, identities)
            # The pinned utf8_upper_bound tokenizer charges the envelope bytes; size the budget from the live Units.
            budget = max(delivered_tokens({"status": "ready", "units": [first, second]})
                         for index, first in enumerate(admitted) for second in admitted[index + 1:])
            state["phase"] = case["id"] + ":recall"
            recalled = operate(bank, {"action": "recall", "query": {"text": case["query"], "time": None,
                                      "kinds": ["world"], "max_tokens": budget}})["value"]
            label_of = {identity: row["label"] for identity, row in zip(identities, case["units"], strict=True)}
            covered = {label_of[unit["id"]] for unit in recalled.get("units", []) if unit["id"] in label_of}
            cases.append({"id": case["id"], "language": case["language"], "bank": bank, "raw_recall": recalled,
                          "metrics": {"expected_labels": len(case["labels"]), "budget_tokens": budget,
                                      "useful_recall": len(covered) / len(case["labels"]),
                                      "delivered_units": len(recalled.get("units", [])),
                                      "delivered_tokens": delivered_tokens(recalled)}})

    def run_arm(agent, provider, cases, requests):
        state, api, operate, usage = helpers(provider, requests)
        for case in labels["cases"]:
            state["phase"] = case["id"] + ":admission"
            participant = api(f"/api/workspaces/{workspace}/memory/participants", {"agent": agent})
            bank = participant["bank"]
            source = str(uuid.uuid4())
            content = {"text": case["text"], "kind": "world", "learning": "fact", "verification": "unverified",
                       "occurred": None, "entities": [], "evidence": [], "links": []}
            mutation_path = f"/api/workspaces/{workspace}/memory/units/mutate"
            admission = api(mutation_path, {"operation_id": str(uuid.uuid4()), "provider": provider, "bank": bank,
                            "changes": [{"operation": "add", "id": source, "content": content}]})[0]
            evidence = {"kind": "unit", "bank": bank, "id": source, "revision": admission["revision"]}
            state["phase"] = case["id"] + ":extraction"
            operation_id = str(uuid.uuid4())
            action = {"action": "retain", "text": case["text"], "evidence": [evidence]}
            extracted = operate(bank, action, operation_id)["value"]
            before_replay = [row for row in usage(bank) if row["id"] == operation_id]
            replay = operate(bank, action, operation_id)["value"]
            after_replay = [row for row in usage(bank) if row["id"] == operation_id]
            assert replay == extracted and before_replay == after_replay, "replay duplicated output or charged work"
            matched = {}
            for label in case["labels"]:
                matched[label["id"]] = [unit["id"] for unit in extracted if
                    unit["content"]["kind"] == label["kind"] and unit["content"]["learning"] == label["learning"] and
                    all(re.search(pattern, unit["content"]["text"]) for pattern in label["patterns"]) and
                    not any(re.search(pattern, unit["content"]["text"]) for pattern in label.get("forbidden", []))]
            predicted = [unit["id"] for unit in extracted]
            supported = [unit["id"] for unit in extracted if evidence in unit["content"]["evidence"]]
            recognized = {identity for values in matched.values() for identity in values}
            state["phase"] = case["id"] + ":indexing"
            deadline = time.monotonic() + 90
            while True:
                entries = api(f"/api/workspaces/{workspace}/semantic/entries")
                states = {entry["id"]: entry["state"] for entry in entries if entry["id"] in predicted}
                if len(states) == len(predicted) and all(state == "READY" for state in states.values()):
                    break
                if any(state in ("ERROR", "REVOKED") for state in states.values()) or time.monotonic() > deadline:
                    raise RuntimeError(f"evaluation index did not become ready: {states}")
                time.sleep(0.25)
            state["phase"] = case["id"] + ":recall"
            recalled = operate(bank, {"action": "recall", "query": {"text": case["query"], "time": None,
                                  "kinds": ["world", "experience"], "max_tokens": 16384}})["value"]
            recall_ids = {unit["id"] for unit in recalled.get("units", [])}
            recalled_labels = [name for name, identities in matched.items() if recall_ids.intersection(identities)]
            state["phase"] = case["id"] + ":correction"
            corrected = {**content, "text": "The source was withdrawn for correction / 訂正のため元資料を撤回。"}
            api(mutation_path, {"operation_id": str(uuid.uuid4()), "provider": provider, "bank": bank,
                "changes": [{"operation": "correct", "id": source, "expected_revision": admission["revision"], "content": corrected}]})
            visible = api(f"/api/workspaces/{workspace}/memory/units/query", {"provider": provider, "bank": bank})
            still_visible = {unit["id"] for unit in visible}.intersection(predicted)
            state["phase"] = case["id"] + ":usage"
            charged = usage(bank)
            item = {"id": case["id"], "language": case["language"], "bank": bank, "source": admission,
                    "raw_extraction": extracted, "labels": matched, "raw_recall": recalled, "charged_operations": charged,
                    "metrics": {"expected_labels": len(case["labels"]), "extracted_units": len(predicted),
                        "extraction_label_recall": sum(bool(ids) for ids in matched.values()) / len(case["labels"]),
                        "unlabeled_claims": len(set(predicted) - recognized), "missing_support": len(set(predicted) - set(supported)),
                        "duplicate_labels": sum(max(0, len(ids) - 1) for ids in matched.values()),
                        "forbidden_success_claims": sum(any(re.search(pattern, unit["content"]["text"]) for label in case["labels"]
                            for pattern in label.get("forbidden", [])) for unit in extracted),
                        "useful_recall": len(recalled_labels) / len(case["labels"]),
                        "delivered_units": len(recalled.get("units", [])), "delivered_tokens": delivered_tokens(recalled),
                        "stale_units_after_correction": len(still_visible),
                        "charged_calls": sum(row["calls"] for row in charged), "charged_tokens": sum(row["tokens"] for row in charged),
                        "charged_cost_micros": sum(row["cost_micros"] for row in charged)}}
            cases.append(item)

    run_arm(agent, provider, report["cases"], report["requests"])
    report["latency"] = latencies(report["requests"])
    if diversity is not None:
        if not labels.get("diversity_cases"):
            raise RuntimeError("the fixture has no near-duplicate diversity_cases to compare")
        paired = report["diversification"] = {"profile": diversity["profile"], "grade": "fixed-near-duplicate-groups",
            "baseline_cases": [], "baseline_requests": [], "cases": [], "requests": []}
        run_near_duplicates(agent, provider, paired["baseline_cases"], paired["baseline_requests"])
        run_near_duplicates(diversity["agent"], diversity["provider"], paired["cases"], paired["requests"])
        paired["comparison"] = compare_diversification(paired["baseline_cases"], paired["cases"])
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("base-url", "workspace", "agent-id", "agent-version", "provider-id", "provider-version", "server-revision", "output"):
        parser.add_argument("--" + name, required=True)
    for name in ("diversity-agent-id", "diversity-agent-version", "diversity-provider-id", "diversity-provider-version"):
        parser.add_argument("--" + name, help="Optional paired arm: an Agent whose memory policy differs only by diversity_lambda_millionths")
    parser.add_argument("--token-env", default="AIDASH_EVALUATION_TOKEN")
    parser.add_argument("--fixture", type=Path, default=ROOT / "tests/fixtures/native_memory_evaluation.toml")
    args = parser.parse_args()
    paired = [args.diversity_agent_id, args.diversity_agent_version, args.diversity_provider_id, args.diversity_provider_version]
    if any(paired) and not all(paired):
        parser.error("the four --diversity-* options must be given together")
    token = os.environ[args.token_env]

    def call(path, body):
        request = urllib.request.Request(args.base_url.rstrip("/") + path,
            data=None if body is None else json.dumps(body).encode(),
            headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                return response.status, json.load(response)
        except urllib.error.HTTPError as error:
            return error.code, None

    profile = {"server_revision": args.server_revision, "agent": {"id": args.agent_id, "version": args.agent_version},
               "provider": {"id": args.provider_id, "version": args.provider_version}, "model_acceptance": "unapproved"}
    report, failure = {}, None
    try:
        diversity = None
        if all(paired):
            diversity = {"agent": {"id": args.diversity_agent_id, "version": args.diversity_agent_version},
                         "provider": {"id": args.diversity_provider_id, "version": args.diversity_provider_version}}
            diversity["profile"] = {**profile, "agent": diversity["agent"], "provider": diversity["provider"]}
        evaluate(call, args.workspace, profile["agent"], profile["provider"], profile, args.fixture, report, diversity)
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
