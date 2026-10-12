#!/usr/bin/env python3
"""Fixed-task quality and cost evaluation for harness changes (#122).

Subcommands:

  deterministic  Run the whole corpus against a disposable local Node, PostgreSQL
                 and NATS with the scripted fixture provider. Needs Docker and a
                 Cargo build; needs no credentials and makes no paid calls.
  live           Opt-in measurement against an operator-chosen Node and model.
                 Refuses to start without an explicitly connected Tenant Provider
                 Credential, a disclosure approval and hard spend/time limits.
  validate       Validate an existing result manifest.

Results are measurements. They approve no accuracy, cost or rollout claim, and
conclusions are limited to the tested tasks and models. See docs/evaluation.md.
"""
import argparse
import concurrent.futures
import contextlib
import datetime
import json
import os
from pathlib import Path
import re
import secrets
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

sys.path.insert(0, str(Path(__file__).resolve().parent))
import fixed_task_evaluation as evaluation  # noqa: E402
from fixed_task_fixture import Fixture  # noqa: E402
import fixed_task_runner as runner  # noqa: E402

ROOT = evaluation.ROOT
POSTGRES_IMAGE = "aidash-postgres:17-pg-jsonschema-0.3.4"
NATS_IMAGE = "nats:2.12-alpine"
DEFAULT_OUTPUT = ROOT / ".ignore/evaluation"
# Emergency cleanups for SIGTERM, which may arrive while worker threads block the
# main thread: remove owned containers and stop the owned Node, then exit.
CLEANUP = []
DETERMINISTIC_LIMITATIONS = [
    "The deterministic tier's Agent is a scripted probe; outcomes measure what the harness delivered to it, "
    "not model quality.",
    "Token counts, cost and prompt-cache hits are synthetic fixture values (4 bytes per token, fixture prices, "
    "a simulated prefix cache); they test accounting and request shape, not provider prices.",
    "The Jev compaction classifier is a fixture that drops every prunable tool result.",
    "Summary recovery (#113) and the other feature-not-available comparisons are reported, not measured.",
]
LIVE_LIMITATIONS = [
    "Provider model aliases and routes can change upstream without notice; the provider-reported identity "
    "recorded per Usage Record is the only evidence of what served a request.",
    "Live outputs are restricted content: the manifest keeps digests and verifier results, not answers.",
    "Spend is enforced from conservative per-attempt reservations; an attempt with unknown cost stays charged "
    "at its full reservation.",
]


def now():
    return datetime.datetime.now(datetime.UTC).strftime("%Y-%m-%dT%H:%M:%S.%fZ")


def http_transport(base, timeout=180):
    def call(method, path, body, token):
        request = urllib.request.Request(base.rstrip("/") + path, method=method,
                                         data=None if body is None else json.dumps(body).encode(),
                                         headers={"Authorization": f"Bearer {token}",
                                                  "Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                raw = response.read()
                return response.status, json.loads(raw) if raw else None
        except urllib.error.HTTPError as error:
            detail = error.read().decode(errors="replace")[:2000]
            try:
                return error.code, json.loads(detail)
            except ValueError:
                return error.code, detail
    return call


def checked(command, **kwargs):
    return subprocess.run(command, check=True, capture_output=True, text=True, **kwargs).stdout


def free_port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


@contextlib.contextmanager
def container(name, image, arguments, environment=None):
    """A disposable container owned by this invocation; removed even on failure."""
    command = ["docker", "run", "-d", "--rm", "--name", name, "--label", "aidash.evaluation=fixed-task"]
    for key, value in (environment or {}).items():
        command += ["-e", f"{key}={value}"]
    checked(command + arguments[0] + [image] + arguments[1])
    CLEANUP.append(lambda: subprocess.run(["docker", "rm", "-f", name], capture_output=True))
    try:
        yield name
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True)


def published_port(name, port):
    mapping = checked(["docker", "port", name, f"{port}/tcp"]).splitlines()[0]
    return int(mapping.rsplit(":", 1)[1])


def wait(predicate, timeout, label):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if predicate():
                return
        except (OSError, subprocess.CalledProcessError, urllib.error.URLError):
            pass
        time.sleep(0.3)
    raise RuntimeError(f"timed out waiting for {label}")


def build_manage():
    """Build the management binary and return Cargo's own artifact path for this worktree."""
    output = subprocess.run(["cargo", "build", "--locked", "-p", "aidash-server", "--bin", "manage",
                             "--message-format=json"], cwd=ROOT, check=True, capture_output=True, text=True,
                            ).stdout
    for line in output.splitlines():
        item = json.loads(line)
        if item.get("reason") == "compiler-artifact" and item["target"]["name"] == "manage" and item.get("executable"):
            return Path(item["executable"])
    raise RuntimeError("cargo did not report the manage executable")


@contextlib.contextmanager
def node(binary, environment, log_path):
    with open(log_path, "wb") as log:
        child = subprocess.Popen([str(binary), "serve"], cwd=ROOT, env=environment, stdout=log, stderr=log)
        CLEANUP.insert(0, child.kill)
        try:
            yield child
        finally:
            if child.poll() is None:
                child.send_signal(signal.SIGTERM)
                try:
                    child.wait(timeout=30)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=10)


def settings_directory(private, cache_salt):
    settings = private / "settings"
    settings.mkdir(mode=0o700)
    template = (ROOT / "server/settings/base.example.toml").read_text().replace(
        "allowed_hosts = []", 'allowed_hosts = ["127.0.0.1", "localhost"]')
    template += f'\n[cache_salt]\ncurrent = 1\n\n[[cache_salt.keys]]\nversion = 1\nsecret = "{cache_salt}"\n'
    (settings / "base.toml").write_text(template)
    (settings / "base.toml").chmod(0o600)
    (settings / "local.toml").write_text("")
    return settings


def usage_reader_for(postgres, database, password, query):
    def read(run_id):
        if not re.fullmatch(r"[0-9a-f-]{36}", run_id):
            raise ValueError("invalid Run id")
        output = checked(["docker", "exec", "-i", "-e", f"PGPASSWORD={password}", postgres, "psql", "-U", "aidash",
                          "-d", database, "-tA", "-v", "ON_ERROR_STOP=1", "-v", f"run={run_id}"], input=query + ";\n")
        return [json.loads(line) for line in output.splitlines() if line.strip()]
    return read


def memory_matrix_deterministic(api, experiment, corpus, memory_corpus, model_refs, fixture_url, skip_rust):
    """Execute deterministic memory scenarios once per memory variant, each in an isolated bank."""
    executed = {}
    api_scenarios = [item for item in memory_corpus["scenarios"] if item["evidence"] == "deterministic-api"]
    roles = {"model": {"id": model_refs["standard"]["id"], "version": "1.0.0"}}
    try:
        embedding_config = {"provider": "openai", "endpoint": fixture_url + "/v1", "credential_env": None,
                            "model": f"{experiment.prefix}-embedding", "model_version": "1", "dimensions": 8}
        embedding = experiment.register("embedding", f"{experiment.prefix}-embedding", embedding_config)
        roles["embedding"] = {"id": embedding["id"], "version": "1.0.0"}
        for kind, identifier, config in (("reranker", "reranker", {"provider": "rrf"}),
                                         ("tokenizer", "tokenizer", {"provider": "utf8_upper_bound"})):
            registered = experiment.register(kind, f"{experiment.prefix}-{identifier}", config)
            roles[kind] = {"id": registered["id"], "version": "1.0.0"}
        variants = []
        for variant in memory_corpus["variants"]:
            provider = experiment.register("memory", f"{experiment.prefix}-memory-{variant['id']}",
                                           runner.memory_policy(roles, variant))
            agent = experiment.register("agent", f"{experiment.prefix}-memory-agent-{variant['id']}", {
                "schema_version": 1, "model": roles["model"], "instructions": "Memory scenario participant.",
                "bindings": [{"kind": "memory", "target": experiment.ref(provider["id"]), "narrow": {}}],
                "remove_default": ["file_search", "file_read"]})
            variants.append((variant, provider, agent))
    except runner.ApiError as error:
        detail = f"native memory setup failed: {str(error)[:300]}"
        return {item["id"]: {"status": "error", "variants": [], "checks": [], "detail": detail} for item in api_scenarios}
    for scenario in api_scenarios:
        results = []
        checks = []
        for variant, provider, agent in variants:
            try:
                workspace = api.post("/api/workspaces", {"title": f"{experiment.prefix} memory {scenario['id']} "
                                                                  f"{variant['id']}", "goal": "Memory scenario"},
                                     experiment.token)["id"]
                # Memory banks require an enabled Workspace index (operator-configured); its embeddings
                # come from the fixture.
                api.post(f"/api/workspaces/{workspace}/semantic/index", {"expected_revision": 0, "spec": {
                    "embedding": embedding_config, "vector": {"provider": "postgres", "endpoint": "local",
                                                              "credential_env": None},
                    "enabled": True, "auto_context": False, "max_sources": 100, "max_results": 10,
                    "max_result_tokens": 32768, "max_input_bytes": 32768}})
                participant = api.post(f"/api/workspaces/{workspace}/memory/participants",
                                       {"agent": {"id": agent["id"], "version": "1.0.0"}}, experiment.token)
                provider_ref = {"id": provider["id"], "version": "1.0.0"}
                variant_checks, seed = runner.run_memory_scenario(api, experiment.token, workspace, provider_ref,
                                                                  participant["bank"], scenario)
                results.append({"variant": variant["id"], "workspace": workspace, "bank": participant["bank"],
                                "seed_sha256": seed, "passed": all(item["passed"] for item in variant_checks)})
                checks += [dict(item, variant=variant["id"]) for item in variant_checks]
            except Exception as error:  # Recorded as an error for this scenario, never as passing.
                results.append({"variant": variant["id"], "error": f"{type(error).__name__}: {error}"[:400]})
        isolated = len({json.dumps(item.get("bank"), sort_keys=True) for item in results}) == len(results)
        same_seed = len({item.get("seed_sha256") for item in results}) == 1
        checks.append({"check": "isolated-equivalent-seeds", "passed": isolated and same_seed})
        failed = any("error" in item for item in results)
        status = "error" if failed else "passed" if all(item["passed"] for item in checks) else "failed"
        executed[scenario["id"]] = {"status": status, "variants": results, "checks": checks, "detail": None}
    for scenario in memory_corpus["scenarios"]:
        if scenario["evidence"] != "deterministic-test" or skip_rust:
            continue
        completed = subprocess.run(scenario["command"], cwd=ROOT, capture_output=True, text=True,
                                   env={**os.environ, "RUSTC_WRAPPER": ""})
        tail = (completed.stdout + completed.stderr).strip().splitlines()[-2:]
        executed[scenario["id"]] = {"status": "passed" if completed.returncode == 0 else "failed", "variants": [],
                                    "checks": [{"check": "command", "passed": completed.returncode == 0,
                                                "command": " ".join(scenario["command"]), "tail": tail}],
                                    "detail": None}
    return executed


def experiment_manifest(tier, corpus, memory_corpus, source, model, harness, variants, repeats, limits, limitations):
    return {"schema_version": 1, "experiment_id": "fte-" + secrets.token_hex(6), "tier": tier, "created_at": now(),
            "aidash": source, "corpus": corpus["_snapshot"], "memory_corpus": memory_corpus["_snapshot"],
            "evaluator": evaluation.evaluator_identity(), "model": model, "harness": harness, "variants": variants,
            "repeat_count": repeats, "limits": limits, "reproducibility_limitations": limitations}


def assemble(corpus, experiment, runs, availability, memory, calibrate, spend, limitations, error):
    aggregates = evaluation.aggregate(runs) if runs else []
    result = {"schema_version": 1, "grade": "measurement-only",
              "approvals": {"accuracy_claim": False, "cost_claim": False, "production_quality": False,
                            "default_rollout": False},
              "conclusion_scope": evaluation.CONCLUSION_SCOPE, "experiment": experiment, "runs": runs,
              "aggregates": aggregates, "comparisons": evaluation.compare(corpus, aggregates, runs),
              "feature_availability": availability, "memory_matrix": memory,
              "calibration": {"applied": calibrate, "failures": evaluation.calibrate(corpus, runs) if calibrate else []},
              "spend": spend, "limitations": limitations, "error": error}
    return result


def deterministic(args):
    corpus = evaluation.load_corpus(args.corpus)
    memory_corpus = evaluation.load_memory_corpus(args.memory_corpus)
    for index, task in enumerate(corpus["tasks"]):
        task["_index"] = index
    planned_runs = evaluation.plan(corpus, args.task, args.comparison)
    for planned in planned_runs:
        planned["corpus"] = corpus
    checked(["docker", "info", "--format", "{{.ServerVersion}}"])
    if subprocess.run(["docker", "image", "inspect", POSTGRES_IMAGE], capture_output=True).returncode != 0:
        # The PostgreSQL image carries the native extensions; build it from compose.yaml.
        checked(["docker", "compose", "build", "postgres"], cwd=ROOT)
    binary = Path(args.binary).resolve(strict=True) if args.binary else build_manage()
    query = json.loads(checked([str(binary), "diagnostics", "evaluation"], cwd=ROOT / "server",
                               env={"PATH": os.environ["PATH"], "TOKIO_WORKER_THREADS": "2"}))["inference_usage"]
    source = evaluation.source_identity(binary)
    tag = secrets.token_hex(6)
    prefix = "fte" + tag[:6]
    password = secrets.token_urlsafe(24)
    output = Path(args.output) if args.output else DEFAULT_OUTPUT / f"deterministic-{tag}.json"
    log_directory = DEFAULT_OUTPUT / f"logs-{tag}"
    log_directory.mkdir(parents=True, mode=0o700, exist_ok=True)
    model_ids = runner.fixture_model_ids(corpus, prefix)
    fixture = Fixture({model_ids[name]: spec["context_window"] for name, spec in corpus["models"].items()})
    server = fixture.serve({task["id"]: task for task in corpus["tasks"]})
    fixture_url = f"http://127.0.0.1:{server.server_port}"
    runs, variants, availability, memory, error = [], [], evaluation.feature_availability(corpus, args.comparison), None, None
    memory = evaluation.memory_matrix(memory_corpus, {})
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="aidash-fixed-task-") as private_path, contextlib.ExitStack() as stack:
        private = Path(private_path)
        postgres = stack.enter_context(container(f"aidash-fte-{tag}-pg", POSTGRES_IMAGE, (
            ["-p", "127.0.0.1::5432"], ["postgres", "-c", "max_connections=200"]),
            {"POSTGRES_USER": "aidash", "POSTGRES_PASSWORD": password, "POSTGRES_DB": "aidash_evaluation"}))
        nats = stack.enter_context(container(f"aidash-fte-{tag}-nats", NATS_IMAGE, (["-p", "127.0.0.1::4222"], ["-js"])))
        wait(lambda: subprocess.run(["docker", "exec", postgres, "pg_isready", "-h", "127.0.0.1", "-U", "aidash",
                                     "-d", "aidash_evaluation"], capture_output=True).returncode == 0, 120, "PostgreSQL")
        print("evaluation: disposable PostgreSQL and NATS ready", flush=True)
        port = free_port()
        base = f"http://127.0.0.1:{port}"
        operator = secrets.token_urlsafe(32)
        node_id = f"aidash://fixed-task-{tag}"
        web = private / "web"
        web.mkdir()
        (web / "index.html").write_text("<!doctype html><title>evaluation</title>\n")
        environment = {key: value for key, value in os.environ.items() if not key.startswith(("AIDASH_", "REINHARDT_"))}
        environment.update({
            "REINHARDT_ENV": "local", "REINHARDT_SETTINGS_DIR": str(settings_directory(private, secrets.token_hex(32))),
            "DATABASE_URL": f"postgres://aidash:{urllib.parse.quote(password, safe='')}@127.0.0.1:"
                            f"{published_port(postgres, 5432)}/aidash_evaluation",
            "NATS_URL": f"nats://127.0.0.1:{published_port(nats, 4222)}", "AIDASH_NODE_ID": node_id,
            "AIDASH_ENDPOINT": base, "AIDASH_LISTEN": f"127.0.0.1:{port}", "AIDASH_API_TOKEN": operator,
            "AIDASH_API_RATE_BURST": "100000", "AIDASH_WEB_DIR": str(web),
            # A loaded host can exceed the default request deadline on reads that poll Runs.
            "AIDASH_HTTP_TIMEOUT_SECONDS": "120",
            # This disposable Node ID owns its JetStream streams and consumers.
            "AIDASH_ACTIVATION_BOOTSTRAP": "true",
            "AIDASH_JEV_ENDPOINT": fixture_url + "/jev", "AIDASH_SECRET_JEV": "fixture-classifier-not-a-secret",
            "NO_PROXY": "127.0.0.1,localhost", "no_proxy": "127.0.0.1,localhost"})
        # Native memory needs its independent recovery ledger, initialized once after the
        # schema exists and before any bank is created (docs/memory-recovery.md).
        recovery = private / "memory-recovery"
        environment["AIDASH_MEMORY_RECOVERY_DIR"] = str(recovery)
        with open(log_directory / "setup.log", "wb") as setup:
            for command in (["migrate"], ["memory-recovery", "init-if-missing", "--directory", str(recovery)]):
                subprocess.run([str(binary), *command], cwd=ROOT, env=environment, stdout=setup, stderr=setup,
                               check=True, timeout=300)
        child = stack.enter_context(node(binary, environment, log_directory / "server.log"))

        def healthy():
            if child.poll() is not None:
                raise RuntimeError(f"Aidash exited during startup with code {child.returncode}; see {log_directory}")
            with urllib.request.urlopen(base + "/health", timeout=2) as response:
                return response.status == 200
        wait(healthy, 180, "Aidash health")
        print(f"evaluation: Node healthy; {len(planned_runs) * args.repeats} Runs planned", flush=True)
        api = runner.Api(http_transport(base), operator)
        experiment = runner.Experiment(api, node_id, "evaluation", prefix)
        try:
            agent_ids = [runner.agent_id(prefix, planned) for planned in planned_runs]
            agent_ids += [f"{prefix}-memory-agent-{variant['id']}" for variant in memory_corpus["variants"]]
            experiment.authorize(agent_ids)
            model_refs = {}
            for name, spec in corpus["models"].items():
                config = runner.model_config(dict(spec, model_id=model_ids[name]), fixture_url, name)
                model_refs[name] = dict(experiment.register("model", f"{prefix}-model-{name}", config),
                                        model_id=model_ids[name])
            variants, registration_errors = runner.register_variants(experiment, corpus, planned_runs, model_refs,
                                                                     fixture_url)
            availability = evaluation.feature_availability(corpus, args.comparison, registration_errors)
            usage = usage_reader_for(postgres, "aidash_evaluation", password, query)
            # Runs are independent (own Workspace, Task and fixture probe state), so up to
            # --parallel of them share the Node's worker slots. Results keep plan order.
            jobs = [(repeat, planned) for repeat in range(1, args.repeats + 1) for planned in planned_runs
                    if "agent_ref" in planned]

            def execute(job):
                repeat, planned = job
                observation = runner.run_task(api, experiment, planned, repeat, planned["agent_ref"], fixture,
                                              usage, args.run_timeout, approve_fixture_tools=True)
                result = evaluation.run_result(corpus, planned, repeat, observation, "deterministic", True)
                detail = f" ({result['error'][:200]})" if result["error"] else ""
                print(f"{result['key']}: {result['outcome']}{detail}", flush=True)
                return result
            with concurrent.futures.ThreadPoolExecutor(max_workers=args.parallel) as pool:
                runs = list(pool.map(execute, jobs))
            if not args.skip_memory:
                print("evaluation: Runs finished; executing memory scenarios", flush=True)
                executed = memory_matrix_deterministic(api, experiment, corpus, memory_corpus, model_refs,
                                                       fixture_url, args.skip_rust_evidence)
                memory = evaluation.memory_matrix(memory_corpus, executed)
        except Exception as failure:
            error = f"{type(failure).__name__}: {failure}"
        if fixture.errors:
            error = (error + "; " if error else "") + "fixture faults: " + "; ".join(fixture.errors[:5])
    server.shutdown()
    model = {"kind": "scripted-fixture", "registrations": {name: ref for name, ref in locals().get("model_refs", {}).items()},
             "reported_identities": sorted(model_ids.values()), "route": "local scripted fixture", "mutable_alias": False}
    harness = {"compaction": "legacy prune-only Context Policy with fixture Jev classifier", "cache_salt": True,
               "provider_credential": None, "disclosure_approval": None, "jev_classifier": "fixture: drop all prunable"}
    limits = {"run_timeout_seconds": args.run_timeout, "wall_seconds": max(1.0, time.monotonic() - started),
              "max_output_tokens": max(spec["max_output_tokens"] for spec in corpus["models"].values()),
              "spend_ceiling_nanocredits": None}
    if error is None and len(runs) != len(variants) * args.repeats:
        error = "not every planned Run was recorded"
    experiment_value = experiment_manifest("deterministic", corpus, memory_corpus, source, model, harness, variants,
                                           args.repeats, limits, DETERMINISTIC_LIMITATIONS)
    result = assemble(corpus, experiment_value, runs, availability, memory, True, None,
                      DETERMINISTIC_LIMITATIONS, error)
    evaluation.validate_result(result)
    evaluation.write_exclusive(output, result)
    print(evaluation.summary_table(result))
    print(f"Result manifest: {output}")
    failures = result["calibration"]["failures"]
    memory_failures = [item["id"] for item in result["memory_matrix"]["scenarios"] if item["status"] in ("failed", "error")]
    if error or failures or memory_failures:
        for line in failures:
            print("calibration: " + line, file=sys.stderr)
        for line in memory_failures:
            print("memory scenario failed: " + line, file=sys.stderr)
        if error:
            print("error: " + error, file=sys.stderr)
        return 1
    return 0


def live(args, read_json=None, transport=None, clock=time.monotonic, sleep=time.sleep, wall_now=now):
    """Opt-in live tier. Every refusal happens before any registration, Run or provider work."""
    corpus = evaluation.load_corpus(args.corpus)
    memory_corpus = evaluation.load_memory_corpus(args.memory_corpus)
    for index, task in enumerate(corpus["tasks"]):
        task["_index"] = index
    read_json = read_json or (lambda path: json.loads(Path(path).read_text()))
    operator = os.environ.get(args.token_env)
    subject = os.environ.get(args.subject_token_env)
    if not operator or not subject:
        raise evaluation.GuardRefusal(f"{args.token_env} (operator) and {args.subject_token_env} (Tenant subject) "
                                      "must both be set")
    if not args.provider_credential:
        raise evaluation.GuardRefusal("an explicitly connected Tenant Provider Credential is required")
    mapping = {}
    for item in args.model:
        name, _, reference = item.partition("=")
        if name not in corpus["models"] or "@" not in reference:
            raise evaluation.GuardRefusal(f"--model must be <corpus model>=<registry id>@<version>, got {item!r}")
        mapping[name] = reference
    if not mapping:
        raise evaluation.GuardRefusal("at least one --model mapping is required")
    api = runner.Api(transport or http_transport(args.base_url), operator)
    credential = api.get(f"/api/tenants/{args.tenant}/provider-credentials/{args.provider_credential}")
    models = {}
    for reference in sorted(set(mapping.values())):
        identifier, _, version = reference.partition("@")
        models[reference] = api.get(f"/api/registry/{identifier}/{version}")["config"]
    approval = read_json(args.disclosure_approval) if args.disclosure_approval else None
    evaluation.require_live_authorization(credential, approval, corpus["_snapshot"]["sha256"], models,
                                          args.max_spend_nanocredits, args.max_wall_seconds, wall_now())
    planned_runs = [planned for planned in evaluation.plan(corpus, args.task, args.comparison)
                    if planned["model"] in mapping]
    if not args.allow_oracle_code_execution and any(p["task"]["oracle"]["kind"] == "python-tests" for p in planned_runs):
        raise evaluation.GuardRefusal("code-execution oracles run model-written code locally; pass "
                                      "--allow-oracle-code-execution or select tasks without them")
    if not planned_runs:
        raise evaluation.GuardRefusal("no planned Run uses a mapped model")
    ledger = evaluation.SpendLedger(args.max_spend_nanocredits, args.max_wall_seconds, clock)
    identity = api.get("/.well-known/aidash")
    prefix = "fte" + secrets.token_hex(3)
    experiment = runner.Experiment(api, identity["id"], args.tenant, prefix)
    experiment.token = subject
    model_refs = {}
    for name, reference in mapping.items():
        identifier, _, version = reference.partition("@")
        model_refs[name] = {"id": identifier, "version": version, "config_sha256":
                            evaluation.canonical_sha256(models[reference]), "model_id": models[reference]["model_id"]}
    for planned in planned_runs:
        planned["corpus"] = corpus
        planned["live_model"] = mapping[planned["model"]]
    # Fixture tools point at an unroutable endpoint; their approvals are denied by
    # default, so a live Run never reaches an external tool effect.
    variants, registration_errors = runner.register_variants(experiment, corpus, planned_runs, model_refs,
                                                             args.tool_endpoint)
    prices = approval["prices"]
    results = runner.live_runs(api, experiment, corpus, planned_runs, args.repeats, ledger, models, prices,
                               args.run_timeout, clock, sleep)
    runs = [evaluation.run_result(corpus, planned, repeat, observation, "live", args.allow_oracle_code_execution)
            for planned, repeat, observation in results]
    source = evaluation.source_identity(server_revision=args.server_revision)
    source.update({"commit": None, "dirty": None, "dirty_diff_sha256": None})
    model = {"kind": "live-provider", "registrations": model_refs,
             "reported_identities": sorted({str(work.get("model")) for attempt in ledger.attempts
                                            for work in attempt.get("provider_work", []) if work.get("model")}),
             "route": credential.get("provider"), "mutable_alias": True}
    harness = {"compaction": "server-configured", "cache_salt": True, "provider_credential": args.provider_credential,
               "disclosure_approval": evaluation.canonical_sha256(approval), "jev_classifier": "server-configured"}
    limits = {"run_timeout_seconds": args.run_timeout, "wall_seconds": args.max_wall_seconds,
              "max_output_tokens": max(config["max_output_tokens"] for config in models.values()),
              "spend_ceiling_nanocredits": args.max_spend_nanocredits}
    experiment_value = experiment_manifest("live", corpus, memory_corpus, source, model, harness, variants,
                                           args.repeats, limits, LIVE_LIMITATIONS)
    availability = evaluation.feature_availability(corpus, args.comparison, registration_errors)
    memory = evaluation.memory_matrix(memory_corpus, {})
    result = assemble(corpus, experiment_value, runs, availability, memory, False, ledger.summary(),
                      LIVE_LIMITATIONS, None)
    evaluation.validate_result(result)
    output = Path(args.output) if args.output else DEFAULT_OUTPUT / f"live-{prefix}.json"
    evaluation.write_exclusive(output, result)
    print(evaluation.summary_table(result))
    print(f"Result manifest: {output}")
    return 0 if ledger.refusal is None else 3


def validate_command(args):
    result = json.loads(Path(args.manifest).read_text())
    evaluation.validate_result(result)
    print(f"valid: {args.manifest}")
    return 0


def terminate(*_):
    """SIGTERM: stop the owned Node and remove owned containers before exiting."""
    for cleanup in CLEANUP:
        try:
            cleanup()
        except Exception:  # Best effort; every remaining cleanup still runs.
            pass
    print("evaluation: terminated; owned containers and Node removed", file=sys.stderr, flush=True)
    os._exit(143)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("deterministic", "live"):
        command = commands.add_parser(name)
        command.add_argument("--corpus", type=Path, default=evaluation.EVALUATION / "corpus.toml")
        command.add_argument("--memory-corpus", type=Path, default=evaluation.EVALUATION / "memory.toml")
        command.add_argument("--task", action="append", help="limit to this task id (repeatable)")
        command.add_argument("--comparison", action="append", help="limit to this comparison id (repeatable)")
        command.add_argument("--repeats", type=int, default=1 if name == "deterministic" else 3,
                             help="Runs per task arm (default 1 deterministic, 3 live)")
        command.add_argument("--run-timeout", type=float, default=180.0)
        command.add_argument("--output", type=Path)
    deterministic_command = commands.choices["deterministic"]
    deterministic_command.add_argument("--parallel", type=int, default=4,
                                       help="concurrent Runs; the Node has four worker slots")
    deterministic_command.add_argument("--binary", help="prebuilt manage executable; default builds this worktree")
    deterministic_command.add_argument("--skip-memory", action="store_true",
                                       help="report every memory scenario as not-run (for narrowed task runs)")
    deterministic_command.add_argument("--skip-rust-evidence", action="store_true",
                                       help="do not run deterministic-test memory scenarios")
    live_command = commands.choices["live"]
    live_command.add_argument("--base-url", required=True)
    live_command.add_argument("--tenant", required=True)
    live_command.add_argument("--token-env", default="AIDASH_EVALUATION_TOKEN", help="operator bearer token variable")
    live_command.add_argument("--subject-token-env", default="AIDASH_EVALUATION_SUBJECT_TOKEN",
                              help="Tenant subject bearer token variable used to create and run Tasks")
    live_command.add_argument("--server-revision", help="deployed source fingerprint of the evaluated Node")
    live_command.add_argument("--tool-endpoint", default="http://127.0.0.1:9/aidash-fixed-task-evaluation",
                              help="endpoint declared by fixture tools; approvals for them are denied")
    live_command.add_argument("--provider-credential", help="connected Tenant Provider Credential ID")
    live_command.add_argument("--model", action="append", default=[],
                              help="corpus model name=Registry model id@version (repeatable)")
    live_command.add_argument("--disclosure-approval", type=Path)
    live_command.add_argument("--max-spend-nanocredits", type=int)
    live_command.add_argument("--max-wall-seconds", type=float)
    live_command.add_argument("--allow-oracle-code-execution", action="store_true")
    validate_parser = commands.add_parser("validate")
    validate_parser.add_argument("manifest", type=Path)
    args = parser.parse_args(argv)
    if args.command == "validate":
        return validate_command(args)
    if not 1 <= args.repeats <= 64:
        parser.error("--repeats must be between 1 and 64")
    if args.command == "deterministic":
        signal.signal(signal.SIGTERM, terminate)
        return deterministic(args)
    try:
        return live(args)
    except evaluation.GuardRefusal as refusal:
        print(f"live evaluation refused: {refusal}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
