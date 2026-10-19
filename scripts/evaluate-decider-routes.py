#!/usr/bin/env python3
"""Compare a Decisions Route with Jev on fixed compaction states (#153, ADR 0066).

Sends every case of `decider_route_fixtures.py` once to Jev through OpenRouter's
System One API and once to the candidate model on each host it names, through
OpenRouter's Decisions API, all with zero data retention. Then it reports, for
each candidate threshold, how often the candidate's keep/truncate/drop branch
agrees with Jev's at 0.5, how often it removes more than Jev would, and how much
it removes. The chosen threshold goes into the Route's `keep_threshold`.

  OPENROUTER_API_KEY=... python3 -I scripts/evaluate-decider-routes.py --out result.json

The results are measurements on synthetic cases, not a quality claim. Each run
costs well under one cent.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import urllib.request

ROOT = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("fixtures", ROOT / "decider_route_fixtures.py")
fixtures = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixtures)

API = "https://openrouter.ai/api"
THRESHOLDS = [round(0.05 * step, 2) for step in range(2, 11)]


def post(path, body, key):
    request = urllib.request.Request(
        f"{API}/{path}",
        data=json.dumps(body).encode(),
        headers={"Authorization": f"Bearer {key}", "Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=120) as response:
        return json.load(response)


def ask(state, questions, key, model, host=None):
    body = {
        "model": model,
        "state": state,
        "questions": {
            name: {"type": "noul", "instructions": text} for name, text in questions.items()
        },
        "provider": {"zdr": True},
    }
    if host is None:
        answer = post("v1/systemone", body, key)
    else:
        body["provider"].update({"only": [host], "allow_fallbacks": False})
        answer = post("alpha/decisions", body, key)
    probabilities = {name: value["noul"] for name, value in answer["answers"].items()}
    if probabilities.keys() != questions.keys():
        raise SystemExit(f"{model} on {host}: answers differ from the questions")
    return probabilities, answer.get("provider"), answer.get("usage", {}).get("cost") or 0.0


def branch(call, result, threshold):
    """The builder's rule: keep, keep the call with a truncated result, or drop."""
    if result >= threshold:
        return "keep"
    if call >= threshold:
        return "truncate"
    return "drop"


# How much of a tool pair each branch removes from the projection.
REMOVED = {"keep": 0, "truncate": 1, "drop": 2}


def compare(reference, candidate, threshold):
    agree = more = removed = total = 0
    for name in reference:
        if not name.endswith("_call"):
            continue
        result = name[: -len("_call")] + "_result"
        expected = branch(reference[name], reference[result], 0.5)
        chosen = branch(candidate[name], candidate[result], threshold)
        agree += expected == chosen
        more += REMOVED[chosen] > REMOVED[expected]
        removed += REMOVED[chosen]
        total += 1
    return {"agreement": agree / total, "removes_more": more / total, "removed": removed / (2 * total)}


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--jev", default="typesafe/jev-1.14")
    parser.add_argument("--model", default="cloudflare/clef")
    parser.add_argument("--hosts", default="cloudflare,deepinfra,primeintellect")
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    key = os.environ.get("OPENROUTER_API_KEY")
    if not key:
        raise SystemExit("OPENROUTER_API_KEY is required")
    hosts = args.hosts.split(",")
    reference, candidates, served, cost = {}, {host: {} for host in hosts}, {}, 0.0
    for name, case in fixtures.CASES.items():
        state, questions = fixtures.state_and_questions(case)
        answers, _, spent = ask(state, questions, key, args.jev)
        reference.update({f"{name}/{q}": p for q, p in answers.items()})
        cost += spent
        for host in hosts:
            answers, reported, spent = ask(state, questions, key, args.model, host)
            candidates[host].update({f"{name}/{q}": p for q, p in answers.items()})
            served.setdefault(host, set()).add(reported)
            cost += spent
    jev_removed = compare(reference, reference, 0.5)["removed"]
    table = {
        host: {str(t): compare(reference, answers, t) for t in THRESHOLDS}
        for host, answers in candidates.items()
    }
    result = {
        "jev": args.jev,
        "model": args.model,
        "cases": len(fixtures.CASES),
        "pairs": sum(name.endswith("_call") for name in reference),
        "jev_removed": jev_removed,
        "served": {host: sorted(filter(None, names)) for host, names in served.items()},
        "cost_usd": round(cost, 6),
        "thresholds": table,
        "answers": {"jev": reference, **candidates},
    }
    args.out.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(f"jev removes {jev_removed:.2f} of tool content; cost ${cost:.6f}")
    for host, rows in table.items():
        for threshold, row in rows.items():
            print(
                f"{host:15} t={threshold:<4} agree={row['agreement']:.2f} "
                f"removes_more={row['removes_more']:.2f} removed={row['removed']:.2f}"
            )


if __name__ == "__main__":
    main()
