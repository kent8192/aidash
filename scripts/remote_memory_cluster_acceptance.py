#!/usr/bin/env python3
"""Two real HTTP Nodes, separate PostgreSQL databases and native PostgreSQL search.

Run only against an explicit disposable Kubernetes/k3s cluster. The ordinary
case kills the inference worker. The generated case kills Home during embedding,
holds a link outage, and restarts both servers, the worker and PostgreSQL. Provider
requests, durable cuts, image IDs and failures are retained as synthetic evidence.
"""
import argparse
import copy
import hashlib
import json
import subprocess
import urllib.error
import urllib.request
import uuid

from transaction_cluster_acceptance import Cluster, ROOT, now


def digest(value):
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    return "sha256:" + hashlib.sha256(encoded).hexdigest()


def ref(entry):
    return {"id": entry["id"], "version": entry["version"]}


class RemoteMemory(Cluster):
    def __init__(self, args):
        super().__init__(args)
        self.tokens = []
        self.entries = [{}, {}]
        self.evidence = {"started_at": now(), "cases": [], "recovery": "Production leases expire naturally; IDs, fences, budgets and retry counters are never changed by the driver."}

    def user(self, node, path, body=None, method=None):
        request = urllib.request.Request(self.forward(node) + path,
            data=None if body is None else json.dumps(body).encode(), method=method,
            headers={"Authorization": f"Bearer {self.tokens[node]}", "Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(request, timeout=25) as response:
                return response.status, json.load(response)
        except urllib.error.HTTPError as error:
            return error.code, json.load(error)

    @staticmethod
    def ok(response):
        status, value = response
        assert status == 200, (status, value)
        return value

    def db(self, node, key, task=None, run=None):
        null = "00000000-0000-0000-0000-000000000000"
        result = self.kube("exec", "-i", "postgres-0", "--", "psql", "-U", "aidash", "-d", f"tx_{node}",
            "-qAt", "-v", "ON_ERROR_STOP=1", "-v", f"task={task or null}", "-v", f"run={run or null}",
            "-f", "-", value=self.queries[key] + ";\n", check=False)
        assert result.returncode == 0, (key, result.stderr)
        output = result.stdout
        return [json.loads(line) for line in output.splitlines() if line.strip()]

    def provider(self, body=None):
        command = ["exec", "deployment/tx-1-server", "--", "curl", "--fail", "--silent", "--show-error", "--max-time", "10"]
        if body is not None:
            command += ["-H", "Content-Type: application/json", "-d", json.dumps(body)]
        command += ["http://provider:8080/control" if body is not None else "http://provider:8080/status"]
        return json.loads(self.kube(*command).stdout)

    def captured(self, kind, task=None, embedding_input=None):
        state = self.provider()
        assert not state["errors"], state["errors"]
        return [item for item in state["requests"]
                if item["kind"] == kind and (task is None or item.get("task") == task)
                and (embedding_input is None or item.get("body", {}).get("input") == embedding_input)]

    def scale(self, node, role, replicas):
        self.kube("scale", f"deployment/tx-{node}-{role}", f"--replicas={replicas}")
        if replicas:
            self.rollout(f"tx-{node}-{role}")
            if role == "server":
                self.ready(node)
        else:
            self.until(lambda: not self.pods(f"app=tx-{node},role={role}"), f"{node}/{role} stopped")

    def peers_ready(self):
        # A host port-forward becoming ready does not establish Pod-to-Service
        # routing after a rollout. Exercise the path used by authority RPCs.
        for local, remote in ((0, 1), (1, 0)):
            self.until(lambda local=local, remote=remote: self.kube("exec", f"deployment/tx-{local}-server", "--", "curl", "--fail", "--silent", "--max-time", "3", "--output", "/dev/null", f"http://tx-{remote}:8080/.well-known/aidash", check=False).returncode == 0, "peer Service routing after restart")

    def kill(self, node, role):
        pods = self.pods(f"app=tx-{node},role={role}")
        assert len(pods) == 1, (node, role, len(pods))
        pod = pods[0]
        # Force deletion acknowledges the API removal before the old container
        # exits. Kill its actual runtime PID inside this disposable cluster node
        # and observe process death before releasing a held provider response.
        runtime_node = pod["spec"]["nodeName"]
        container = pod["status"]["containerStatuses"][0]["containerID"].removeprefix("containerd://")
        inspected = json.loads(subprocess.check_output(["docker", "exec", runtime_node, "crictl", "inspect", container], text=True))
        pid = int(inspected["info"]["pid"])
        assert pid > 1, inspected["status"]
        subprocess.run(["docker", "exec", runtime_node, "kill", "-STOP", str(pid)], check=True, capture_output=True, text=True)
        self.kube("scale", f"deployment/tx-{node}-{role}", "--replicas=0")
        subprocess.run(["docker", "exec", runtime_node, "kill", "-KILL", str(pid)], check=True, capture_output=True, text=True)
        self.until(lambda: subprocess.run(["docker", "exec", runtime_node, "test", "!", "-e", f"/proc/{pid}"], capture_output=True).returncode == 0, "runtime PID exited after SIGKILL")
        self.kube("delete", "pod", pod["metadata"]["name"], "--grace-period=0", "--force", "--wait=true", "--ignore-not-found")
        self.until(lambda: not self.pods(f"app=tx-{node},role={role}"), "killed Pod disappeared")
        return {"node": node, "role": role, "pod_uid": pod["metadata"]["uid"], "container": container,
                "runtime_node": runtime_node, "runtime_pid": pid, "signal": "SIGKILL", "process_exit_observed": True, "at": now()}

    def providers(self):
        self.apply({"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": "provider"},
                    "data": {"fixture.py": (ROOT / "scripts/remote_memory_fixture.py").read_text()}})
        self.apply({"apiVersion": "apps/v1", "kind": "Deployment", "metadata": {"name": "provider"}, "spec": {
            "replicas": 1, "selector": {"matchLabels": {"app": "provider"}}, "template": {
                "metadata": {"labels": {"app": "provider"}}, "spec": {"containers": [{"name": "provider", "image": "python:3.12-alpine",
                    "command": ["python3", "/fixture/fixture.py"], "volumeMounts": [{"name": "fixture", "mountPath": "/fixture"}]}],
                    "volumes": [{"name": "fixture", "configMap": {"name": "provider"}}]}}}})
        self.service("provider", 8080, {"app": "provider"})
        self.rollout("provider")
        for node in range(2):
            for role in ("server", "worker"):
                if node == 0:
                    self.kube("set", "env", f"deployment/tx-{node}-{role}", "AIDASH_SECRET_TEST_REMOTE_EMBEDDING=fixture-embedding-initial")
            self.rollout(f"tx-{node}-server")
            self.ready(node)
        self.peers_ready()
        for node in range(2):
            self.until(lambda node=node: self.kube("exec", f"deployment/tx-{node}-server", "--", "aidash", "activation-provision", check=False).returncode == 0, "activation broker provision")

    def register(self, node, kind, name, config, capabilities=None):
        entry = {"id": name, "version": "1.0.0", "kind": kind, "name": {"en": name}, "description": {"en": "Synthetic cluster fixture"},
                 "capabilities": capabilities or [], "languages": ["en"], "schema": {"type": "object"}, "config": config}
        self.ok(self.api(node, "/api/registry", entry))
        self.ok(self.api(node, "/api/authorization/acme/catalog", {"entry": ref(entry), "expected_revision": 0, "enabled": True}))
        # Hash the canonical stored definition, including server-default fields.
        stored = self.ok(self.api(node, f"/api/registry/{name}/1.0.0"))
        self.entries[node][name] = stored
        return stored

    def configure(self):
        for node in range(2):
            subjects = {"alice": {"kind": "user"}, **{f"aidash://tx-{n:02}/agents/research@1.0.0": {"kind": "agent"} for n in range(2)}}
            bundle = {"tenant": "acme", "subjects": subjects, "policies": [{"id": "approved-work", "effect": "allow", "subjects": {"any": True}, "actions": ["*"], "resources": {"kinds": ["*"]}}]}
            self.ok(self.api(node, "/api/authorization/acme", {"expected_revision": 0, "bundle": bundle}))
            credential = self.ok(self.api(node, "/api/authorization/acme/credentials", {"subject": "alice"}))
            self.tokens.append(credential["token"])
            self.ok(self.api(node, "/api/authorization/acme/peer-mappings", {"source_node": f"aidash://tx-{1-node:02}", "source_tenant": "acme", "source_subject": "alice", "credential_id": credential["credential"]["id"], "enabled": True, "expected_revision": 0}))
            self.register(node, "model", "model", {"provider": "openrouter", "model_id": "fixture", "endpoint": "http://provider:8080/v1", "context_window": 128000, "max_output_tokens": 4096, "modalities": ["text"], "cost": {}})
            self.register(node, "agent", "research", {"model": {"id": "model", "version": "1.0.0"}, "instructions": "Use approved Home memory", "tools": [], "skills": []})
        self.embedding = {"provider": "openai", "endpoint": "http://provider:8080/v1", "credential_env": "AIDASH_SECRET_TEST_REMOTE_EMBEDDING", "model": "home-vector", "model_version": "1", "dimensions": 3}
        self.register(0, "embedding", "home-embedding", self.embedding)

    def workspace(self):
        workspace = self.ok(self.user(0, "/api/workspaces", {"title": "Remote memory acceptance", "goal": "Search the approved archive"}))["id"]
        self.ok(self.api(0, f"/api/workspaces/{workspace}/semantic/index", {"expected_revision": 0, "spec": {
            "embedding": self.embedding, "vector": {"provider": "postgres", "endpoint": "local", "credential_env": None},
            "enabled": True, "auto_context": True, "max_sources": 100, "max_results": 10, "max_result_tokens": 4096, "max_input_bytes": 32768}}))
        entries = []
        for key, owner, text in [("shared", None, "Iridium archive marker: ochre falcon."),
                                 ("exact", "aidash://tx-01/agents/research@1.0.0", "Exact executor marker: silver fern."),
                                 ("other-node", "aidash://tx-00/agents/research@1.0.0", "DO NOT DISCLOSE: Home Agent scope."),
                                 ("other-version", "aidash://tx-01/agents/research@1.0.1", "DO NOT DISCLOSE: other definition version.")]:
            entries.append(self.ok(self.user(0, f"/api/workspaces/{workspace}/semantic/entries", {"key": key, "expected_revision": 0, "source": {"kind": "memory", "text": text}, "agent": owner, "metadata": {}})))
        self.until(lambda: all(e["state"] == "READY" for e in self.ok(self.user(0, f"/api/workspaces/{workspace}/semantic/entries"))), "Home index ready")
        return workspace, entries[0]

    def activate(self, task, agent):
        grant = str(uuid.uuid4())
        payload = {"id": grant, "node_id": "aidash://tx-01", "agent": agent, "ttl_seconds": 1800,
                   "semantic": {"mode": "required_home", "embedding": {"id": "home-embedding", "version": "1.0.0"}}}
        self.ok(self.user(0, f"/api/tasks/{task}/remote-grants", payload))
        activated = self.ok(self.user(0, f"/api/tasks/{task}/remote-grants/{grant}/activate", {}))
        replay = self.ok(self.user(0, f"/api/tasks/{task}/remote-grants/{grant}/activate", {}))
        assert replay["admission_id"] == activated["admission_id"]
        return grant, activated["admission_id"]

    def finish(self, task, run):
        def complete():
            rows = self.db(1, "runs", task, run)
            assert len(rows) == 1 and rows[0]["id"] == run, rows
            assert rows[0]["control"] != "PAUSED" and rows[0]["phase"] != "FAILED", rows
            return rows[0]["phase"] == "COMPLETED"
        self.until(complete, "same remote Run completed", timeout=180)
        self.scale(1, "worker", 0)

    def snapshot(self, task, run):
        return {str(node): {key: self.db(node, key, task, run) for key in ("runs", "requests", "origins", "operations", "attempts", "receipts", "usage", "dispatches", "budgets")} for node in range(2)}

    def ordinary(self):
        record = {"name": "ordinary-worker-sigkill", "started_at": now(), "result": "failed"}
        self.evidence["cases"].append(record)
        workspace, _ = self.workspace()
        task = self.ok(self.user(0, f"/api/workspaces/{workspace}/tasks", {"title": "Remote research", "description": "Find the relevant non-keyword archive marker"}))["id"]
        grant, run = self.activate(task, {"id": "research", "version": "1.0.0"})
        record.update(task=task, grant=grant, run=run)
        embeddings = len(self.captured("embedding"))
        self.provider({"hold_inference": True})
        self.scale(1, "worker", 1)
        self.until(lambda: len(self.captured("inference", task)) == 1, "ordinary inference dispatched")
        receipt = self.db(1, "receipts", task, run)
        assert len(receipt) == 1 and len(receipt[0]["receipt"]["sources"]) == 2, receipt
        record["cut"] = self.snapshot(task, run)
        record["killed"] = self.kill(1, "worker")
        self.provider({"hold_inference": False})
        self.scale(1, "worker", 1)
        self.finish(task, run)
        assert len(self.captured("embedding")) == embeddings + 1, "durable receipt replay must avoid another embedding"
        assert len(self.captured("inference", task)) == 2, self.captured("inference", task)
        assert self.db(1, "receipts", task, run) == receipt
        record["recovered"] = self.snapshot(task, run)
        record["result"] = "passed"

    def descriptor(self, node, name):
        entry = self.entries[node][name]
        return {"node_id": f"aidash://tx-{node:02}", "entry": ref(entry), "digest": digest(entry), "configuration_digest": digest(entry["config"])}

    def credential_rotation(self):
        record = {"name": "embedding-credential-revocation-missing-and-rotation", "started_at": now(), "result": "failed"}
        self.evidence["cases"].append(record)
        workspace, _ = self.workspace()
        task = self.ok(self.user(0, f"/api/workspaces/{workspace}/tasks", {"title": "Credential recovery", "description": "Find the relevant non-keyword archive marker"}))["id"]
        grant, run = self.activate(task, {"id": "research", "version": "1.0.0"})
        record.update(task=task, grant=grant, run=run)
        descriptor = self.descriptor(0, "home-embedding")
        baseline = len(self.captured("embedding"))
        rejected = len(self.captured("embedding_rejected"))
        self.provider({"embedding_key": "fixture-embedding-rotated"})

        def paused():
            row = self.db(1, "runs", task, run)[0]
            assert row["phase"] != "FAILED", row
            if row["control"] != "PAUSED":
                return False
            assert row["pending"]["recovery"]["semantic_reason"] == "configuration", row
            return True

        self.scale(1, "worker", 1)
        self.until(paused, "revoked embedding credential pauses the same Run")
        assert not self.captured("inference", task)
        assert len(self.captured("embedding_rejected")) == rejected + 1
        self.scale(1, "worker", 0)
        record["revoked"] = self.snapshot(task, run)
        # A completed rolling update can still leave a terminating Home Pod
        # eligible for kubectl's Service port-forward. Stop it fully before
        # changing credentials so resume cannot reach its old configuration.
        self.scale(0, "server", 0)
        self.kube("set", "env", "deployment/tx-0-server", "AIDASH_SECRET_TEST_REMOTE_EMBEDDING-")
        self.scale(0, "server", 1)
        self.peers_ready()
        route = f"/api/tasks/{task}/remote-grants/{grant}/control"
        self.ok(self.user(0, route, {"action": "resume"}))
        self.scale(1, "worker", 1)
        self.until(paused, "missing embedding credential pauses without HTTP")
        assert not self.captured("inference", task)
        assert len(self.captured("embedding")) == baseline
        assert len(self.captured("embedding_rejected")) == rejected + 1
        self.scale(1, "worker", 0)
        record["missing"] = self.snapshot(task, run)
        self.scale(0, "server", 0)
        for role in ("server", "worker"):
            self.kube("set", "env", f"deployment/tx-0-{role}", "AIDASH_SECRET_TEST_REMOTE_EMBEDDING=fixture-embedding-rotated")
        self.scale(0, "server", 1)
        self.peers_ready()
        self.ok(self.user(0, route, {"action": "resume"}))
        self.scale(1, "worker", 1)
        self.finish(task, run)
        stored = self.ok(self.api(0, "/api/registry/home-embedding/1.0.0"))
        assert digest(stored) == descriptor["digest"] and digest(stored["config"]) == descriptor["configuration_digest"]
        assert len(self.captured("embedding")) == baseline + 1
        assert len(self.captured("inference", task)) == 1
        record["recovered"] = self.snapshot(task, run)
        record["result"] = "passed"

    def generated_task(self, workspace, policy_suffix=""):
        parent_policy = "remote-parent" + policy_suffix
        child_policy = "remote-child" + policy_suffix
        capability = "unique-parent-specialist" + policy_suffix
        common = {"enabled": True, "permissions": {"roles": [], "groups": [], "attributes": {}}, "approval_required": False,
                  "limits": {"max_agents": 4, "max_concurrent": 4, "max_depth": 4, "token_budget": 4000000, "tokens_per_agent": 800000, "lifetime_seconds": 3600}}
        parent = copy.deepcopy(common)
        parent["template"] = copy.deepcopy(self.entries[0]["research"])
        parent["template"]["capabilities"] = [capability]
        parent["embedding"] = {"provider": {"id": "home-embedding", "version": "1.0.0"}, "calls_per_agent": 10, "call_budget": 40}
        parent["remote"] = {"inference": [self.descriptor(1, "model")]}
        self.ok(self.api(0, f"/api/generation/acme/policies/{parent_policy}", {"expected_revision": 0, "spec": parent}))
        parent_task = self.ok(self.user(0, f"/api/workspaces/{workspace}/tasks", {"title": "Generated origin", "description": "Delegate scoped memory research", "requirements": {"capability": capability}}))["id"]
        assigned = self.ok(self.user(0, f"/api/generation/acme/tasks/{parent_task}/assign", {"policy_id": parent_policy, "reason": "Generated ancestor acceptance"}))
        assert assigned["kind"] == "generated", assigned
        self.scale(0, "worker", 1)
        def waiting():
            runs = self.db(0, "runs", parent_task)
            return bool(runs and runs[0]["phase"] == "WAITING")
        self.until(waiting, "actual generated task_create and human_request")
        self.scale(0, "worker", 0)
        tasks = self.ok(self.user(0, f"/api/workspaces/{workspace}"))["tasks"]
        children = [t for t in tasks if t["title"] == "Generated remote research"]
        assert len(children) == 1, tasks
        child = children[0]["id"]
        origins = self.db(0, "origins", child)
        assert len(origins) == 1 and len(origins[0]["subject_chain"]) == 2, origins
        child_spec = copy.deepcopy(common)
        child_spec.update(template=self.entries[1]["research"], approval_required=True,
            remote={"embedding": {"provider": self.descriptor(0, "home-embedding"), "calls_per_agent": 10, "call_budget": 40}})
        self.ok(self.api(1, f"/api/generation/acme/policies/{child_policy}", {"expected_revision": 0, "spec": child_spec}))
        intent = {"id": str(uuid.uuid4()), "node_id": "aidash://tx-01", "policy_id": child_policy, "policy_revision": 1, "ttl_seconds": 1800, "reason": "Foreign generated child acceptance"}
        route = f"/api/tasks/{child}/remote-generation"
        pending = self.ok(self.user(0, route, intent))
        assert pending["status"] == "PENDING_APPROVAL" and not pending["prepared"], pending
        self.kill(1, "server")
        self.scale(1, "server", 1)
        self.peers_ready()
        assert self.ok(self.user(0, route, intent)) == pending
        assert not self.db(1, "runs", child), "preparation cannot start an unbound Run"
        self.ok(self.api(1, f"/api/generation/acme/requests/{pending['request_id']}/control", {"action": "approve", "reason": "Approve the exact cluster intent"}))
        prepared = self.ok(self.user(0, route, intent))
        assert prepared["prepared"] and prepared["agent"] == pending["agent"]
        assert len(self.db(1, "requests", child)) == 1
        return children[0], prepared["agent"], parent_task

    def generated(self):
        record = {"name": "generated-home-sigkill-link-outage-and-restart", "started_at": now(), "result": "failed"}
        self.evidence["cases"].append(record)
        workspace, source = self.workspace()
        child, agent, parent = self.generated_task(workspace)
        task = child["id"]
        # PostgreSQL restarts can legitimately trigger background source reindexing.
        # Count this task's query input so those calls cannot imitate a resend.
        embedding_input = f"{child['title']}\n{child['description']}"
        grant, run = self.activate(task, agent)
        record.update(task=task, grant=grant, run=run, parent=parent, agent=agent)
        baseline = len(self.captured("embedding", embedding_input=embedding_input))
        self.provider({"hold_embedding": True, "hold_inference": True})
        self.scale(1, "worker", 1)
        self.until(lambda: len(self.captured("embedding", embedding_input=embedding_input)) == baseline + 1, "generated embedding dispatched")
        cut = self.snapshot(task, run)
        for node in ("0", "1"):
            usage = cut[node]["usage"]
            assert len(usage) == 1 and usage[0]["state"] == "RESERVED" and usage[0]["purpose"] == "embedding", usage
        old = cut["0"]["usage"][0]
        assert cut["0"]["dispatches"][0]["state"] == "DISPATCHED"
        record["cut"] = cut
        record["killed_home"] = self.kill(0, "server")
        self.provider({"hold_embedding": False})
        def retried():
            retry = self.db(1, "runs", task, run)[0]["pending"]["recovery"]["retry"]
            return retry is not None and retry["count"] > 0

        self.until(retried, "durable outage retry", timeout=40)
        during = self.db(1, "runs", task, run)[0]
        assert during["phase"] != "FAILED" and not self.captured("inference", task), during
        record["during_outage"] = during
        record["killed_worker"] = self.kill(1, "worker")
        record["killed_receiver"] = self.kill(1, "server")
        self.kube("delete", "pod", "postgres-0", "--grace-period=0", "--force", "--wait=true")
        self.rollout("postgres", "statefulset")
        self.scale(0, "server", 1)
        self.scale(1, "server", 1)
        self.peers_ready()
        self.scale(1, "worker", 1)
        self.until(lambda: len(self.captured("inference", task)) == 1, "fresh generated inference after recovery")
        recovered = self.snapshot(task, run)
        for node in ("0", "1"):
            usage = recovered[node]["usage"]
            original = next(u for u in usage if u["attempt_id"] == old["attempt_id"])
            assert original == cut[node]["usage"][0], (node, original, cut[node]["usage"])
            assert any(u["purpose"] == "inference" and u["state"] == "RESERVED" for u in usage), usage
            assert any(u["purpose"] == "embedding" and u["state"] == "SETTLED" and u["reported_tokens"] == 1 for u in usage), usage
        attempts = recovered["0"]["attempts"]
        assert len(attempts) == 2 and len({a["id"] for a in attempts}) == 2, attempts
        assert any(a["state"] == "UNCERTAIN" for a in attempts), attempts
        assert len(self.captured("embedding", embedding_input=embedding_input)) == baseline + 2, "no same-attempt embedding resend"
        record["recovered_before_inference_response"] = recovered
        self.provider({"hold_inference": False})
        self.finish(task, run)
        final = self.snapshot(task, run)
        for node in ("0", "1"):
            usage = final[node]["usage"]
            assert next(u for u in usage if u["attempt_id"] == old["attempt_id"])["state"] == "RESERVED"
            assert len(usage) == 3 and sum(u["state"] == "SETTLED" for u in usage) == 2, usage
            budget = next(b for b in final[node]["budgets"] if b["request_id"] == usage[0]["request_id"])
            assert budget["used_tokens"] >= old["reserved_tokens"] + 3 and budget["embedding_calls"] >= 2, budget
        record["completed"] = final
        self.ok(self.user(0, f"/api/workspaces/{workspace}/semantic/entries/{source['id']}", {"expected_revision": source["revision"]}, "DELETE"))
        assert self.user(0, f"/api/tasks/{task}/remote-grants/{grant}/semantic")[0] == 403
        assert self.user(1, f"/api/runs/{run}")[0] == 403
        control = self.ok(self.user(1, f"/api/runs/{run}/management"))
        assert not {"context", "pending", "error"}.intersection(control), control
        assert control["id"] == run and control["phase"] == "COMPLETED", control
        assert self.user(1, f"/api/runs/{run}/management", {"action": "cancel"})[0] == 409
        record["content_free_control"] = control
        record["result"] = "passed"

    def save(self):
        self.evidence["finished_at"] = now()
        (self.directory / "remote-memory.json").write_text(json.dumps(self.evidence, indent=2) + "\n")
        try:
            (self.directory / "provider-requests.json").write_text(json.dumps(self.provider(), indent=2) + "\n")
            images = [(p["metadata"]["name"], [c.get("imageID") for c in p.get("status", {}).get("containerStatuses", [])]) for p in self.pods("")]
            (self.directory / "images-final.json").write_text(json.dumps(images, indent=2) + "\n")
        except Exception as error:
            (self.directory / "capture-error.txt").write_text(type(error).__name__)
        for node in range(2):
            for role in ("server", "worker"):
                if not self.pods(f"app=tx-{node},role={role}"):
                    (self.directory / f"node-{node}-{role}.log").write_text("Deployment scaled to zero; no live pod log.\n")
                    continue
                log = self.kube("logs", f"deployment/tx-{node}-{role}", "--tail=300", check=False)
                (self.directory / f"node-{node}-{role}.log").write_text(log.stdout + log.stderr)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kubeconfig", required=True)
    parser.add_argument("--distribution", choices=["kubernetes", "k3s"], required=True)
    parser.add_argument("--image", required=True)
    parser.add_argument("--postgres-image", required=True)
    parser.add_argument("--queries", required=True)
    parser.add_argument("--keep", action="store_true")
    args = parser.parse_args()
    args.nodes, args.repetitions, args.phase, args.lifecycle = [2], 1, None, ["remote-semantic-memory"]
    cluster = RemoteMemory(args)
    print(f"Evidence: {cluster.directory}", flush=True)
    try:
        cluster.provision(2)
        cluster.providers()
        cluster.configure()
        cluster.ordinary()
        print("Ordinary worker recovery passed", flush=True)
        cluster.credential_rotation()
        print("Provider credential rejection, absence and same-binding rotation passed", flush=True)
        cluster.generated()
        print("Generated lineage, Home outage and restart recovery passed", flush=True)
    except Exception as error:
        cluster.evidence["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        cluster.save()
        cluster.close()


if __name__ == "__main__":
    main()
