"""Kubernetes/Helm boundary for Environments on the shared GKE cluster. Infrastructure only.

Charts come only from this trusted checkout. Secrets reach the cluster through
kubectl stdin; Helm values files never contain secret material.
"""

import base64
from copy import deepcopy
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
from tempfile import TemporaryDirectory
import time

from cloud import bounded_timeout, private_json, run
from policy import ENVIRONMENT, Refused, idle_due

ROOT = Path(__file__).resolve().parents[3]
APP_CHART = ROOT / "deploy/helm/aidash"
ENV_CHART = ROOT / "infra/gcp/helm/environment"

STORAGE_CLASS = "aidash-retain"
# Production keeps every retained disk on regional pd-balanced, replicated to
# the cluster's standby zone, and fails its StatefulSets over there (ADR 0067).
PRODUCTION = "production"
REGIONAL_DISK = {"replication-type": "regional-pd", "availability-class": "regional-hard-failover"}
JOURNAL_CLAIM = "app-execution-journal"
PREVIEW_VOLUME = "aidash-preview-tls"
PREVIEW_CLAIM = "preview-tls"
PREVIEW_SIZE = "10Gi"
PREVIEW_TLS_SECRET = "aidash-preview-tls"
PREVIEW_HOST = re.compile(r"[a-f0-9]{32}\.preview\.aidash\.run")
CLAIMS = {"memory-recovery": "2Gi", "capability-objects": "10Gi"}
# The Home ledger lives in a child of its claim. A fresh disk's root belongs to
# root, so the application UID can create the child (fsGroup grants group write)
# but could never chmod the root itself as ledger initialization requires.
RECOVERY_ROOT = "/var/lib/aidash/memory-recovery"
RECOVERY_HOME = "home"
# Demo data: the structure ships in the image; the people come from a Secret
# mounted only into the migration Job (ADR 0080).
DEMO_STRUCTURE = "/app/deploy/demo-data/structure.json"
DEMO_PEOPLE_DIR = "/run/aidash/demo-data"
DEMO_SECRET = "app-demo-data"
# Deployments outside production; see `target`.
SERVER = "deployment/app-aidash-server"
WORKER = "deployment/app-aidash-worker"
RUNNER = "deployment/app-execution-runner"
EDGE = "deployment/env-environment-edge"
STATEFULSETS = ("statefulset/env-environment-postgres", "statefulset/env-environment-nats")
ACTIVITY = "env-environment-activity"
# Controller-owned desired admission state, read by every edge at start. Never
# rendered by a chart, so a Helm upgrade cannot reset it.
ADMISSION = "env-environment-admission"
LOAD_BALANCER = "env-environment-edge"
WRITER_PODS = "app.kubernetes.io/instance=app,app.kubernetes.io/component in (server,worker)"
MIGRATION = "app.kubernetes.io/instance=app,app.kubernetes.io/component=migration"
RUNNER_PODS = "aidash.run/runner=app"
EDGE_PODS = "aidash.run/edge=env"
WRITER_REPLICAS = 1
LABEL = "aidash.run/environment"
DEFAULT_TOLERATIONS = [
    {"key": "node.kubernetes.io/not-ready", "operator": "Exists", "effect": "NoExecute", "tolerationSeconds": 30},
    {"key": "node.kubernetes.io/unreachable", "operator": "Exists", "effect": "NoExecute", "tolerationSeconds": 30},
]
DISK = re.compile(r"projects/([a-z][a-z0-9-]{4,28}[a-z0-9])/zones/([a-z0-9-]+)/disks/([a-z0-9-]+)")
LEGACY_FINGERPRINT_KEY = "AIDASH_SECRET_PROVIDER_FINGERPRINT"
RUNTIME_KEYS = {
    "AIDASH_PROVIDER_FINGERPRINT_KEY",
    "AIDASH_OIDC_CLIENT_ID",
    "AIDASH_OIDC_CLIENT_SECRET",
    "AIDASH_OIDC_SESSION_ABSOLUTE_SECONDS",
    "AIDASH_OIDC_SESSION_IDLE_SECONDS",
}
GCIP_KEYS = {
    "project_id", "web_api_key", "public_origin", "shared_pool", "tenant_bindings",
    "providers", "password_sign_up", "auth_helper", "sign_in_domains", "mfa_mandatory",
    "session_absolute_seconds", "session_idle_seconds",
}
IDENTITY_TOKENS = ("database", "api", "runner", "observer", "owner", "server")
# Migrations log in as the owner of every table. Servers and workers log in as a
# role that holds only DML, so row-level security binds it (#262). The bootstrap
# superuser (the "database" token) only provisions these roles and extensions.
OWNER_ROLE = "aidash_owner"
SERVER_ROLE = "aidash_server"
# The activity observer's own login. It reads only the tables queried by
# infra/gcp/observer/src/main.rs, plus pg_read_all_stats: without it,
# pg_stat_activity hides other sessions' state and in-flight work would look idle.
OBSERVER_ROLE = "aidash_observer"
OBSERVER_TABLES = (
    "runs", "human_requests", "core_records", "agent_test_sessions", "generation_requests",
    "invocations", "atomic_participants", "core_operations", "atomic_coordinators",
)
DATABASE = "aidash_a"
POSTGRES = STATEFULSETS[0]


class Cluster:
    """One private kubeconfig. Every method is a single kubectl/helm/gcloud command."""

    def __init__(self, config, cluster, directory):
        if cluster.get("project_id") != config["project_id"]:
            raise Refused("Cluster output belongs to another project")
        self.directory = Path(directory)
        self.project = cluster["project_id"]
        self.location = cluster["location"]
        self.pod_cidr = cluster["pod_cidr"]
        # Where regional retained disks replicate, the pools' zone first.
        self.zones = list(cluster.get("zones") or [])
        self.kubeconfig = self.directory / "kubeconfig"
        os.close(os.open(self.kubeconfig, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600))
        run(
            "gcloud", "container", "clusters", "get-credentials", cluster["name"],
            "--location", self.location, "--project", self.project, "--dns-endpoint",
            env={"KUBECONFIG": str(self.kubeconfig)}, timeout=60,
        )

    def kubectl(self, *args, data=None, timeout=120):
        return run("kubectl", "--kubeconfig", self.kubeconfig, *args, data=data, timeout=timeout)

    def helm(self, *args, timeout=900):
        return run("helm", "--kubeconfig", self.kubeconfig, *args, timeout=timeout)

    @staticmethod
    def scope(namespace):
        return ("--namespace", namespace) if namespace else ()

    def get(self, kind, name, namespace=None):
        value = self.kubectl("get", kind, name, *self.scope(namespace), "--ignore-not-found", "-o", "json")
        return json.loads(value) if value.strip() else None

    def items(self, kind, namespace=None, selector=None):
        selection = ("--selector", selector) if selector else ()
        return json.loads(self.kubectl("get", kind, *self.scope(namespace), *selection, "-o", "json"))["items"]

    def apply(self, manifest):
        # Server-side apply keeps no last-applied annotation copy of Secret data.
        self.kubectl(
            "apply", "--server-side", "--field-manager=aidash-controller", "--force-conflicts",
            "-f", "-", data=json.dumps(manifest).encode(),
        )

    def create(self, manifest):
        self.kubectl("create", "-f", "-", data=json.dumps(manifest).encode())

    def delete(self, kind, name, namespace=None, timeout=600):
        self.kubectl(
            "delete", kind, name, *self.scope(namespace), "--ignore-not-found", "--wait=true",
            f"--timeout={int(timeout)}s", timeout=timeout + 30,
        )

    def patch(self, kind, name, body, namespace=None):
        self.kubectl("patch", kind, name, *self.scope(namespace), "--type", "merge", "-p", json.dumps(body))

    def scale(self, namespace, target, replicas):
        self.kubectl("scale", target, "--namespace", namespace, f"--replicas={int(replicas)}")

    def rollout(self, namespace, target, timeout=600):
        self.kubectl(
            "rollout", "status", target, "--namespace", namespace, f"--timeout={int(timeout)}s",
            timeout=timeout + 30,
        )

    def exec(self, namespace, target, container, *command, data=None):
        # Secret input travels on stdin; exec arguments reach the API audit log.
        stdin = ("-i",) if data is not None else ()
        return self.kubectl(
            "exec", "--namespace", namespace, *stdin, target, "-c", container, "--", *command, data=data, timeout=60,
        )

    def job_from_cron(self, namespace, cronjob, name):
        self.kubectl("create", "job", name, f"--from=cronjob/{cronjob}", "--namespace", namespace)

    def upgrade(self, namespace, release, chart, values):
        path = self.directory / f"{namespace}-{release}.values.json"
        private_json(path, values)
        try:
            # The controller scales workloads with `kubectl scale`, which owns
            # spec.replicas afterwards; without forcing, Helm's server-side apply
            # refuses every later upgrade that renders a different replica count.
            self.helm(
                "upgrade", "--install", release, chart, "--namespace", namespace,
                "--values", path, "--timeout", "15m", "--server-side", "true", "--force-conflicts",
            )
        finally:
            path.unlink(missing_ok=True)

    def uninstall(self, namespace, release):
        self.helm("uninstall", release, "--namespace", namespace, "--ignore-not-found", "--wait", "--timeout", "10m")

    def values(self, namespace, release):
        return json.loads(self.helm("get", "values", release, "--namespace", namespace, "--output", "json"))

    def delete_disk(self, project, zone, name):
        # List by exact name; a describe error is not proof that the disk is gone.
        found = json.loads(run(
            "gcloud", "compute", "disks", "list", "--project", project, "--zones", zone,
            "--filter", "name=" + name, "--format=json(name)", timeout=60,
        ))
        if found:
            run("gcloud", "compute", "disks", "delete", name, "--project", project, "--zone", zone, "--quiet", timeout=300)


def connect(config, cluster, directory):
    return Cluster(config, cluster, directory)


def target(identity, workload):
    """A disk-holding workload: a StatefulSet in production, which Stateful HA
    fails over to the standby zone, and a Deployment elsewhere (ADR 0067)."""
    if identity == PRODUCTION:
        return workload.replace("deployment/", "statefulset/", 1)
    return workload


def storage_class(cluster, identity):
    """The retention class, regional in production's own cluster (ADR 0067)."""
    manifest = {
        "apiVersion": "storage.k8s.io/v1", "kind": "StorageClass", "metadata": {"name": STORAGE_CLASS},
        "provisioner": "pd.csi.storage.gke.io", "parameters": {"type": "pd-balanced"},
        "reclaimPolicy": "Retain", "volumeBindingMode": "WaitForFirstConsumer", "allowVolumeExpansion": True,
    }
    if identity != PRODUCTION:
        return manifest
    if len(cluster.zones) != 2:
        raise Refused("Production needs exactly two zones for its regional disks")
    manifest["parameters"].update(REGIONAL_DISK)
    manifest["allowedTopologies"] = [{"matchLabelExpressions": [
        {"key": "topology.gke.io/zone", "values": cluster.zones},
    ]}]
    return manifest


def namespaces(identity):
    if not ENVIRONMENT.fullmatch(identity):
        raise Refused("Invalid environment identity")
    namespace = "aidash-" + identity
    return namespace, namespace + "-sandbox", namespace + "-trusted", namespace + "-installer"


def namespace_of(identity):
    return namespaces(identity)[0]


def wait_until(predicate, message, seconds=300, interval=5):
    deadline = time.monotonic() + seconds
    while not predicate():
        if time.monotonic() >= deadline:
            raise RuntimeError(message)
        time.sleep(bounded_timeout(interval))


def live_pods(cluster, namespace, selector=None):
    return [
        pod for pod in cluster.items("pods", namespace, selector)
        if pod.get("status", {}).get("phase") not in {"Succeeded", "Failed"}
    ]


def exists(cluster, namespace, workload):
    kind, name = workload.split("/", 1)
    return cluster.get(kind, name, namespace) is not None


def secret(namespace, name, values, environment=True):
    # envFrom Secrets carry single-line environment strings only.
    for key, value in values.items():
        if environment and (
            not re.fullmatch(r"[A-Z][A-Z0-9_]*", key) or not isinstance(value, str) or any(c in value for c in "\r\n\x00")
        ):
            raise Refused("runtime configuration must contain single-line environment strings")
    return {
        "apiVersion": "v1", "kind": "Secret", "type": "Opaque",
        "metadata": {"name": name, "namespace": namespace},
        "data": {key: base64.b64encode(value.encode()).decode() for key, value in sorted(values.items())},
    }


def claim(namespace, name, size, volume=None):
    spec = {
        "accessModes": ["ReadWriteOnce"],
        "storageClassName": "" if volume else STORAGE_CLASS,
        "resources": {"requests": {"storage": size}},
    }
    if volume:
        spec["volumeName"] = volume
    return {"apiVersion": "v1", "kind": "PersistentVolumeClaim", "metadata": {"name": name, "namespace": namespace}, "spec": spec}


def identity_tokens(cluster, namespace):
    value = cluster.get("secret", "aidash-identity", namespace)
    if value is None:
        # Generated exactly once; create fails rather than replace a concurrent value.
        cluster.create(secret(namespace, "aidash-identity", {
            name: secrets.token_hex(32) for name in IDENTITY_TOKENS
        }, environment=False))
        value = cluster.get("secret", "aidash-identity", namespace)
    tokens = {key: base64.b64decode(data).decode() for key, data in value["data"].items()}
    missing = [name for name in IDENTITY_TOKENS if name not in tokens]
    if missing:
        # An Environment created before a token existed gains it once; existing
        # tokens are kept. The resourceVersion precondition makes a concurrent
        # addition fail instead of being replaced.
        tokens.update({name: secrets.token_hex(32) for name in missing})
        manifest = secret(namespace, "aidash-identity", tokens, environment=False)
        manifest["metadata"]["resourceVersion"] = value["metadata"]["resourceVersion"]
        cluster.apply(manifest)
    return tokens


def preview_attached(cluster):
    return [
        item for item in cluster.items("volumeattachments")
        if item.get("spec", {}).get("source", {}).get("persistentVolumeName") == PREVIEW_VOLUME
    ]


def preview_owner(cluster, identity):
    """Return the other namespace still claiming the preview disk, if any."""
    volume = cluster.get("persistentvolume", PREVIEW_VOLUME)
    reference = ((volume or {}).get("spec") or {}).get("claimRef") or {}
    namespace = reference.get("namespace")
    if not namespace or (namespace, reference.get("name")) == (namespace_of(identity), PREVIEW_CLAIM):
        return None
    return namespace


def preview_tls_available(cluster, identity):
    owner = preview_owner(cluster, identity)
    return owner is None or not (cluster.items("pods", owner, EDGE_PODS) or preview_attached(cluster))


def install_preview_tls(cluster, identity, certificate_pem, private_key_pem):
    """Validate the managed wildcard and give its key only to the trusted edge.

    Temporary files are private, CLI output is captured, and only kubectl stdin
    carries the PEM to Kubernetes. Source builds and app-runtime never see it.
    """
    namespace = namespace_of(identity)
    if identity != "develop" and not identity.startswith("pr-"):
        raise Refused("Wildcard preview TLS is limited to staging and PR environments")
    if any(not isinstance(value, str) or not value or len(value.encode()) > 1024 * 1024 or "\x00" in value
           for value in (certificate_pem, private_key_pem)):
        raise Refused("Invalid preview TLS certificate or private key")
    with TemporaryDirectory(prefix="aidash-preview-tls-") as directory:
        certificate = Path(directory) / "certificate.pem"
        private_key = Path(directory) / "private-key.pem"
        for path, value in ((certificate, certificate_pem), (private_key, private_key_pem)):
            with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as file:
                file.write(value)
        try:
            alternatives = run("openssl", "x509", "-in", certificate, "-noout", "-ext", "subjectAltName", timeout=30).decode()
            names = re.findall(r"DNS:([^,\s]+)", alternatives)
            if names != ["*.preview.aidash.run"]:
                raise Refused("Preview TLS certificate must cover only *.preview.aidash.run")
            dates = run("openssl", "x509", "-in", certificate, "-noout", "-dates", timeout=30).decode()
            validity = dict(line.split("=", 1) for line in dates.splitlines())
            start, end = (datetime.strptime(validity[field], "%b %d %H:%M:%S %Y %Z").replace(tzinfo=timezone.utc)
                          for field in ("notBefore", "notAfter"))
            if not start <= datetime.now(timezone.utc) < end:
                raise Refused("Preview TLS certificate is outside its validity period")
            certificate_public = run("openssl", "x509", "-in", certificate, "-pubkey", "-noout", timeout=30)
            key_public = run("openssl", "pkey", "-in", private_key, "-pubout", timeout=30)
            if certificate_public.strip() != key_public.strip():
                raise Refused("Preview TLS certificate and private key do not match")
        except (RuntimeError, ValueError, KeyError) as error:
            raise Refused("Invalid preview TLS certificate or private key") from error
    manifest = secret(namespace, PREVIEW_TLS_SECRET, {"tls.crt": certificate_pem, "tls.key": private_key_pem}, environment=False)
    manifest["type"] = "kubernetes.io/tls"
    cluster.apply(manifest)
    # Caddy reads the mounted certificate at startup. A changed certificate must
    # roll the edge even when its routing values and release digest are unchanged.
    kind, name = target(identity, EDGE).split("/", 1)
    edge = cluster.get(kind, name, namespace)
    if edge:
        digest = hashlib.sha256(certificate_pem.encode()).hexdigest()
        annotations = edge.get("spec", {}).get("template", {}).get("metadata", {}).get("annotations", {})
        changed = annotations.get("aidash.run/preview-tls-sha256") != digest
        if changed:
            cluster.patch(kind, name, {"spec": {"template": {"metadata": {"annotations": {
                "aidash.run/preview-tls-sha256": digest,
            }}}}}, namespace)
        # A prior reconcile may have timed out after patching. Wait again for
        # that rollout without creating another restart or deploying app code.
        cluster.rollout(namespace, target(identity, EDGE))
        return changed
    return False


def bind_preview_tls(cluster, identity, volume_handle):
    namespace = namespace_of(identity)
    if not DISK.fullmatch(volume_handle or ""):
        raise Refused("Preview TLS volume handle is invalid")
    zone = DISK.fullmatch(volume_handle).group(2)
    reference = {"apiVersion": "v1", "kind": "PersistentVolumeClaim", "namespace": namespace, "name": PREVIEW_CLAIM}
    volume = cluster.get("persistentvolume", PREVIEW_VOLUME)
    if volume is None:
        cluster.create({
            "apiVersion": "v1", "kind": "PersistentVolume", "metadata": {"name": PREVIEW_VOLUME},
            "spec": {
                "capacity": {"storage": PREVIEW_SIZE},
                "accessModes": ["ReadWriteOnce"],
                "persistentVolumeReclaimPolicy": "Retain",
                "storageClassName": "",
                "claimRef": reference,
                "csi": {"driver": "pd.csi.storage.gke.io", "volumeHandle": volume_handle, "fsType": "ext4"},
                "nodeAffinity": {"required": {"nodeSelectorTerms": [{"matchExpressions": [
                    {"key": "topology.gke.io/zone", "operator": "In", "values": [zone]},
                ]}]}},
            },
        })
    else:
        if volume["spec"].get("csi", {}).get("volumeHandle") != volume_handle:
            raise Refused("Preview TLS volume does not reference the managed disk")
        owner = preview_owner(cluster, identity)
        if owner is not None:
            # A ReadWriteOnce disk must be detached from the previous preview first.
            wait_until(
                lambda: not cluster.items("pods", owner, EDGE_PODS) and not preview_attached(cluster),
                f"Preview TLS disk is still in use by {owner}",
            )
            cluster.delete("persistentvolumeclaim", PREVIEW_CLAIM, owner)
            cluster.patch("persistentvolume", PREVIEW_VOLUME, {"spec": {"claimRef": dict(reference, uid=None, resourceVersion=None)}})
        elif volume.get("status", {}).get("phase") == "Released" or not volume["spec"].get("claimRef"):
            # A recreated claim has a new UID; clear the stale one so it can bind.
            cluster.patch("persistentvolume", PREVIEW_VOLUME, {"spec": {"claimRef": dict(reference, uid=None, resourceVersion=None)}})
    if cluster.get("persistentvolumeclaim", PREVIEW_CLAIM, namespace) is None:
        cluster.create(claim(namespace, PREVIEW_CLAIM, PREVIEW_SIZE, PREVIEW_VOLUME))


def prepare(cluster, identity, preview_handle=None):
    namespace = namespace_of(identity)
    desired = storage_class(cluster, identity)
    existing = cluster.get("storageclass", STORAGE_CLASS)
    # Class parameters are immutable, and a zonal disk would keep the zone a
    # single point of failure: production is created on regional disks.
    if existing is not None and (
        existing.get("parameters") != desired["parameters"]
        or existing.get("allowedTopologies") != desired.get("allowedTopologies")
    ):
        raise Refused(f"StorageClass {STORAGE_CLASS} differs from this tier's retained disks")
    cluster.apply(desired)
    cluster.apply({
        "apiVersion": "v1", "kind": "Namespace",
        "metadata": {"name": namespace, "labels": {LABEL: identity}},
    })
    identity_tokens(cluster, namespace)
    for name, size in CLAIMS.items():
        if cluster.get("persistentvolumeclaim", name, namespace) is None:
            cluster.create(claim(namespace, name, size))
    if identity.startswith("pr-") and preview_handle is not None:
        bind_preview_tls(cluster, identity, preview_handle)


def provider_settings(descriptor, external):
    """Validate the managed Store/broker descriptor; key material never appears here."""
    if descriptor is None:
        descriptor = {"fingerprint_key": None, "store": None, "broker": None}
    if not isinstance(descriptor, dict) or set(descriptor) != {"fingerprint_key", "store", "broker"}:
        raise Refused("invalid managed Provider Credential configuration")
    store = descriptor["store"]
    if store is not None:
        if (
            not isinstance(store, dict)
            or set(store) != {"kind", "byok_project_id", "environment_id"}
            or store["kind"] != "secret_manager"
            or not isinstance(store["byok_project_id"], str)
            or not re.fullmatch(r"[a-z][a-z0-9-]{4,28}[a-z0-9]", store["byok_project_id"])
            or not isinstance(store["environment_id"], str)
            or not ENVIRONMENT.fullmatch(store["environment_id"])
            or descriptor["fingerprint_key"] != {"env": "AIDASH_PROVIDER_FINGERPRINT_KEY"}
        ):
            raise Refused("invalid managed Provider Credential Store")
        fingerprint = external.get("AIDASH_PROVIDER_FINGERPRINT_KEY")
        if not isinstance(fingerprint, str) or len(fingerprint.strip().encode()) < 32:
            raise Refused("BYOK requires a stable Provider Credential fingerprint key of at least 32 bytes")
    elif descriptor["fingerprint_key"] is not None:
        raise Refused("invalid managed Provider Credential configuration")
    broker = descriptor["broker"]
    if broker is not None and (
        store is None
        or not isinstance(broker, dict)
        or set(broker) != {"endpoint", "issuer", "audience", "kid"}
        or any(not isinstance(value, str) or not value for value in broker.values())
        or broker["audience"] != store["environment_id"]
    ):
        raise Refused("invalid managed Provider Credential broker")
    return deepcopy(descriptor)


def gcip_settings(project, output, dashboard):
    """Only the managed public GCIP fragment; separate from runtime secrets."""
    if dashboard is None:
        if PREVIEW_HOST.fullmatch(output["hostname"]):
            raise Refused("Random preview origins require managed GCIP authentication")
        return None
    if not isinstance(dashboard, dict) or set(dashboard) != {"gcip"}:
        raise Refused("managed dashboard configuration must select GCIP only")
    settings = dashboard["gcip"]
    if not isinstance(settings, dict) or set(settings) - GCIP_KEYS:
        raise Refused("unsupported managed GCIP configuration")
    if (
        settings.get("project_id") != project
        or settings.get("public_origin") != "https://" + output["hostname"]
        or not isinstance(settings.get("web_api_key"), str)
        or not settings["web_api_key"].strip()
        or not isinstance(settings.get("tenant_bindings"), dict)
        or not isinstance(settings.get("shared_pool", ""), str)
        or not isinstance(settings.get("sign_in_domains", {}), dict)
        or settings.get("auth_helper", "firebase") not in {"firebase", "public_origin"}
    ):
        raise Refused("managed GCIP configuration must match this environment")
    if PREVIEW_HOST.fullmatch(output["hostname"]) and (
        settings.get("auth_helper", "firebase") != "firebase"
        or not (settings["tenant_bindings"] or settings.get("shared_pool"))
    ):
        raise Refused("Random preview origins require the fixed Firebase helper and a managed GCIP Tenant")
    return {"dashboard": {"gcip": deepcopy(settings)}}


def materialize(cluster, identity, project, output, runtime):
    """Write the runtime Secrets from the selected Secret Manager JSON; return chart settings."""
    namespace = namespace_of(identity)
    external = deepcopy(runtime)
    dashboard = external.pop("dashboard", None)
    # Older BYOK configuration stored the fingerprint under its Registry-resolvable
    # name. Carry the same value over and never emit the legacy name.
    legacy = external.pop(LEGACY_FINGERPRINT_KEY, None)
    if legacy is not None:
        external.setdefault("AIDASH_PROVIDER_FINGERPRINT_KEY", legacy)
    if dashboard is not None and any(key.startswith("AIDASH_OIDC_") for key in external):
        raise Refused("GCIP and OIDC runtime configuration cannot coexist")
    if not all(key.startswith("AIDASH_SECRET_") or key in RUNTIME_KEYS for key in external):
        raise Refused("runtime secret may contain only Google client/session and provider credential configuration")
    gcip = gcip_settings(project, output, dashboard)
    if gcip is None and (not external.get("AIDASH_OIDC_CLIENT_ID") or not external.get("AIDASH_OIDC_CLIENT_SECRET")):
        raise Refused("Google OAuth client configuration is required")
    provider = provider_settings(output.get("provider_credentials"), external)
    tokens = identity_tokens(cluster, namespace)
    server = f"postgres://{SERVER_ROLE}:{tokens['server']}@env-environment-postgres:5432/{DATABASE}"
    # Only the migration Job receives the owner's login; servers never do.
    owner = f"postgres://{OWNER_ROLE}:{tokens['owner']}@env-environment-postgres:5432/{DATABASE}"
    # The observer never receives the application's administrative login.
    observer = f"postgres://{OBSERVER_ROLE}:{tokens['observer']}@env-environment-postgres:5432/{DATABASE}"
    values = dict(
        external,
        DATABASE_URL=server,
        NATS_URL="nats://env-environment-nats:4222",
        AIDASH_API_TOKEN=tokens["api"],
        AIDASH_CORE_RUNNER_TOKEN=tokens["runner"],
    )
    if gcip is None:
        values.update(AIDASH_OIDC_ISSUER="https://accounts.google.com", AIDASH_OIDC_PUBLIC_ORIGIN="https://" + output["hostname"])
    for manifest in (
        secret(namespace, "app-runtime", values),
        secret(namespace, "app-migration", {"DATABASE_URL": owner}),
        secret(namespace, "app-runner", {"AIDASH_CORE_RUNNER_TOKEN": tokens["runner"]}),
        secret(namespace, "env-postgres", {"POSTGRES_USER": "aidash", "POSTGRES_PASSWORD": tokens["database"], "POSTGRES_DB": DATABASE}),
        secret(namespace, "env-activity", {"DATABASE_URL": observer, "AIDASH_CORE_RUNNER_TOKEN": tokens["runner"]}),
    ):
        cluster.apply(manifest)
    return {"gcip": gcip, "provider": provider}


def failover(identity):
    """Chart values that make production's disk-holding workloads StatefulSets
    which Stateful HA moves to the standby zone (ADR 0067)."""
    if identity != PRODUCTION:
        return {}
    return {"workloadKind": "StatefulSet", "highAvailability": {"enabled": True}}


def scheduling(identity):
    return {LABEL: identity}, [
        {"key": LABEL, "operator": "Equal", "value": identity, "effect": "NoSchedule"},
        *DEFAULT_TOLERATIONS,
    ]


def image(release, kind):
    value = release["images"][kind]
    if not re.fullmatch(r"[a-z0-9.-]+/[a-z0-9._/-]+@sha256:[a-f0-9]{64}", value):
        raise Refused("Every deployed image must be pinned by digest")
    return value


def app_values(cluster, identity, output, release, sha, settings):
    namespace, sandbox, trusted, installer = namespaces(identity)
    selector, tolerations = scheduling(identity)
    repository, digest = image(release, "app").split("@", 1)
    control = image(release, "control")
    # A journal PVC from an earlier release may have the cluster's former
    # default StorageClass. Render its current immutable value on upgrade.
    # Only a fresh claim should select the retention class.
    journal_claim = cluster.get("persistentvolumeclaim", JOURNAL_CLAIM, namespace)
    journal_class = (journal_claim.get("spec", {}).get("storageClassName", "")
                     if journal_claim else STORAGE_CLASS)

    def role(account):
        # Writers start only through start_writers, after migration and checks.
        return {"replicas": 0, "serviceAccount": {"annotations": {"iam.gke.io/gcp-service-account": account}}}

    return {
        "image": {"repository": repository, "digest": digest},
        "node": {"id": "aidash://" + output["runtime_secret"], "endpoint": "https://" + output["hostname"]},
        "existingSecret": "app-runtime",
        "memoryRecovery": {
            "existingClaim": "memory-recovery",
            "directory": f"{RECOVERY_ROOT}/{RECOVERY_HOME}",
            "subPath": RECOVERY_HOME,
        },
        "capabilities": {"storage": {"existingClaim": "capability-objects"}},
        "frontend": {"enabled": False},
        "release": {"sourceSha": sha},
        "trustedProxy": {"cidrs": [cluster.pod_cidr]},
        "backendIngress": {"podSelectors": [{"aidash.run/edge": "env"}]},
        "providerCredentials": {"settings": settings["provider"]},
        "gcip": {"settings": settings["gcip"]},
        "server": role(output["server_service_account"]),
        "worker": role(output["worker_service_account"]),
        "environment": {"nodeSelector": selector, "tolerations": tolerations},
        "execution": {
            "sandboxNamespace": sandbox,
            "trustedNamespace": trusted,
            # The Runner may exec into guard Pods; the privileged installer is
            # kept in a namespace where the Runner has no rights.
            "installerNamespace": installer,
            "createNamespaces": True,
            "sandboxImage": image(release, "sandbox"),
            "runtimeClass": {"create": True, "name": "aidash-gvisor-" + identity},
            "installer": {"enabled": True, "image": control},
            "guard": {"enabled": True, "image": control},
            # The journal proves termination and recovers uncertain work, so it is
            # retained like every other Environment disk and deleted only by destroy.
            # The activity collector reads /v1/activity; nothing else outside the
            # application may connect to the Runner.
            "runner": {"enabled": True, "image": control, "existingSecret": "app-runner",
                       "ingress": {"podSelectors": [{"aidash.run/activity": "env"}]},
                       "journal": {"storageClassName": journal_class}},
        },
        **failover(identity),
    }


def env_values(identity, output, release, settings):
    selector, tolerations = scheduling(identity)
    gcip = (settings["gcip"] or {}).get("dashboard", {}).get("gcip", {})
    preview = bool(PREVIEW_HOST.fullmatch(output["hostname"]))
    return {
        "nodeSelector": selector,
        "tolerations": tolerations,
        "postgres": {"image": image(release, "postgres"), "existingSecret": "env-postgres"},
        "nats": {"image": image(release, "nats")},
        "edge": {
            "hostname": output["hostname"],
            "backend": "app-backend",
            "caddyImage": image(release, "caddy"),
            "admissionImage": image(release, "edge"),
            "existingTlsClaim": PREVIEW_CLAIM if identity.startswith("pr-") and not preview else "",
            "tlsSecret": PREVIEW_TLS_SECRET if preview else "",
            "loadBalancer": True,
            # Serve Firebase's sign-in helper exactly when the server tells the
            # browser to use this origin for it.
            "firebaseProject": gcip["project_id"] if gcip.get("auth_helper") == "public_origin" else "",
        },
        "activity": {
            "observerImage": image(release, "observer"),
            "collectorImage": image(release, "control"),
            "existingSecret": "env-activity",
            "runnerEndpoint": "http://app-execution-runner:8949",
        },
        **failover(identity),
    }


def desired_admission(cluster, namespace, state):
    cluster.apply({
        "apiVersion": "v1", "kind": "ConfigMap",
        "metadata": {"name": ADMISSION, "namespace": namespace},
        "data": {"state": state},
    })


def admission(cluster, identity, action):
    """Toggle the live edge and its durable state so that any partial failure
    leaves a restarted edge closed: close records first, open records last."""
    if action not in {"close", "open"}:
        raise Refused("Invalid admission action")
    namespace = namespace_of(identity)
    command = ("curl", "-fsS", "-X", "POST", f"http://127.0.0.1:8089/admission/{action}")
    if action == "close":
        desired_admission(cluster, namespace, "closed")
        # Without an edge Pod there is no live gate; a new one starts closed.
        if live_pods(cluster, namespace, EDGE_PODS):
            cluster.exec(namespace, target(identity, EDGE), "admission", *command)
    else:
        cluster.exec(namespace, target(identity, EDGE), "admission", *command)
        desired_admission(cluster, namespace, "open")


def wait_job(cluster, namespace, name, seconds):
    deadline = time.monotonic() + seconds
    while True:
        status = (cluster.get("job", name, namespace) or {}).get("status", {})
        if status.get("succeeded"):
            return True
        if status.get("failed") or time.monotonic() >= deadline:
            return False
        time.sleep(bounded_timeout(3))


def activity(cluster, identity):
    value = cluster.get("configmap", ACTIVITY, namespace_of(identity))
    try:
        return json.loads(value["data"]["snapshot.json"])
    except (TypeError, KeyError, ValueError):
        return None


def idle_now(snapshot):
    counts = snapshot.get("counts")
    return (
        snapshot.get("protocol") == "aidash-infra-activity/1"
        and not snapshot.get("error")
        and not snapshot.get("observation_gap")
        and snapshot.get("busy") is False
        and isinstance(counts, dict)
        # Database bookkeeping does not renew idle, but a drained Environment
        # must have none in flight before its dependencies stop.
        and all(type(value) is int and value == 0 for value in counts.values())
    )


def observe(cluster, identity, since):
    """Run the activity collector now; only a snapshot taken after `since` counts."""
    namespace = namespace_of(identity)
    name = "seal-" + secrets.token_hex(4)
    cluster.job_from_cron(namespace, ACTIVITY, name)
    try:
        if not wait_job(cluster, namespace, name, 180):
            return None
    finally:
        try:
            cluster.delete("job", name, namespace, timeout=60)
        except Exception:
            pass
    snapshot = activity(cluster, identity)
    if not isinstance(snapshot, dict) or not isinstance(snapshot.get("observed_at"), (int, float)):
        return None
    # A minute CronJob run can publish after this Job with evidence gathered
    # before the drain; only this Job's own post-drain snapshot counts.
    if snapshot.get("observation_job") != name:
        return None
    return snapshot if snapshot["observed_at"] >= since else None


def drain(cluster, identity, workloads=(SERVER, WORKER), selector=WRITER_PODS):
    namespace = namespace_of(identity)
    if cluster.get("namespace", namespace) is None:
        return
    for workload in workloads:
        if exists(cluster, namespace, target(identity, workload)):
            cluster.scale(namespace, target(identity, workload), 0)
    wait_until(lambda: not live_pods(cluster, namespace, selector), "Application Pods did not drain", 300)


def start_writers(cluster, identity):
    namespace = namespace_of(identity)
    for workload in (SERVER, WORKER):
        cluster.scale(namespace, target(identity, workload), WRITER_REPLICAS)
    for workload in (SERVER, WORKER):
        cluster.rollout(namespace, target(identity, workload))


def unseal(cluster, identity):
    start_writers(cluster, identity)
    admission(cluster, identity, "open")


def seal(cluster, identity, force=False, idle_only=False, keepalive_at=0):
    """Close admission, drain every producer, then require a fresh idle observation.

    The Runner, edge, dependencies and collector keep running; they are stopped
    only after this returns True. Any doubt restores service and defers. A
    namespace without live Pods (stopped, or a stop whose node pool apply
    failed) has no producer to drain and is already sealed.
    """
    namespace = namespace_of(identity)
    if cluster.get("namespace", namespace) is None:
        return True
    if not live_pods(cluster, namespace):
        desired_admission(cluster, namespace, "closed")
        return True
    try:
        admission(cluster, identity, "close")
        drain(cluster, identity)
        if force:
            return True
        since = time.time()
        snapshot = observe(cluster, identity, since)
        # A restarted edge may restore a stale `open` desired state; only an
        # observation proving the live gate closed may authorize the stop.
        idle = snapshot is not None and snapshot.get("edge_closed") is True and idle_now(snapshot)
        if idle and idle_only:
            try:
                idle = idle_due(snapshot, time.time(), keepalive_at)
            except Refused:
                idle = False
        if not idle:
            unseal(cluster, identity)
            return False
        return True
    except BaseException:
        if not force:
            try:
                unseal(cluster, identity)
            except Exception:
                pass
        raise


def quiesce(cluster, identity):
    """Fence old application authority: admission, writers and Runner stop; no rollback."""
    namespace = namespace_of(identity)
    if cluster.get("namespace", namespace) is None:
        return True
    admission(cluster, identity, "close")
    drain(cluster, identity)
    drain(cluster, identity, (RUNNER,), RUNNER_PODS)
    return True


def gate(cluster, identity):
    admission(cluster, identity, "close")


def deploy(cluster, identity, output, release, sha, settings, demo_people=None, fork=False):
    """Install both releases with writers stopped, start dependencies, then migrate."""
    namespace = namespace_of(identity)
    # An edge started by this deploy stays closed until readiness opens it, even
    # after a forced stop that skipped the seal.
    desired_admission(cluster, namespace, "closed")
    # The app release goes first with writers at zero: the edge's Nginx resolves the
    # backend Service name at start and crash-loops until that Service exists.
    cluster.upgrade(namespace, "app", APP_CHART, app_values(cluster, identity, output, release, sha, settings))
    cluster.upgrade(namespace, "env", ENV_CHART, env_values(identity, output, release, settings))
    for workload in (*STATEFULSETS, target(identity, EDGE)):
        cluster.scale(namespace, workload, 1)
    cluster.patch("cronjob", ACTIVITY, {"spec": {"suspend": False}}, namespace)
    for workload in STATEFULSETS:
        cluster.rollout(namespace, workload)
    cluster.scale(namespace, target(identity, RUNNER), 1)
    cluster.rollout(namespace, target(identity, RUNNER))
    database_roles(cluster, identity)
    migrate(cluster, identity, sha, demo_people, fork)
    observer_role(cluster, identity)
    cluster.rollout(namespace, target(identity, EDGE))


def migrate(cluster, identity, sha, demo_people=None, fork=False):
    """One-shot Job with the server's exact image, env, mounts and identity.

    Every Environment but production also loads Demo data in the Job (ADR 0080).
    `demo_people` is the people file's JSON; an Environment built from a fork
    never receives it, whatever the caller passes.
    """
    namespace = namespace_of(identity)
    demo = identity != PRODUCTION
    people = demo_people if demo and not fork else None
    template = deepcopy(cluster.get(*target(identity, SERVER).split("/", 1), namespace)["spec"]["template"])
    spec = template["spec"]
    container = next(item for item in spec["containers"] if item["name"] == "aidash")
    for key in ("startupProbe", "readinessProbe", "livenessProbe", "ports"):
        container.pop(key, None)
    container.pop("args", None)
    for mount in container.get("volumeMounts", []):
        if mount["name"] == "memory-recovery":
            # The claim root, not the subPath: kubelet would create a missing subPath
            # as root, while init-if-missing creates the Home as the application UID.
            mount.pop("subPath", None)
            mount["mountPath"] = RECOVERY_ROOT
    # The Job, unlike the server, logs in as the owner of every table.
    container["env"] = [item for item in container.get("env", []) if item["name"] != "DATABASE_URL"] + [
        {"name": "DATABASE_URL", "valueFrom": {"secretKeyRef": {"name": "app-migration", "key": "DATABASE_URL"}}},
    ]
    container["command"] = ["/bin/sh", "-ec"]
    container["args"] = [
        'aidash migrate && aidash memory-recovery init-if-missing --directory "$AIDASH_MEMORY_RECOVERY_DIR"'
        " && aidash activation-provision"
    ]
    if demo:
        container["args"][0] += f" && aidash demo-data --from {DEMO_STRUCTURE}"
        container["env"].append({"name": "AIDASH_DEMO_DATA_ENABLED", "value": "true"})
        if people is not None:
            # Mounted into this Job's container only, never into a server or worker.
            container["args"][0] += f" --people {DEMO_PEOPLE_DIR}/people.json"
            spec.setdefault("volumes", []).append({"name": "demo-data", "secret": {"secretName": DEMO_SECRET}})
            container.setdefault("volumeMounts", []).append(
                {"name": "demo-data", "mountPath": DEMO_PEOPLE_DIR, "readOnly": True},
            )
    spec["restartPolicy"] = "Never"
    # Never matches the backend Service or the writer drain selector. The copied
    # template keeps the required co-location term for the shared RWO claims;
    # with the writers at zero the Job Pod can only satisfy it by matching it.
    labels = {"app.kubernetes.io/instance": "app", "app.kubernetes.io/component": "migration"}
    if template.get("metadata", {}).get("labels", {}).get("aidash.run/co-located") == "true":
        labels["aidash.run/co-located"] = "true"
    # An interrupted reconciliation can leave its Job running. A second one would
    # mount the same RWO claims and run init-if-missing and activation
    # provisioning concurrently, so every predecessor must finish first; a
    # finished one, failed or not, has no Pod left that could still write.
    wait_until(
        lambda: not live_pods(cluster, namespace, MIGRATION) and all(
            job.get("status", {}).get("succeeded") or job.get("status", {}).get("failed")
            for job in cluster.items("job", namespace, MIGRATION)
        ),
        "A previous migration Job is still running; refusing to start another",
        960,
    )
    name = f"migrate-{sha[:12]}-{secrets.token_hex(3)}"
    if demo:
        # Whatever an earlier deploy left is gone before this Job can mount it,
        # so a fork or an unconfigured Environment never sees old people.
        drop_demo_people(cluster, namespace)
        if people is not None:
            cluster.apply(secret(namespace, DEMO_SECRET, {"people.json": people}, environment=False))
    try:
        cluster.create({
            "apiVersion": "batch/v1", "kind": "Job",
            "metadata": {"name": name, "namespace": namespace, "labels": labels},
            "spec": {
                "backoffLimit": 0,
                "activeDeadlineSeconds": 900,
                "ttlSecondsAfterFinished": 3600,
                "template": {
                    "metadata": {"labels": labels},
                    "spec": spec,
                },
            },
        })
        if not wait_job(cluster, namespace, name, 900):
            raise RuntimeError("Migration Job failed; the retained database was not replaced")
    finally:
        if demo:
            # The finished Job needs the people no longer; do not leave them in the namespace.
            drop_demo_people(cluster, namespace)


def drop_demo_people(cluster, namespace):
    if cluster.get("secret", DEMO_SECRET, namespace) is not None:
        cluster.delete("secret", DEMO_SECRET, namespace)


# A table owned by another role predates the owner and server roles: the
# migration Job could not change it and the server could not read it. This
# holds even before the owner role exists, so ownership is matched by name.
LEGACY_OWNERSHIP_SQL = (
    "SELECT EXISTS (SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace"
    " WHERE n.nspname = 'public' AND c.relkind IN ('r', 'p')"
    f" AND NOT EXISTS (SELECT 1 FROM pg_roles r WHERE r.oid = c.relowner AND r.rolname = '{OWNER_ROLE}')"
    " AND NOT EXISTS (SELECT 1 FROM pg_depend d WHERE d.classid = 'pg_class'::regclass"
    " AND d.objid = c.oid AND d.deptype = 'e'));"
)
# PGroonga's crash-safe worker starts asynchronously in each database; flushing
# its catalog proves it is ready before the migration Job builds indexes.
PGROONGA_SQL = "CREATE EXTENSION IF NOT EXISTS pgroonga;\nSELECT pgroonga_command('io_flush');\n"


def database_roles_sql(owner_password, server_password, database=DATABASE):
    """Idempotent SQL for the owner and server logins, the database's ownership,
    the superuser-only extensions and the server's default privileges."""
    if not all(re.fullmatch(r"[0-9a-f]{64}", value) for value in (owner_password, server_password)):
        raise Refused("Database role tokens are invalid")
    lines = []
    for role, password in ((OWNER_ROLE, owner_password), (SERVER_ROLE, server_password)):
        lines += [
            f"DO $$ BEGIN IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '{role}')"
            f" THEN CREATE ROLE {role}; END IF; END $$;",
            f"ALTER ROLE {role} WITH LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION"
            f" NOBYPASSRLS INHERIT PASSWORD '{password}';",
        ]
    return "\n".join((
        *lines,
        f"ALTER DATABASE {database} OWNER TO {OWNER_ROLE};",
        "CREATE EXTENSION IF NOT EXISTS pg_jsonschema WITH SCHEMA public;",
        "CREATE EXTENSION IF NOT EXISTS vector;",
        # Default privileges apply only to objects the owner creates afterwards.
        f"ALTER DEFAULT PRIVILEGES FOR ROLE {OWNER_ROLE} GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO {SERVER_ROLE};",
        f"ALTER DEFAULT PRIVILEGES FOR ROLE {OWNER_ROLE} GRANT USAGE, SELECT ON SEQUENCES TO {SERVER_ROLE};",
        "",
    ))


def database_roles(cluster, identity):
    """Provision the owner and server logins before the migration Job runs.

    Passwords travel on stdin through the PostgreSQL container's local socket,
    as the observer's do.
    """
    namespace = namespace_of(identity)
    tokens = identity_tokens(cluster, namespace)

    def psql(sql, *options):
        return cluster.exec(
            namespace, POSTGRES, "postgres", "sh", "-ec",
            'exec psql --no-psqlrc --quiet -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB" ' + " ".join(options),
            data=sql.encode(),
        )

    if psql(LEGACY_OWNERSHIP_SQL, "--tuples-only", "--no-align").strip() == b"t":
        raise Refused(
            "This Environment's database predates the owner and server roles (#262);"
            " destroy and recreate the Environment"
        )
    psql(database_roles_sql(tokens["owner"], tokens["server"]), "--single-transaction")
    # A failed PGroonga start is remembered by its backend, so each attempt is a
    # new psql session; command output stays private, so every failure retries.
    for attempt in range(30):
        try:
            psql(PGROONGA_SQL)
            return
        except RuntimeError:
            if attempt == 29:
                raise
            time.sleep(1)


def observer_sql(password, database=DATABASE):
    """Idempotent SQL for the observer's login: SELECT on the tables it reads and
    pg_read_all_stats, nothing else. The tables must exist (after migration)."""
    if not re.fullmatch(r"[0-9a-f]{64}", password):
        raise Refused("Observer database token is invalid")
    return "\n".join((
        f"DO $$ BEGIN IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '{OBSERVER_ROLE}')"
        f" THEN CREATE ROLE {OBSERVER_ROLE}; END IF; END $$;",
        f"ALTER ROLE {OBSERVER_ROLE} WITH LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION"
        f" NOBYPASSRLS INHERIT PASSWORD '{password}';",
        f"GRANT CONNECT ON DATABASE {database} TO {OBSERVER_ROLE};",
        f"GRANT USAGE ON SCHEMA public TO {OBSERVER_ROLE};",
        f"GRANT SELECT ON TABLE {', '.join(OBSERVER_TABLES)} TO {OBSERVER_ROLE};",
        f"GRANT pg_read_all_stats TO {OBSERVER_ROLE};",
        "",
    ))


def observer_role(cluster, identity):
    """Create or update the activity observer's login after migration.

    A compromised observer must not be able to change retained data, so it never
    holds the application's administrative login. The SQL, including the
    password, travels on stdin through the PostgreSQL container's local socket.
    """
    namespace = namespace_of(identity)
    sql = observer_sql(identity_tokens(cluster, namespace)["observer"])
    cluster.exec(
        namespace, POSTGRES, "postgres", "sh", "-ec",
        'exec psql --no-psqlrc --quiet --single-transaction -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB"',
        data=sql.encode(),
    )


def reload(cluster, identity, output, settings):
    """Apply new settings to the deployed release without changing its images."""
    namespace = namespace_of(identity)
    values = cluster.values(namespace, "app")
    values.update(
        providerCredentials={"settings": settings["provider"]},
        gcip={"settings": settings["gcip"]},
    )
    # Releases installed before the installer had its own namespace carry no
    # value for it; the chart default would be shared by every Environment.
    values.setdefault("execution", {})["installerNamespace"] = namespaces(identity)[3]
    for role in ("server", "worker"):
        values.setdefault(role, {})["replicas"] = 0
    cluster.upgrade(namespace, "app", APP_CHART, values)
    cluster.scale(namespace, target(identity, RUNNER), 1)
    cluster.rollout(namespace, target(identity, RUNNER))
    # materialize() points the runtime Secrets at the server and observer logins.
    database_roles(cluster, identity)
    observer_role(cluster, identity)
    start_writers(cluster, identity)


def health(cluster, identity):
    """Return the authorized source of a completely rolled-out server."""
    namespace = namespace_of(identity)
    for workload in (SERVER, WORKER, RUNNER):
        cluster.rollout(namespace, target(identity, workload))
    value = cluster.get(*target(identity, SERVER).split("/", 1), namespace) or {}
    status = value.get("status", {})
    if not status.get("readyReplicas"):
        raise RuntimeError("Application server is not ready")
    return value.get("metadata", {}).get("annotations", {}).get("aidash.run/source-sha")


def address(cluster, identity):
    namespace = namespace_of(identity)
    found = []

    def assigned():
        service = cluster.get("service", LOAD_BALANCER, namespace) or {}
        ingress = service.get("status", {}).get("loadBalancer", {}).get("ingress") or []
        found[:] = [item["ip"] for item in ingress if item.get("ip")]
        return bool(found)

    wait_until(assigned, "Edge LoadBalancer has no address", 600, 10)
    if not re.fullmatch(r"(?:[0-9]{1,3}\.){3}[0-9]{1,3}", found[0]):
        raise Refused("Edge LoadBalancer address is not IPv4")
    return found[0]


def stop(cluster, identity):
    """Stop every workload after an idle verdict or force; claims and Secrets stay."""
    namespace = namespace_of(identity)
    if cluster.get("namespace", namespace) is None:
        return
    desired_admission(cluster, namespace, "closed")
    for workload in (*(target(identity, w) for w in (SERVER, WORKER, RUNNER, EDGE)), *STATEFULSETS):
        if exists(cluster, namespace, workload):
            cluster.scale(namespace, workload, 0)
    if cluster.get("cronjob", ACTIVITY, namespace) is not None:
        cluster.patch("cronjob", ACTIVITY, {"spec": {"suspend": True}}, namespace)
    cluster.delete("service", LOAD_BALANCER, namespace)
    # Dependencies shut down gracefully before their node pool is removed.
    wait_until(lambda: not live_pods(cluster, namespace), "Environment Pods did not stop", 600)


def destroy(cluster, identity):
    """Delete namespaces and their retained disks; the shared preview disk is only released."""
    names = namespaces(identity)
    retained = []
    for volume in cluster.items("persistentvolumes"):
        if ((volume.get("spec") or {}).get("claimRef") or {}).get("namespace") not in names:
            continue
        name = volume["metadata"]["name"]
        if name == PREVIEW_VOLUME or volume["spec"].get("persistentVolumeReclaimPolicy") != "Retain":
            retained.append((name, None))
            continue
        handle = DISK.fullmatch(volume["spec"].get("csi", {}).get("volumeHandle", ""))
        # Refuse before anything is deleted, so a foreign disk never loses its namespace.
        if not handle or handle.group(1) != cluster.project or handle.group(3) == PREVIEW_VOLUME:
            raise Refused("Retained volume does not reference an Environment disk")
        retained.append((name, handle.groups()))
    for release in ("app", "env"):
        cluster.uninstall(names[0], release)
    for namespace in names:
        cluster.delete("namespace", namespace)
    for name, disk in retained:
        if name == PREVIEW_VOLUME:
            cluster.patch("persistentvolume", PREVIEW_VOLUME, {"spec": {"claimRef": None}})
            continue
        if disk is None:
            continue
        wait_until(
            lambda name=name: not any(
                item.get("spec", {}).get("source", {}).get("persistentVolumeName") == name
                for item in cluster.items("volumeattachments")
            ),
            f"Retained volume {name} is still attached",
        )
        # The disk goes first: a retry still finds it through the remaining PV.
        cluster.delete_disk(*disk)
        cluster.delete("persistentvolume", name)
