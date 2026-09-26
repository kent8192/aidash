"""Single-writer, durable Kubernetes runner control service.

The journal is an operator-owned persistent volume. An exclusive process lock
prevents two controllers from claiming it. Kubernetes object names are derived
from operation UUIDs; recovery observes those objects and never recreates a
missing object after acceptance. No shell command runs on the controller host.
"""
import base64
import fcntl
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import hmac
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import threading
import time
from urllib.parse import urlsplit, parse_qs
import uuid

TERMINAL = {"completed", "cancelled", "failed", "uncertain"}


class Rejected(Exception):
    def __init__(self, code, message):
        self.code, self.message = code, message


def save(path, value):
    temporary = path.with_suffix(".new")
    with temporary.open("w") as f:
        json.dump(value, f, separators=(",", ":"), sort_keys=True)
        f.flush()
        os.fsync(f.fileno())
    os.replace(temporary, path)
    fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def identity(value):
    parsed = str(uuid.UUID(value))
    if value != parsed:
        raise Rejected(400, "canonical UUID required")
    return parsed


class Runner:
    def __init__(self, config):
        self.config = config
        self.root = Path(config["journal"])
        self.root.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.owner = (self.root / "owner.lock").open("a")
        fcntl.flock(self.owner, fcntl.LOCK_EX | fcntl.LOCK_NB)
        self.lock = threading.RLock()
        self.active = set()
        self.admission_operation = None

        identity_path = self.root / "instance"
        if not identity_path.exists():
            save(identity_path, str(uuid.uuid4()))
        self.instance = json.loads(identity_path.read_bytes())
        self.token = os.environ[config["token_env"]]
        if len(self.token) < 32:
            raise ValueError("runner token must have at least 32 characters")
        if not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,61}[a-z0-9]", config["namespace"]):
            raise ValueError("invalid dedicated namespace")
        if not re.search(r"@sha256:[0-9a-f]{64}$", config["image"]):
            raise ValueError("runner image must be digest pinned")
        self.command = [config["kubectl"], "--kubeconfig", config["kubeconfig"], "--namespace", config["namespace"]]
        self.environment = {"PATH": os.defpath, "HOME": str(self.root)}
        runtime = self.kube_json(["get", "runtimeclass", config["runtime_class"], "-o", "json"])
        if runtime["handler"] != "runsc":
            raise ValueError("verified runsc runtime class required")
        self.ensure_network_policy()
        self.verified = False
        self.python_verified = False
        # Existing journals contain untrusted work too. Re-prove the current
        # isolation boundary before any recovery thread can execute that work.
        self.verify_isolation()
        if self.config.get("node_guard"):
            self.verify_freeze()
        for path in self.root.glob("*.json"):
            if path.name.startswith(("area-", "session-")):
                continue
            record = json.loads(path.read_bytes())
            if record["status"] not in TERMINAL and record["status"] != "awaiting_files":
                self.start(record["operation_id"])

    def verify_isolation(self):
        operation = str(uuid.uuid4())
        self.admission_operation = operation
        self.accept({"operation_id": operation, "area_id": str(uuid.uuid4()), "epoch": 1,
                     "digest": "immutable-admission-probe/3", "kind": "shell", "code": f"python -I /opt/aidash/probe.py {self.config['working_bytes']} {self.config['temporary_bytes']}",
                     "seconds": 30, "files": []})
        deadline = time.monotonic() + 150
        while time.monotonic() < deadline:
            record = self.get(operation)
            if record["status"] in TERMINAL:
                if record["status"] != "completed":
                    raise RuntimeError("isolation probe failed: " + str(record.get("error")))
                report = json.loads(base64.b64decode(record["stdout"]))
                if ("gvisor" not in report["kernel"].lower() or report["uid"] == 0
                        or report["process_limit"] != self.config["processes"]
                        or not all(report["egress_denied"].values())):
                    raise RuntimeError("isolation evidence does not match the execution profile")
                self.verified = True
                self.probe = dict(report, resources=record['resource_evidence'])
                self.admission_operation = None
                return
            time.sleep(0.2)
        raise RuntimeError("isolation probe did not finish")

    def verify_freeze(self):
        operation = str(uuid.uuid4())
        record = {"operation_id": operation, "area_id": str(uuid.uuid4()), "epoch": 1,
                  "wire_digest": "freeze-admission-probe/1",
                  "request": {"kind": "python", "files": []}}
        manifest = self.manifest(record)
        manifest["spec"]["containers"][0]["command"] = ["python", "-u", "-c", "import time; from pathlib import Path\nwhile True: Path('/work/tick').write_text(str(time.monotonic())); time.sleep(.01)"]
        self.kube(["create", "-f", "-"], json.dumps(manifest).encode())
        stopped = False
        try:
            self.kube(["wait", "--for=condition=Ready", "pod/" + self.pod_name(operation), "--timeout=60s"], timeout=70)
            pod = self.pod(operation)
            status = next(s for s in pod["status"]["containerStatuses"] if s["name"] == "execution")
            record.update(pod_uid=pod["metadata"]["uid"], container_id=status["containerID"].split("://", 1)[1])
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
        policy = {"apiVersion": "networking.k8s.io/v1", "kind": "NetworkPolicy",
                  "metadata": {"name": "aidash-deny", "namespace": self.config["namespace"]},
                  "spec": {"podSelector": {"matchLabels": {"aidash-sandbox": "true"}},
                           "policyTypes": ["Ingress", "Egress"], "ingress": [], "egress": []}}
        self.kube(["apply", "-f", "-"], json.dumps(policy).encode())

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
            return record

    def public(self, record):
        return {k: v for k, v in record.items() if k not in ("request", "pod_uid")}

    def accept(self, request):
        if set(request) - {"session_id"} != {"operation_id", "area_id", "epoch", "digest", "kind", "code", "seconds", "files"}:
            raise Rejected(400, "invalid runner contract")
        operation, area = identity(request["operation_id"]), identity(request["area_id"])
        if request["kind"] not in ("shell", "python") or not isinstance(request["code"], str):
            raise Rejected(400, "invalid command kind")
        if len(request["code"].encode()) > self.config.get("limits", {}).get("command_bytes", 65536) or len(request["files"]) > 4096:
            raise Rejected(400, "command or file limit")
        if not isinstance(request["epoch"], int) or request["epoch"] < 1:
            raise Rejected(400, "invalid epoch")
        if not isinstance(request["seconds"], int) or not 1 <= request["seconds"] <= self.config["maximum_seconds"]:
            raise Rejected(400, "operation time limit")
        session = None
        if request["kind"] == "python":
            if not self.config.get("node_guard"):
                raise Rejected(409, "PYTHON_RUNTIME_UNAVAILABLE")
            sid = identity(request.get("session_id"))
            session_path = self.root / ("session-" + sid + ".json")
            if session_path.exists():
                session = json.loads(session_path.read_bytes())
                if session["area_id"] != area or session["status"] != "frozen" or session["manifest"] != self.file_identity(request["files"]):
                    raise Rejected(409, "SESSION_RESET: incompatible live environment")
        size = 0
        identities = set()
        for file in request["files"]:
            if set(file) != {"file_id", "path", "scope", "size", "digest"}:
                raise Rejected(400, "invalid file contract")
            identity(file["file_id"])
            if (file["file_id"] in identities or file["scope"] not in ("working", "references", "received")
                    or not isinstance(file["size"], int) or file["size"] < 0
                    or not re.fullmatch(r"[0-9a-f]{64}", file["digest"])):
                raise Rejected(400, "invalid input manifest")
            identities.add(file["file_id"])
            size += file["size"]
        if size > self.config["working_bytes"]:
            raise Rejected(413, "working quota")
        # The transport digest binds the complete request independently of the
        # opaque caller digest (which binds its authenticated public request).
        wire_digest = hashlib.sha256(json.dumps(request, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
        with self.lock:
            if self.path(operation).exists():
                old = self.get(operation)
                if old["wire_digest"] != wire_digest:
                    raise Rejected(409, "IDEMPOTENCY_CONFLICT")
                return self.public(old)
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
                      "digest": request["digest"], "wire_digest": wire_digest, "status": "awaiting_files" if request["files"] and not session else "accepted",
                      "request": request, "created_at": time.time(), "cancel_requested": False,
                      "termination_confirmed": False, "stdout": "", "truncated": False,
                      "files": [], "error": None, "executed": False}
            if request["kind"] == "python":
                record["session_id"] = sid
                if session:
                    record["pod_operation"] = session["pod_operation"]
                    record["pod_uid"] = session["pod_uid"]
                    record["container_id"] = session["container_id"]
            save(self.path(operation), record)
            save(area_path, {"epoch": request["epoch"], "operation_id": operation})
            if not request["files"] or session:
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

    def acknowledge(self, operation, digest):
        with self.lock:
            record = self.get(operation)
            if record["digest"] != digest or record["status"] not in ("completed", "cancelled", "failed"):
                raise Rejected(409, "outcome is not ready to acknowledge")
            record = self.update(operation, acknowledged=True, request={}, stdout="", files=[], displays=[])
            for suffix in (".inputs", ".files"):
                directory = self.root / (operation + suffix)
                if directory.exists():
                    for path in directory.iterdir():
                        path.unlink()
                    directory.rmdir()
            return self.public(record)

    def start(self, operation):
        with self.lock:
            if operation not in self.active:
                self.active.add(operation)
                threading.Thread(target=self.reconcile_python if self.get(operation)["request"]["kind"] == "python" else self.reconcile, args=(operation,), daemon=True).start()

    def pod_name(self, operation):
        return "operation-" + operation

    def writable_bytes(self, record):
        mounted = sum(file["size"] for file in record["request"]["files"] if file["scope"] != "working")
        remaining = self.config["working_bytes"] - mounted
        if remaining < 0:
            raise Rejected(413, "working quota")
        return remaining

    def execution_limits(self, record):
        limits = {k: self.config[k] for k in ('cpu', 'memory_bytes', 'processes', 'temporary_bytes')}
        # tmpfs capacity is rounded up to a physical page. Export still enforces
        # the exact remaining aggregate byte budget, including the zero case.
        page = os.sysconf('SC_PAGE_SIZE')
        limits['working_bytes'] = max(page, ((self.writable_bytes(record) + page - 1) // page) * page)
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
        worker_mounts = [{"name": name, "mountPath": path, "readOnly": name in ("references", "received", "request")}
                         for name, path in (("work", "/work"), ("references", "/references"),
                                            ("received", "/received"), ("request", "/request"), ("temp", "/tmp"))]
        collector_mounts = [{"name": m["name"], "mountPath": m["mountPath"]} for m in worker_mounts if m["name"] != "temp"]
        collector_mounts.append({"name": "control-temp", "mountPath": "/tmp"})
        worker = container("execution", ["python", "-I", "/opt/aidash/sandbox.py"], profile["cpu"], profile["memory_bytes"], worker_mounts)
        worker["livenessProbe"] = {"httpGet": {"path": "/health", "port": 7070}, "periodSeconds": 1,
                                   "timeoutSeconds": 1, "failureThreshold": 1, "terminationGracePeriodSeconds": 1}
        worker["startupProbe"] = {"httpGet": {"path": "/health", "port": 7070}, "periodSeconds": 1,
                                  "timeoutSeconds": 1, "failureThreshold": 120}
        if record["request"]["kind"] == "python":
            worker.pop("livenessProbe")
            worker.pop("startupProbe")
        collector = container("collector", ["python", "-I", "/opt/aidash/collector.py", "serve"], "250m", "256Mi", collector_mounts)
        collector["env"].append({"name": "AIDASH_OUTPUT_BYTES", "value": str(profile["output_bytes"])})
        volumes = [{"name": name, "emptyDir": {"medium": "Memory", "sizeLimit": str(size)}} for name, size in (
            ("work", max(1, self.writable_bytes(record))), ("references", profile["working_bytes"]),
            ("received", profile["working_bytes"]), ("request", 1048576),
            ("temp", profile["temporary_bytes"]), ("control-temp", profile["output_bytes"] * 4 + 1048576))]
        return {"apiVersion": "v1", "kind": "Pod", "metadata": {"name": self.pod_name(record["operation_id"]),
                "labels": {"aidash-sandbox": "true", "aidash-area": record["area_id"]}, "annotations": {"aidash/digest": record["wire_digest"],
                "aidash/epoch": str(record["epoch"]), "aidash/area": record["area_id"]}},
                "spec": {"runtimeClassName": profile["runtime_class"], "restartPolicy": "Never",
                         "resources": {"limits": {"cpu":str(profile['cpu']), "memory":str(profile['memory_bytes'])}},
                         "automountServiceAccountToken": False, "enableServiceLinks": False,
                         "shareProcessNamespace": False, "hostNetwork": False, "hostPID": False, "hostIPC": False,
                         "dnsPolicy": "None", "dnsConfig": {"nameservers": ["127.0.0.1"]},
                         "securityContext": {"fsGroup": 10000, "fsGroupChangePolicy": "OnRootMismatch"},
                         "terminationGracePeriodSeconds": 1, "containers": [worker, collector], "volumes": volumes}}

    def pod(self, operation):
        result = self.kube(["get", "pod", self.pod_name(operation), "-o", "json", "--ignore-not-found"])
        return json.loads(result.stdout) if result.stdout.strip() else None

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
        if image is None:
            # Upgrade old journals without trusting caller-controlled image
            # names. A live API object must match the already observed Pod UID.
            pod = self.pod(record.get('pod_operation',record['operation_id']))
            if not pod or pod['metadata']['uid'] != record['pod_uid']:
                raise RuntimeError('old journal image binding unavailable')
            image = next(c['image'] for c in pod['spec']['containers'] if c['name']=='execution')
        request = dict(container_id=record["container_id"], pod_uid=record["pod_uid"],
                       area_id=record["area_id"], epoch=record["epoch"], image=image,
                       action=action, **fields)
        result = subprocess.run(arguments, input=json.dumps(request).encode(), stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, timeout=45, check=False)
        value = json.loads(result.stdout)
        if result.returncode:
            raise RuntimeError("node lifecycle adapter: " + str(value.get("error", "failed")))
        return value

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

    def stop_session(self, sid):
        path = self.root / ("session-" + identity(sid) + ".json")
        if not path.exists():
            return {"status": "absent", "termination_confirmed": True}
        session = json.loads(path.read_bytes())
        record = self.get(session["operation_id"])
        self.guard(record, "terminate")
        session["status"] = "stopped"
        save(path, session)
        # Force removes only the API object after the independent pidfd proof;
        # a Kubernetes deletion acknowledgement alone never proves termination.
        self.kube(["delete", "pod", self.pod_name(session["pod_operation"]), "--wait=false", "--force", "--grace-period=0"], check=False)
        return {"status": "stopped", "termination_confirmed": True}

    def reconcile_python(self, operation):
        try:
            record = self.get(operation)
            pod_operation = record.get("pod_operation", operation)
            pod = self.pod(pod_operation)
            cold = "pod_operation" not in record
            if cold and pod is None:
                if record["status"] != "accepted":
                    raise RuntimeError("Python creation outcome uncertain; no replay")
                self.update(operation, status="starting")
                self.kube(["create", "-f", "-"], json.dumps(self.manifest(record)).encode(), check=False)
            start_deadline = time.monotonic() + 120
            while True:
                pod = self.pod(pod_operation)
                if pod:
                    statuses = {s["name"]: s for s in pod.get("status", {}).get("containerStatuses", [])}
                    if all("running" in statuses.get(n, {}).get("state", {}) for n in ("execution", "collector")):
                        break
                    if any("terminated" in s.get("state", {}) for s in statuses.values()):
                        raise RuntimeError("Python environment stopped before dispatch; no replay")
                if time.monotonic() > start_deadline:
                    raise RuntimeError("Python environment not ready")
                time.sleep(.25)
            if record.get("pod_uid", pod["metadata"]["uid"]) != pod["metadata"]["uid"]:
                raise RuntimeError("Python environment identity changed")
            cid = statuses["execution"]["containerID"].split("://", 1)[1]
            record = self.update(operation, pod_operation=pod_operation, pod_uid=pod["metadata"]["uid"], container_id=cid)
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
                    payload = dict(record["request"], working_bytes=self.config["working_bytes"], processes=self.config["processes"])
                    self.exec_collector(pod_operation, ["hydrate"], json.dumps(payload).encode())
                elif prepared["digest"] != record["request"]["digest"]:
                    raise RuntimeError("Python input identity changed")
                record = self.update(operation, hydrated=True)
            record = self.update(operation, executed=True, status="running")
            self.guard(record, "cell", operation_id=operation, code=record["request"]["code"],
                       digest=record["digest"], seconds=record["request"]["seconds"])
            memory_live = True
            while True:
                current = self.get(operation)
                if current["cancel_requested"]:
                    self.guard(record, "terminate")
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
                    raise RuntimeError("Python reply uncertain; code will not be replayed")
                self.update(operation, stdout=result.get("stdout", ""), truncated=result.get("truncated", False))
                pod = self.pod(pod_operation)
                if not pod or any("terminated" in s.get("state", {}) for s in pod.get("status", {}).get("containerStatuses", [])):
                    self.guard(record, "terminate")
                    memory_live = False
                    result = dict(status="failed", exit_code=137, stdout=result.get("stdout", ""),
                                  truncated=True, error={"code":"SESSION_LOST", "message":"Interpreter stopped during the cell; code was not replayed."})
                    break
                time.sleep(.2)
            if memory_live:
                frozen = self.guard(record, "freeze", idle_seconds=self.config["idle_seconds"])
                if not frozen["writer_frozen"]:
                    raise RuntimeError("Python memory no longer live")
            self.update(operation, status="finishing", writer_frozen=memory_live, termination_confirmed=not memory_live)
            exported = self.guard(record, "export", working_bytes=self.writable_bytes(record))
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
            manifest = [f for f in record["request"]["files"] if f["scope"] != "working"]
            manifest += [dict(f, scope="working") for f in exported["files"]]
            session = dict(session_id=record["session_id"], area_id=record["area_id"], operation_id=operation,
                           pod_operation=pod_operation, pod_uid=record["pod_uid"], container_id=cid,
                           status="frozen" if memory_live else "stopped", last_used=time.time(), manifest=self.file_identity(manifest))
            save(self.root / ("session-" + record["session_id"] + ".json"), session)
            self.update(operation, status=result["status"], writer_frozen=memory_live, termination_confirmed=not memory_live,
                        finished_at=time.time(), stdout=result.get("stdout", ""), truncated=result.get("truncated", False),
                        displays=result.get("displays", []), files=exported["files"], error=result.get("error"), exit_code=result["exit_code"])
            if not memory_live:
                self.kube(["delete", "pod", self.pod_name(pod_operation), "--wait=false", "--force", "--grace-period=0"], check=False)
        except Exception as error:
            self.update(operation, status="uncertain", error=str(error)[-2000:])
        finally:
            with self.lock:
                self.active.discard(operation)

    def recover_stopped_shell(self, record):
        if self.guard(record, 'status').get('termination_confirmed') is not True:
            raise RuntimeError('physical termination proof unavailable')
        exported = self.guard(record, 'export', working_bytes=self.writable_bytes(record))
        captured = self.guard(record, 'logs')
        operation = record['operation_id']
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
        self.update(operation, status='failed', exit_code=137, termination_confirmed=True,
                    stdout=captured['stdout'], truncated=True, files=exported['files'],
                    finished_at=time.time(), error={'code':'SANDBOX_LOST', 'message':'Sandbox stopped; saved files were recovered and output may be incomplete.'})
        self.kube(['delete','pod',self.pod_name(operation),'--wait=false','--force','--grace-period=0'], check=False)

    def reconcile(self, operation):
        try:
            record = self.get(operation)
            pod = self.pod(operation)
            if pod is None:
                if record["status"] != "accepted":
                    raise RuntimeError("accepted sandbox disappeared; effects require reconciliation")
                # Persist the attempt before talking to Kubernetes. A lost reply
                # may be recovered by observing the named object, never by replay.
                record = self.update(operation, status="starting")
                result = self.kube(["create", "-f", "-"], json.dumps(self.manifest(record)).encode(), check=False)
                pod = self.pod(operation)
                if pod is None:
                    raise RuntimeError("sandbox creation outcome unknown: " + result.stderr.decode(errors="replace")[-500:])
            if pod["metadata"]["annotations"].get("aidash/digest") != record["wire_digest"]:
                raise RuntimeError("sandbox identity mismatch")
            uid = pod["metadata"]["uid"]
            if record.get("pod_uid", uid) != uid:
                raise RuntimeError("sandbox replaced")
            self.update(operation, pod_uid=uid)
            startup_deadline = time.monotonic() + 120
            hydrated = record["status"] in ("running", "finishing")
            preview_at = 0
            while True:
                pod = self.pod(operation)
                if pod is None or pod["metadata"]["uid"] != uid:
                    raise RuntimeError("sandbox disappeared before termination was confirmed")
                statuses = {s["name"]: s for s in pod.get("status", {}).get("containerStatuses", [])}
                execution = statuses.get("execution", {}).get("state", {})
                collector = statuses.get("collector", {}).get("state", {})
                if "running" in execution and "running" in collector and not hydrated:
                    record = self.update(operation, pod_uid=uid, container_id=statuses['execution']['containerID'].split('://',1)[1])
                    limits = self.guard(record, 'limits', **self.execution_limits(record))
                    self.update(operation, resource_evidence=limits)
                    payload = dict(record["request"], working_bytes=self.config["working_bytes"], processes=self.config["processes"])
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
                        raise RuntimeError("running input identity changed")
                    self.update(operation, executed=True)
                    self.exec_collector(operation, ["hydrate"], json.dumps(payload).encode(), timeout=120)
                    self.update(operation, status="running", started_at=time.time())
                    hydrated = True
                current = self.get(operation)
                if current["cancel_requested"] and "running" in collector:
                    self.exec_collector(operation, ["cancel"])
                if "terminated" in execution:
                    terminal = execution["terminated"]
                    self.update(operation, status="finishing", termination_confirmed=True, exit_code=terminal["exitCode"])
                    if "running" not in collector:
                        self.recover_stopped_shell(self.get(operation))
                        break
                    captured = json.loads(self.exec_collector(operation, ["logs", str(self.config["output_bytes"]), "final"]).stdout)
                    diagnostic = None
                    if terminal["exitCode"] != 0 and not captured["stdout"]:
                        diagnostic = self.kube(["logs", self.pod_name(operation), "-c", "execution", "--limit-bytes=16384"]).stdout.decode(errors="replace")
                    exported = json.loads(self.exec_collector(operation, ["export", str(self.writable_bytes(record))], timeout=120).stdout)
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
                    self.update(operation, status="cancelled" if current["cancel_requested"] else
                                ("completed" if terminal["exitCode"] == 0 else "failed"),
                                finished_at=time.time(), stdout=captured["stdout"],
                                truncated=captured["truncated"], files=exported["files"], error=diagnostic)
                    # Retain the terminal journal and files before collecting Kubernetes resources.
                    self.kube(["delete", "pod", self.pod_name(operation), "--wait=false"], check=False)
                    break
                # Collect the bounded admission probe only after it reaps its
                # children, avoiding needless processes during the fork check.
                # Ordinary operation previews continue while execution runs.
                if hydrated and operation != self.admission_operation and time.monotonic() - preview_at > 1:
                    captured = json.loads(self.exec_collector(operation, ["logs", "16384"]).stdout)
                    self.update(operation, stdout=captured["stdout"], truncated=captured["truncated"])
                    preview_at = time.monotonic()
                if not hydrated and time.monotonic() > startup_deadline:
                    raise RuntimeError("sandbox startup not confirmed")
                if hydrated and time.time() > self.get(operation).get("started_at", time.time()) + record["request"]["seconds"] + 30:
                    raise RuntimeError("sandbox termination not confirmed after deadline")
                time.sleep(0.25)
        except Exception as error:
            print(f"Shell reconciliation {operation}: {error}", file=sys.stderr, flush=True)
            try:
                record = self.get(operation)
                if not record.get('executed'):
                    raise RuntimeError('not dispatched')
                self.recover_stopped_shell(record)
            except Exception:
                self.update(operation, status="uncertain", error=str(error)[-2000:])
        finally:
            with self.lock:
                self.active.discard(operation)

    def cancel(self, operation):
        record = self.get(operation)
        if record["status"] == "awaiting_files":
            record = self.update(operation, status="cancelled", cancel_requested=True, termination_confirmed=True)
        elif record["status"] == "uncertain" and not record["termination_confirmed"]:
            record = self.update(operation, status="cancelling", cancel_requested=True)
            self.start(operation)
        elif record["status"] not in TERMINAL:
            record = self.update(operation, cancel_requested=True)
        return self.public(record)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def handle_request(self):
        runner = self.server.runner
        if not hmac.compare_digest(self.headers.get("Authorization", ""), "Bearer " + runner.token):
            raise Rejected(401, "runner authentication required")
        path = urlsplit(self.path).path
        if self.command == "POST" and path == "/v1/operations":
            size = int(self.headers.get("Content-Length", "0"))
            if not 0 < size <= 8 * 1048576:
                raise Rejected(413, "runner request limit")
            return runner.accept(json.loads(self.rfile.read(size)))
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
                return runner.cancel(operation)
            if self.command == "POST" and parts[4:] == ["start"]:
                return runner.commit_inputs(operation)
            if self.command == "POST" and parts[4:] == ["ack"]:
                size = int(self.headers.get("Content-Length", "0"))
                if not 0 < size <= 256:
                    raise Rejected(400, "invalid receipt")
                return runner.acknowledge(operation, json.loads(self.rfile.read(size))["digest"])
        session_match = re.fullmatch(r"/v1/sessions/([0-9a-f-]+)(/stop)?", path)
        if session_match:
            if self.command == "GET" and not session_match[2]:
                return runner.session(session_match[1])
            if self.command == "POST" and session_match[2]:
                return runner.stop_session(session_match[1])
        if self.command == "GET" and path == "/v1/health":
            return {"protocol": "aidash-runner/1", "runtime_class": runner.config["runtime_class"],
                    "image": runner.config["image"], "maximum_seconds": runner.config["maximum_seconds"],
                    "instance": runner.instance, "verified": runner.verified, "probe": runner.probe, "python_verified": runner.python_verified}
        raise Rejected(404, "runner route unavailable")

    def respond(self):
        try:
            data, status = self.handle_request(), 200
        except Rejected as error:
            data, status = {"error": error.message}, error.code
        except (ValueError, KeyError, TypeError) as error:
            data, status = {"error": str(error)}, 400
        except Exception:
            data, status = {"error": "runner control unavailable"}, 503
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


if __name__ == "__main__":
    profile = json.loads(Path(os.environ["AIDASH_CAPABILITY_PROFILE"]).read_bytes())
    config = dict(profile, **profile["runner"])
    runner = Runner(config)
    server = ThreadingHTTPServer((config["listen_host"], config["listen_port"]), Handler)
    server.runner = runner
    server.serve_forever()
