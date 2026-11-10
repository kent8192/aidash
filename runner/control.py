"""Single-writer, durable Kubernetes runner control service.

The journal is a persistent volume owned by whoever runs the Deployment. An exclusive process lock
prevents two controllers from claiming it. Kubernetes object names are derived
from operation UUIDs; recovery observes those objects and never recreates a
missing object after acceptance. No shell command runs on the controller host.
"""
import base64
from datetime import datetime
import fcntl
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import hmac
import ipaddress
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import threading
import time
import traceback
from urllib.parse import urlsplit, parse_qs
import uuid

# -I excludes the script directory. Only this trusted controller installation
# supplies its journal helpers; sandbox paths and environment imports stay excluded.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from allocation_usage import AllocationJournal, FINALIZER
from runner_journal import Rejected, identity, save, sync_directory
from execution_pod import ExecutionPodController, ExecutionPodError, v2_network_policy

TERMINAL = {"completed", "cancelled", "failed", "uncertain"}
# The sandbox image's runner/probe.py must emit exactly this evidence contract.
PROBE_PROTOCOL = "aidash-admission-probe/2"
ALLOCATION_OBSERVATION_ATTEMPTS = 5
STOP_WATCH_SECONDS = 1800
# Bounds the wait for a Pod to be ready and the retries of a start that keeps
# failing. It does not bound the input upload or the dispatch: a command that
# is ready to start is not refused for having uploaded many files (#709).
START_LIMIT_SECONDS = 120
DEFAULT_UNCERTAIN_RETENTION_SECONDS = 86400


class SandboxGone(Rejected):
    """A successful Kubernetes observation found the original Pod absent."""

    def __init__(self):
        super().__init__(409, 'original sandbox is absent or replaced')


class StartupIdentityMismatch(Exception):
    """A different sandbox or input was observed before dispatch."""


class StoppedExportRefused(Exception):
    def __init__(self, refusal):
        self.refusal = refusal


def scratch_resource_name(scratch_space_id):
    """Derive a stable Kubernetes DNS label without exposing the opaque ID."""
    if not isinstance(scratch_space_id, str) or not scratch_space_id:
        raise ValueError("scratch_space_id must be a nonempty string")
    return "scratch-" + hashlib.sha256(scratch_space_id.encode("utf-8")).hexdigest()[:48]


# Guidance shown to the model when the collector refuses to export /work.
EXPORT_GUIDANCE = {
    'symlink': 'symlinks are not supported in /work: {path}; use npm install --no-bin-links, or keep dependency trees under /tmp',
    'hard_link': 'hard-link aliases are not supported in /work: {path}; write independent copies, or keep dependency trees under /tmp',
    'file_count': 'the working area holds at most {limit} files; remove files (first over the limit: {path}), or keep dependency trees under /tmp',
    'bytes': 'the working area holds at most {limit} bytes; remove files (first over the limit: {path}), or keep large output under /tmp',
    'unsafe_directory': 'unsupported directory in /work: {path}; rename or remove it, or use /tmp',
    'unsafe_file': 'unsupported file in /work: {path}; keep only regular files with printable names, or use /tmp',
}


def export_error(refused):
    """The typed tool error of an export refusal: final, never retried."""
    message = EXPORT_GUIDANCE[refused['reason']].format(path=refused['path'], limit=refused['limit'])
    return {'code': 'WORKING_EXPORT_REFUSED', 'message': message, 'details': refused, 'retryable': False}



def termination_time(terminated, started_at, observed):
    """When a command stopped, for charging a Check's runtime. The controller
    polls, so `observed` can trail the stop, for example after a restart.
    Kubernetes records `finishedAt` in whole seconds, so the stop came before
    `finishedAt` + 1: cap the observation there. A missing or unparseable
    `finishedAt`, or one the clocks put before the start, leaves `observed`.
    This is an upper bound for charging only; whether the Check timed out is
    the collector's `deadline_expired`."""
    try:
        finished = datetime.fromisoformat(terminated["finishedAt"].replace("Z", "+00:00")).timestamp()
    except (KeyError, TypeError, ValueError, AttributeError):
        return observed
    if started_at is not None and finished + 1 < started_at:
        return observed
    return min(observed, finished + 1)


def cell_timed_out(record):
    """Whether a Python cell ran into its limit. The node watchdog stops it there
    without saying why, so a stop at or past the limit counts as one."""
    stopped_at, running_at = record.get('stop_confirmed_at'), record.get('running_at')
    return stopped_at is not None and running_at is not None and stopped_at >= running_at + record['request']['seconds']


def settled(record):
    """No writer can still be running: terminal, with termination proven or frozen."""
    return record['status'] in TERMINAL and bool(record.get('termination_confirmed') or record.get('writer_frozen'))


class ActivityAggregate:
    """Aggregate of the operation journal for /v1/activity.

    The journal keeps every operation file for the life of the volume (an
    acknowledgement clears the payload, never the file), so deriving activity from
    the files costs more with each operation ever run. This keeps only the IDs of
    unsettled operations and the latest completion time, updated whenever a record
    is saved, so a read is independent of how many operations are retained.
    Callers serialize updates with the Runner's lock."""

    def __init__(self):
        self.unsettled = set()
        self.last_completed = 0

    def track(self, operation, record):
        if settled(record):
            self.unsettled.discard(operation)
        else:
            self.unsettled.add(operation)
        self.last_completed = max(self.last_completed, record.get('finished_at', 0))


class Runner(AllocationJournal):
    # Set once the startup isolation proof passes. Without it the Runner takes
    # no new work and restarts nothing it recovered; stops and status still work.
    proven = False

    def __init__(self, config):
        self.config = config
        retention = config.get('uncertain_retention_seconds', DEFAULT_UNCERTAIN_RETENTION_SECONDS)
        if type(retention) is not int or retention < 1:
            raise ValueError('uncertain output retention must be a positive integer')
        self.root = Path(config["journal"])
        self.root.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.owner = (self.root / "owner.lock").open("a")
        fcntl.flock(self.owner, fcntl.LOCK_EX | fcntl.LOCK_NB)
        self.lock = threading.RLock()
        self.active = set()
        execution_config = {'namespace': config['namespace'], 'runtime_class': config['runtime_class'],
                            'processes': config.get('processes', 128), 'host_tasks': config.get('host_tasks', 512),
                            'output_bytes': config.get('output_bytes', 8388608)}
        execution_config_path = os.environ.get('AIDASH_EXECUTION_POD_CONFIG')
        if execution_config_path:
            supplied = json.loads(Path(execution_config_path).read_bytes())
            if not isinstance(supplied, dict) or {'namespace', 'runtime_class', 'processes', 'host_tasks'} & supplied.keys():
                raise ValueError('invalid Execution Pod configuration overrides')
            execution_config.update(supplied)
        self.execution_pods = ExecutionPodController(self, execution_config)
        self.admission_operation = None
        # Seeded by recover(), the final startup scan; until then only the
        # operations saved during startup are tracked, and nothing serves it yet.
        self.aggregate = ActivityAggregate()

        identity_path = self.root / "instance"
        fresh_instance = not identity_path.exists()
        if fresh_instance:
            save(identity_path, str(uuid.uuid4()))
        self.instance = json.loads(identity_path.read_bytes())
        self.initialize_allocations(fresh_instance=fresh_instance)
        self.token = os.environ[config["token_env"]]
        if len(self.token) < 32:
            raise ValueError("runner token must have at least 32 characters")
        # A Kubernetes namespace is an RFC 1123 label, which may be one character.
        if not re.fullmatch(r"[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?", config["namespace"]):
            raise ValueError("invalid dedicated namespace")
        if not re.search(r"@sha256:[0-9a-f]{64}$", config["image"]):
            raise ValueError("runner image must be digest pinned")
        self.command = [config["kubectl"]]
        if config.get('kubeconfig'):
            self.command += ['--kubeconfig', config['kubeconfig']]
        self.command += ['--namespace', config['namespace']]
        self.environment = {"PATH": os.defpath, "HOME": str(self.root)}
        # kubectl's in-cluster authentication needs these coordinates and the
        # mounted ServiceAccount token. Never inherit unrelated credentials.
        for key in ('KUBERNETES_SERVICE_HOST', 'KUBERNETES_SERVICE_PORT'):
            if key in os.environ:
                self.environment[key] = os.environ[key]
        runtime = self.kube_json(["get", "runtimeclass", config["runtime_class"], "-o", "json"])
        if runtime["handler"] != "runsc":
            raise ValueError("verified runsc runtime class required")
        self.ensure_network_policy()
        self.ensure_v2_network_policy()
        self.verified = False
        self.python_verified = False
        self.probe = None
        # Existing journals contain untrusted work too. Re-prove the current
        # isolation boundary before any recovery thread can execute that work.
        # A failed proof refuses new work (the health report says unverified)
        # and starts no recovery, but it must not stop the Runner watching the
        # commands it already tracks. Execution Pod commands outlive a restart,
        # and a crash loop here would leave nobody to deliver their stops (#722).
        proof_failure = None
        try:
            self.verify_isolation()
            if self.config.get("node_guard"):
                self.verify_freeze()
            self.proven = True
        except Exception as failure:
            # A proof that stopped half way proves nothing: the health report
            # must not say verified because only the first step passed.
            self.verified = self.python_verified = False
            self.probe = None
            proof_failure = str(failure) or type(failure).__name__
            print(f"Deployment Administrator: isolation proof failed, so new work is refused: {proof_failure}",
                  file=sys.stderr, flush=True)
        try:
            self.retire_admission_probes()
        except Exception as failure:
            print(f"Admission probe retirement failed: {failure}", file=sys.stderr, flush=True)
        self.recover(resume=self.proven)
        self.execution_pods.recover_commands()
        if self.proven:
            self.activate_execution_pods()
        else:
            # Execution Pods stay unadmitted: commands already running are watched, none are accepted.
            self.execution_pods.admission_error = 'Runner isolation proof failed: ' + proof_failure
        threading.Thread(target=self.allocation_loop, daemon=True).start()
        threading.Thread(target=self.uncertain_output_loop, daemon=True).start()

    def operation_files(self):
        for path in self.root.glob("*.json"):
            if not path.name.startswith(("area-", "session-")):
                yield path

    def recover(self, resume=True):
        """Seed the activity aggregate and resume interrupted work in one journal
        scan at startup, never per request. Each record is tracked before its work
        restarts, so no later update of it can be overwritten by this read. Without
        an isolation proof nothing restarts, yet the aggregate is still seeded: an
        idle stop must not take the Runner down with that work in flight."""
        for path in self.operation_files():
            record = json.loads(path.read_bytes())
            with self.lock:
                self.aggregate.track(path.stem, record)
            if record.get('output_harvested') and record['status'] in TERMINAL:
                self.allocation_harvested(record, idle=record['status'] != 'uncertain' and record.get('writer_frozen') is True)
            if resume and record["status"] not in TERMINAL and record["status"] != "awaiting_files":
                self.start(record["operation_id"])

    def uncertain_output_loop(self):
        while True:
            self.sweep_uncertain_output()
            time.sleep(30)

    def sweep_uncertain_output(self):
        for path in self.operation_files():
            try:
                record = json.loads(path.read_bytes())
                if record.get('startup_cleanup_pending'):
                    self.cleanup_failed_start(record)
                    continue
                if (record.get('status') != 'uncertain' or not record.get('termination_confirmed')
                        or record.get('output_harvested') or record.get('acknowledged')
                        or record.get('request', {}).get('kind') not in ('shell', 'verify', 'python', 'journal')):
                    continue
                operation = record['operation_id']
                with self.lock:
                    if operation in self.active:
                        continue
                    record = self.get(operation)
                    if (record.get('status') != 'uncertain' or not record.get('termination_confirmed')
                            or record.get('output_harvested') or record.get('acknowledged')):
                        continue
                    self.active.add(operation)
                try:
                    since = record.get('uncertain_at')
                    if since is None:
                        since = time.time()
                        record = self.update(operation, uncertain_at=since)
                    if time.time() < since + self.config.get('uncertain_retention_seconds', DEFAULT_UNCERTAIN_RETENTION_SECONDS):
                        try:
                            self.harvest_stopped_output(record)
                        except StoppedExportRefused as refusal:
                            detail = export_error(refusal.refusal)
                            record = self.update(operation, output_harvested=True, output_discarded=True,
                                                 files=[], displays=[], error=detail)
                            self.allocation_harvested(record)
                            self.close_allocation(record)
                        except Exception:
                            try:
                                pod = self.pod(record.get('pod_operation', operation))
                            except SandboxGone:
                                pod = None
                            if pod is not None:
                                raise
                            self.update(operation, output_harvested=True, output_discarded=True, files=[],
                                        error={'code':'SANDBOX_LOST', 'message':"Sandbox is gone; the command's file changes were not saved."})
                    else:
                        self.allocation_harvested(record)
                        self.close_allocation(record)
                        try:
                            pod = self.pod(record.get('pod_operation', operation))
                        except SandboxGone:
                            pod = None
                        if pod is None or pod.get('metadata', {}).get('uid') != record.get('pod_uid'):
                            record = self.update(operation, output_discarded=True, output_harvested=True,
                                                 files=[], displays=[], truncated=True)
                            proof = self.output_digest(record)
                            self.update(operation, acknowledged=True, acknowledged_output_digest=proof)
                            print(f"Deployment Administrator: discarded unreadable output for {operation}", file=sys.stderr, flush=True)
                finally:
                    with self.lock:
                        self.active.discard(operation)
            except Exception as error:
                print(f"Uncertain output recovery {path.stem}: {error}", file=sys.stderr, flush=True)

    def verify_isolation(self):
        operation = str(uuid.uuid4())
        self.admission_operation = operation
        self.accept({"operation_id": operation, "area_id": str(uuid.uuid4()), "epoch": 1,
                     "digest": "immutable-admission-probe/3", "kind": "shell", "code": f"python -I /opt/aidash/probe.py {self.config['working_bytes']} {self.config['temporary_bytes']}",
                     "seconds": 30, "files": []}, admission_probe=True)
        deadline = time.monotonic() + 150
        while time.monotonic() < deadline:
            record = self.get(operation)
            if record["status"] in TERMINAL:
                if record["status"] != "completed":
                    raise RuntimeError("isolation probe failed: " + str(record.get("error")))
                report = json.loads(base64.b64decode(record["stdout"]))
                if report.get("protocol") != PROBE_PROTOCOL:
                    raise RuntimeError(f"sandbox image admission probe is not {PROBE_PROTOCOL}; rebuild it from a compatible source")
                if ("gvisor" not in report["kernel"].lower() or report["uid"] == 0
                        or report["process_limit"] != self.config["processes"]
                        or report.get('guest_capacity_verified') != max(1, self.config['processes'] - 16)
                        or record['resource_evidence']['host_tasks'] != self.config.get('host_tasks', 512)
                        or record['resource_evidence']['processes'] != self.config['processes']
                        or not isinstance(report.get("physical_page_size"), int)
                        or report["physical_page_size"] <= 0
                        or not all(report["egress_denied"].values())):
                    raise RuntimeError("isolation evidence does not match the execution profile")
                self.verified = True
                self.probe = dict(report, resources=record['resource_evidence'])
                self.admission_operation = None
                return
            time.sleep(0.2)
        raise RuntimeError("isolation probe did not finish")

    def retire_admission_probes(self):
        """A startup probe that failed before termination was confirmed stays
        terminal `uncertain`, and activity would count it as busy forever. It runs
        only the Runner's own probe in a private area, so once Kubernetes confirms
        its Pod is gone (graceful deletion completes only after the kubelet stops
        every container) no writer remains. A Pod that cannot be removed keeps the
        record busy: activity stays fail-closed."""
        for path in self.operation_files():
            record = json.loads(path.read_bytes())
            if (not record.get("admission_probe") or record["status"] not in TERMINAL
                    or record.get("termination_confirmed") or record.get("writer_frozen")):
                continue
            operation = record["operation_id"]
            self.kube(["delete", "pod", self.pod_name(operation), "--ignore-not-found", "--wait=true", "--timeout=60s"],
                      timeout=70, check=False)
            if self.pod(operation) is None:
                self.update(operation, termination_confirmed=True)

    def verify_freeze(self):
        operation = str(uuid.uuid4())
        record = {"operation_id": operation, "area_id": str(uuid.uuid4()), "epoch": 1,
                  "wire_digest": "freeze-admission-probe/1",
                  "request": {"kind": "python", "files": []}, "admission_probe": True}
        manifest = self.manifest(record)
        manifest["spec"]["containers"][0]["command"] = ["python", "-u", "-c", "import time; from pathlib import Path\nwhile True: Path('/work/tick').write_text(str(time.monotonic())); time.sleep(.01)"]
        self.kube(["create", "-f", "-"], json.dumps(manifest).encode())
        stopped = False
        try:
            self.kube(["wait", "--for=condition=Ready", "pod/" + self.pod_name(operation), "--timeout=60s"], timeout=70)
            pod = self.pod(operation)
            status = next(s for s in pod["status"]["containerStatuses"] if s["name"] == "execution")
            record.update(pod_uid=pod["metadata"]["uid"], container_id=status["containerID"].split("://", 1)[1],
                          cluster_node=pod['spec']['nodeName'], image=self.config['image'])
            self.guard(record, "freeze", idle_seconds=30)
            first = self.guard(record, "read", path="tick", offset=0)
            time.sleep(.3)
            second = self.guard(record, "read", path="tick", offset=0)
            if first != second:
                raise RuntimeError("gVisor freeze did not stop the writer")
            self.guard(record, "terminate")
            stopped = True
            self.python_verified = True
        finally:
            self.kube(["delete", "pod", self.pod_name(operation), "--wait=false"] +
                      (["--force", "--grace-period=0"] if stopped else []), check=False)

    def kube(self, arguments, data=None, timeout=45, check=True):
        result = subprocess.run(self.command + arguments, input=data, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, env=self.environment, timeout=timeout)
        if check and result.returncode:
            raise RuntimeError(result.stderr.decode(errors="replace")[-2000:])
        return result

    def kube_json(self, arguments, data=None):
        return json.loads(self.kube(arguments, data).stdout)

    def ensure_network_policy(self):
        journal = {"apiVersion": "networking.k8s.io/v1", "kind": "NetworkPolicy",
                   "metadata": {"name": "aidash-journal-deny", "namespace": self.config["namespace"]},
                   "spec": {"podSelector": {"matchLabels": {"aidash-sandbox": "true", "aidash-journal": "true"}},
                            "policyTypes": ["Ingress", "Egress"], "ingress": [], "egress": []}}
        self.kube(["apply", "-f", "-"], json.dumps(journal).encode())
        policy = {"apiVersion": "networking.k8s.io/v1", "kind": "NetworkPolicy",
                  "metadata": {"name": "aidash-deny", "namespace": self.config["namespace"]},
                  "spec": {"podSelector": {"matchLabels": {"aidash-sandbox": "true"},
                                           "matchExpressions": [{"key": "aidash-v2", "operator": "DoesNotExist"},
                                                                {"key": "aidash-journal", "operator": "DoesNotExist"}]},
                           "policyTypes": ["Ingress", "Egress"], "ingress": [], "egress": []}}
        settings = self.config.get('egress')
        if settings:
            endpoint = urlsplit(settings['endpoint'])
            address = ipaddress.ip_address(endpoint.hostname)
            if endpoint.scheme != 'http' or endpoint.username or endpoint.password or endpoint.path not in ('', '/') or endpoint.query or endpoint.fragment:
                raise ValueError('Egress endpoint must be an internal HTTP IP origin')
            destination = {'ipBlock': {'cidr': str(address) + ('/32' if address.version == 4 else '/128')}}
            if settings.get('namespace') and settings.get('pod_selector'):
                destination = {'namespaceSelector': {'matchLabels': {'kubernetes.io/metadata.name': settings['namespace']}},
                               'podSelector': {'matchLabels': settings['pod_selector']}}
            policy['spec']['egress'] = [{'to': [destination], 'ports': [{'protocol': 'TCP', 'port': endpoint.port or 80}]}]
        self.kube(["apply", "-f", "-"], json.dumps(policy).encode())

    def ensure_v2_network_policy(self):
        config = self.execution_pods.config
        if config.get('proxy_endpoint'):
            policy = v2_network_policy(config)
        else:
            policy = {'apiVersion': 'networking.k8s.io/v1', 'kind': 'NetworkPolicy',
                      'metadata': {'name': 'aidash-v2-egress', 'namespace': self.config['namespace']},
                      'spec': {'podSelector': {'matchLabels': {'aidash-sandbox': 'true', 'aidash-v2': 'true'}},
                               'policyTypes': ['Ingress', 'Egress'], 'ingress': [], 'egress': []}}
        self.kube(['apply', '-f', '-'], json.dumps(policy).encode())

    def activate_execution_pods(self):
        """Enable v2 only after a disposable Pod passes its complete admission."""
        config = self.execution_pods.config
        self.execution_pods.admission_error = 'Execution Pods are disabled'
        if not config.get('enabled'):
            return
        scratch_id = 'admission-' + str(uuid.uuid4())
        error = None
        self.execution_pods.admitted = True  # startup is not serving requests yet
        try:
            if config.get('allow_docker'):
                runtime = self.kube_json(['get', 'runtimeclass', config['docker_runtime_class'], '-o', 'json'])
                if runtime.get('handler') != 'runsc-docker':
                    raise ValueError('Docker RuntimeClass does not use the verified raw-socket handler')
            settings = {'scratch_space_id': scratch_id, 'image': self.config['image'],
                        'cpu': min(self.config.get('cpu', 2), 2),
                        'memory_bytes': min(self.config.get('memory_bytes', 2 << 30), 2 << 30),
                        'capacity_bytes': config.get('admission_capacity_bytes', 5 << 30),
                        'docker_enabled': False}
            self.execution_pods.ensure(settings)
        except Exception as failure:
            error = str(failure) or type(failure).__name__
        try:
            self.execution_pods.delete(scratch_id)
        except Exception as failure:
            detail = str(failure) or type(failure).__name__
            error = (error + '; ' if error else '') + 'admission cleanup failed: ' + detail
        self.execution_pods.admitted = error is None
        self.execution_pods.admission_error = error
        if error:
            print('Execution Pod admission failed: ' + error, file=sys.stderr, flush=True)

    def path(self, operation):
        return self.root / f"{identity(operation)}.json"

    def get(self, operation):
        with self.lock:
            try:
                return json.loads(self.path(operation).read_bytes())
            except FileNotFoundError:
                raise Rejected(404, "operation unavailable") from None

    def update(self, operation, **changes):
        with self.lock:
            record = self.get(operation)
            record.update(changes)
            save(self.path(operation), record)
            self.aggregate.track(operation, record)
            return record

    def public(self, record):
        public = {k: v for k, v in record.items() if k not in ("request", "pod_uid")}
        if record.get('status') not in TERMINAL and isinstance(record.get('stdout'), str):
            try:
                data = base64.b64decode(record['stdout'], validate=True)
                if len(data) > 16384:
                    data = data[-16384:] if record.get('request', {}).get('kind') == 'verify' else data[:16384]
                    public.update(stdout=base64.b64encode(data).decode(), truncated=True)
            except ValueError:
                pass
        if record.get('status') == 'uncertain' and record.get('output_harvested') is True and not record.get('acknowledged'):
            public['output_digest'] = self.output_digest(record)
        return public

    def output_digest(self, record):
        snapshot = {key: record.get(key) for key in ('operation_id', 'digest', 'stdout', 'files', 'displays', 'truncated')}
        snapshot['runner_instance'] = self.instance
        return hashlib.sha256(json.dumps(snapshot, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()).hexdigest()

    def accept(self, request, admission_probe=False):
        if not self.proven and not admission_probe:
            raise Rejected(503, "runner isolation is not proven; new work is refused")
        kind = request.get("kind")
        optional = {"session_id", "egress", "usage_tenant"} | ({"port"} if kind == "serve" else set()) | ({"runner_instance"} if kind == "journal" else set())
        if set(request) - optional != {"operation_id", "area_id", "epoch", "digest", "kind", "code", "seconds", "files"}:
            raise Rejected(400, "invalid runner contract")
        if not admission_probe and (not isinstance(request.get("usage_tenant"), str) or not request["usage_tenant"] or len(request["usage_tenant"].encode()) > 255):
            raise Rejected(400, "native allocation usage context required")
        operation, area = identity(request["operation_id"]), identity(request["area_id"])
        if kind == "journal" and request.get("runner_instance") != self.instance:
            raise Rejected(409, "Journal Query runner instance changed")
        if kind == "journal" and 'egress' in request:
            raise Rejected(400, "Journal Query cannot use egress")
        if 'egress' in request:
            lease = request['egress']
            settings = self.config.get('egress') or {}
            # A Live preview's Web server holds a session-lifetime token; an
            # operation holds one that ends with its command (ADR 0070).
            owner = 'session:' + operation if kind == "serve" else operation
            if (not isinstance(lease, dict) or set(lease) != {'url', 'token'}
                    or lease['url'] != settings.get('endpoint')
                    or not isinstance(lease['token'], str)
                    or not re.fullmatch(re.escape(owner) + r'\.[A-Za-z0-9_-]{43}', lease['token'])):
                raise Rejected(400, 'invalid Egress lease')
        # "verify" runs like "shell" but never exports /work: a Completion Gate
        # Check must not change the Working Area it verifies. "serve" is a
        # long-lived Live preview whose writes are never published (ADR 0070).
        if kind not in ("shell", "verify", "python", "journal", "serve") or not isinstance(request["code"], str):
            raise Rejected(400, "invalid command kind")
        if len(request["code"].encode()) > self.config.get("limits", {}).get("command_bytes", 65536) or len(request["files"]) > 4096:
            raise Rejected(400, "command or file limit")
        if not isinstance(request["epoch"], int) or request["epoch"] < 1:
            raise Rejected(400, "invalid epoch")
        maximum = (self.config.get("preview_maximum_seconds", 7200) if kind == "serve"
                   else self.config["maximum_seconds"])
        if not isinstance(request["seconds"], int) or not 1 <= request["seconds"] <= maximum:
            raise Rejected(400, "operation time limit")
        port = None
        if kind == "serve":
            port = request.get("port")
            if not isinstance(port, int) or not 1024 <= port <= 65535:
                raise Rejected(400, "invalid preview port")
            allowed = self.config.get("preview_port")
            if allowed is not None and port != allowed:
                raise Rejected(400, "preview port not allowed")
        session = None
        if request["kind"] in ("python", "journal"):
            if not self.config.get("node_guard"):
                raise Rejected(409, "PYTHON_RUNTIME_UNAVAILABLE")
            sid = identity(request.get("session_id"))
            session_path = self.root / ("session-" + sid + ".json")
            if session_path.exists():
                session = json.loads(session_path.read_bytes())
                fixed_files = [file for file in request["files"] if file.get("scope") != "journal"]
                if (session["area_id"] != area or session["status"] != "frozen"
                        or session.get("kind", "python") != kind
                        or session["manifest"] != self.file_identity(fixed_files)):
                    raise Rejected(409, "SESSION_RESET: incompatible live environment")
        if kind == "journal" and (len(request["files"]) != 1 or request["files"][0].get("scope") != "journal" or request["files"][0].get("path") != "journal.jsonl"):
            raise Rejected(400, "Journal Query requires one journal input")
        size = 0
        mounted = 0
        identities = set()
        for file in request["files"]:
            if set(file) != {"file_id", "path", "scope", "size", "digest"}:
                raise Rejected(400, "invalid file contract")
            identity(file["file_id"])
            if (file["file_id"] in identities or file["scope"] not in (("journal",) if kind == "journal" else ("working", "references", "received"))
                    or not isinstance(file["size"], int) or file["size"] < 0
                    or not re.fullmatch(r"[0-9a-f]{64}", file["digest"])):
                raise Rejected(400, "invalid input manifest")
            identities.add(file["file_id"])
            size += file["size"]
            if file["scope"] != "working":
                mounted += file["size"]
        if size > self.config["working_bytes"]:
            raise Rejected(413, "working quota")
        if self.config["working_bytes"] - mounted <= 0:
            raise Rejected(413, "working quota has no writable space")
        # The transport digest binds the complete request independently of the
        # opaque caller digest (which binds its authenticated public request).
        wire_digest = hashlib.sha256(json.dumps(request, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
        with self.lock:
            if self.path(operation).exists():
                old = self.get(operation)
                # A refusal (`refuse_undelivered`) was never sent a request, so it has no
                # transport digest. A send of the same intent is answered with the refusal:
                # a caller whose cancel reply was lost sends again, and only this record
                # lets it settle. Nothing starts. Another intent is a conflict.
                same = (old["digest"] == request["digest"] if old["wire_digest"] is None
                        else old["wire_digest"] == wire_digest)
                if not same:
                    raise Rejected(409, "IDEMPOTENCY_CONFLICT")
                return self.public(old)
            # A Live preview is a long-lived reader that never publishes its
            # writes, so it takes no writer slot in the Area and does not fence
            # later turns (ADR 0070).
            if kind != "serve":
                pods = self.kube_json(["get", "pods", "-l", "aidash-area=" + area, "-o", "json"])["items"]
                for pod in pods:
                    if session and pod["metadata"]["uid"] == session["pod_uid"]:
                        continue
                    execution = next((s.get("state", {}) for s in pod.get("status", {}).get("containerStatuses", [])
                                      if s["name"] == "execution"), {})
                    if "terminated" not in execution or pod["metadata"]["name"] == self.pod_name(operation):
                        raise Rejected(409, "AREA_BUSY: unaccounted sandbox requires reconciliation")
                area_path = self.root / f"area-{area}.json"
                previous = json.loads(area_path.read_bytes()) if area_path.exists() else None
                if previous:
                    if request["epoch"] < previous["epoch"]:
                        raise Rejected(409, "STALE_EPOCH")
                    old = self.get(previous["operation_id"])
                    if old["status"] not in TERMINAL or not (old.get("termination_confirmed", False) or old.get("writer_frozen", False)):
                        raise Rejected(409, "AREA_BUSY: previous writer has not been confirmed stopped")
            record = {"operation_id": operation, "area_id": area, "epoch": request["epoch"],
                      "image": self.config['image'],
                      "digest": request["digest"], "wire_digest": wire_digest, "status": "awaiting_files" if request["files"] and (not session or kind == "journal") else "accepted",
                      "request": request, "created_at": time.time(), "cancel_requested": False,
                      "termination_confirmed": False, "stdout": "", "truncated": False,
                      "files": [], "error": None, "executed": False}
            if kind == "serve":
                record["port"] = port
                record["serve"] = {"status": "starting", "startup_log": "", "reason": None}
            # Only verify_isolation sets this; no request field can claim it.
            if admission_probe:
                record["admission_probe"] = True
            if request["kind"] in ("python", "journal"):
                record["session_id"] = sid
                if session:
                    record["pod_operation"] = session["pod_operation"]
                    record["pod_uid"] = session["pod_uid"]
                    record["container_id"] = session["container_id"]
            save(self.path(operation), record)
            if session:
                self.begin_allocation_work(record)
            self.aggregate.track(operation, record)
            if kind != "serve":
                save(area_path, {"epoch": request["epoch"], "operation_id": operation})
            if not request["files"] or (session and kind != "journal"):
                self.start(operation)
            return self.public(record)

    def input_chunk(self, operation, file_id, offset, data):
        file_id = identity(file_id)
        with self.lock:
            record = self.get(operation)
            if record["status"] != "awaiting_files":
                raise Rejected(409, "input staging is closed")
            file = next((f for f in record["request"]["files"] if f["file_id"] == file_id), None)
            if file is None:
                raise Rejected(404, "input unavailable")
            if offset < 0 or len(data) > 4194304 or offset + len(data) > file["size"]:
                raise Rejected(400, "invalid chunk range")
            directory = self.root / (operation + ".inputs")
            directory.mkdir(exist_ok=True, mode=0o700)
            path = directory / file_id
            if not path.exists():
                path.touch(mode=0o600)
            with path.open("r+b") as f:
                length = os.fstat(f.fileno()).st_size
                if offset < length:
                    f.seek(offset)
                    if f.read(len(data)) != data:
                        raise Rejected(409, "chunk digest conflict")
                elif offset == length:
                    f.seek(offset)
                    f.write(data)
                    f.flush()
                    os.fsync(f.fileno())
                else:
                    raise Rejected(409, "missing preceding chunk")
            return {"received": offset + len(data)}

    def commit_inputs(self, operation):
        if not self.proven:
            raise Rejected(503, "runner isolation is not proven; new work is refused")
        with self.lock:
            record = self.get(operation)
            if record["status"] != "awaiting_files":
                return self.public(record)
            for file in record["request"]["files"]:
                path = self.root / (operation + ".inputs") / file["file_id"]
                if not path.exists() or path.stat().st_size != file["size"]:
                    raise Rejected(409, "incomplete input")
                with path.open("rb") as f:
                    if hashlib.file_digest(f, "sha256").hexdigest() != file["digest"]:
                        raise Rejected(409, "input digest mismatch")
            record = self.update(operation, status="accepted")
            self.start(operation)
            return self.public(record)

    def output_chunk(self, operation, object_id, offset):
        record = self.get(operation)
        file = next((f for f in record["files"] if f["object_id"] == object_id), None)
        if file is None or record["status"] not in TERMINAL or not re.fullmatch(r"[0-9a-f]{64}", object_id):
            raise Rejected(404, "output unavailable")
        if not 0 <= offset <= file["size"]:
            raise Rejected(400, "invalid output offset")
        with (self.root / (operation + ".files") / object_id).open("rb") as f:
            f.seek(offset)
            data = f.read(4194304)
        return {"data": base64.b64encode(data).decode(), "offset": offset, "size": file["size"], "digest": file["digest"]}

    def acknowledge(self, operation, digest, output_digest=None):
        with self.lock:
            record = self.get(operation)
            uncertain = record['status'] == 'uncertain'
            if record['digest'] != digest or (record['status'] not in ('completed', 'cancelled', 'failed') and
                                               not (uncertain and record.get('termination_confirmed') is True)):
                raise Rejected(409, 'outcome is not ready to acknowledge')
            if operation in self.active:
                raise Rejected(409, 'output collection remains active')
            if record.get('startup_cleanup_pending'):
                raise Rejected(409, 'startup cleanup remains pending')
            if record.get('acknowledged'):
                if uncertain and output_digest != record.get('acknowledged_output_digest'):
                    raise Rejected(409, 'saved output acknowledgement proof changed')
            harvest = uncertain and not record.get('acknowledged') and record.get('output_harvested') is not True
            if harvest:
                self.active.add(operation)
        if harvest:
            try:
                self.harvest_stopped_output(record)
            finally:
                with self.lock:
                    self.active.discard(operation)
            # The caller has not yet adopted the newly recovered bytes.
            raise Rejected(409, 'harvested output must be persisted before acknowledgement')
        with self.lock:
            record = self.get(operation)
            if uncertain and not record.get('acknowledged'):
                if output_digest != self.output_digest(record):
                    raise Rejected(409, 'output snapshot has not been adopted')
                self.update(operation, acknowledged_output_digest=output_digest)
            record = self.update(operation, acknowledged=True, request={}, stdout='', files=[], displays=[])
            for suffix in ('.inputs', '.files'):
                directory = self.root / (operation + suffix)
                if directory.exists():
                    for path in directory.iterdir():
                        path.unlink()
                    directory.rmdir()
            sync_directory(self.root)
            return self.public(record)

    def start(self, operation):
        with self.lock:
            if operation not in self.active:
                record = self.get(operation)
                if record['request']['kind'] in ('python', 'journal') and record.get('pod_uid'):
                    self.begin_allocation_work(record)
                self.active.add(operation)
                kind = record["request"]["kind"]
                target = {"python": self.reconcile_python, "journal": self.reconcile_python, "serve": self.reconcile_serve}.get(kind, self.reconcile)
                threading.Thread(target=target, args=(operation,), daemon=True).start()

    def pod_name(self, operation):
        return "operation-" + operation

    def writable_bytes(self, record):
        mounted = sum(file["size"] for file in record["request"]["files"] if file["scope"] != "working")
        remaining = self.config["working_bytes"] - mounted
        if remaining <= 0:
            raise Rejected(413, "working quota has no writable space")
        return remaining

    def execution_limits(self, record):
        limits = {k: self.config[k] for k in ('cpu', 'memory_bytes', 'processes', 'temporary_bytes')}
        limits['host_tasks'] = self.config.get('host_tasks', 512)
        # Keep the logical size requested by the Pod manifest. The trusted
        # node adapter rounds to its own physical pages; a remote controller
        # can have a different page size from the Linux execution node.
        limits['working_bytes'] = self.writable_bytes(record)
        return limits

    def manifest(self, record):
        profile = self.config
        security = {"allowPrivilegeEscalation": False, "readOnlyRootFilesystem": True,
                    "runAsNonRoot": True, "runAsUser": 10000, "runAsGroup": 10000,
                    "capabilities": {"drop": ["ALL"]}, "seccompProfile": {"type": "RuntimeDefault"}}
        def container(name, command, cpu, memory, mounts):
            return {"name": name, "image": profile["image"], "imagePullPolicy": "IfNotPresent",
                    "command": command, "securityContext": security,
                    "env": [{"name": k, "value": v} for k, v in {
                        "PATH": "/usr/local/bin:/usr/bin:/bin", "HOME": "/tmp/home",
                        "PYTHONUNBUFFERED": "1", "PYTHONDONTWRITEBYTECODE": "1",
                        "LANG": "C.UTF-8", "MPLCONFIGDIR": "/tmp/matplotlib",
                        "OPENBLAS_NUM_THREADS": str(profile["cpu"]), "OMP_NUM_THREADS": str(profile["cpu"])
                    }.items()], "resources": {"requests": {"cpu": "100m", "memory": "64Mi"},
                                              "limits": {"cpu": str(cpu), "memory": str(memory)}},
                    "volumeMounts": mounts}
        worker_mounts = [{"name": name, "mountPath": path, "readOnly": name in ("references", "received", "request") or (name == "work" and record["request"]["kind"] == "journal")}
                         for name, path in (("work", "/work"), ("references", "/references"),
                                            ("received", "/received"), ("request", "/request"), ("temp", "/tmp"))]
        if record["request"]["kind"] == "journal":
            worker_mounts.append({"name": "journal", "mountPath": "/journal", "readOnly": True})
        collector_mounts = [{"name": m["name"], "mountPath": m["mountPath"]} for m in worker_mounts if m["name"] != "temp"]
        collector_mounts.append({"name": "control-temp", "mountPath": "/tmp"})
        worker = container("execution", ["python", "-I", "/opt/aidash/sandbox.py"], profile["cpu"], profile["memory_bytes"], worker_mounts)
        worker["livenessProbe"] = {"httpGet": {"path": "/health", "port": 7070}, "periodSeconds": 1,
                                   "timeoutSeconds": 1, "failureThreshold": 1, "terminationGracePeriodSeconds": 1}
        worker["startupProbe"] = {"httpGet": {"path": "/health", "port": 7070}, "periodSeconds": 1,
                                  "timeoutSeconds": 1, "failureThreshold": 120}
        if record["request"]["kind"] in ("python", "journal"):
            worker.pop("livenessProbe")
            worker.pop("startupProbe")
        if record["request"]["kind"] == "serve":
            # Declare the port the Preview gateway proxies to. Only a TCP
            # readiness probe decides "the port accepts connections": a Web
            # server may answer 404 for /, which an HTTP probe would refuse.
            port = record["request"]["port"]
            worker["ports"] = [{"name": "preview", "containerPort": port}]
            worker["readinessProbe"] = {"tcpSocket": {"port": port}, "periodSeconds": 1,
                                        "timeoutSeconds": 1, "failureThreshold": 120}
        collector = container("collector", ["python", "-I", "/opt/aidash/collector.py", "serve"], "250m", "256Mi", collector_mounts)
        collector["env"].append({"name": "AIDASH_OUTPUT_BYTES", "value": str(profile["output_bytes"])})
        volumes = [{"name": name, "emptyDir": {"medium": "Memory", "sizeLimit": str(size)}} for name, size in (
            ("work", self.writable_bytes(record)), ("references", profile["working_bytes"]),
            ("received", profile["working_bytes"]), ("request", 1048576),
            ("temp", profile["temporary_bytes"]), ("control-temp", profile["output_bytes"] * 4 + 1048576))]
        if record["request"]["kind"] == "journal":
            volumes.append({"name": "journal", "emptyDir": {"medium": "Memory", "sizeLimit": str(2 * profile["working_bytes"])}})
        labels = {"aidash-sandbox": "true", "aidash-area": record["area_id"]}
        if record["request"]["kind"] == "journal":
            labels["aidash-journal"] = "true"
        return {"apiVersion": "v1", "kind": "Pod", "metadata": {"name": self.pod_name(record["operation_id"]),
                "finalizers": [] if record.get("admission_probe") else [FINALIZER],
                "labels": labels, "annotations": {"aidash/digest": record["wire_digest"],
                "aidash/epoch": str(record["epoch"]), "aidash/area": record["area_id"]}},
                "spec": {"runtimeClassName": profile["runtime_class"], "restartPolicy": "Never",
                         "resources": {"limits": {"cpu":str(profile['cpu']), "memory":str(profile['memory_bytes'])}},
                         "automountServiceAccountToken": False, "enableServiceLinks": False,
                         "shareProcessNamespace": False, "hostNetwork": False, "hostPID": False, "hostIPC": False,
                         "dnsPolicy": "None", "dnsConfig": {"nameservers": ["127.0.0.1"]},
                         "securityContext": {"fsGroup": 10000, "fsGroupChangePolicy": "OnRootMismatch"},
                         "nodeSelector": profile.get('node_selector', {}),
                         "tolerations": profile.get('tolerations', []),
                         "terminationGracePeriodSeconds": 1, "containers": [worker, collector], "volumes": volumes}}

    def pod(self, operation):
        for _ in range(ALLOCATION_OBSERVATION_ATTEMPTS):
            # Retry if another observer committed while the provider read was in flight.
            with self.lock:
                record = self.get(operation) if self.path(operation).exists() else None
                uid = record.get('pod_uid') if record else None
                path = self.allocations / (identity(uid) + '.json') if uid else None
                seen = json.loads(path.read_bytes()) if path and path.exists() else None
                seen = tuple(seen.get(k) for k in ('resource_version', 'receipt', 'quarantined', 'released')) if seen else None
            result = self.kube(["get", "pod", self.pod_name(operation), "-o", "json", "--ignore-not-found"])
            pod = json.loads(result.stdout) if result.stdout.strip() else None
            with self.lock:
                if pod and self.path(operation).exists():
                    record = self.get(operation)
                    if record.get('pod_uid') and pod.get('metadata', {}).get('uid') != record['pod_uid']:
                        if not record.get('admission_probe'):
                            try:
                                self.observe_allocation(record, pod)
                            except Rejected:
                                pass
                        raise SandboxGone()
                    if not record.get("admission_probe"):
                        path = path or self.allocations / (identity(pod['metadata']['uid']) + '.json')
                        current = json.loads(path.read_bytes()) if path.exists() else None
                        current = tuple(current.get(k) for k in ('resource_version', 'receipt', 'quarantined', 'released')) if current else None
                        if current != seen:
                            continue
                        self.observe_allocation(record, pod)
                return pod
        # Leave evidence unchanged; the next poll can try the provider again.
        return pod

    def exec_collector(self, operation, action, data=None, timeout=45):
        return self.kube(["exec", "-i", self.pod_name(operation), "-c", "collector", "--",
                          "python", "-I", "/opt/aidash/collector.py", *action], data=data, timeout=timeout)

    @staticmethod
    def file_identity(files):
        selected = sorted(({k: f[k] for k in ("path", "scope", "size", "digest")} for f in files), key=lambda f: (f["scope"], f["path"]))
        return hashlib.sha256(json.dumps(selected, sort_keys=True, separators=(",", ":")).encode()).hexdigest()

    def guard(self, record, action, **fields):
        arguments = self.config.get("node_guard")
        if not arguments:
            raise RuntimeError("Execution requires a verified node lifecycle adapter")
        image = record.get('image')
        cluster_node = record.get('cluster_node')
        remote = isinstance(arguments, dict)
        if image is None or (remote and cluster_node is None):
            # Upgrade old journals without trusting caller-controlled image
            # names. A live API object must match the already observed Pod UID.
            pod = self.pod(record.get('pod_operation',record['operation_id']))
            if not pod or pod['metadata']['uid'] != record['pod_uid']:
                raise RuntimeError('old journal image binding unavailable')
            image = next(c['image'] for c in pod['spec']['containers'] if c['name']=='execution')
            cluster_node = pod['spec']['nodeName']
            # Persist bindings for pre-cluster journals before using them.
            if self.path(record['operation_id']).exists():
                self.update(record['operation_id'], image=image, cluster_node=cluster_node)
        if remote:
            guards = self.kube_json(['--namespace', arguments['namespace'], 'get', 'pods',
                                    '-l', arguments['selector'], '--field-selector',
                                    'spec.nodeName=' + cluster_node, '-o', 'json'])['items']
            ready = [pod for pod in guards if not pod['metadata'].get('deletionTimestamp')
                     and pod['spec']['nodeName'] == cluster_node
                     and any(c['type'] == 'Ready' and c['status'] == 'True'
                             for c in pod.get('status', {}).get('conditions', []))]
            if len(ready) != 1:
                raise RuntimeError('exact Cluster Node guard unavailable; termination remains unconfirmed')
            arguments = self.command + ['--namespace', arguments['namespace'], 'exec', '-i',
                         ready[0]['metadata']['name'], '-c', arguments.get('container', 'guard'),
                         '--', *arguments['command']]
        request = dict(container_id=record["container_id"], pod_uid=record["pod_uid"],
                       area_id=record["area_id"], epoch=record["epoch"], image=image,
                       action=action, **fields)
        result = subprocess.run(arguments, input=json.dumps(request).encode(), stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, timeout=45, check=False,
                                **({'env': self.environment} if remote else {}))
        value = json.loads(result.stdout)
        if result.returncode:
            raise RuntimeError("node lifecycle adapter: " + str(value.get("error", "failed")))
        return value

    def guard_execution_pod(self, pod, settings, container_name, container_id, epoch, *, action='v2_limits', **fields):
        name = pod['metadata']['name']
        if not re.fullmatch(r'scratch-[0-9a-f]{48}', name):
            raise ValueError('invalid Scratch Space Pod name')
        area = str(uuid.UUID(hex=name[8:40]))
        entries = pod['spec']['initContainers'] if container_name == 'helpers' else pod['spec']['containers']
        image = next(item['image'] for item in entries if item['name'] == container_name)
        if image != self.execution_pods.config['helper_image']:
            raise ValueError('Execution Pod helper image differs from the verified image')
        record = {'operation_id': area, 'area_id': area, 'pod_uid': pod['metadata']['uid'],
                  'container_id': container_id, 'epoch': epoch, 'image': image,
                  'cluster_node': pod['spec']['nodeName']}
        return self.guard(record, action, protocol='aidash-runner/2', pod_name=name,
                          container_name=container_name, cpu=settings['cpu'],
                          memory_bytes=settings['memory_bytes'], capacity_bytes=settings['capacity_bytes'],
                          host_tasks=self.execution_pods.config.get('host_tasks', 512), **fields)

    def terminate_execution_pod(self, guard):
        name = guard['pod_name']
        if not re.fullmatch(r'scratch-[0-9a-f]{48}', name):
            raise ValueError('invalid Scratch Space Pod name')
        area = str(uuid.UUID(hex=name[8:40]))
        record = {'operation_id': area, 'area_id': area, 'pod_uid': guard['pod_uid'],
                  'container_id': guard['container_id'], 'epoch': guard['epoch'],
                  'image': guard['image'], 'cluster_node': guard['cluster_node']}
        return self.guard(record, 'terminate', protocol='aidash-runner/2', pod_name=name,
                          container_name=guard['container_name'])

    def session(self, sid):
        path = self.root / ("session-" + identity(sid) + ".json")
        if not path.exists():
            return {"status": "absent", "session_id": sid}
        session = json.loads(path.read_bytes())
        pod = self.pod(session["pod_operation"])
        states = {s["name"]: s.get("state", {}) for s in pod.get("status", {}).get("containerStatuses", [])} if pod else {}
        live = bool(pod and pod["metadata"]["uid"] == session["pod_uid"] and "running" in states.get("execution", {}) and "running" in states.get("collector", {}))
        if live:
            live = self.guard(self.get(session["operation_id"]), "status")["status"] == "paused"
        return {"status": session["status"] if live else "reset", "session_id": sid, "live": live,
                "reason": None if live else "interpreter_lost_or_idle", "last_used": session.get("last_used")}

    def stop_session(self, sid, expected_instance=None):
        if expected_instance is not None and expected_instance != self.instance:
            raise Rejected(409, "Python session runner instance changed")
        path = self.root / ("session-" + identity(sid) + ".json")
        if not path.exists():
            return {"status": "absent", "termination_confirmed": True}
        session = json.loads(path.read_bytes())
        record = self.get(session["operation_id"])
        if session.get("termination_confirmed") is True:
            self.close_allocation(record)
            return {"status": "stopped", "termination_confirmed": True}
        if self.guard(record, "terminate").get("termination_confirmed") is not True:
            raise RuntimeError("Python termination not confirmed")
        session["status"] = "stopped"
        session["termination_confirmed"] = True
        save(path, session)
        # Physical stop proof is distinct from provider timestamps. Keep the API
        # object until allocation housekeeping has durably captured those timestamps.
        self.close_allocation(record)
        return {"status": "stopped", "termination_confirmed": True}

    def reconcile_python(self, operation):
        last_warning = 0
        try:
            while True:
                try:
                    record = self.get(operation)
                    if not record.get('executed') and record.get('startup_at') is None:
                        record = self.update(operation, startup_at=time.time())
                    if not record.get('executed') and time.time() >= record['startup_at'] + START_LIMIT_SECONDS:
                        self.fail_start(operation, 'Python startup timed out before dispatch')
                        return
                    self._reconcile_python_once(operation)
                    return
                except SandboxGone:
                    if not self.get(operation).get('executed'):
                        self.fail_start(operation, 'Python sandbox identity changed before dispatch')
                    else:
                        self.update(operation, status='uncertain', termination_confirmed=True, writer_frozen=False,
                                    output_harvested=True, output_discarded=True, files=[], finished_at=time.time(),
                                    uncertain_at=time.time(), error={'code':'SANDBOX_LOST',
                                    'message':"Sandbox is gone; the command's file changes were not saved."})
                    return
                except Exception as error:
                    record = self.get(operation)
                    if not record.get('executed'):
                        if isinstance(error, StartupIdentityMismatch):
                            self.fail_start(operation, str(error))
                            return
                        if time.time() >= record['startup_at'] + START_LIMIT_SECONDS:
                            self.fail_start(operation, 'Python startup timed out: ' + str(error))
                            return
                        time.sleep(1)
                        continue
                    now = time.time()
                    stopped_at = record.get('stop_confirmed_at')
                    if stopped_at is not None:
                        if now >= stopped_at + STOP_WATCH_SECONDS:
                            self.update(operation, status='uncertain', uncertain_at=now, finished_at=now,
                                        error={'code':'OUTPUT_LOST', 'message':"The command's file changes were not saved."})
                            return
                        try:
                            collected = self.collect_stopped_output(record)
                            if collected is None:
                                return
                            exported, captured = collected
                            saved = self.update(operation, status=record.get('pending_status', 'failed'), termination_confirmed=True,
                                                writer_frozen=False, output_harvested=True, finished_at=now, files=exported['files'],
                                                stdout=captured['stdout'], truncated=True, exit_code=record.get('exit_code', 137),
                                                deadline_expired=cell_timed_out(record),
                                                error=record.get('pending_error') or {'code':'SESSION_LOST', 'message':'Interpreter stopped; its saved files were recovered.'})
                            self.allocation_harvested(saved)
                            self.close_allocation(saved)
                            return
                        except Exception:
                            pass
                    started = record.get('running_at') or now
                    if not record.get('running_at'):
                        record = self.update(operation, running_at=started)
                    deadline = started + record['request']['seconds'] + 30
                    if not record.get('termination_confirmed') and now >= deadline:
                        record = self.update(operation, status='cancelling', limit_stop_requested=True)
                        if now >= started + record['request']['seconds'] + STOP_WATCH_SECONDS and now - last_warning >= 60:
                            try:
                                self.allocation_harvested(record)
                                self.close_allocation(record)
                            except Exception as cleanup_error:
                                error = cleanup_error
                            print(f"Deployment Administrator: awaiting Python Pod deletion for {operation}: {error}", file=sys.stderr, flush=True)
                            last_warning = now
                    time.sleep(1)
        finally:
            with self.lock:
                self.active.discard(operation)

    def refresh_journal(self, record, pod_operation):
        if self.guard(record, "status").get("status") != "paused":
            raise RuntimeError("Journal kernel is not frozen")
        operation = record["operation_id"]
        file = record["request"]["files"][0]
        with (self.root / (operation + ".inputs") / file["file_id"]).open("rb") as source:
            offset = 0
            while True:
                data = source.read(4194304)
                if not data and offset:
                    break
                self.exec_collector(pod_operation, ["journal-stage", operation, str(offset)], data)
                offset += len(data)
                if not data:
                    break
        self.exec_collector(pod_operation, ["journal-commit", operation, str(file["size"]), file["digest"]])

    def _reconcile_python_once(self, operation):
        record = self.get(operation)
        pod_operation = record.get("pod_operation", operation)
        pod = self.pod(pod_operation)
        cold = pod_operation == operation
        if pod is None and record.get('executed'):
            raise SandboxGone()
        if pod and record.get('pod_uid') and pod.get('metadata', {}).get('uid') != record['pod_uid']:
            raise SandboxGone()
        if cold and pod and not record.get('executed') and pod.get('metadata', {}).get('annotations', {}).get('aidash/digest') != record['wire_digest']:
            raise StartupIdentityMismatch('Python sandbox identity mismatch')
        if cold and pod is None:
            if record["status"] not in ("accepted", "starting"):
                raise SandboxGone()
            self.update(operation, status="starting")
            self.kube(["create", "-f", "-"], json.dumps(self.manifest(record)).encode(), check=False)
        if record.get('executed') and record.get('cancel_requested'):
            if self.guard(record, 'terminate').get('termination_confirmed') is not True:
                raise RuntimeError('Python termination not confirmed')
            self.update(operation, status='finishing', termination_confirmed=True, writer_frozen=False,
                        stop_confirmed_at=record.get('stop_confirmed_at') or time.time(),
                        pending_status='cancelled', pending_error={'code':'CANCELLED', 'message':'Python stopped; session reset.'}, exit_code=137)
            raise RuntimeError('Python stopped; collecting its output')
        while True:
            pod = self.pod(pod_operation)
            if pod is None and record.get('executed'):
                raise SandboxGone()
            if pod is None and cold:
                self.update(operation, pod_uid=None)
                raise RuntimeError('Python sandbox absent before dispatch')
            if pod:
                if cold and not record.get('executed') and pod.get('metadata', {}).get('annotations', {}).get('aidash/digest') != record['wire_digest']:
                    raise StartupIdentityMismatch('Python sandbox identity mismatch')
                statuses = {s["name"]: s for s in pod.get("status", {}).get("containerStatuses", [])}
                execution = statuses.get('execution', {}).get('state', {})
                if record.get('executed') and 'terminated' in execution:
                    self.update(operation, status='finishing', termination_confirmed=True, writer_frozen=False,
                                stop_confirmed_at=record.get('stop_confirmed_at') or time.time(),
                                pending_status='failed', pending_error={'code':'SESSION_LOST', 'message':'Interpreter stopped during the cell; code was not replayed.'}, exit_code=137)
                    raise RuntimeError('Python stopped; collecting its output')
                if record.get('executed') and 'running' in execution:
                    break
                if all("running" in statuses.get(n, {}).get("state", {}) for n in ("execution", "collector")):
                    break
                if any("terminated" in s.get("state", {}) for s in statuses.values()):
                    raise RuntimeError("Python environment stopped before dispatch; no replay")
            if not record.get('executed') and time.time() >= record['startup_at'] + START_LIMIT_SECONDS:
                raise RuntimeError("Python environment not ready")
            time.sleep(.25)
        if (record.get("pod_uid") or pod["metadata"]["uid"]) != pod["metadata"]["uid"]:
            raise SandboxGone()
        cid = statuses["execution"]["containerID"].split("://", 1)[1]
        record = self.update(operation, pod_operation=pod_operation, pod_uid=pod["metadata"]["uid"], container_id=cid,
                             cluster_node=pod['spec']['nodeName'], image=pod['spec']['containers'][0]['image'])
        for attempt in range(ALLOCATION_OBSERVATION_ATTEMPTS):
            with self.lock:
                allocation = self.allocations / (identity(record['pod_uid']) + '.json')
                current = json.loads(allocation.read_bytes()) if allocation.exists() else None
                if not current:
                    raise RuntimeError('allocation journal unavailable')
                if current.get('receipt'):
                    break
                if current.get('resource_version') == pod['metadata'].get('resourceVersion'):
                    self.observe_allocation(record, pod)
                    break
            if attempt + 1 == ALLOCATION_OBSERVATION_ATTEMPTS:
                raise RuntimeError('allocation observation did not stabilize')
            pod = self.pod(pod_operation)
            if not pod or pod['metadata']['uid'] != record['pod_uid']:
                raise RuntimeError('Python environment identity changed')
        if not record.get('executed'):
            limits = self.guard(record, 'limits', **self.execution_limits(record))
            self.update(operation, resource_evidence=limits)
        if pod_operation == operation and not record.get("hydrated"):
            prepared = json.loads(self.exec_collector(pod_operation, ["prepared"]).stdout)
            if prepared["digest"] is None:
                for file in record["request"]["files"]:
                    with (self.root / (operation + ".inputs") / file["file_id"]).open("rb") as source:
                        offset = 0
                        while True:
                            data = source.read(4194304)
                            if not data and offset:
                                break
                            self.exec_collector(pod_operation, ["write", file["scope"], file["path"], str(offset)], data)
                            offset += len(data)
                            if not data:
                                break
                payload = dict({k:v for k,v in record["request"].items() if k != "usage_tenant"}, working_bytes=self.config["working_bytes"], processes=self.config["processes"])
                self.exec_collector(pod_operation, ["hydrate"], json.dumps(payload).encode())
            elif prepared["digest"] != record["request"]["digest"]:
                raise StartupIdentityMismatch("Python input identity changed")
            record = self.update(operation, hydrated=True)
        elif record["request"]["kind"] == "journal" and not record.get("journal_refreshed"):
            # Only the dedicated Journal mount may change while this heap is frozen.
            self.refresh_journal(record, pod_operation)
            record = self.update(operation, journal_refreshed=True)
        record = self.update(operation, executed=True, status=record.get('status') if record.get('status') == 'cancelling' else 'running',
                             running_at=record.get('running_at') or time.time())
        if not record.get('cancel_requested'):
            self.guard(record, "cell", operation_id=operation, code=record["request"]["code"],
                       digest=record["digest"], seconds=record["request"]["seconds"], egress=record["request"].get("egress"))
        memory_live = True
        reply_lost = False
        while True:
            current = self.get(operation)
            if current["cancel_requested"]:
                if self.guard(record, "terminate").get('termination_confirmed') is not True:
                    raise RuntimeError('Python termination not confirmed')
                memory_live = False
                result = dict(status="cancelled", exit_code=137, stdout=current.get("stdout", ""),
                              truncated=True, error={"code":"CANCELLED", "message":"Python stopped; session reset."})
                break
            result = self.guard(record, "cell_status", operation_id=operation)
            if result["status"] == "stopped" and result.get("termination_confirmed"):
                memory_live = False
                result = dict(status="failed", exit_code=137, stdout=current.get("stdout", ""),
                              truncated=True, error={"code":"SESSION_LOST", "message":"Interpreter stopped during the cell; code was not replayed."})
                break
            if result["status"] in ("completed", "failed"):
                break
            if result["status"] == "uncertain":
                # The interpreter's reply was lost, so the cell may have finished. Its code is never replayed.
                # Watch it like any running cell: the stop below ends it at its limit, and only then does its
                # outcome stay uncertain. Until then the Working Area stays busy, not unavailable (#709).
                reply_lost = True
            elif 'stdout' in result:
                self.update(operation, stdout=result['stdout'], stdout_offset=result.get('stdout_offset', 0),
                            truncated=result.get('truncated', False), output_fetched_at=time.time())
            pod = self.pod(pod_operation)
            if not pod or pod.get('metadata', {}).get('uid') != record['pod_uid']:
                raise SandboxGone()
            if any("terminated" in s.get("state", {}) for s in pod.get("status", {}).get("containerStatuses", [])):
                if self.guard(record, "terminate").get('termination_confirmed') is not True:
                    raise RuntimeError('Python termination not confirmed')
                memory_live = False
                result = dict(status="failed", exit_code=137, stdout=result.get("stdout", ""),
                              truncated=True, error={"code":"SESSION_LOST", "message":"Interpreter stopped during the cell; code was not replayed."})
                break
            if current.get('limit_stop_requested') or time.time() > record['running_at'] + record['request']['seconds'] + 30:
                # The cell is seen unfinished past its limit: the Runner's watchdog stops it. Nobody cancelled, so
                # it never reads as cancelled; a cell that finished during an outage was settled above instead.
                self.update(operation, status='cancelling', limit_stop_requested=True)
                if self.guard(record, "terminate").get('termination_confirmed') is not True:
                    raise RuntimeError('Python termination not confirmed')
                memory_live = False
                if reply_lost:
                    result = dict(status="uncertain", exit_code=None, stdout=current.get("stdout", ""), truncated=True,
                                  error={"code":"EXECUTION_UNVERIFIED", "message":"The Python reply was lost, so the cell's outcome is unknown. It was stopped, its code was not replayed, and the session was reset."})
                else:
                    result = dict(status="failed", exit_code=137, stdout=current.get("stdout", ""), truncated=True,
                                  error={"code":"SESSION_LOST", "message":"Interpreter stopped during the cell; code was not replayed."})
                break
            time.sleep(.2)
        if memory_live:
            frozen = self.guard(record, "freeze", idle_seconds=self.config["idle_seconds"])
            if not frozen["writer_frozen"]:
                raise RuntimeError("Python memory no longer live")
        finishing = self.update(operation, status="finishing", writer_frozen=memory_live, termination_confirmed=not memory_live,
                                stop_confirmed_at=None if memory_live else (self.get(operation).get('stop_confirmed_at') or time.time()),
                                pending_status=result['status'], pending_error=result.get('error'), exit_code=result['exit_code'],
                                output_fetched_at=time.time() if memory_live and 'stdout' in result else self.get(operation).get('output_fetched_at'),
                                stdout=result.get('stdout', ''), displays=result.get('displays', []), truncated=result.get('truncated', False))
        deadline_expired = cell_timed_out(finishing)
        exported = self.guard(record, "export", working_bytes=self.writable_bytes(record))
        if 'refused' in exported:
            if memory_live:
                self.guard(record, 'terminate')
            session = dict(session_id=record['session_id'], area_id=record['area_id'], operation_id=operation, kind=record['request']['kind'],
                           pod_operation=pod_operation, pod_uid=record['pod_uid'], container_id=cid,
                           status='stopped', last_used=time.time(), manifest=self.file_identity([f for f in record['request']['files'] if f['scope'] != 'journal']))
            save(self.root / ('session-' + record['session_id'] + '.json'), session)
            self.refuse_export(operation, exported['refused'], pod_operation, writer_frozen=False,
                               stdout=result.get('stdout', ''), truncated=result.get('truncated', False),
                               exit_code=result['exit_code'], deadline_expired=deadline_expired)
            return
        directory = self.root / (operation + ".files")
        directory.mkdir(exist_ok=True, mode=0o700)
        for file in exported["files"]:
            digest = hashlib.sha256()
            with (directory / file["object_id"]).open("wb") as output:
                offset = 0
                while offset < file["size"]:
                    chunk = self.guard(record, "read", path=file["path"], offset=offset)
                    data = base64.b64decode(chunk["data"], validate=True)
                    if not data or offset + len(data) > file["size"]:
                        raise RuntimeError("Python file changed after freeze")
                    output.write(data)
                    digest.update(data)
                    offset += len(data)
                output.flush()
                os.fsync(output.fileno())
            if digest.hexdigest() != file["digest"]:
                raise RuntimeError("Python file integrity")
        sync_directory(directory)
        manifest = [f for f in record["request"]["files"] if f["scope"] not in ("working", "journal")]
        manifest += [dict(f, scope="working") for f in exported["files"]]
        session = dict(session_id=record["session_id"], area_id=record["area_id"], operation_id=operation, kind=record["request"]["kind"],
                       pod_operation=pod_operation, pod_uid=record["pod_uid"], container_id=cid,
                       status="frozen" if memory_live else "stopped", last_used=time.time(), manifest=self.file_identity(manifest))
        save(self.root / ("session-" + record["session_id"] + ".json"), session)
        self.update(operation, status=result["status"], writer_frozen=memory_live, termination_confirmed=not memory_live,
                    output_harvested=True, finished_at=time.time(), stdout=result.get("stdout", ""), truncated=result.get("truncated", False),
                    displays=result.get("displays", []), files=exported["files"], error=result.get("error"), exit_code=result["exit_code"],
                    deadline_expired=deadline_expired)
        self.allocation_harvested(self.get(operation), idle=memory_live)
        if not memory_live:
            self.close_allocation(record)

    def startup_log(self, operation):
        """A bounded tail of the Web server's own startup output."""
        try:
            result = self.kube(["logs", self.pod_name(operation), "-c", "execution",
                                "--limit-bytes=16384"], check=False, timeout=20)
            return result.stdout.decode(errors="replace")[-16384:]
        except Exception:
            return ""

    def stop_serve(self, record, status, reason):
        """Stop a Live preview's Pod. Termination is confirmed only by the
        independent node proof; an unconfirmed stop stays retryable."""
        operation = record["operation_id"]
        log = self.startup_log(operation)
        confirmed = False
        try:
            if record.get("container_id") and record.get("pod_uid"):
                self.guard(record, "terminate")
                confirmed = True
                arguments = ["delete", "pod", self.pod_name(operation), "--wait=false",
                             "--force", "--grace-period=0"]
            else:
                # Nothing ran yet: a completed graceful deletion is the proof
                # that the kubelet stopped every container.
                arguments = ["delete", "pod", self.pod_name(operation), "--wait=true",
                             "--timeout=60s"]
            self.kube(arguments, timeout=70, check=False)
            if self.pod(operation) is None:
                confirmed = True
        except Exception as error:
            print(f"Live preview stop {operation}: {error}", file=sys.stderr, flush=True)
        # The terminal status and startup log are the preview's whole durable
        # output; marking it harvested lets restart recovery replay the handoff.
        self.update(operation, status=status, finished_at=time.time(),
                    termination_confirmed=confirmed, output_harvested=True,
                    serve={"status": "stopped" if confirmed else "stopping",
                           "startup_log": log, "reason": reason,
                           "pod_host": (record.get("serve") or {}).get("pod_host")})
        # A preview exports nothing: its terminal status and startup log are the
        # durable output, so they hand the allocation to finalizer cleanup.
        self.allocation_harvested(self.get(operation))

    def reconcile_serve(self, operation):
        """Hold a Live preview's Pod open and stop it on cancel or its absolute
        deadline. It never exports /work: its writes are not published."""
        try:
            record = self.get(operation)
            pod = self.pod(operation)
            if pod is None:
                if record["status"] != "accepted":
                    raise RuntimeError("preview creation outcome uncertain; no replay")
                record = self.update(operation, status="starting")
                result = self.kube(["create", "-f", "-"], json.dumps(self.manifest(record)).encode(), check=False)
                pod = self.pod(operation)
                if pod is None:
                    raise RuntimeError("preview creation outcome unknown: " + result.stderr.decode(errors="replace")[-500:])
            uid = pod["metadata"]["uid"]
            if record.get("pod_uid", uid) != uid:
                raise RuntimeError("preview env replaced")
            deadline = record["created_at"] + record["request"]["seconds"]
            while True:
                current = self.get(operation)
                if current["cancel_requested"]:
                    self.stop_serve(current, "cancelled", "cancelled")
                    return
                pod = self.pod(operation)
                if pod is None or pod["metadata"]["uid"] != uid:
                    raise RuntimeError("preview Pod disappeared before it was ready")
                statuses = {s["name"]: s for s in pod.get("status", {}).get("containerStatuses", [])}
                execution = statuses.get("execution", {}).get("state", {})
                if "terminated" in execution:
                    raise RuntimeError("preview server stopped during startup")
                if "running" in execution and not record.get("executed"):
                    record = self.update(operation, pod_uid=uid,
                                         container_id=statuses['execution']['containerID'].split('://', 1)[1],
                                         cluster_node=pod['spec']['nodeName'],
                                         image=pod['spec']['containers'][0]['image'])
                    limits = self.guard(record, 'limits', **self.execution_limits(record))
                    self.update(operation, resource_evidence=limits)
                    prepared = json.loads(self.exec_collector(operation, ["prepared"]).stdout)
                    if prepared["digest"] is None:
                        for file in record["request"]["files"]:
                            with (self.root / (operation + ".inputs") / file["file_id"]).open("rb") as source:
                                offset = 0
                                while True:
                                    data = source.read(4194304)
                                    if not data and offset:
                                        break
                                    self.exec_collector(operation, ["write", file["scope"], file["path"], str(offset)], data)
                                    offset += len(data)
                                    if not data:
                                        break
                    elif prepared["digest"] != record["request"]["digest"]:
                        raise RuntimeError("preview input identity changed")
                    payload = dict(record["request"], working_bytes=self.config["working_bytes"],
                                   processes=self.config["processes"])
                    reply = json.loads(self.exec_collector(
                        operation, ["hydrate"], json.dumps(payload).encode(), timeout=120).stdout)
                    now = time.time()
                    record = self.update(operation, executed=True, status="running", running_at=now,
                                         started_at=reply.get("started_at") or now)
                    deadline = record["started_at"] + record["request"]["seconds"]
                ready = any(condition["type"] == "Ready" and condition["status"] == "True"
                            for condition in pod.get("status", {}).get("conditions", []))
                if ready:
                    self.update(operation, serve={"status": "running",
                                                  "startup_log": self.startup_log(operation),
                                                  "reason": None,
                                                  "pod_host": pod["status"].get("podIP")})
                    break
                if time.monotonic() - record.get("running_at", time.monotonic()) > 120:
                    self.stop_serve(self.get(operation), "failed", "port_timeout")
                    return
                time.sleep(.25)
            while True:
                current = self.get(operation)
                if current["cancel_requested"]:
                    self.stop_serve(current, "cancelled", "cancelled")
                    return
                if time.time() >= deadline:
                    self.stop_serve(current, "cancelled", "absolute")
                    return
                pod = self.pod(operation)
                if pod is None:
                    raise RuntimeError("preview Pod disappeared")
                statuses = {s["name"]: s for s in pod.get("status", {}).get("containerStatuses", [])}
                if any("terminated" in status.get("state", {}) for status in statuses.values()):
                    self.kube(["delete", "pod", self.pod_name(operation), "--wait=false"], check=False)
                    self.update(operation, status="failed", finished_at=time.time(),
                                termination_confirmed=True, output_harvested=True,
                                serve={"status": "stopped", "startup_log": self.startup_log(operation),
                                       "reason": "server_exited",
                                       "pod_host": (current.get("serve") or {}).get("pod_host")})
                    self.allocation_harvested(self.get(operation))
                    return
                time.sleep(1)
        except Exception as error:
            print(f"Live preview reconciliation {operation}: {error}", file=sys.stderr, flush=True)
            try:
                record = self.get(operation)
                if record.get("container_id") and record.get("pod_uid"):
                    self.stop_serve(record, "failed", "start_failed")
                else:
                    self.kube(["delete", "pod", self.pod_name(operation), "--wait=false",
                               "--force", "--grace-period=0"], check=False)
                    self.update(operation, status="failed", finished_at=time.time(),
                                termination_confirmed=True,
                                serve={"status": "stopped", "startup_log": self.startup_log(operation),
                                       "reason": "start_failed"})
            except Exception as failed:
                print(f"Live preview cleanup {operation}: {failed}", file=sys.stderr, flush=True)
                self.update(operation, status="failed", error=str(failed)[-2000:])
        finally:
            with self.lock:
                self.active.discard(operation)

    def refuse_export(self, operation, refused, _pod_operation, **fields):
        """End an operation whose Working Area export was refused.

        The refusal is certain and final: nothing is published, so the area
        keeps its previous revision, and the export is never retried.
        """
        fields['output_harvested'] = True
        record = self.update(operation, status='failed', termination_confirmed=True, finished_at=time.time(),
                             error=export_error(refused), **fields)
        self.allocation_harvested(record, idle=False)
        self.close_allocation(record)

    def recover_stopped_shell(self, record):
        # Collector RPCs can fail before the Sentry's death becomes observable.
        # Wait for exact-bound physical stop evidence; never replay the command
        # or export files while a writer may still be alive.
        deadline = time.monotonic() + 10
        while True:
            try:
                if self.guard(record, 'status').get('termination_confirmed') is not True:
                    raise RuntimeError('physical termination proof unavailable')
                break
            except Exception:
                if time.monotonic() >= deadline:
                    raise
                time.sleep(.25)
        collected = self.collect_stopped_output(record)
        if collected is None:
            return
        exported, captured = collected
        operation = record['operation_id']
        self.update(operation, status='failed', exit_code=137, termination_confirmed=True,
                    output_harvested=True, stdout=captured['stdout'], truncated=True, files=exported['files'],
                    finished_at=time.time(), error={'code':'SANDBOX_LOST', 'message':'Sandbox stopped; saved files were recovered and output may be incomplete.'})
        self.allocation_harvested(self.get(operation))
        self.close_allocation(record)

    def collect_stopped_output(self, record, *, refuse=True):
        exported = ({'files': []} if record['request']['kind'] == 'verify' else
                    self.guard(record, 'export', working_bytes=self.writable_bytes(record)))
        operation = record['operation_id']
        if 'refused' in exported:
            if not refuse:
                raise StoppedExportRefused(exported['refused'])
            captured = ({'stdout':record.get('stdout', '')} if record['request']['kind'] in ('python', 'journal') else self.guard(record, 'logs'))
            self.refuse_export(operation, exported['refused'], operation, stdout=captured['stdout'], truncated=True, output_harvested=True,
                               deadline_expired=record['request']['kind'] in ('python', 'journal') and cell_timed_out(record))
            return
        captured = ({'stdout':record.get('stdout', '')} if record['request']['kind'] in ('python', 'journal') else self.guard(record, 'logs'))
        directory = self.root / (operation + '.files')
        directory.mkdir(exist_ok=True, mode=0o700)
        for file in exported['files']:
            digest, offset = hashlib.sha256(), 0
            with (directory / file['object_id']).open('wb') as output:
                while offset < file['size']:
                    chunk = self.guard(record, 'read', path=file['path'], offset=offset)
                    data = base64.b64decode(chunk['data'], validate=True)
                    if not data or offset + len(data) > file['size']:
                        raise RuntimeError('stopped output length changed')
                    output.write(data)
                    digest.update(data)
                    offset += len(data)
                output.flush()
                os.fsync(output.fileno())
            if digest.hexdigest() != file['digest']:
                raise RuntimeError('stopped output integrity')
        sync_directory(directory)
        return exported, captured

    def collector_holds_output(self, record):
        # A finished execution container leaves no runsc state for the guard to
        # read, while its collector still serves /work, so reading stays possible.
        if record['request']['kind'] in ('python', 'journal'):
            return False
        try:
            pod = self.pod(record['operation_id'])
        except Exception:
            return False
        if not pod or pod['metadata']['uid'] != record.get('pod_uid'):
            return False
        states = {s['name']: s.get('state', {}) for s in pod.get('status', {}).get('containerStatuses', [])}
        return 'terminated' in states.get('execution', {}) and 'running' in states.get('collector', {})

    def read_collector_files(self, operation, exported):
        directory = self.root / (operation + ".files")
        directory.mkdir(exist_ok=True, mode=0o700)
        for file in exported["files"]:
            output_path = directory / file["object_id"]
            digest = hashlib.sha256()
            with output_path.open("wb") as output:
                offset = 0
                while offset < file["size"]:
                    data = self.exec_collector(operation, ["read", file["path"], str(offset)]).stdout
                    if not data or offset + len(data) > file["size"]:
                        raise RuntimeError("output file length changed")
                    output.write(data)
                    digest.update(data)
                    offset += len(data)
                output.flush()
                os.fsync(output.fileno())
            if digest.hexdigest() != file["digest"]:
                raise RuntimeError("output file digest changed")
        sync_directory(directory)

    def collect_through_collector(self, record):
        operation, kind = record['operation_id'], record['request']['kind']
        captured = json.loads(self.exec_collector(operation, ["logs", str(self.config["output_bytes"]), "final",
                                                              *(["tail"] if kind == "verify" else [])]).stdout)
        exported = ({"files": []} if kind == "verify" else
                    json.loads(self.exec_collector(operation, ["export", str(self.writable_bytes(record))], timeout=120).stdout))
        if 'refused' in exported:
            raise StoppedExportRefused(exported['refused'])
        self.read_collector_files(operation, exported)
        return exported, captured

    def harvest_stopped_output(self, record):
        # Never re-run code, reinterpret the semantic outcome, or substitute
        # shell output.bin for Python's saved stdout/displays.
        if record.get('termination_confirmed') is not True:
            raise Rejected(409, 'physical termination proof unavailable')
        collected = None
        if self.collector_holds_output(record):
            try:
                collected = self.collect_through_collector(record)
            except StoppedExportRefused:
                raise
            except Exception as error:
                print(f"Collector harvest {record['operation_id']}: {error}", file=sys.stderr, flush=True)
        if collected is None:
            if self.guard(record, 'status').get('termination_confirmed') is not True:
                raise Rejected(409, 'physical termination proof unavailable')
            collected = self.collect_stopped_output(record, refuse=False)
            if collected is None:
                return
        exported, captured = collected
        self.update(record['operation_id'], output_harvested=True, stdout=captured['stdout'],
                    files=exported['files'], displays=record.get('displays', []), truncated=True)
        self.allocation_harvested(self.get(record['operation_id']), idle=False)
        self.close_allocation(record)

    def cleanup_failed_start(self, record):
        operation = record['operation_id']
        try:
            if not record.get('pod_uid'):
                pod = self.pod(record.get('pod_operation', operation))
                if pod:
                    if pod.get('metadata', {}).get('annotations', {}).get('aidash/digest') != record['wire_digest']:
                        self.update(operation, startup_cleanup_pending=False)
                        return
                    record = self.update(operation, pod_uid=pod['metadata']['uid'])
            self.allocation_harvested(record)
            self.close_allocation(record)
            self.update(operation, startup_cleanup_pending=False)
        except Exception as error:
            print(f"Deployment Administrator: startup cleanup pending for {operation}: {error}", file=sys.stderr, flush=True)

    def fail_start(self, operation, message):
        record = self.update(operation, status='failed', executed=False, termination_confirmed=True,
                             output_harvested=True, startup_cleanup_pending=True, finished_at=time.time(),
                             error={'code':'STARTUP_FAILED', 'message':message[-2000:]})
        self.cleanup_failed_start(record)

    def reconcile(self, operation):
        last_warning = 0
        try:
            while True:
                try:
                    record = self.get(operation)
                    if not record.get('executed') and record.get('startup_at') is None:
                        record = self.update(operation, startup_at=time.time())
                    if not record.get('executed') and time.time() >= record['startup_at'] + START_LIMIT_SECONDS:
                        self.fail_start(operation, 'Sandbox startup timed out before dispatch')
                        return
                    self._reconcile_shell_once(operation)
                    return
                except SandboxGone:
                    record = self.get(operation)
                    if not record.get('executed'):
                        self.fail_start(operation, 'Sandbox identity changed before dispatch')
                    else:
                        self.update(operation, status='uncertain', termination_confirmed=True,
                                    output_harvested=True, output_discarded=True, files=[], finished_at=time.time(),
                                    uncertain_at=time.time(), error={'code':'SANDBOX_LOST',
                                    'message':"Sandbox is gone; the command's file changes were not saved."})
                    return
                except StartupIdentityMismatch as error:
                    if self.get(operation).get('executed'):
                        self.update(operation, status='uncertain', error=str(error)[-2000:])
                    else:
                        self.fail_start(operation, str(error))
                    return
                except Exception as error:
                    record = self.get(operation)
                    if not record.get('executed'):
                        if time.time() >= record['startup_at'] + START_LIMIT_SECONDS:
                            self.fail_start(operation, 'Sandbox startup timed out: ' + str(error))
                            return
                        time.sleep(1)
                        continue
                    now = time.time()
                    stopped_at = record.get('stop_confirmed_at')
                    if stopped_at is not None and now >= stopped_at + STOP_WATCH_SECONDS:
                        self.update(operation, status='uncertain', uncertain_at=now, finished_at=now,
                                    error={'code':'OUTPUT_LOST', 'message':"The command's file changes were not saved."})
                        return
                    started = record.get('running_at') or record.get('started_at') or record.get('dispatch_at') or now
                    deadline = started + record['request']['seconds'] + 30
                    if not record.get('termination_confirmed') and now >= deadline:
                        record = self.update(operation, status='cancelling', limit_stop_requested=True)
                        if now >= started + record['request']['seconds'] + STOP_WATCH_SECONDS and now - last_warning >= 60:
                            # A normal completed Pod deletion means kubelet stopped every container.
                            try:
                                self.allocation_harvested(record)
                                self.close_allocation(record)
                            except Exception as cleanup_error:
                                error = cleanup_error
                            print(f"Deployment Administrator: awaiting Pod deletion for {operation}: {error}", file=sys.stderr, flush=True)
                            last_warning = now
                    time.sleep(1)
        finally:
            with self.lock:
                self.active.discard(operation)

    def _reconcile_shell_once(self, operation):
        record = self.get(operation)
        pod = self.pod(operation)
        if pod is None:
            if record.get('executed') or record["status"] not in ("accepted", "starting"):
                raise SandboxGone()
            # Persist the attempt before talking to Kubernetes. A lost reply
            # may be recovered by observing the named object, never by replay.
            record = self.update(operation, status="starting")
            result = self.kube(["create", "-f", "-"], json.dumps(self.manifest(record)).encode(), check=False)
            pod = self.pod(operation)
            if pod is None:
                raise RuntimeError("sandbox creation outcome unknown: " + result.stderr.decode(errors="replace")[-500:])
        if pod["metadata"]["annotations"].get("aidash/digest") != record["wire_digest"]:
            if record.get('executed'):
                raise SandboxGone()
            raise StartupIdentityMismatch("sandbox identity mismatch")
        uid = pod["metadata"]["uid"]
        if (record.get("pod_uid") or uid) != uid:
            raise SandboxGone()
        self.update(operation, pod_uid=uid)
        hydrated = record["status"] in ("running", "finishing", "cancelling")
        preview_at = 0
        while True:
            pod = self.pod(operation)
            if pod is None and not self.get(operation).get('executed'):
                self.update(operation, pod_uid=None)
                raise RuntimeError('sandbox absent before dispatch')
            if pod is None or pod["metadata"]["uid"] != uid:
                raise SandboxGone()
            statuses = {s["name"]: s for s in pod.get("status", {}).get("containerStatuses", [])}
            execution = statuses.get("execution", {}).get("state", {})
            collector = statuses.get("collector", {}).get("state", {})
            if "running" in execution and "running" in collector and not hydrated:
                record = self.update(operation, pod_uid=uid, container_id=statuses['execution']['containerID'].split('://',1)[1],
                                     cluster_node=pod['spec']['nodeName'], image=pod['spec']['containers'][0]['image'])
                limits = self.guard(record, 'limits', **self.execution_limits(record))
                self.update(operation, resource_evidence=limits)
                payload = dict({k:v for k,v in record["request"].items() if k != "usage_tenant"}, working_bytes=self.config["working_bytes"], processes=self.config["processes"])
                prepared = json.loads(self.exec_collector(operation, ["prepared"]).stdout)
                if prepared["digest"] is None:
                    for file in record["request"]["files"]:
                        with (self.root / (operation + ".inputs") / file["file_id"]).open("rb") as source:
                            offset = 0
                            while True:
                                data = source.read(4194304)
                                if not data and offset:
                                    break
                                self.exec_collector(operation, ["write", file["scope"], file["path"], str(offset)], data)
                                offset += len(data)
                                if not data:
                                    break
                elif prepared["digest"] != record["request"]["digest"]:
                    raise StartupIdentityMismatch("running input identity changed")
                self.update(operation, executed=True, dispatch_at=record.get('dispatch_at') or time.time())
                # The collector stamps the start when it sets the deadline
                # and publishes the command, after verifying the inputs,
                # and returns the same stamp when hydrate is repeated after
                # a restart: a Check is charged from its command's start,
                # never for input checks or controller downtime. The
                # watchdog below counts from when hydrate returned.
                hydrated_reply = json.loads(self.exec_collector(
                    operation, ["hydrate"], json.dumps(payload).encode(), timeout=120).stdout)
                now = time.time()
                self.update(operation, status="running", running_at=now,
                            started_at=hydrated_reply.get("started_at") or now)
                hydrated = True
            current = self.get(operation)
            # A watchdog stop is delivered like a cancel, but it only settles the command as cancelled when
            # someone cancelled: a command that finished meanwhile settles from how it ended.
            if (current["cancel_requested"] or current.get("limit_stop_requested")) and "terminated" not in execution:
                try:
                    if "running" not in collector:
                        raise RuntimeError('collector unavailable for cancellation')
                    self.exec_collector(operation, ["cancel"])
                except Exception:
                    started = current.get('running_at') or current.get('started_at') or current.get('dispatch_at') or time.time()
                    if time.time() <= started + current['request']['seconds'] + 30:
                        raise
                    if self.guard(current, 'terminate').get('termination_confirmed') is not True:
                        raise RuntimeError('sandbox termination not confirmed') from None
                    current = self.update(operation, status='finishing', termination_confirmed=True,
                                          stop_confirmed_at=current.get('stop_confirmed_at') or time.time())
                    self.recover_stopped_shell(current)
                    break
            if "terminated" in execution:
                terminal = execution["terminated"]
                current = self.update(operation, status="finishing", termination_confirmed=True,
                                      exit_code=terminal["exitCode"],
                                      stop_confirmed_at=current.get('stop_confirmed_at') or time.time(),
                                      terminated_at=current.get("terminated_at")
                                      or termination_time(terminal, current.get("started_at"), time.time()))
                if current.get("started_at") is None and "running" in collector:
                    # The controller stopped after hydrate published the
                    # command but before it journaled the start, and the
                    # command ended meanwhile: the collector kept the stamp.
                    stamp = json.loads(self.exec_collector(operation, ["prepared"]).stdout).get("started_at")
                    if stamp is not None:
                        current = self.update(operation, started_at=stamp)
                # A Check is charged until its command stopped, not until
                # its output and files were harvested below.
                if "running" not in collector:
                    self.recover_stopped_shell(self.get(operation))
                    break
                # A Check keeps the end of its output, where the failure is.
                captured = json.loads(self.exec_collector(operation, ["logs", str(self.config["output_bytes"]), "final",
                                                                      *(["tail"] if record["request"]["kind"] == "verify" else [])]).stdout)
                self.update(operation, stdout=captured['stdout'], truncated=captured['truncated'], output_fetched_at=time.time())
                diagnostic = None
                if terminal["exitCode"] != 0 and not captured["stdout"]:
                    log = self.kube(["logs", self.pod_name(operation), "-c", "execution", "--limit-bytes=16384"]).stdout.decode(errors="replace")
                    # An empty log says nothing: the exit code stands alone.
                    if log.strip():
                        diagnostic = {"code": "COMMAND_FAILED", "message": log}
                exported = ({"files": []} if record["request"]["kind"] == "verify" else
                            json.loads(self.exec_collector(operation, ["export", str(self.writable_bytes(record))], timeout=120).stdout))
                if 'refused' in exported:
                    self.refuse_export(operation, exported['refused'], operation,
                                       stdout=captured['stdout'], truncated=captured['truncated'],
                                       deadline_expired=captured.get("deadline_expired"))
                    break
                self.read_collector_files(operation, exported)
                self.update(operation, status="cancelled" if current["cancel_requested"] else
                            ("completed" if terminal["exitCode"] == 0 else "failed"),
                            output_harvested=True, finished_at=time.time(), stdout=captured["stdout"],
                            truncated=captured["truncated"], files=exported["files"], error=diagnostic,
                            deadline_expired=captured.get("deadline_expired"))
                self.allocation_harvested(self.get(operation))
                # Retain the terminal journal and files before collecting Kubernetes resources.
                self.close_allocation(self.get(operation))
                break
            # Collect the bounded admission probe only after it reaps its
            # children, avoiding needless processes during the fork check.
            # Ordinary operation previews continue while execution runs.
            if hydrated and operation != self.admission_operation and time.monotonic() - preview_at > 1:
                captured = json.loads(self.exec_collector(operation, ["logs", "16384", "window"]).stdout)
                self.update(operation, stdout=captured["stdout"], stdout_offset=captured["offset"],
                            truncated=captured["truncated"], output_fetched_at=time.time())
                preview_at = time.monotonic()
            if not hydrated and time.time() >= record['startup_at'] + START_LIMIT_SECONDS:
                raise RuntimeError("sandbox startup not confirmed")
            latest = self.get(operation)
            running_at = latest.get('running_at') or latest.get('started_at') or latest.get('dispatch_at') or time.time()
            if hydrated and time.time() > running_at + record["request"]["seconds"] + 30:
                raise RuntimeError("sandbox termination not confirmed after deadline")
            time.sleep(0.25)

    def refuse_undelivered(self, operation, refusal):
        """Record that an operation the caller has not seen delivered will never run.

        The caller cannot tell an unsent operation from a send it gave up on: `accept`
        may still be waiting for this lock, or listing Pods under it, long after the
        caller's timeout. This takes the same lock, so either `accept` recorded the
        operation first and this is an ordinary cancel, or this record stands and
        `accept` starts nothing and answers the send with this record. Another instance
        has another journal, whose silence proves nothing about the operation, so its
        cancel stays absent."""
        if (set(refusal) != {"digest", "instance"} or not isinstance(refusal["instance"], str)
                or not isinstance(refusal["digest"], str) or not refusal["digest"]):
            raise Rejected(400, "invalid cancel refusal")
        with self.lock:
            if refusal["instance"] != self.instance or self.path(operation).exists():
                return
            record = {"operation_id": operation, "digest": refusal["digest"], "wire_digest": None,
                      "status": "cancelled", "request": {}, "created_at": time.time(), "cancel_requested": True,
                      "termination_confirmed": True, "stdout": "", "truncated": False,
                      "files": [], "error": None, "executed": False}
            save(self.path(operation), record)
            self.aggregate.track(operation, record)

    def cancel(self, operation, refusal=None):
        # One decision under one lock. `commit_inputs` starts the command under this
        # lock, so a cancel that read `awaiting_files` and let go before writing
        # `cancelled` could be overtaken by that start, and its reply would say
        # "never ran" about a command that did.
        with self.lock:
            if refusal is not None:
                self.refuse_undelivered(operation, refusal)
            record = self.get(operation)
            if record["status"] == "awaiting_files":
                record = self.update(operation, status="cancelled", cancel_requested=True, termination_confirmed=True)
            elif record["status"] == "uncertain" and not record["termination_confirmed"]:
                record = self.update(operation, status="cancelling", cancel_requested=True)
                self.start(operation)
            elif record["status"] not in TERMINAL:
                record = self.update(operation, cancel_requested=True)
            return self.public(record)

    def activity(self):
        """Read-only activity; an unproven writer always prevents idle stop. It reads
        the aggregate only: no journal file is opened and the lock is held for two reads."""
        with self.lock:
            busy, completed = len(self.aggregate.unsettled), self.aggregate.last_completed
        return {'protocol': 'aidash-runner-activity/1', 'counts': {'runner': busy},
                'busy': busy > 0, 'last_work_completed': completed}


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def handle_request(self):
        runner = self.server.runner
        if not hmac.compare_digest(self.headers.get("Authorization", ""), "Bearer " + runner.token):
            raise Rejected(401, "runner authentication required")
        path = urlsplit(self.path).path
        if self.command == 'GET' and path == '/v2/health':
            return {'protocol': 'aidash-runner/2', 'verified': runner.execution_pods.admitted,
                    'error': getattr(runner.execution_pods, 'admission_error', None)}
        if self.command == 'POST' and path in (
                '/v2/pods/ensure', '/v2/pods/stop', '/v2/pods/delete', '/v2/pods/reset',
                '/v2/commands/submit', '/v2/commands/result', '/v2/runs/cancel',
                '/v2/background/list', '/v2/background/stop', '/v2/changes', '/v2/viewers/heartbeat',
                '/v2/python/start', '/v2/python/cell', '/v2/python/stop'):
            size = int(self.headers.get('Content-Length', '0'))
            if not 0 < size <= 1048576:
                raise Rejected(413, 'runner request limit')
            request = json.loads(self.rfile.read(size))
            if not isinstance(request, dict):
                raise ValueError('object required')
            if path == '/v2/pods/ensure':
                return runner.execution_pods.ensure(request)
            if path == '/v2/pods/stop':
                return runner.execution_pods.stop(request['scratch_space_id'])
            if path == '/v2/pods/delete':
                return runner.execution_pods.delete(request['scratch_space_id'])
            if path == '/v2/pods/reset':
                return runner.execution_pods.reset(request)
            if path == '/v2/commands/submit':
                return runner.execution_pods.submit_command(request)
            if path == '/v2/commands/result':
                return runner.execution_pods.command_result(request['operation_id'])
            if path == '/v2/runs/cancel':
                return runner.execution_pods.cancel_run(request['scratch_space_id'], request['run_id'])
            if path == '/v2/background/list':
                return runner.execution_pods.background_processes(request['scratch_space_id'])
            if path == '/v2/background/stop':
                return runner.execution_pods.stop_background(request['operation_id'])
            if path == '/v2/viewers/heartbeat':
                return runner.execution_pods.viewer_heartbeat(request['scratch_space_id'], request['viewer_id'])
            if path == '/v2/python/start':
                return runner.execution_pods.start_python(request['scratch_space_id'], request['run_id'])
            if path == '/v2/python/cell':
                return runner.execution_pods.python_cell(request)
            if path == '/v2/python/stop':
                return runner.execution_pods.stop_python(request['scratch_space_id'], request['run_id'])
            return runner.execution_pods.change_log(request['scratch_space_id'], request.get('offset', 0),
                                                    request.get('limit', 65536))
        if self.command == "POST" and path == "/v1/operations":
            size = int(self.headers.get("Content-Length", "0"))
            if not 0 < size <= 8 * 1048576:
                raise Rejected(413, "runner request limit")
            return runner.accept(json.loads(self.rfile.read(size)))
        if self.command == "GET" and path == "/v1/allocations/receipts":
            query = parse_qs(urlsplit(self.path).query)
            return runner.allocation_receipts(query.get('after', [''])[0], int(query.get('limit', ['16'])[0]))
        allocation_match = re.fullmatch(r"/v1/allocations/([0-9a-f-]+)/ack", path)
        if self.command == "POST" and allocation_match:
            size = int(self.headers.get('Content-Length', '0'))
            if not 0 < size <= 256:
                raise Rejected(400, 'invalid allocation acknowledgement')
            receipt = json.loads(self.rfile.read(size))
            return runner.acknowledge_allocation(allocation_match[1], receipt['instance'], receipt['digest'])
        parts = path.split("/")
        if len(parts) == 6 and parts[1:3] == ["v1", "operations"]:
            operation = identity(parts[3])
            offset = int(parse_qs(urlsplit(self.path).query).get("offset", ["0"])[0])
            if self.command == "POST" and parts[4] == "inputs":
                size = int(self.headers.get("Content-Length", "0"))
                if not 0 < size <= 6 * 1048576:
                    raise Rejected(413, "chunk request limit")
                data = base64.b64decode(json.loads(self.rfile.read(size))["data"], validate=True)
                return runner.input_chunk(operation, parts[5], offset, data)
            if self.command == "GET" and parts[4] == "files":
                return runner.output_chunk(operation, parts[5], offset)
        if len(parts) in (4, 5) and parts[1:3] == ["v1", "operations"]:
            operation = identity(parts[3])
            if self.command == "GET" and len(parts) == 4:
                return runner.public(runner.get(operation))
            if self.command == "POST" and parts[4:] == ["cancel"]:
                size = int(self.headers.get("Content-Length", "0"))
                if size > 256:
                    raise Rejected(413, "cancel request limit")
                return runner.cancel(operation, json.loads(self.rfile.read(size)) if size else None)
            if self.command == "POST" and parts[4:] == ["start"]:
                return runner.commit_inputs(operation)
            if self.command == "POST" and parts[4:] == ["ack"]:
                size = int(self.headers.get("Content-Length", "0"))
                if not 0 < size <= 256:
                    raise Rejected(400, "invalid receipt")
                body = json.loads(self.rfile.read(size))
                return runner.acknowledge(operation, body["digest"], body.get("output_digest"))
        session_match = re.fullmatch(r"/v1/sessions/([0-9a-f-]+)(/stop)?", path)
        if session_match:
            if self.command == "GET" and not session_match[2]:
                return runner.session(session_match[1])
            if self.command == "POST" and session_match[2]:
                size = int(self.headers.get("Content-Length", "0"))
                if not 0 <= size <= 256:
                    raise Rejected(413, "session stop request limit")
                body = json.loads(self.rfile.read(size)) if size else None
                if body is not None and (not isinstance(body, dict) or set(body) not in (set(), {"instance"}) or ("instance" in body and not isinstance(body["instance"], str))):
                    raise Rejected(400, "invalid session stop request")
                return runner.stop_session(session_match[1], body.get("instance") if body else None)
        if self.command == "GET" and path == "/v1/health":
            return {"protocol": "aidash-runner/1", "runtime_class": runner.config["runtime_class"],
                    "image": runner.config["image"], "maximum_seconds": runner.config["maximum_seconds"],
                    "instance": runner.instance, "verified": runner.verified, "probe": runner.probe, "python_verified": runner.python_verified}
        if self.command == 'GET' and path == '/v1/activity':
            return runner.activity()
        raise Rejected(404, "runner route unavailable")

    def respond(self):
        try:
            data, status = self.handle_request(), 200
        except ExecutionPodError as error:
            status = (404 if error.code == 'command_not_found' else
                      503 if error.code in ('admission_unverified', 'startup_failed', 'pod_unavailable', 'pod_updating',
                                            'change_log_unavailable') else 409)
            data = {'code': error.code, 'message': error.message}
        except Rejected as error:
            data, status = ({'code': 'request_rejected', 'message': error.message}
                            if urlsplit(self.path).path.startswith('/v2/') else {'error': error.message}), error.code
        except (ValueError, KeyError, TypeError) as error:
            data, status = ({'code': 'invalid_request', 'message': str(error) or 'invalid request'}
                            if urlsplit(self.path).path.startswith('/v2/') else {'error': str(error)}), 400
        except Exception:
            data, status = ({'code': 'runner_unavailable', 'message': 'runner control unavailable'}
                            if urlsplit(self.path).path.startswith('/v2/') else {'error': 'runner control unavailable'}), 503
        encoded = json.dumps(data, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(encoded)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(encoded)

    do_POST = respond
    do_GET = respond

    def log_message(self, *_):
        pass


def main():
    try:
        profile = json.loads(Path(os.environ["AIDASH_CAPABILITY_PROFILE"]).read_bytes())
        config = dict(profile, **profile["runner"])
        runner = Runner(config)
        runner.execution_pods.start_idle_monitor()
        server = ThreadingHTTPServer((config["listen_host"], config["listen_port"]), Handler)
        server.runner = runner
        server.serve_forever()
    except BaseException:
        # Recovered commands are watched on non-daemon threads. An exception
        # that ends startup after they began would otherwise leave this process
        # alive and serving nothing, so leave explicitly and let it restart.
        traceback.print_exc()
        sys.stderr.flush()
        os._exit(1)


if __name__ == "__main__":
    main()
