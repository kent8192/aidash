#!/usr/bin/env python3
"""Plan, execute and report bounded matched-projection experiments (#181).

`plan`, `report` and `freeze` perform no provider calls. `run` requires an exact
approved execution manifest and reuses the connected-credential fixed-task
runner; it reserves both routes before dispatching either.
"""
import argparse
import copy
import os
from pathlib import Path
import sys

import fixed_task_evaluation as evaluation
import projection_campaign as campaign


def assignments(values, option):
    result = {}
    for value in values:
        name, separator, path = value.partition("=")
        if not separator or name not in campaign.ARMS or not path or name in result:
            raise campaign.CampaignRefusal(f"{option} needs unique automatic=PATH or explicit=PATH values")
        result[name] = path
    return result


def plan_command(args):
    config = campaign.load_config(args.config)
    corpus = evaluation.load_corpus(args.corpus)
    plan = campaign.make_plan(config, corpus)
    campaign.write_json(args.output, plan, exclusive=True)
    counts = ", ".join(f"{route}: {item['planned_runs']} planned Runs" for route, item in plan["routes"].items())
    print(f"Frozen pilot plan: {args.output} ({counts})")
    print("Paid execution needs the listed connection, disclosure and complete-phase budget prerequisites.")
    return 0


def report_command(args):
    import projection_evaluation as evidence
    plan = campaign.validate_plan(campaign.read_json(args.plan))
    paths, exports = assignments(args.result, "--result"), assignments(args.export, "--export")
    if not paths or set(exports) - set(paths):
        raise campaign.CampaignRefusal("reports need result manifests; each export must have a matching result")
    config = copy.deepcopy(plan["config"])
    if args.analysis_config:
        override = campaign.read_json(args.analysis_config)
        if override.get("plan_sha256") != plan["plan_sha256"]:
            raise campaign.CampaignRefusal("analysis configuration does not name this frozen plan")
        if set(override) - {"plan_sha256", "notes"}:
            raise campaign.CampaignRefusal("post hoc analysis configuration accepts notes only; freeze settings before dispatch")
        config.update(override)
    candidate = args.candidate if plan["phase"] == "pilot" else plan["candidate"]
    if candidate not in (None, "ordered", "native"):
        raise campaign.CampaignRefusal("candidate must be Ordered or Native")
    config.update(candidate=candidate, phase=plan["phase"], plan_sha256=plan["plan_sha256"],
                  confirmation_frozen=plan["phase"] != "pilot", experiments={}, cost_bounds={},
                  cost_bounds_enforced=False, independent_blocks=False)
    config["prompt_cache_policies"] = {"automatic": "off", "explicit": args.explicit_cache}
    if plan["phase"] != "pilot":
        config["candidate_arms"] = plan["candidate_arms"]
        config["confirmation_repeats"] = plan["repeats"]
        config["prompt_cache_policies"] = {route: "explicit" if label.endswith("-explicit") else "off"
                                            for route, label in plan["candidate_arms"].items()}
    experiments = []
    corpus = evaluation.load_corpus(args.corpus)
    if corpus["_snapshot"] != plan["corpus"]:
        raise campaign.CampaignRefusal("report corpus differs from the frozen experiment")
    reservation_evidence = []
    def import_phase(phase_plan, results, phase_exports, execution_path=None):
        root_id = plan.get("campaign_plan_sha256", plan["plan_sha256"])
        if phase_plan.get("campaign_plan_sha256", phase_plan["plan_sha256"]) != root_id:
            raise campaign.CampaignRefusal("retained evidence belongs to a different campaign")
        if phase_plan["source"] != plan["source"] or phase_plan["corpus"] != plan["corpus"] \
                or phase_plan["evaluator"] != plan["evaluator"]:
            raise campaign.CampaignRefusal("retained phase changed the source, corpus or evaluator")
        if phase_plan["phase"] != "pilot" and (phase_plan["candidate"] != candidate or
                phase_plan["candidate_arms"] != plan.get("candidate_arms")):
            raise campaign.CampaignRefusal("retained phase changed the frozen candidate/cache policies")
        if phase_plan["phase"] == "deployment-mode":
            config["deployment_repeats"] = phase_plan["repeats"]
            config["deployment_estimator"] = phase_plan["deployment_estimator"]
        for route, path in results.items():
            if route not in campaign.ARMS:
                raise campaign.CampaignRefusal("retained result route is not registered")
            manifest = campaign.read_json(path)
            campaign.validate_result_plan(manifest, phase_plan, route)
            if execution_path:
                bound = campaign.analysis_reservation_evidence(manifest, phase_plan, route,
                            campaign.read_json(execution_path), corpus)
                if phase_plan["phase"] == "confirmation" or (phase_plan["phase"] == "pilot" and
                                                               route not in config["cost_bounds"]):
                    config["cost_bounds"][route] = bound
            reservation_evidence.append(bool(execution_path) and manifest["experiment"]["tier"] == "live")
            imported = evidence.load_evidence(manifest, phase_exports.get(route), corpus=corpus)
            exp = imported["experiment"]
            if exp["experiment_id"] in config["experiments"]:
                raise campaign.CampaignRefusal("an experiment cannot be imported twice")
            config["experiments"][exp["experiment_id"]] = {
                "route": route, "phase": phase_plan["phase"], "condition": "primary",
                "tenant_id": imported["tenant_id"],
            }
            experiments.append(imported)

    import_phase(plan, paths, exports, args.execution_manifest)
    for descriptor in args.retained_evidence:
        retained = campaign.read_json(descriptor)
        if set(retained) - {"plan", "results", "exports", "execution_manifest"} \
                or not {"plan", "results", "exports"} <= retained.keys() or not isinstance(retained["results"], dict) \
                or not isinstance(retained["exports"], dict) or set(retained["exports"]) - set(retained["results"]):
            raise campaign.CampaignRefusal("retained evidence needs plan, results and matching exports mappings")
        retained_plan = campaign.validate_plan(campaign.read_json(retained["plan"]))
        import_phase(retained_plan, retained["results"], retained["exports"], retained.get("execution_manifest"))
    config["cost_bounds_enforced"] = bool(reservation_evidence) and all(reservation_evidence)
    config["independent_blocks"] = plan["config"].get("independent_blocks") is True and bool(
        plan["config"].get("independence_evidence", "").strip())
    analysis = evidence.analyze_campaign(experiments, config)
    analysis["campaign_plan_sha256"] = plan["plan_sha256"]
    # Reserve both artifact names before creating either: never overwrite evidence.
    output, markdown = Path(args.output), Path(args.markdown)
    if output.resolve() == markdown.resolve() or output.exists() or markdown.exists():
        raise campaign.CampaignRefusal("report output paths must be distinct and unused")
    text = evidence.render_report(analysis)
    campaign.write_json(output, analysis, exclusive=True)
    markdown.parent.mkdir(parents=True, exist_ok=True)
    with os.fdopen(os.open(markdown, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w", encoding="utf-8") as stream:
        stream.write(text)
    print(f"Decision: {analysis['decision']['status']}; report: {markdown}; artifact: {output}")
    return 0


def freeze_command(args):
    from projection_statistics import choose_confirmation_n
    plan = campaign.validate_plan(campaign.read_json(args.plan))
    if plan["phase"] != "pilot":
        raise campaign.CampaignRefusal("only the pilot plan can select a confirmation candidate")
    analysis = campaign.read_json(args.pilot_analysis)
    if analysis.get("campaign_plan_sha256") != plan["plan_sha256"] \
            or analysis.get("configuration", {}).get("candidate") != args.candidate:
        raise campaign.CampaignRefusal("pilot analysis does not bind this plan and candidate")
    planning = choose_confirmation_n(analysis.get("pilot_statistics", {}), seed=plan["config"]["execution_seed"])
    if planning["selected_n"] is None:
        campaign.write_json(args.output, {"schema": "aidash-projection-confirmation-refusal-v1",
                            "campaign_plan_sha256": plan["plan_sha256"], "planning": planning,
                            "default_change": False, "keep_legacy": True}, exclusive=True)
        print(f"Confirmation inconclusive; keep Legacy. Planning artifact: {args.output}")
        return 3
    frozen = copy.deepcopy(plan)
    frozen.update(phase="confirmation", repeats=planning["selected_n"], candidate=args.candidate,
                  campaign_plan_sha256=plan["plan_sha256"], pilot_analysis_sha256=campaign.digest(analysis),
                  planning=planning)
    policy = analysis["configuration"].get("prompt_cache_policies", {})
    selected = {route: args.candidate + ("-explicit" if policy.get(route) == "explicit" else "")
                for route in campaign.ARMS}
    frozen["candidate_arms"] = selected
    for route, settings in frozen["routes"].items():
        originals = {trial["key"]: trial for trial in settings["trials"] if trial["repeat"] == 1
                     and trial["label"] in ("legacy", selected[route])}
        if len(originals) != len(settings["task_ids"]) * 2:
            raise campaign.CampaignRefusal("pilot lacks the selected candidate for every required task")
        settings["trials"] = []
        for repeat in range(1, frozen["repeats"] + 1):
            for trial in originals.values():
                value = dict(trial, repeat=repeat,
                             key=evaluation.run_key(trial["task"], settings["comparison"], trial["arm"], repeat))
                settings["trials"].append(value)
        settings["planned_runs"] = len(settings["trials"])
    frozen.pop("plan_sha256")
    frozen["plan_sha256"] = campaign.digest(frozen)
    campaign.write_json(args.output, frozen, exclusive=True)
    print(f"Confirmation frozen: {frozen['repeats']} repeats; complete-phase budget admission still required.")
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    plan = commands.add_parser("plan", help="freeze a local pilot plan; no network or paid work")
    plan.add_argument("--config", type=Path, default=campaign.DEFAULT_CONFIG)
    plan.add_argument("--corpus", type=Path, default=evaluation.EVALUATION / "corpus.toml")
    plan.add_argument("--output", type=Path, required=True)
    run = commands.add_parser("run", help="run a complete approved phase inside a shared persisted budget")
    run.add_argument("--plan", type=Path, required=True)
    run.add_argument("--execution-manifest", type=Path, required=True)
    run.add_argument("--corpus", type=Path, default=evaluation.EVALUATION / "corpus.toml")
    run.add_argument("--state", type=Path, required=True)
    run.add_argument("--output-dir", type=Path, required=True)
    report = commands.add_parser("report", help="import evidence and write Markdown/JSON without provider calls")
    report.add_argument("--plan", type=Path, required=True)
    report.add_argument("--corpus", type=Path, default=evaluation.EVALUATION / "corpus.toml")
    report.add_argument("--result", action="append", default=[], help="automatic=PATH or explicit=PATH")
    report.add_argument("--export", action="append", default=[], help="matching Tenant-scoped evidence export")
    report.add_argument("--analysis-config", type=Path)
    report.add_argument("--execution-manifest", type=Path,
                        help="predispatch approved execution manifest for derived statistical cost bounds")
    report.add_argument("--retained-evidence", action="append", type=Path, default=[],
                        help="descriptor with frozen phase plan, route results and matching exports")
    report.add_argument("--candidate", choices=("ordered", "native"))
    report.add_argument("--explicit-cache", choices=("off", "explicit"), default="off")
    report.add_argument("--output", type=Path, required=True)
    report.add_argument("--markdown", type=Path, required=True)
    freeze = commands.add_parser("freeze", help="freeze candidate/sample size using pilot evidence only")
    freeze.add_argument("--plan", type=Path, required=True)
    freeze.add_argument("--pilot-analysis", type=Path, required=True)
    freeze.add_argument("--candidate", choices=("ordered", "native"), required=True)
    freeze.add_argument("--output", type=Path, required=True)
    deployment = commands.add_parser("deployment-plan", help="freeze fresh trials with the intended deployment estimator")
    deployment.add_argument("--plan", type=Path, required=True)
    deployment.add_argument("--estimator-snapshot", type=Path, required=True)
    deployment.add_argument("--output", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        if args.command == "plan":
            return plan_command(args)
        if args.command == "report":
            return report_command(args)
        if args.command == "freeze":
            return freeze_command(args)
        if args.command == "deployment-plan":
            frozen = campaign.deployment_plan(campaign.read_json(args.plan),
                                               campaign.read_json(args.estimator_snapshot))
            campaign.write_json(args.output, frozen, exclusive=True)
            print(f"Deployment confirmation frozen: {args.output}; paid execution still requires admission.")
            return 0
        campaign.run_phase(campaign.read_json(args.plan), campaign.read_json(args.execution_manifest),
                           state_path=args.state, corpus_path=args.corpus, output_dir=args.output_dir)
        return 0
    except (campaign.CampaignRefusal, evaluation.GuardRefusal, evaluation.CorpusError, ValueError,
            OSError, KeyError, TypeError) as error:
        print(f"projection campaign refused: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
