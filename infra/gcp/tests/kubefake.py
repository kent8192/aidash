"""In-memory stand-in for kube.Cluster: one method per kubectl/helm/gcloud command."""

import base64
from copy import deepcopy
import json

from cloud import bounded_timeout

PROJECT = "aidash-fixture"
KINDS = {
    "persistentvolume": "pv", "persistentvolumes": "pv",
    "persistentvolumeclaim": "pvc", "persistentvolumeclaims": "pvc",
    "deployment": "deployment", "statefulset": "statefulset", "cronjob": "cronjob",
    "configmap": "configmap", "secret": "secret", "namespace": "namespace",
    "service": "service", "job": "job", "storageclass": "storageclass",
    "pods": "pod", "volumeattachments": "volumeattachment",
}
SHORT = {
    "app-aidash-server": "server", "app-aidash-worker": "worker", "app-execution-runner": "runner",
    "env-environment-edge": "edge", "env-environment-postgres": "postgres", "env-environment-nats": "nats",
}
SELECTORS = {
    "app.kubernetes.io/instance=app,app.kubernetes.io/component in (server,worker)": {"app-aidash-server", "app-aidash-worker"},
    "aidash.run/runner=app": {"app-execution-runner"},
    "aidash.run/edge=env": {"env-environment-edge"},
}


def identity(namespace):
    return namespace[len("aidash-"):] if namespace and namespace.startswith("aidash-") else namespace


def merge(target, patch):
    for key, value in patch.items():
        if value is None:
            target.pop(key, None)
        elif isinstance(value, dict) and isinstance(target.get(key), dict):
            merge(target[key], value)
        else:
            target[key] = deepcopy(value)


class FakeCluster:
    def __init__(self, calls, now=10000):
        self.calls = calls
        self.now = now
        self.project = PROJECT
        self.pod_cidr = "10.44.0.0/14"
        self.objects = {}
        self.releases = {}
        self.disks = {"aidash-preview-tls"}
        self.attached = {}
        self.busy = set()
        self.idle = set()
        self.script = []
        self.results = {}
        self.failures = {}
        self.on_observe = None
        self.addresses = 0

    # Fixture controls
    def fail(self, event, error):
        self.failures[event] = error

    def record(self, *event):
        bounded_timeout(1)  # Like cloud.run: no command starts after the deadline.
        self.calls.append(event)
        if event in self.failures:
            raise self.failures[event]

    def key(self, kind, name, namespace=None):
        kind = KINDS[kind.lower()]
        scoped = kind not in {"pv", "namespace", "storageclass", "volumeattachment"}
        return kind, namespace if scoped else None, name

    def put(self, obj):
        kind = obj["kind"].lower()
        metadata = obj.setdefault("metadata", {})
        self.objects[self.key(kind, metadata["name"], metadata.get("namespace"))] = obj
        return obj

    def snapshot(self, namespace, mode="flags"):
        name = identity(namespace)
        busy = mode == "busy" or (mode == "flags" and name in self.busy)
        value = {
            "protocol": "aidash-infra-activity/1",
            "busy": busy,
            "observation_gap": False,
            "observed_at": self.now - 1000 if mode == "stale" else self.now,
            "last_active": 0 if name in self.idle else self.now,
            "counts": {"database_work": 0, "runs": int(busy)},
        }
        if mode == "error":
            value.update(busy=True, observation_gap=True, error="activity unavailable")
            value.pop("counts")
        return value

    def workload(self, kind, namespace, name, replicas, **extra):
        current = self.objects.get(self.key(kind, name, namespace))
        obj = current or {"kind": kind.capitalize(), "metadata": {"name": name, "namespace": namespace}, "spec": {}}
        obj["spec"]["replicas"] = replicas
        merge(obj, extra)
        self.put(obj)
        return obj

    def dynamic_claim(self, namespace, name):
        disk = f"pvc-{namespace}-{name}"
        self.disks.add(disk)
        self.put({"kind": "PersistentVolumeClaim", "metadata": {"name": name, "namespace": namespace}, "spec": {"volumeName": disk}})
        self.put({
            "kind": "PersistentVolume", "metadata": {"name": disk},
            "spec": {
                "persistentVolumeReclaimPolicy": "Retain",
                "claimRef": {"namespace": namespace, "name": name},
                "csi": {"volumeHandle": f"projects/{PROJECT}/zones/us-central1-a/disks/{disk}"},
            },
        })

    # Cluster interface
    def get(self, kind, name, namespace=None):
        bounded_timeout(1)
        key = self.key(kind, name, namespace)
        if key[0] == "configmap" and name == "env-environment-activity":
            self.record("activity", identity(namespace))
            if key not in self.objects:
                return None
            snapshot = self.results.pop(namespace, None) or self.snapshot(namespace)
            return {"data": {"snapshot.json": json.dumps(snapshot)}}
        if key[0] == "deployment" and key in self.objects:
            obj = deepcopy(self.objects[key])
            obj["status"] = {"readyReplicas": obj["spec"]["replicas"]}
            return obj
        return deepcopy(self.objects.get(key))

    def items(self, kind, namespace=None, selector=None):
        bounded_timeout(1)
        kind = KINDS[kind.lower()]
        if kind == "pod":
            names = SELECTORS.get(selector)
            pods = []
            for (other, scope, name), obj in self.objects.items():
                if other in {"deployment", "statefulset"} and scope == namespace and (names is None or name in names):
                    pods += [{"status": {"phase": "Running"}}] * obj["spec"]["replicas"]
            return pods
        if kind == "volumeattachment":
            result = []
            for volume, remaining in list(self.attached.items()):
                if remaining > 0:
                    self.attached[volume] = remaining - 1
                    result.append({"spec": {"source": {"persistentVolumeName": volume}}})
            return result
        return [deepcopy(obj) for (other, scope, _), obj in self.objects.items() if other == kind and scope == namespace]

    def apply(self, manifest):
        bounded_timeout(1)
        self.put(deepcopy(manifest))

    def create(self, manifest):
        manifest = deepcopy(manifest)
        metadata = manifest["metadata"]
        key = self.key(manifest["kind"], metadata["name"], metadata.get("namespace"))
        if key in self.objects:
            raise RuntimeError("kubectl failed (exit 1); no private command output was logged")
        if manifest["kind"] == "Job":
            self.record("migrate", identity(metadata["namespace"]))
            manifest["status"] = {"succeeded": 1}
        elif manifest["kind"] == "PersistentVolumeClaim" and manifest["spec"]["storageClassName"]:
            bounded_timeout(1)
            self.dynamic_claim(metadata["namespace"], metadata["name"])
            return
        else:
            bounded_timeout(1)
        self.put(manifest)

    def delete(self, kind, name, namespace=None, timeout=600):
        key = self.key(kind, name, namespace)
        if key[0] == "job":
            bounded_timeout(1)
            self.objects.pop(key, None)
            return
        if key[0] == "namespace":
            self.record("delete_namespace", name)
            for other in [item for item in self.objects if item[1] == name]:
                del self.objects[other]
        elif key[0] == "service":
            self.record("delete_lb", identity(namespace))
        else:
            self.record("delete", key[0], identity(namespace) if namespace else name, name)
        self.objects.pop(key, None)

    def patch(self, kind, name, body, namespace=None):
        key = self.key(kind, name, namespace)
        if key[0] == "pv":
            self.record("patch_pv", name, (body["spec"]["claimRef"] or {}).get("namespace"))
        elif key[0] == "cronjob":
            self.record("suspend", identity(namespace), body["spec"]["suspend"])
        else:
            bounded_timeout(1)
        if key not in self.objects:
            raise RuntimeError("kubectl failed (exit 1); no private command output was logged")
        merge(self.objects[key], body)

    def scale(self, namespace, target, replicas):
        kind, name = target.split("/", 1)
        self.record("scale", identity(namespace), SHORT[name], replicas)
        key = self.key(kind, name, namespace)
        if key not in self.objects:
            raise RuntimeError("kubectl failed (exit 1); no private command output was logged")
        self.objects[key]["spec"]["replicas"] = replicas

    def rollout(self, namespace, target, timeout=600):
        bounded_timeout(1)
        kind, name = target.split("/", 1)
        if self.key(kind, name, namespace) not in self.objects:
            raise RuntimeError("kubectl failed (exit 1); no private command output was logged")

    def exec(self, namespace, target, container, *command):
        self.record("admission", identity(namespace), command[-1].rsplit("/", 1)[1])
        edge = self.objects.get(self.key("deployment", "env-environment-edge", namespace))
        if not edge or not edge["spec"]["replicas"]:
            raise RuntimeError("kubectl failed (exit 1); no private command output was logged")
        return b""

    def job_from_cron(self, namespace, cronjob, name):
        self.record("observe", identity(namespace))
        if self.on_observe:
            self.on_observe(namespace)
        mode = self.script.pop(0) if self.script else "flags"
        status = {"failed": 1} if mode == "failed" else {"succeeded": 1}
        self.put({"kind": "Job", "metadata": {"name": name, "namespace": namespace}, "status": status})
        if mode != "failed":
            self.results[namespace] = self.snapshot(namespace, mode)

    def upgrade(self, namespace, release, chart, values):
        self.record("helm", identity(namespace), release)
        self.releases[(namespace, release)] = deepcopy(values)
        if release == "env":
            for name in ("env-environment-postgres", "env-environment-nats"):
                if self.key("statefulset", name, namespace) not in self.objects:
                    self.dynamic_claim(namespace, "data-" + name + "-0")
                self.workload("statefulset", namespace, name, 1)
            self.workload("deployment", namespace, "env-environment-edge", 1)
            if self.key("cronjob", "env-environment-activity", namespace) not in self.objects:
                self.put({"kind": "CronJob", "metadata": {"name": "env-environment-activity", "namespace": namespace}, "spec": {}})
                self.put({"kind": "ConfigMap", "metadata": {"name": "env-environment-activity", "namespace": namespace}})
            if self.key("service", "env-environment-edge", namespace) not in self.objects:
                self.addresses += 1
                self.put({
                    "kind": "Service", "metadata": {"name": "env-environment-edge", "namespace": namespace},
                    "status": {"loadBalancer": {"ingress": [{"ip": f"203.0.113.{self.addresses}"}]}},
                })
        else:
            annotations = {"aidash.run/source-sha": values["release"]["sourceSha"]}
            template = {"spec": {"containers": [{
                "name": "aidash", "args": ["server"], "ports": [{"containerPort": 8080}],
                "readinessProbe": {}, "envFrom": [{"secretRef": {"name": "app-runtime"}}],
            }]}}
            for role in ("server", "worker"):
                self.workload(
                    "deployment", namespace, "app-aidash-" + role, values[role]["replicas"],
                    metadata={"annotations": annotations}, spec={"template": template},
                )
            self.workload("deployment", namespace, "app-execution-runner", 1)
            execution = values["execution"]
            for name in (execution["sandboxNamespace"], execution["trustedNamespace"]):
                self.put({"kind": "Namespace", "metadata": {"name": name}})

    def uninstall(self, namespace, release):
        self.record("uninstall", identity(namespace), release)
        self.releases.pop((namespace, release), None)

    def values(self, namespace, release):
        bounded_timeout(1)
        return deepcopy(self.releases[(namespace, release)])

    def delete_disk(self, project, zone, name):
        self.record("delete_disk", name)
        assert project == PROJECT and zone == "us-central1-a"
        self.disks.discard(name)

    # Assertions
    def secret(self, namespace, name):
        value = self.objects[self.key("secret", name, namespace)]
        return {key: base64.b64decode(data).decode() for key, data in value["data"].items()}

    def replicas(self, namespace, kind, name):
        return self.objects[self.key(kind, name, namespace)]["spec"]["replicas"]
