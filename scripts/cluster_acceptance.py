#!/usr/bin/env python3
"""Exercise real Kubernetes/k3s pods in a new, disposable namespace.

Requires an explicit kubeconfig. Creates only task-owned namespaced resources;
never changes the user's current context or touches an existing namespace.
"""
import argparse
import json
import os
import pathlib
import socket
import subprocess
import time
import uuid

from golden_path import ROOT, TOKEN, PEER_TOKEN, api_request, entity, wait_for


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--kubeconfig", required=True)
    parser.add_argument("--image", default="aidash:orchestration-local")
    parser.add_argument("--postgres-image", default="aidash-postgres:17-pg-jsonschema-0.3.4")
    parser.add_argument("--distribution", choices=["kubernetes", "k3s"], required=True)
    parser.add_argument("--keep", action="store_true")
    parser.add_argument("--dashboard", action="store_true")
    args = parser.parse_args()
    namespace = "aidash-ops-" + uuid.uuid4().hex[:10]
    report_dir = ROOT / ".ignore" / "platform" / namespace
    report_dir.mkdir(parents=True)
    env = {**os.environ, "KUBECONFIG": str(pathlib.Path(args.kubeconfig).resolve())}
    forwards = []
    logs = []

    def kube(*command, value=None):
        result = subprocess.run(["kubectl", "--namespace", namespace, *command], input=json.dumps(value) if value is not None else None, text=True, capture_output=True, env=env, check=True)
        return result.stdout

    def apply(value):
        kube("apply", "-f", "-", value=value)

    def service(name, port):
        return {"apiVersion": "v1", "kind": "Service", "metadata": {"name": name}, "spec": {"selector": {"app": name}, "ports": [{"port": port, "targetPort": port}]}}

    def stateful(name, image, port, container, mount, size="1Gi"):
        container = {"name": name, "image": image, "ports": [{"containerPort": port}], "resources": {"requests": {"cpu": "100m", "memory": "128Mi"}, "limits": {"cpu": "2", "memory": "1Gi"}}, "volumeMounts": [{"name": "data", "mountPath": mount}], **container}
        return {"apiVersion": "apps/v1", "kind": "StatefulSet", "metadata": {"name": name}, "spec": {"serviceName": name, "replicas": 1, "selector": {"matchLabels": {"app": name}}, "template": {"metadata": {"labels": {"app": name}}, "spec": {"automountServiceAccountToken": False, "containers": [container]}}, "volumeClaimTemplates": [{"metadata": {"name": "data"}, "spec": {"accessModes": ["ReadWriteOnce"], "resources": {"requests": {"storage": size}}}}]}}

    def forward(name, remote):
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        log = open(report_dir / f"forward-{name}.log", "w")
        logs.append(log)
        proc = subprocess.Popen(["kubectl", "-n", namespace, "port-forward", f"service/{name}", f"{port}:{remote}"], stdout=log, stderr=log, env=env)
        forwards.append(proc)
        return f"http://127.0.0.1:{port}"

    def rollout(name):
        kube("rollout", "status", f"deployment/{name}", "--timeout=180s")

    created = False
    try:
        kube("create", "namespace", namespace)
        created = True
        print(f"Created {args.distribution} acceptance namespace {namespace}", flush=True)
        apply({"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": "fixture-code"}, "data": {name: (ROOT / "scripts" / name).read_text() for name in ["golden_path.py", "cluster_fixture.py"]}})
        database = stateful("postgres", args.postgres_image, 5432, {"args": ["-c", "max_connections=400"], "env": [{"name": "POSTGRES_USER", "value": "aidash"}, {"name": "POSTGRES_PASSWORD", "value": "acceptance-local-password"}, {"name": "POSTGRES_DB", "value": "aidash_a"}], "readinessProbe": {"exec": {"command": ["pg_isready", "-h", "127.0.0.1", "-U", "aidash", "-d", "aidash_a"]}, "periodSeconds": 2}}, "/var/lib/postgresql/data")
        nats = stateful("nats", "nats:2.12-alpine", 4222, {"args": ["-js", "-sd", "/data"], "readinessProbe": {"tcpSocket": {"port": 4222}, "periodSeconds": 2}}, "/data")
        qdrant = stateful("qdrant", "qdrant/qdrant:v1.19.1", 6333, {"readinessProbe": {"tcpSocket": {"port": 6333}, "periodSeconds": 2}}, "/qdrant/storage")
        fixture = {"apiVersion": "apps/v1", "kind": "Deployment", "metadata": {"name": "fixture"}, "spec": {"replicas": 1, "selector": {"matchLabels": {"app": "fixture"}}, "template": {"metadata": {"labels": {"app": "fixture"}}, "spec": {"automountServiceAccountToken": False, "containers": [{"name": "fixture", "image": "python:3.13-alpine", "command": ["python3", "/scripts/cluster_fixture.py"], "ports": [{"containerPort": 8000}], "volumeMounts": [{"name": "code", "mountPath": "/scripts", "readOnly": True}], "readinessProbe": {"httpGet": {"path": "/status", "port": 8000}}, "resources": {"requests": {"cpu": "50m", "memory": "64Mi"}, "limits": {"cpu": "1", "memory": "256Mi"}}}], "volumes": [{"name": "code", "configMap": {"name": "fixture-code"}}]}}}}
        apply({"apiVersion": "v1", "kind": "List", "items": [database, nats, qdrant, fixture, service("postgres", 5432), service("nats", 4222), service("qdrant", 6333), service("fixture", 8000)]})
        kube("rollout", "status", "statefulset/postgres", "--timeout=180s")
        for node in ["a", "b"]:
            apply({"apiVersion": "v1", "kind": "Secret", "metadata": {"name": f"aidash-{node}"}, "stringData": {"DATABASE_URL": f"postgres://aidash:acceptance-local-password@postgres:5432/aidash_{node}", "NATS_URL": "nats://nats:4222", "AIDASH_API_TOKEN": TOKEN, "AIDASH_SECRET_PEER": PEER_TOKEN}})
            repo, tag = args.image.rsplit(":", 1)
            subprocess.run(["helm", "upgrade", "--install", f"ops-{node}", str(ROOT / "deploy/helm/aidash"), "--namespace", namespace, "--set", f"node.id=aidash://ops-{node}", "--set", f"existingSecret=aidash-{node}", "--set", f"image.repository={repo}", "--set", f"image.tag={tag}", "--wait", "--timeout", "5m"], check=True, env=env, stdout=subprocess.DEVNULL)
        for service_name in ["nats", "qdrant"]:
            kube("rollout", "status", f"statefulset/{service_name}", "--timeout=180s")
        rollout("fixture")
        fixture_url = forward("fixture", 8000)
        bases = [forward(f"ops-{node}-aidash", 8080) for node in ["a", "b"]]
        for base in bases:
            wait_for(lambda base=base: api_request(base, "/health"), label="cluster server")
        base_a, base_b = bases
        for base, other in [(base_a, "b"), (base_b, "a")]:
            api_request(base, "/api/peers", {"node_id": f"aidash://ops-{other}", "endpoint": f"http://ops-{other}-aidash:8080", "credential_env": "AIDASH_SECRET_PEER", "protocol_version": "0.1", "enabled": True})
            api_request(base, "/api/registry", entity("model", "fixture-model", {"provider": "openrouter", "model_id": "protocol-fixture", "endpoint": "http://fixture:8000/v1", "context_window": 256000, "max_output_tokens": 4096, "modalities": ["text"], "cost": {}, "credential_env": None}))
            tool = entity("tool", "research-http", {"transport": "http", "endpoint": "http://fixture:8000/research", "credential_env": None, "replay": "idempotent"})
            tool["schema"] = {"type": "object", "required": ["topic"], "properties": {"topic": {"type": "string"}}, "additionalProperties": False}
            api_request(base, "/api/registry", tool)
            api_request(base, "/api/registry", entity("agent", "researcher", {"model": {"id": "fixture-model", "version": "1.0.0"}, "instructions": "Research and publish findings.", "tools": [{"id": "research-http", "version": "1.0.0"}], "skills": [], "max_steps": 128}))
        api_request(base_a, "/api/registry", entity("agent", "coordinator", {"model": {"id": "fixture-model", "version": "1.0.0"}, "instructions": "Discover, delegate and synthesize.", "tools": [], "skills": [], "max_steps": 128}, capability="task.coordinate"))
        api_request(base_a, "/api/registry", entity("cluster", "research-cluster", {"coordinator": {"id": "coordinator", "version": "1.0.0"}}))
        discovered = api_request(base_a, "/api/discover", {"capability": "web.search", "language": "ja"})
        assert {a["node_id"] for a in discovered["agents"]} == {"aidash://ops-a", "aidash://ops-b"}
        goal = api_request(base_a, "/api/conversations", {"title": "Cluster recovery acceptance", "goal": "Compare Rust web frameworks", "target": {"id": "research-cluster", "version": "1.0.0"}, "target_kind": "cluster"})
        workspace = goal["workspace"]["id"]
        wait_for(lambda: api_request(fixture_url, "/status")["remote_effect_started"], timeout=90, label="remote external effect")
        before = api_request(base_b, "/api/state")["runs"][0]
        pods = json.loads(kube("get", "pods", "-l", "app.kubernetes.io/instance=ops-b,app.kubernetes.io/component=worker", "-o", "json"))["items"]
        assert len(pods) == 1
        kube("delete", "pod", pods[0]["metadata"]["name"], "--grace-period=0", "--force", "--wait=false")
        kube("scale", "deployment/ops-b-aidash-worker", "--replicas=3")
        kube("scale", "deployment/ops-a-aidash-server", "--replicas=2")
        print("Killed remote worker after effect; scaled workers to three and servers to two", flush=True)

        def complete():
            value = api_request(base_a, f"/api/workspaces/{workspace}")
            return value if len(value["tasks"]) == 4 and all(t["status"] == "COMPLETED" for t in value["tasks"]) else False

        snapshot = wait_for(complete, timeout=180, label="cluster golden path")
        recovered = wait_for(lambda: next((r for r in api_request(base_b, "/api/state")["runs"] if r["id"] == before["id"] and r["phase"] == "COMPLETED"), None), label="same remote run")
        assert len(snapshot["artifacts"]) == 4
        effects = api_request(fixture_url, "/status")
        assert len(effects["effects"]) == 3
        assert max(effects["requests"].values()) >= 2
        assert any(e["kind"] == "run.recovered" and e["data"].get("run_id") == recovered["id"] for e in api_request(base_b, "/api/events"))
        for node in ["a", "b"]:
            for role in ["server", "worker"]:
                name = f"ops-{node}-aidash-{role}"
                kube("rollout", "restart", f"deployment/{name}")
                rollout(name)
        # Port forwarding attaches to a Pod, not the Service's changing endpoints.
        bases = [forward(f"ops-{node}-aidash", 8080) for node in ["a", "b"]]
        for node, base in zip(["a", "b"], bases):
            identity = wait_for(lambda base=base: api_request(base, "/health"), label="rolled server")
            assert identity["node_id"] == f"aidash://ops-{node}"
        after = api_request(bases[0], f"/api/workspaces/{workspace}")
        assert {t["id"] for t in after["tasks"]} == {t["id"] for t in snapshot["tasks"]}
        assert {a["id"] for a in after["artifacts"]} == {a["id"] for a in snapshot["artifacts"]}
        observation = api_request(bases[1], "/api/deployment")
        assert observation["enabled"] and observation["namespace"] == namespace
        assert next(d for d in observation["deployments"] if d["role"] == "worker")["ready"] == 3
        assert all(p["name"].startswith("ops-b-") for p in observation["pods"])
        if args.dashboard:
            subprocess.run(["node", "web/scripts/inspect-deployment.mjs"], cwd=ROOT, env={**env, "AIDASH_E2E_URL": bases[1]}, check=True)
        kube("scale", "deployment/ops-b-aidash-worker", "--replicas=0")
        wait_for(lambda: not json.loads(kube("get", "pods", "-l", "app.kubernetes.io/instance=ops-b,app.kubernetes.io/component=worker", "-o", "json"))["items"], timeout=60, label="graceful stop")
        kube("scale", "deployment/ops-b-aidash-worker", "--replicas=1")
        rollout("ops-b-aidash-worker")
        assert len(api_request(fixture_url, "/status")["effects"]) == 3
        report = {"distribution": args.distribution, "kubernetes": json.loads(kube("version", "-o", "json"))["serverVersion"]["gitVersion"], "namespace": namespace, "image": args.image, "workspace": workspace, "recovered_run": recovered["id"], "tasks": 4, "artifacts": 4, "external_effects": 3, "worker_sigkill": "passed", "worker_scale_up_down": "passed", "server_scale_up": "passed", "rolling_updates": "passed", "stable_identity": "passed", "deployment_observation": observation, "base_a": bases[0], "base_b": bases[1]}
        (report_dir / "report.json").write_text(json.dumps(report, indent=2))
        print(f"Cluster acceptance passed: {report_dir / 'report.json'}", flush=True)
        if args.keep:
            print(f"Retained until interrupted: {bases}", flush=True)
            while True:
                time.sleep(1)
    finally:
        if created:
            try:
                (report_dir / "pods.json").write_text(kube("get", "pods", "-o", "json"))
                (report_dir / "events.json").write_text(kube("get", "events", "-o", "json"))
                for node in ["a", "b"]:
                    for role in ["server", "worker"]:
                        (report_dir / f"{node}-{role}.log").write_text(kube("logs", f"deployment/ops-{node}-aidash-{role}", "--all-pods=true", "--tail=100"))
            except subprocess.CalledProcessError:
                pass
        for process in forwards:
            process.terminate()
            process.wait(timeout=10)
        for log in logs:
            log.close()
        if created:
            kube("delete", "namespace", namespace, "--wait=false")


if __name__ == "__main__":
    main()
