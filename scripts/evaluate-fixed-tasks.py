#!/usr/bin/env python3
"""Fixed-task quality and cost evaluation for harness changes (#122).

Subcommands:

  deterministic  Run the whole corpus against a disposable local Node, PostgreSQL
                 and NATS with the scripted fixture provider. Needs Docker and a
                 Cargo build; needs no credentials and makes no paid calls.
  live           Opt-in measurement against a Node and model the evaluator chooses.
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
import ipaddress
import json
import math
import os
from pathlib import Path
import re
import secrets
import signal
import socket
import subprocess
import sys
import tempfile
import threading
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
DEFAULT_OUTPUT = ROOT / "private/evaluation"
# A deployed source fingerprint: a git commit (abbreviated or full) or a SHA-256, lowercase hexadecimal.
SERVER_REVISION = re.compile(r"[0-9a-f]{7,64}")
# Emergency cleanups for a second deterministic SIGTERM, if the first one's unwind through
# cancellation and finalization hangs: remove owned containers and stop the owned Node, then exit.
CLEANUP = []
# Seconds one Run's Usage Record read (docker exec ... psql) may take before it counts as failed.
USAGE_READ_TIMEOUT = 60.0
DETERMINISTIC_LIMITATIONS = [
    "The deterministic tier's Agent is a scripted probe; outcomes measure what the harness delivered to it, "
    "not model quality.",
    "Token counts, cost and prompt-cache hits are synthetic fixture values (one token per byte, fixture prices, "
    "a simulated prefix cache); they test accounting and request shape, not provider prices.",
    "Ordinary Runs are not pruned until #108 binds the built-in compaction Decider (ADR 0041), so long-context "
    "tasks stop at the window instead of measuring pruning.",
    "Summary recovery (#113) and the other feature-not-available comparisons are reported, not measured.",
]
LIVE_LIMITATIONS = [
    "Provider model aliases and routes can change upstream without notice. This tier records no provider-reported "
    "model or route identity (model.reported_identities is null): Usage Records have no HTTP read endpoint and "
    "Workspace events do not carry the identity. model.route is only the connected Provider Credential's "
    "catalog provider.",
    "Live outputs are restricted content: the manifest keeps digests and verifier results, not answers.",
    "Spend is enforced from conservative per-Run worst-case reservations that cover every harness retry of every "
    "step at the dearer of the input and cache-write prices. Until Usage Records can be read over HTTP, every "
    "dispatched Run stays charged at its full reservation as uncertain, because interrupted and retried attempts "
    "emit no model.completed event; spend.attempts[].provider_work lists the accepted attempts the Node reported, "
    "for information only.",
    "Code-execution oracles require an explicitly approved pinned Docker image and run only in disposable "
    "resource-bounded containers with no network, host environment or parent process visibility.",
    "Auxiliary definitions can describe planned Decider/Summary work, but live dispatch is refused until an "
    "enforced provider/broker monetary bound exists. A declared reservation is not a provider cap "
    "or a reported charge; recovery diagnostics remain not measured in this live tier.",
]


def utc_now():
    """The evaluator's wall clock: an aware UTC datetime. Injectable in live()."""
    return datetime.datetime.now(datetime.UTC)


def timestamp(moment):
    return moment.astimezone(datetime.UTC).strftime("%Y-%m-%dT%H:%M:%S.%fZ")


class Terminated(BaseException):
    """SIGTERM, raised so a run unwinds through cancellation, cleanup and finalization."""


class Cancelled(BaseException):
    """Raised inside a deterministic worker's Run after Ctrl-C, so the Run is cancelled and recorded."""


def raise_terminated(*_):
    # One unwind only: a second SIGTERM must not interrupt writing the manifest.
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    raise Terminated()


def raise_terminated_deterministic(*_):
    # The first SIGTERM unwinds: Runs are cancelled, the Node and containers removed and the
    # manifest written. Should that hang, a second SIGTERM falls back to the emergency cleanup.
    signal.signal(signal.SIGTERM, terminate)
    raise Terminated()


def require_https(base_url):
    """Bearer tokens travel only over HTTPS, or plain HTTP to a loopback test or development Node."""
    parsed = urllib.parse.urlsplit(base_url)
    if parsed.scheme == "https" and parsed.hostname:
        return
    if parsed.scheme == "http" and parsed.hostname:
        if parsed.hostname == "localhost":
            return
        try:
            if ipaddress.ip_address(parsed.hostname).is_loopback:
                return
        except ValueError:
            pass
    raise evaluation.GuardRefusal(f"--base-url must be https:// (plain http:// only for a loopback Node), got "
                                  f"{base_url!r}; the Deployment Administrator and Tenant subject tokens would travel unencrypted")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """Refuse every redirect: urllib would resend the bearer token to the new location, which
    require_https never checked. The 30x response is returned to the caller as a failure."""

    def redirect_request(self, *_):
        return None


def http_transport(base):
    handlers = [NoRedirect]
    if urllib.parse.urlsplit(base).scheme == "http":
        # require_https allows plain HTTP only to loopback; an environment proxy (HTTP_PROXY)
        # would receive both bearer tokens in plaintext instead, so plain HTTP never uses one.
        handlers.append(urllib.request.ProxyHandler({}))
    opener = urllib.request.build_opener(*handlers)

    def call(method, path, body, token, timeout):
        request = urllib.request.Request(base.rstrip("/") + path, method=method,
                                         data=None if body is None else json.dumps(body).encode(),
                                         headers={"Authorization": f"Bearer {token}",
                                                  "Content-Type": "application/json"})
        try:
            with opener.open(request, timeout=timeout) as response:
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
    """Run a command; a failure or timeout never carries database passwords from its arguments."""
    redacted = [evaluation.redact_secrets(part) for part in command]
    try:
        return subprocess.run(command, check=True, capture_output=True, text=True, **kwargs).stdout
    except subprocess.CalledProcessError as error:
        raise subprocess.CalledProcessError(error.returncode, redacted, error.stdout, error.stderr) from None
    except subprocess.TimeoutExpired as error:
        raise subprocess.TimeoutExpired(redacted, error.timeout) from None


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
    """Read one Run's Usage Records from the disposable database.

    The query selects every record of this invocation's own database, so it
    needs no SQL parameter; the Run is selected here by its identifier.
    """
    def read(run_id):
        try:
            output = checked(["docker", "exec", "-i", "-e", f"PGPASSWORD={password}", postgres, "psql", "-U", "aidash",
                              "-d", database, "-tA", "-v", "ON_ERROR_STOP=1"], input=query + ";\n",
                             timeout=USAGE_READ_TIMEOUT)
        except subprocess.CalledProcessError as error:
            # The message lands in runs[].error: keep the exit status and a redacted stderr tail only.
            tail = " | ".join((error.stderr or "").strip().splitlines()[-3:])
            raise RuntimeError("usage record read failed (exit "
                               f"{error.returncode}): {evaluation.redact_secrets(tail, password)[:300]}") from None
        except subprocess.TimeoutExpired:
            # A stalled Docker or PostgreSQL must not hold the worker forever; the Run records the failure.
            raise RuntimeError(f"usage record read timed out after {USAGE_READ_TIMEOUT}s") from None
        records = (json.loads(line) for line in output.splitlines() if line.strip())
        return [record for record in records if record["run_id"] == run_id]
    return read


def memory_matrix_deterministic(api, experiment, corpus, memory_corpus, model_refs, fixture_url, skip_rust,
                                evidence_timeout, source_drift):
    """Execute deterministic memory scenarios once per memory variant, each in an isolated bank."""
    executed = {}
    api_scenarios = [item for item in memory_corpus["scenarios"] if item["evidence"] == "deterministic-api"]
    roles = {"model": {"id": model_refs["standard"]["id"], "version": "1.0.0"}}
    try:
        embedding_config = {"provider": "openai", "endpoint": fixture_url + "/v1",
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
                # Memory banks require an enabled Workspace index (enabled per Workspace); its embeddings
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
    if not skip_rust:
        executed.update(run_evidence(memory_corpus, evidence_timeout, source_drift))
    return executed


def run_evidence(memory_corpus, timeout, source_drift):
    """Run every deterministic-test scenario's command from the checkout.

    The commands build and test the working tree, so they run only while it is
    still the source the manifest records: `source_drift()` returns a reason
    when it is not, and the scenarios are then recorded as errors.
    """
    executed = {}
    for scenario in memory_corpus["scenarios"]:
        if scenario["evidence"] != "deterministic-test":
            continue
        drift = source_drift()
        if drift:
            executed[scenario["id"]] = {"status": "error", "variants": [], "detail": drift, "checks": [
                {"check": "command", "passed": False, "command": " ".join(scenario["command"]), "tail": [drift]}]}
        else:
            executed[scenario["id"]] = evidence_command(scenario["command"], timeout)
    return executed


def evidence_command(command, timeout):
    """Run a deterministic-test scenario's command, bounded by `timeout` seconds.

    The command runs in its own process group, so a timeout, Ctrl-C or SIGTERM
    kills Cargo and the test binaries it started; a timed-out command is an
    `error`, never passing.
    """
    process = subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                               env={**os.environ, "RUSTC_WRAPPER": ""}, start_new_session=True)
    try:
        output, _ = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.communicate()
        detail = f"timed out after {timeout}s"
        return {"status": "error", "variants": [], "detail": detail,
                "checks": [{"check": "command", "passed": False, "command": " ".join(command), "tail": [detail]}]}
    except BaseException:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()
        raise
    passed = process.returncode == 0
    return {"status": "passed" if passed else "failed", "variants": [], "detail": None,
            "checks": [{"check": "command", "passed": passed, "command": " ".join(command),
                        "tail": output.strip().splitlines()[-2:]}]}


def pin_binary(binary, directory):
    """Copy the tested executable into a private directory and return the copy.

    The manifest fingerprints the copy, and diagnostics, migrations and the Node
    all run it, so a rebuild of the original path during the run changes nothing.
    """
    pinned = directory / "manage"
    pinned.write_bytes(Path(binary).read_bytes())
    pinned.chmod(0o700)
    return pinned


def source_drift(source, binary):
    """Why the checkout or executable no longer match the recorded source, or None."""
    current = evaluation.source_identity(binary)
    changed = [field for field in ("commit", "dirty", "dirty_diff_sha256", "binary_sha256")
               if current[field] != source[field]]
    return f"the tested source changed during the evaluation ({', '.join(changed)})" if changed else None


def experiment_manifest(tier, corpus, memory_corpus, source, model, harness, variants, repeats, limits, limitations,
                        *, evaluator, created_at=None):
    """`evaluator` is the identity pinned by evaluation.pin_evaluator() before the experiment began."""
    return {"schema_version": 2, "experiment_id": "fte-" + secrets.token_hex(6), "tier": tier,
            "created_at": created_at or timestamp(utc_now()),
            "aidash": source, "corpus": corpus["_snapshot"], "memory_corpus": memory_corpus["_snapshot"],
            "evaluator": evaluator, "model": model, "harness": harness, "variants": variants,
            "repeat_count": repeats, "limits": limits, "reproducibility_limitations": limitations}


def assemble(corpus, experiment, runs, availability, memory, calibrate, spend, limitations, error,
             skipped_runs=()):
    aggregates = evaluation.aggregate(runs) if runs else []
    result = {"schema_version": 2, "grade": "measurement-only",
              "approvals": {"accuracy_claim": False, "cost_claim": False, "production_quality": False,
                            "default_rollout": False},
              "conclusion_scope": evaluation.CONCLUSION_SCOPE, "experiment": experiment, "runs": runs,
              "aggregates": aggregates,
              "comparisons": evaluation.compare({item["id"]: item["issue"] for item in corpus["comparisons"]},
                                                aggregates, runs),
              "feature_availability": availability, "memory_matrix": memory,
              "calibration": {"applied": calibrate, "failures": evaluation.calibrate(corpus, runs) if calibrate else []},
              "spend": spend, "skipped_runs": [{"key": key, "reason": reason} for key, reason in skipped_runs],
              "limitations": limitations, "error": error}
    return result


def finalize(result, reservation, corpus, memory_corpus):
    """Validate a manifest and write it into its reservation. The record is written even when validation fails.

    A manifest that fails validation is annotated in its `error` field rather
    than discarded: it may be the only record of paid work. `corpus` and
    `memory_corpus` are the corpora the run loaded. Returns the validation
    failure, or None.
    """
    failure = None
    try:
        evaluation.validate_result(result, corpus, memory_corpus)
    except evaluation.CorpusError as invalid:
        failure = invalid
        summary = str(invalid).splitlines()[1:3]
        result["error"] = "; ".join(filter(None, [result["error"], "manifest failed validation: " + " | ".join(summary)]))
    evaluation.write_manifest(reservation, result)
    return failure


def run_parallel(execute, jobs, parallel, stop, results):
    """Run `execute` over `jobs` on `parallel` threads and append the results to `results` in job order.

    On an interrupt (Ctrl-C reaches only this thread) the queued jobs are
    cancelled first, so none starts, and then `stop` is set, so active jobs
    cancel their Runs; the pool is drained, every finished result is kept, and
    the interrupt is re-raised.
    """
    pool = concurrent.futures.ThreadPoolExecutor(max_workers=parallel)
    futures = [pool.submit(execute, job) for job in jobs]
    try:
        concurrent.futures.wait(futures)
    except BaseException:
        for future in futures:
            future.cancel()
        stop.set()
        raise
    finally:
        pool.shutdown(wait=True)
        results.extend(future.result() for future in futures if not future.cancelled() and future.exception() is None)
    failed = next((future.exception() for future in futures if future.exception() is not None), None)
    if failed is not None:
        raise failed


def unavailable_arms_error(registration_errors):
    detail = "; ".join(f"{name}: {reason}" for name, reason in sorted(registration_errors.items()))
    return ("the Node rejected Agent definitions for comparisons the corpus marks available (" + detail +
            "); pass --allow-unavailable-arms only when the tested Node is expected to lack the feature")


def deterministic(args):
    """The deterministic tier. Its executable is first copied into a private directory (pin_binary)."""
    with tempfile.TemporaryDirectory(prefix="aidash-fixed-task-binary-") as directory:
        return run_deterministic(args, Path(directory))


def run_deterministic(args, binary_directory):
    evaluator = evaluation.pin_evaluator()
    corpus = evaluation.load_corpus(args.corpus)
    memory_corpus = evaluation.load_memory_corpus(args.memory_corpus)
    for index, task in enumerate(corpus["tasks"]):
        task["_index"] = index
    planned_runs = evaluation.plan(corpus, args.task, args.comparison, getattr(args, "arm", None))
    for planned in planned_runs:
        planned["corpus"] = corpus
    checked(["docker", "info", "--format", "{{.ServerVersion}}"])
    if subprocess.run(["docker", "image", "inspect", POSTGRES_IMAGE], capture_output=True).returncode != 0:
        # The PostgreSQL image carries the native extensions; build it from compose.yaml.
        checked(["docker", "compose", "build", "postgres"], cwd=ROOT)
    binary = pin_binary(Path(args.binary).resolve(strict=True) if args.binary else build_manage(), binary_directory)
    query = json.loads(checked([str(binary), "diagnostics", "evaluation"], cwd=ROOT / "server",
                               env={"PATH": os.environ["PATH"], "TOKIO_WORKER_THREADS": "2"}))["inference_usage"]
    tag = secrets.token_hex(6)
    prefix = "fte" + tag[:6]
    password = secrets.token_urlsafe(24)
    output = Path(args.output) if args.output else DEFAULT_OUTPUT / f"deterministic-{tag}.json"
    try:
        reservation = evaluation.reserve_exclusive(output)
    except FileExistsError:
        print(f"error: --output {output} already exists; manifests are never overwritten", file=sys.stderr)
        return 2
    # Recorded after the reservation, so an --output inside the checkout is part of the recorded
    # state; source_drift() later compares the checkout and executable with it.
    source = evaluation.source_identity(binary)
    log_directory = DEFAULT_OUTPUT / f"logs-{tag}"
    model_ids = runner.fixture_model_ids(corpus, prefix)
    runs, variants, availability, error = [], [], evaluation.feature_availability(corpus, args.comparison), None
    memory = evaluation.memory_matrix(memory_corpus, {})
    model_refs = {}
    fixture = server = None
    stopped = False
    started = time.monotonic()
    # Everything after the reservation ends in finalize(): a failed setup leaves a manifest with
    # its error, not an empty file that would refuse a retry with the same --output.
    try:
        log_directory.mkdir(parents=True, mode=0o700, exist_ok=True)
        fixture = Fixture({model_ids[name]: spec["context_window"] for name, spec in corpus["models"].items()})
        server = fixture.serve({task["id"]: task for task in corpus["tasks"]})
        fixture_url = f"http://127.0.0.1:{server.server_port}"
        with tempfile.TemporaryDirectory(prefix="aidash-fixed-task-") as private_path, contextlib.ExitStack() as stack:
            private = Path(private_path)
            postgres = stack.enter_context(container(f"aidash-fte-{tag}-pg", POSTGRES_IMAGE, (
                ["-p", "127.0.0.1::5432"], ["postgres", "-c", "max_connections=200"]),
                # The image's init script creates the owner and server roles (#262).
                {"POSTGRES_USER": "aidash", "POSTGRES_PASSWORD": password, "POSTGRES_DB": "aidash_evaluation",
                 "AIDASH_OWNER_PASSWORD": password, "AIDASH_SERVER_PASSWORD": password}))
            nats = stack.enter_context(container(f"aidash-fte-{tag}-nats", NATS_IMAGE,
                                                 (["-p", "127.0.0.1::4222"], ["-js"])))
            wait(lambda: subprocess.run(["docker", "exec", postgres, "pg_isready", "-h", "127.0.0.1", "-U", "aidash",
                                         "-d", "aidash_evaluation"], capture_output=True).returncode == 0, 120,
                 "PostgreSQL")
            print("evaluation: disposable PostgreSQL and NATS ready", flush=True)
            port = free_port()
            base = f"http://127.0.0.1:{port}"
            deployment_administrator = secrets.token_urlsafe(32)
            node_id = f"aidash://fixed-task-{tag}"
            web = private / "web"
            web.mkdir()
            (web / "index.html").write_text("<!doctype html><title>evaluation</title>\n")
            environment = {key: value for key, value in os.environ.items()
                           if not key.startswith(("AIDASH_", "REINHARDT_"))}
            environment.update({
                "REINHARDT_ENV": "local",
                "REINHARDT_SETTINGS_DIR": str(settings_directory(private, secrets.token_hex(32))),
                # Servers log in as the role row-level security binds; setup below as the owner.
                "DATABASE_URL": f"postgres://aidash_server:{urllib.parse.quote(password, safe='')}@127.0.0.1:"
                                f"{published_port(postgres, 5432)}/aidash_evaluation",
                "NATS_URL": f"nats://127.0.0.1:{published_port(nats, 4222)}", "AIDASH_DEPLOYMENT_ID": node_id,
                "AIDASH_ENDPOINT": base, "AIDASH_LISTEN": f"127.0.0.1:{port}", "AIDASH_API_TOKEN": deployment_administrator,
                "AIDASH_API_RATE_BURST": "100000", "AIDASH_WEB_DIR": str(web),
                # A loaded host can exceed the default request deadline on reads that poll Runs.
                "AIDASH_HTTP_TIMEOUT_SECONDS": "120",
                # This disposable Node ID owns its JetStream streams and consumers.
                "AIDASH_ACTIVATION_BOOTSTRAP": "true",
                # The loopback model and embedding fixture is a Keyless endpoint.
                "AIDASH_KEYLESS_PRIVATE_ORIGINS": fixture_url,
                "NO_PROXY": "127.0.0.1,localhost", "no_proxy": "127.0.0.1,localhost"})
            # Native memory needs its independent recovery ledger, initialized once after the
            # schema exists and before any bank is created (docs/memory-recovery.md).
            recovery = private / "memory-recovery"
            environment["AIDASH_MEMORY_RECOVERY_DIR"] = str(recovery)
            owner = dict(environment, DATABASE_URL=environment["DATABASE_URL"].replace(
                "postgres://aidash_server:", "postgres://aidash_owner:", 1))
            with open(log_directory / "setup.log", "wb") as setup:
                for command in (["migrate"], ["memory-recovery", "init-if-missing", "--directory", str(recovery)]):
                    subprocess.run([str(binary), *command], cwd=ROOT, env=owner, stdout=setup, stderr=setup,
                                   check=True, timeout=300)
            child = stack.enter_context(node(binary, environment, log_directory / "server.log"))

            def healthy():
                if child.poll() is not None:
                    raise RuntimeError(f"Aidash exited during startup with code {child.returncode}; see "
                                       f"{log_directory}")
                with urllib.request.urlopen(base + "/health", timeout=2) as response:
                    return response.status == 200
            wait(healthy, 180, "Aidash health")
            print(f"evaluation: Node healthy; {len(planned_runs) * args.repeats} Runs planned", flush=True)
            api = runner.Api(http_transport(base), deployment_administrator)
            experiment = runner.Experiment(api, node_id, "evaluation", prefix)
            agent_ids = [runner.agent_id(prefix, planned) for planned in planned_runs]
            agent_ids += [f"{prefix}-memory-agent-{variant['id']}" for variant in memory_corpus["variants"]]
            experiment.authorize(agent_ids)
            for name, spec in corpus["models"].items():
                config = runner.model_config(dict(spec, model_id=model_ids[name]), fixture_url, name)
                model_refs[name] = dict(experiment.register("model", f"{prefix}-model-{name}", config),
                                        model_id=model_ids[name])
            variants, registration_errors = runner.register_variants(
                experiment, corpus, planned_runs, model_refs, fixture_url,
                remove_all_defaults=corpus["version"] != "fixed-task-corpus-v1")
            availability = evaluation.feature_availability(corpus, args.comparison, registration_errors)
            if registration_errors and not args.allow_unavailable_arms:
                # An arm the corpus marks available was rejected: a harness regression, not an absent feature.
                error = unavailable_arms_error(registration_errors)
            usage = usage_reader_for(postgres, "aidash_evaluation", password, query)
            # Runs are independent (own Workspace, Task and fixture probe state), so up to
            # --parallel of them share the Node's worker slots. Results keep plan order.
            jobs = [(repeat, planned) for planned, repeat in evaluation.execution_schedule(
                planned_runs, args.repeats, args.execution_seed)
                    if "agent_ref" in planned and not planned["task"].get("requires_compaction")]
            stop = threading.Event()

            def execute(job):
                repeat, planned = job
                observation = runner.blank_observation()

                def sleep(seconds):
                    # Ctrl-C reaches only the main thread, which sets `stop`; the Run's next wait
                    # then raises, so run_task cancels the Run and records it as interrupted.
                    if observation["interrupted"]:
                        time.sleep(seconds)  # the cancellation's own retries
                    elif stop.wait(seconds):
                        raise Cancelled()
                try:
                    runner.run_task(api, experiment, planned, repeat, planned["agent_ref"], fixture, usage,
                                    args.run_timeout, sleep=sleep, approve_fixture_tools=True, observation=observation)
                except Cancelled:
                    pass  # run_task has cancelled the Run and marked it interrupted.
                result = evaluation.run_result(corpus, planned, repeat, observation, "deterministic", True)
                detail = f" ({result['error'][:200]})" if result["error"] else ""
                print(f"{result['key']}: {result['outcome']}{detail}", flush=True)
                return result
            run_parallel(execute, jobs, args.parallel, stop, runs)
            if not args.skip_memory:
                print("evaluation: Runs finished; executing memory scenarios", flush=True)
                executed = memory_matrix_deterministic(api, experiment, corpus, memory_corpus, model_refs,
                                                       fixture_url, args.skip_rust_evidence, args.evidence_timeout,
                                                       lambda: source_drift(source, binary))
                memory = evaluation.memory_matrix(memory_corpus, executed)
    except (Exception, KeyboardInterrupt, Terminated) as failure:
        # SIGTERM unwinds like an error: the Runs are cancelled and the Node and containers removed
        # on the way out, and the manifest below records every finished Run.
        stopped = isinstance(failure, Terminated)
        error = (error + "; " if error else "") + f"{type(failure).__name__}: {failure}"
    finally:
        if server is not None:
            server.shutdown()
    if signal.getsignal(signal.SIGTERM) in (raise_terminated_deterministic, terminate):
        # Nothing owned is left to clean up; let no further SIGTERM interrupt writing the manifest.
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
    if fixture is not None and fixture.errors:
        error = (error + "; " if error else "") + "fixture faults: " + "; ".join(fixture.errors[:5])
    drift = source_drift(source, binary)
    if drift:
        error = (error + "; " if error else "") + drift
    model = {"kind": "scripted-fixture", "registrations": dict(model_refs),
             "reported_identities": sorted(model_ids.values()), "route": "local scripted fixture", "mutable_alias": False}
    harness = {"compaction": "none: ordinary Runs are not pruned until #108 (ADR 0041)", "cache_salt": True,
               "provider_credential": None, "disclosure_approval": None, "jev_classifier": "none",
               "builtin_policy": "historical-defaults/1" if corpus["version"] == "fixed-task-corpus-v1"
                                 else "minimal-task-support/1"}
    limits = {"run_timeout_seconds": args.run_timeout, "wall_seconds": max(1.0, time.monotonic() - started),
              "max_output_tokens": max(spec["max_output_tokens"] for spec in corpus["models"].values()),
              "spend_ceiling_nanocredits": None}
    unmeasured_tasks = {task["id"] for task in corpus["tasks"] if task.get("requires_compaction")}
    expected_runs = sum(variant["task"] not in unmeasured_tasks for variant in variants) * args.repeats
    if error is None and len(runs) != expected_runs:
        error = "not every planned Run was recorded"
    # Every planned Run without a record (queued when interrupted, or lost to a failure) is listed as skipped.
    recorded = {run["key"] for run in runs}
    skipped = [(key, "not measured: this disposable Node has no approved fixture Decider route; "
                     "run the independent Rust recovery evidence" if key.split("/")[0] in unmeasured_tasks else
                     "not recorded: " + (error or "the evaluation stopped")) for key in (
        evaluation.run_key(variant["task"], variant["comparison"], variant["arm"], repeat)
        for repeat in range(1, args.repeats + 1) for variant in variants) if key not in recorded]
    experiment_value = experiment_manifest("deterministic", corpus, memory_corpus, source, model, harness, variants,
                                           args.repeats, limits, DETERMINISTIC_LIMITATIONS, evaluator=evaluator)
    experiment_value["execution_order"] = {
        "seed": args.execution_seed, "method": "seeded-rotating-arm-blocks/1",
        "planned": [evaluation.run_key(p["task"]["id"], p["comparison"]["id"], p["arm"], repeat)
                    for p, repeat in evaluation.execution_schedule(planned_runs, args.repeats, args.execution_seed)]}
    result = assemble(corpus, experiment_value, runs, availability, memory, True, None,
                      DETERMINISTIC_LIMITATIONS, error, skipped)
    invalid = finalize(result, reservation, corpus, memory_corpus)
    error = result["error"]
    print(evaluation.summary_table(result))
    print(f"Result manifest: {output}")
    failures = result["calibration"]["failures"]
    memory_failures = [item["id"] for item in result["memory_matrix"]["scenarios"] if item["status"] in ("failed", "error")]
    if error or failures or memory_failures or invalid:
        for line in failures:
            print("calibration: " + line, file=sys.stderr)
        for line in memory_failures:
            print("memory scenario failed: " + line, file=sys.stderr)
        if error:
            print("error: " + error, file=sys.stderr)
        return 143 if stopped else 1
    return 0


@contextlib.contextmanager
def deferred_interrupts():
    """Record SIGINT instead of raising KeyboardInterrupt; yields the list of deferred interrupts.

    Only the main thread receives signals and may install handlers; elsewhere nothing is deferred.
    """
    interrupts = []
    if threading.current_thread() is not threading.main_thread():
        yield interrupts
        return
    previous = signal.signal(signal.SIGINT, lambda *_: interrupts.append(signal.SIGINT))
    try:
        yield interrupts
    finally:
        # None means a handler installed outside Python; the default action is the closest restore.
        signal.signal(signal.SIGINT, signal.SIG_DFL if previous is None else previous)


def live(args, read_json=None, transport=None, clock=time.monotonic, sleep=time.sleep, wall_now=utc_now):
    """Opt-in live tier. Every refusal happens before any registration, Run or provider work.

    After the guard rails pass, the manifest is always written, including after
    an error or an interrupt: it may be the only record of paid work.
    """
    evaluator = evaluation.pin_evaluator()
    corpus = evaluation.load_corpus(args.corpus)
    memory_corpus = evaluation.load_memory_corpus(args.memory_corpus)
    for index, task in enumerate(corpus["tasks"]):
        task["_index"] = index
    read_json = read_json or (lambda path: json.loads(Path(path).read_text()))
    deployment_administrator = os.environ.get(args.token_env)
    subject = os.environ.get(args.subject_token_env)
    if not deployment_administrator or not subject:
        raise evaluation.GuardRefusal(f"{args.token_env} (Deployment Administrator) and {args.subject_token_env} (Tenant subject) "
                                      "must both be set")
    if not args.server_revision or not SERVER_REVISION.fullmatch(args.server_revision):
        raise evaluation.GuardRefusal("--server-revision must be the evaluated Node's source fingerprint: 7 to 64 "
                                      "lowercase hexadecimal characters (a git commit or a SHA-256)")
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
    require_https(args.base_url)
    api = runner.Api(transport or http_transport(args.base_url), deployment_administrator)
    credential = api.get(f"/api/tenants/{args.tenant}/provider-credentials/{args.provider_credential}")
    binding = None
    if isinstance(credential, dict) and credential.get("provider"):
        try:
            binding = api.get(f"/api/tenants/{args.tenant}/provider-credential-bindings/{credential['provider']}")
        except runner.ApiError as missing:
            if missing.status != 404:
                raise
    models = {}
    for reference in sorted(set(mapping.values())):
        identifier, _, version = reference.partition("@")
        models[reference] = api.get(f"/api/registry/{identifier}/{version}", subject)["config"]
    approval = read_json(args.disclosure_approval) if args.disclosure_approval else None
    evaluation.check_model_config_digests(approval, models, required=corpus["version"] != "fixed-task-corpus-v1")
    execution_estimator = getattr(args, "execution_estimator", "bytes")
    evaluation.check_execution_estimator_approval(approval, execution_estimator)
    source_attestation = None
    if getattr(args, "source_attestation", None):
        source_attestation = evaluation.validate_source_attestation(read_json(args.source_attestation), approval,
                                                                   args.server_revision)
    expires_at = evaluation.require_live_authorization(credential, binding, approval, corpus["_snapshot"]["sha256"],
                                                       models, args.max_spend_nanocredits, args.max_wall_seconds,
                                                       wall_now())
    evaluation.check_live_models(models)
    selected = evaluation.plan(corpus, args.task, args.comparison, getattr(args, "arm", None))
    planned_runs = [planned for planned in selected if planned["model"] in mapping]
    if not planned_runs:
        raise evaluation.GuardRefusal("no planned Run uses a mapped model")
    # Every explicit selector must still contribute a runnable pair once unmapped models are dropped.
    uncovered = sorted(set(args.task or []) - {p["task"]["id"] for p in planned_runs}) + sorted(
        set(args.comparison or []) - {p["comparison"]["id"] for p in planned_runs})
    if uncovered:
        unmapped = sorted({p["model"] for p in selected if p["model"] not in mapping})
        raise evaluation.GuardRefusal(f"selectors {uncovered} match only pairs whose corpus models {unmapped} have no "
                                      "--model mapping; map them or drop the selectors")
    fixture_endpoint, code_oracle = evaluation.live_task_support(approval, corpus, planned_runs,
                                                                 args.tool_endpoint)
    auxiliary = evaluation.auxiliary_allowance(approval)
    auxiliary_entries = {}
    for kind in ("decider", "summary"):
        allowance = auxiliary[kind]
        if allowance:
            identifier, _, version = allowance["reference"].partition("@")
            auxiliary_entries[allowance["reference"]] = api.get(f"/api/registry/{identifier}/{version}", subject)["config"]
    evaluation.verify_auxiliary_configs(auxiliary, auxiliary_entries, credential["provider"])
    evaluation.require_enforceable_auxiliary_bound(auxiliary)
    missing = sorted({p["task"]["id"] for p in planned_runs if p["task"].get("requires_compaction")
                      and (not auxiliary["decider"] or p["task"].get("requires_summary") and not auxiliary["summary"])})
    if missing:
        raise evaluation.GuardRefusal(f"tasks {missing} require approved bounded Decider/Summary allowances; "
                                      "declare their exact references, configuration digests and all-call cost caps")
    evaluation.check_matched_live_models(corpus, planned_runs, mapping, models, execution_estimator=execution_estimator)
    if code_oracle:
        try:
            checked(["docker", "image", "inspect", code_oracle.image], timeout=20)
        except (OSError, subprocess.SubprocessError):
            raise evaluation.GuardRefusal("approved code-oracle image is not locally available; pull its pinned digest "
                                          "before the live campaign, then rerun preflight") from None
    ledger = evaluation.SpendLedger(args.max_spend_nanocredits, args.max_wall_seconds, clock, expires_at, wall_now)
    created_at = timestamp(wall_now())
    prefix = "fte" + secrets.token_hex(3)
    # Reserved before any Registry write or Run: the manifest may be the only record of paid work.
    output = Path(args.output) if args.output else DEFAULT_OUTPUT / f"live-{prefix}.json"
    try:
        reservation = evaluation.reserve_exclusive(output)
    except FileExistsError:
        raise evaluation.GuardRefusal(f"--output {output} already exists; live manifests are never "
                                      "overwritten") from None
    model_refs = {name: {"id": reference.partition("@")[0], "version": reference.partition("@")[2],
                         "config_sha256": evaluation.canonical_sha256(models[reference]),
                         "model_id": models[reference]["model_id"]} for name, reference in mapping.items()}
    variants, registration_errors, observations, skipped, error = [], {}, [], [], None
    stopped = None
    try:
        identity = api.get("/.well-known/aidash")
        experiment = runner.Experiment(api, identity["id"], args.tenant, prefix)
        experiment.token = subject
        for planned in planned_runs:
            planned["corpus"] = corpus
            planned["live_model"] = mapping[planned["model"]]
            planned["auxiliary"] = auxiliary
        # Only an exact approved synthetic service can receive fixture Tool calls; absent that
        # approval the endpoint remains unreachable and all Tool call approvals are denied.
        variants, registration_errors = runner.register_variants(experiment, corpus, planned_runs, model_refs,
                                                                 fixture_endpoint or args.tool_endpoint, remove_all_defaults=True)
        if registration_errors and not args.allow_unavailable_arms:
            error = unavailable_arms_error(registration_errors)
        else:
            skipped = runner.live_runs(api, experiment, corpus, planned_runs, args.repeats, ledger, models,
                                       approval["prices"], args.run_timeout, observations, clock, sleep,
                                       runner.binding_guard(args.tenant, credential["provider"],
                                                            (binding["provider_credential_id"], binding["revision"])),
                                       execution_seed=args.execution_seed,
                                       auxiliary=auxiliary, approve_fixture_tools=fixture_endpoint is not None)
    except (KeyboardInterrupt, Terminated) as stop:
        stopped = stop
        error = f"interrupted by the evaluator ({type(stop).__name__})"
    except Exception as failure:  # Recorded; the ledger and any finished Runs are still written.
        error = f"{type(failure).__name__}: {failure}"
    if signal.getsignal(signal.SIGTERM) is raise_terminated:
        # Finalization is short and writes the only record of paid work: let nothing interrupt it.
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
    # Ctrl-C while the Runs are graded and the manifest is written is honored only once it is written.
    with deferred_interrupts() as late_interrupts:
        runs = []
        for planned, repeat, observation in observations:
            try:
                runs.append(evaluation.run_result(corpus, planned, repeat, observation, "live", code_oracle or False))
            except Exception as failure:  # A grading fault must not discard the record of paid work.
                runs.append(evaluation.failed_run_result(corpus, planned, repeat, observation, "live", failure))
                error = (error + "; " if error else "") + f"grading failed for run {repeat} of " \
                    f"{planned['task']['id']}: {type(failure).__name__}"
        dispatched = {evaluation.run_key(planned["task"]["id"], planned["comparison"]["id"], planned["arm"], repeat)
                      for planned, repeat, _ in observations}
        graded = {run["key"] for run in runs}
        listed = {key for key, _ in skipped}
        # Every planned Run without a graded record is listed: undispatched, or dispatched but ungradable.
        skipped += [(key, ("dispatched, but grading failed: " if key in dispatched else "not dispatched: ") +
                     (error or "live dispatch stopped")) for key in (
            evaluation.run_key(planned["task"]["id"], planned["comparison"]["id"], planned["arm"], repeat)
            for repeat in range(1, args.repeats + 1) for planned in planned_runs if "agent_ref" in planned)
            if key not in graded and key not in listed]
        source = source_attestation or evaluation.source_identity(server_revision=args.server_revision)
        if source_attestation is None:
            source.update({"commit": None, "dirty": None, "dirty_diff_sha256": None})
        # Neither Usage Records nor model.completed events expose the provider-reported identity (I-5).
        model = {"kind": "live-provider", "registrations": model_refs, "reported_identities": None,
                 "route": f"configured credential provider: {credential.get('provider')}", "mutable_alias": True,
                 "requested_routes": evaluation.requested_model_routes(models), "reported_routes": None}
        harness = {"compaction": "server-configured", "cache_salt": True,
                   "builtin_policy": "minimal-task-support/1",
                   "execution_estimator": execution_estimator,
                   "source_provenance": "source-attestation" if source_attestation else "remote-unverified",
                   "source_attestation_sha256": evaluation.canonical_sha256(source_attestation) if source_attestation else None,
                   "provider_credential": args.provider_credential,
                   "disclosure_approval": evaluation.canonical_sha256(approval),
                   "jev_classifier": "bounded approved Decider" if auxiliary["decider"] else
                                     "excluded: the approval attests the Tenant approves no compaction Decider"}
        limits = {"run_timeout_seconds": args.run_timeout, "wall_seconds": args.max_wall_seconds,
                  "max_output_tokens": max(config["max_output_tokens"] for config in models.values()),
                  "spend_ceiling_nanocredits": args.max_spend_nanocredits}
        experiment_value = experiment_manifest("live", corpus, memory_corpus, source, model, harness, variants,
                                               args.repeats, limits, LIVE_LIMITATIONS, evaluator=evaluator,
                                               created_at=created_at)
        experiment_value["execution_order"] = {
            "seed": args.execution_seed, "method": "seeded-rotating-arm-blocks/1",
            "planned": [evaluation.run_key(p["task"]["id"], p["comparison"]["id"], p["arm"], repeat)
                        for p, repeat in evaluation.execution_schedule(planned_runs, args.repeats, args.execution_seed)]}
        availability = evaluation.feature_availability(corpus, args.comparison, registration_errors)
        memory = evaluation.memory_matrix(memory_corpus, {})
        result = assemble(corpus, experiment_value, runs, availability, memory, False, ledger.summary(),
                          LIVE_LIMITATIONS, error, skipped)
        invalid = finalize(result, reservation, corpus, memory_corpus)
    print(evaluation.summary_table(result))
    print(f"Result manifest: {output}")
    if result["error"]:
        print("error: " + result["error"], file=sys.stderr)
    if stopped is not None:
        return 143 if isinstance(stopped, Terminated) else 130
    if late_interrupts:
        return 130
    if result["error"] or invalid:
        return 1
    return 0 if ledger.refusal is None else 3


def validate_command(args):
    result = json.loads(Path(args.manifest).read_text())
    evaluation.validate_result(result, evaluation.load_corpus(args.corpus),
                               evaluation.load_memory_corpus(args.memory_corpus))
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
        command.add_argument("--arm", action="append", help="execute only this frozen arm id (repeatable)")
        command.add_argument("--repeats", type=int, default=1 if name == "deterministic" else 3,
                             help="Runs per task arm (default 1 deterministic, 3 live)")
        command.add_argument("--run-timeout", type=float, default=180.0)
        command.add_argument("--output", type=Path)
        command.add_argument("--execution-seed", type=int, default=181,
                             help="recorded seed for balanced randomized task/arm execution order")
        command.add_argument("--allow-unavailable-arms", action="store_true",
                             help="treat an Agent definition the Node rejects for an available comparison as "
                                  "feature-not-available instead of failing (for a baseline Node that predates it)")
    deterministic_command = commands.choices["deterministic"]
    deterministic_command.add_argument("--parallel", type=int, default=4,
                                       help="concurrent Runs; the Node has four worker slots")
    deterministic_command.add_argument("--binary", help="prebuilt manage executable; default builds this worktree")
    deterministic_command.add_argument("--skip-memory", action="store_true",
                                       help="report every memory scenario as not-run (for narrowed task runs)")
    deterministic_command.add_argument("--skip-rust-evidence", action="store_true",
                                       help="do not run deterministic-test memory scenarios")
    deterministic_command.add_argument("--evidence-timeout", type=float, default=1800.0,
                                       help="seconds each deterministic-test memory scenario command may take "
                                            "(default 1800); a timed-out command is recorded as an error")
    live_command = commands.choices["live"]
    live_command.add_argument("--base-url", required=True)
    live_command.add_argument("--tenant", required=True)
    live_command.add_argument("--token-env", default="AIDASH_EVALUATION_TOKEN", help="Deployment Administrator bearer token variable")
    live_command.add_argument("--subject-token-env", default="AIDASH_EVALUATION_SUBJECT_TOKEN",
                              help="Tenant subject bearer token variable used to create and run Tasks")
    live_command.add_argument("--server-revision", required=True,
                              help="deployed source fingerprint of the evaluated Node: 7 to 64 lowercase "
                                   "hexadecimal characters (a git commit or SHA-256)")
    live_command.add_argument("--tool-endpoint", default="https://tools.invalid/aidash-fixed-task-evaluation",
                              help="endpoint declared by fixture tools; approvals for them are denied")
    live_command.add_argument("--provider-credential", help="connected Tenant Provider Credential ID")
    live_command.add_argument("--execution-estimator", choices=("bytes", "calibrated"), default="bytes",
                              help="Bytes primary comparison; calibrated requires an explicit matching disclosure approval")
    live_command.add_argument("--source-attestation", type=Path,
                              help="explicitly attested clean remote source/binary identity, bound by disclosure approval")
    live_command.add_argument("--model", action="append", default=[],
                              help="corpus model name=Registry model id@version (repeatable)")
    live_command.add_argument("--disclosure-approval", type=Path)
    live_command.add_argument("--max-spend-nanocredits", type=int)
    live_command.add_argument("--max-wall-seconds", type=float)
    validate_parser = commands.add_parser("validate")
    validate_parser.add_argument("manifest", type=Path)
    validate_parser.add_argument("--corpus", type=Path, default=evaluation.EVALUATION / "corpus.toml",
                                 help="the corpus the experiment used (its SHA-256 must match the manifest)")
    validate_parser.add_argument("--memory-corpus", type=Path, default=evaluation.EVALUATION / "memory.toml",
                                 help="the memory corpus the experiment used")
    args = parser.parse_args(argv)
    if args.command == "validate":
        return validate_command(args)
    if not 1 <= args.repeats <= 250:
        parser.error("--repeats must be between 1 and 250")
    # NaN would make every deadline comparison false, so a Run could poll past every limit, and the
    # manifest records limits of at least one second, so a shorter one is refused before any work.
    if not math.isfinite(args.run_timeout) or args.run_timeout < 1:
        parser.error("--run-timeout must be a finite number of seconds, at least 1")
    if args.command == "deterministic" and (not math.isfinite(args.evidence_timeout) or args.evidence_timeout <= 0):
        parser.error("--evidence-timeout must be a positive, finite number of seconds")
    try:
        # Both tiers unwind on SIGTERM, so Runs are cancelled and the manifest is written.
        if args.command == "deterministic":
            signal.signal(signal.SIGTERM, raise_terminated_deterministic)
            return deterministic(args)
        signal.signal(signal.SIGTERM, raise_terminated)
        return live(args)
    except (evaluation.GuardRefusal, evaluation.CorpusError) as refusal:
        print(f"{args.command} evaluation refused: {refusal}", file=sys.stderr)
        return 2
    except Terminated:  # Before anything was reserved: there is no manifest to write.
        print(f"{args.command} evaluation terminated before it started", file=sys.stderr)
        return 143


if __name__ == "__main__":
    sys.exit(main())
