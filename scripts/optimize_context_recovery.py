#!/usr/bin/env -S uv run --with dspy>=3.4,<4 --with gepa>=0.0.21
"""Optimize one summary prompt and one offload marker with DSPy models and GEPA.

Only synthetic or Aidash evaluation Run exports may be used as examples.
"""
import argparse
import json
import string
from pathlib import Path

from context_recovery_probes import generate

SEED = {
    "summary_prompt": "Summarize the journal faithfully. Preserve corrections, people’s instructions, identifiers and completed side effects.",
    "marker": "Original content is in Context Journal range {from_seq}-{through_seq}; use Journal Query to read it.",
}


def json_text(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


class FakeLM:
    """Deterministic offline model: provides a lossy summary and misses probes."""
    def __call__(self, prompt):
        if isinstance(prompt, list):
            prompt = json_text(prompt)
        if "Return a JSON object mapping probe IDs" in prompt:
            return "{}"
        if "<curr_param>" in prompt or "Current component" in prompt or "```" in prompt:
            return "```\nPreserve exact corrections, identifiers and tool results.\n```"
        return "A short summary without the original details."


class DSPyLM:
    def __init__(self, name):
        import dspy
        self.lm = dspy.LM(name, temperature=0, max_tokens=2048, cache=False)

    def __call__(self, prompt):
        messages = prompt if isinstance(prompt, list) else [{"role": "user", "content": prompt}]
        result = self.lm(messages=messages)
        return str(result[0])


def prepare(example):
    if example.get("source") not in ("synthetic", "aidash-evaluation"):
        raise ValueError("examples must come from synthetic data or Aidash evaluation Runs")
    journal = example.get("journal")
    probes = generate(journal, example.get("side_effect_tools", []))
    if not probes:
        raise ValueError("example has no extractable probes")
    seqs = [item.get("seq", i) for i, item in enumerate(journal, 1)]
    return {**example, "probes": probes, "from_seq": min(seqs), "through_seq": max(seqs)}


def evaluate_episode(example, candidate, models):
    """Each model summarizes and answers every probe; exact answers only."""
    if not models:
        raise ValueError("model panel must not be empty")
    try:
        fields = {name for _, name, _, _ in string.Formatter().parse(candidate["marker"]) if name is not None}
        if fields != {"from_seq", "through_seq"}:
            raise ValueError("range placeholders required")
        marker = candidate["marker"].format(from_seq=example["from_seq"], through_seq=example["through_seq"])
    except (KeyError, ValueError, IndexError):
        return 0.0, {"error": "marker must use only {from_seq} and {through_seq}", "models": {}}
    if str(example["from_seq"]) not in marker or str(example["through_seq"]) not in marker:
        return 0.0, {"error": "marker must include both journal range endpoints", "models": {}}
    questions = [{"id": p["id"], "question": p["question"]} for p in example["probes"]]
    results = {}
    for name, lm in models.items():
        summary = lm(candidate["summary_prompt"] + "\nJournal data:\n" + json_text(example["journal"]))
        raw = lm("Answer from the summary. Return a JSON object mapping probe IDs to exact string answers; "
                 "use an empty string if unknown.\nSummary:\n" + summary + "\nMarker:\n" + marker +
                 "\nProbes:\n" + json_text(questions))
        try:
            answers = json.loads(raw)
            if not isinstance(answers, dict):
                answers = {}
        except (ValueError, TypeError):
            answers = {}
        missed = [{"id": p["id"], "type": p["type"], "question": p["question"],
                   "expected": p["answer"], "actual": answers.get(p["id"])}
                  for p in example["probes"] if answers.get(p["id"]) != p["answer"]]
        results[name] = {"score": 1 - len(missed) / len(example["probes"]), "missed": missed}
    return min(item["score"] for item in results.values()), {"models": results}


def optimize(train, validation, models, reflection, max_calls, run_dir=None):
    import gepa
    from gepa.core.adapter import EvaluationBatch

    class Adapter:
        propose_new_texts = None

        def evaluate(self, batch, candidate, capture_traces=False):
            pairs = [evaluate_episode(example, candidate, models) for example in batch]
            return EvaluationBatch(outputs=[detail for _, detail in pairs], scores=[score for score, _ in pairs],
                                   trajectories=[detail for _, detail in pairs] if capture_traces else None)

        def make_reflective_dataset(self, candidate, eval_batch, components_to_update):
            records = []
            for score, detail in zip(eval_batch.scores, eval_batch.trajectories or []):
                lines = [detail.get("error", "")]
                for model, result in detail.get("models", {}).items():
                    lines.extend(f"{model}: missed {p['id']} ({p['type']}): {p['question']} "
                                 f"expected={p['expected']!r}, actual={p['actual']!r}"
                                 for p in result["missed"])
                records.append({"score": score, "feedback": "\n".join(line for line in lines if line)})
            return {name: records for name in components_to_update}

    return gepa.optimize(seed_candidate=SEED, trainset=train, valset=validation, adapter=Adapter(),
                         reflection_lm=reflection, max_metric_calls=max_calls, reflection_minibatch_size=1,
                         skip_perfect_score=False, run_dir=run_dir, display_progress_bar=False, seed=850,
                         raise_on_exception=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--examples", type=Path, help="JSON list with id, split, source, journal and side_effect_tools")
    parser.add_argument("--model", action="append", help="DSPy model identifier; repeat for each model family")
    parser.add_argument("--reflection-model", help="DSPy model identifier for GEPA reflection")
    parser.add_argument("--max-metric-calls", type=int, default=20)
    parser.add_argument("--run-dir", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--dry-run", action="store_true", help="fake models only; no network or paid calls")
    args = parser.parse_args()
    if args.max_metric_calls < 3:
        parser.error("--max-metric-calls must be at least 3")
    if args.dry_run:
        examples = [{"id": split, "split": split, "source": "synthetic", "task_completed": False,
                     "journal": [{"seq": 1, "event": {"kind": "human", "response": "BUDGET-USD=300"}}]}
                    for split in ("train", "validation", "test")]
        models = {"fake-a": FakeLM(), "fake-b": FakeLM()}
        reflection = FakeLM()
    else:
        if not args.examples or not args.model or not args.reflection_model:
            parser.error("--examples, --model and --reflection-model are required outside --dry-run")
        if len(set(args.model)) != len(args.model) or len(args.model) < 2:
            parser.error("the panel needs at least two distinct model identifiers")
        examples = json.loads(args.examples.read_text())
        models = {name: DSPyLM(name) for name in args.model}
        reflection = DSPyLM(args.reflection_model)
    train = [prepare(e) for e in examples if e.get("split") == "train"]
    validation = [prepare(e) for e in examples if e.get("split") == "validation"]
    test = [prepare(e) for e in examples if e.get("split") == "test"]
    ids = [e.get("id") for e in examples]
    if not train or not validation or not test or len(train) + len(validation) + len(test) != len(examples) or \
            any(not isinstance(i, str) or not i for i in ids) or len(ids) != len(set(ids)) or \
            any(not isinstance(e.get("task_completed"), bool) for e in test):
        parser.error("examples require unique IDs, nonempty train/validation/test splits, and boolean test task_completed")
    result = optimize(train, validation, models, reflection, args.max_metric_calls,
                      str(args.run_dir) if args.run_dir else None)
    best = result.best_candidate
    scores = [evaluate_episode(item, best, models) for item in validation]
    test_scores = [evaluate_episode(item, best, models) for item in test]
    groups = {}
    for complete in (False, True):
        values = [score for item, (score, _) in zip(test, test_scores) if item["task_completed"] == complete]
        groups[str(complete).lower()] = {"count": len(values), "mean_probe_score": sum(values) / len(values) if values else None}
    failed, succeeded = groups["false"], groups["true"]
    agreement = "insufficient-data" if not failed["count"] or not succeeded["count"] else (
        "higher-on-success" if succeeded["mean_probe_score"] > failed["mean_probe_score"] else "not-higher-on-success")
    output = {"candidate": best, "validation_mean_worst_model_score": sum(s for s, _ in scores) / len(scores),
              "held_out_task_completion_groups": groups, "held_out_probe_comparison": agreement,
              "held_out": [{"id": e["id"], "task_completed": e["task_completed"], "score": score,
                             "feedback": detail} for e, (score, detail) in zip(test, test_scores)],
              "panel": list(models), "validation": [{"id": e["id"], "score": s, "feedback": detail}
                                                        for e, (s, detail) in zip(validation, scores)]}
    args.output.write_text(json.dumps(output, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(output, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
