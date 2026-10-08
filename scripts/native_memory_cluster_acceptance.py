#!/usr/bin/env python3
"""Native Rust memory on two real HTTP Nodes in an explicit disposable cluster.

Record ordinary/generated Home identity, exact Registry roles, en/ja content,
worker/server/PostgreSQL restart persistence, origin-owned usage and withdrawal.
Never change the default Kubernetes context or rewrite production lease fences.
"""
import argparse
import copy
import json
import importlib.util
import hashlib
import os
import socket
import subprocess
import tempfile
import shutil
from pathlib import Path
import uuid

from remote_memory_cluster_acceptance import RemoteMemory, ref
from transaction_cluster_acceptance import ROOT, now


class NativeMemory(RemoteMemory):
    def __init__(self, args):
        super().__init__(args)
        self.claims = set()
        self.selection = None
        self.language = "en-US"
        self.evidence["driver_file_hashes"] = {name: hashlib.sha256((ROOT / "scripts" / name).read_bytes()).hexdigest()
            for name in ("native_memory_cluster_acceptance.py", "native_memory_fixture.py", "evaluate-native-memory.py",
                         "remote_memory_cluster_acceptance.py", "transaction_cluster_acceptance.py")}

    def apply(self, value):
        value = copy.deepcopy(value)
        if value.get("kind") == "ConfigMap" and value["metadata"]["name"] == "provider":
            value["data"]["native_memory_fixture.py"] = (ROOT / "scripts/native_memory_fixture.py").read_text()
            value["data"]["remote_memory_fixture.py"] = (ROOT / "scripts/remote_memory_fixture.py").read_text()
        if value.get("kind") == "Deployment":
            name = value["metadata"]["name"]
            spec = value["spec"]["template"]["spec"]
            if name == "provider":
                spec["containers"][0]["command"] = ["python3", "/fixture/native_memory_fixture.py"]
            if name.startswith("tx-"):
                node = int(name.split("-")[1])
                claim = f"memory-home-{node}"
                if claim not in self.claims:
                    super().apply({"apiVersion": "v1", "kind": "PersistentVolumeClaim", "metadata": {"name": claim},
                        "spec": {"accessModes": ["ReadWriteOnce"], "resources": {"requests": {"storage": "1Gi"}}}})
                    self.claims.add(claim)
                spec["securityContext"] = {"runAsUser": 10001, "runAsGroup": 10001, "fsGroup": 10001}
                spec["volumes"] = [{"name": "memory", "persistentVolumeClaim": {"claimName": claim}}]
                container = spec["containers"][0]
                # The fsGroup grants creation under the volume root. The child
                # directory is created and secured by the application UID.
                container["volumeMounts"] = [{"name": "memory", "mountPath": "/var/lib/aidash"}]
                container["env"].append({"name": "AIDASH_MEMORY_RECOVERY_DIR", "value": "/var/lib/aidash/memory-recovery"})
        super().apply(value)

    def provision(self, count):
        super().provision(count)
        for node in range(count):
            self.kube("exec", f"deployment/tx-{node}-server", "--", "aidash", "memory-recovery", "init", "--directory", "/var/lib/aidash/memory-recovery")

    def providers(self):
        super().providers()
        # Both Nodes validate the six role definitions at registration. The
        # Receiver has the same synthetic credential, though required-Home
        # retrieval sends actual embedding requests only from the Home.
        for role in ("server", "worker"):
            self.kube("set", "env", f"deployment/tx-1-{role}", "AIDASH_SECRET_TEST_REMOTE_EMBEDDING=fixture-embedding-initial")
        self.rollout("tx-1-server")
        self.ready(1)
        self.peers_ready()

    def save(self):
        # Capture content-free ledger diagnostics before the disposable cluster
        # disappears. Never include bodies, credentials or archive payloads.
        for node in range(2):
            try:
                status = self.kube("exec", f"deployment/tx-{node}-server", "--", "aidash", "memory-recovery", "status",
                    "--directory", "/var/lib/aidash/memory-recovery", check=False)
                (self.directory / f"node-{node}-memory-recovery.log").write_text(status.stdout + status.stderr)
            except Exception as error:
                (self.directory / f"node-{node}-memory-recovery.log").write_text(type(error).__name__)
        super().save()

    def api(self, node, path, body=None):
        if body is not None and "/generation/" in path and "/policies/" in path:
            body = copy.deepcopy(body)
            body["spec"].setdefault("remote", {})["memory"] = [self.descriptor(0, "home-embedding")]
        return super().api(node, path, body)

    def generated_task(self, workspace):
        return super().generated_task(workspace, policy_suffix="-" + self.language.lower())

    def configure(self):
        zero = {"input_per_million": 0, "output_per_million": 0}
        def reference(name):
            return {"id": name, "version": "1.0.0"}
        self.embedding = {"provider": "openai", "endpoint": "http://provider:8080/v1", "credential_env": "AIDASH_SECRET_TEST_REMOTE_EMBEDDING", "model": "home-vector", "model_version": "1", "dimensions": 3}
        self.memory = reference("native-memory")
        self.learning_memory = reference("native-learning-memory")
        policy = {"engine": "hindsight_rust", "policy": {
            "extraction": reference("model"), "derivation": reference("model"), "reflection": reference("model"),
            "embedding": reference("home-embedding"), "reranker": reference("native-reranker"), "tokenizer": reference("native-tokenizer"),
            "prices": {role: zero for role in ("extraction", "derivation", "reflection", "embedding", "reranker")},
            "retention": {"unit_max_age_days": None, "candidate_days": 7, "history_days": 30, "history_versions": 16,
                "model_result_days": 7, "backup_days": 7, "purge_after_seconds": 60, "purge_batch": 32, "max_unit_records": 128, "max_model_operations": 1024},
            "bounds": {"max_unit_bytes": 8192, "max_input_bytes": 32768, "max_units": 16, "max_candidates": 8, "max_entities": 8,
                "max_evidence": 8, "max_links": 8, "max_graph_hops": 3, "max_graph_visits": 32, "max_results": 4, "max_context_tokens": 16384,
                "max_model_calls": 4, "max_model_tokens": 32768, "max_cost_micros": 10000, "max_retries": 2, "max_call_seconds": 30},
            "semantic_link_min_similarity_millionths": 700000, "learn_from_runs": False, "maintain_observations": False, "refresh_mental_models": False}}
        for node in range(2):
            def binding(kind, target, origin=node):
                return {"kind": kind, "target": {"registry_node": f"aidash://tx-{origin:02}", **target}, "narrow": {}}
            subjects = {"alice": {"kind": "user"}, **{f"aidash://tx-{n:02}/agents/research@1.0.0": {"kind": "agent"} for n in range(2)}}
            subjects[f"aidash://tx-{node:02}/agents/learning@1.0.0"] = {"kind": "agent"}
            bundle = {"tenant": "acme", "subjects": subjects, "policies": [{"id": "approved-work", "effect": "allow", "subjects": {"any": True}, "actions": ["*"], "resources": {"kinds": ["*"]}}]}
            self.ok(self.api(node, "/api/authorization/acme", {"expected_revision": 0, "bundle": bundle}))
            credential = self.ok(self.api(node, "/api/authorization/acme/credentials", {"subject": "alice"}))
            self.tokens.append(credential["token"])
            self.ok(self.api(node, "/api/authorization/acme/peer-mappings", {"source_node": f"aidash://tx-{1-node:02}", "source_tenant": "acme", "source_subject": "alice", "credential_id": credential["credential"]["id"], "enabled": True, "expected_revision": 0}))
            for kind, name, config in [
                ("model", "model", {"provider": "openrouter", "model_id": "fixture", "endpoint": "http://provider:8080/v1", "context_window": 128000, "max_output_tokens": 4096, "modalities": ["text"], "cost": {}}),
                ("embedding", "home-embedding", self.embedding), ("reranker", "native-reranker", {"provider": "rrf"}),
                ("tokenizer", "native-tokenizer", {"provider": "utf8_upper_bound"}), ("memory", "native-memory", policy),
                ("source", "native-shared", {"memory": self.memory, "scope": "workspace", "max_tokens": 16384}),
                ("agent", "research", {"model": reference("model"), "instructions": "Use current Home memory / 現在のHome記憶を参照", "schema_version": 1, "bindings": [binding("memory", self.memory), binding("source", reference("native-shared"))], "remove_default": ["file_search", "file_read"]}),
            ]:
                self.register(node, kind, name, config)
            learning = copy.deepcopy(policy)
            learning["policy"]["learn_from_runs"] = True
            self.register(node, "memory", "native-learning-memory", learning)
            self.register(node, "agent", "learning", {"model": reference("model"), "instructions": "Report fixture failures with their actual verification status / 検証状態を保持する",
                "schema_version": 1, "bindings": [binding("memory", self.learning_memory)], "remove_default": ["file_search", "file_read", "memory_mutate"]}, ["native-learning-ordinary"])

    def mutate(self, workspace, bank, changes):
        return self.ok(self.user(0, f"/api/workspaces/{workspace}/memory/units/mutate", {
            "operation_id": str(uuid.uuid4()), "provider": self.memory, "bank": bank, "changes": changes}))

    def workspace(self):
        workspace = self.ok(self.user(0, "/api/workspaces", {"title": f"Native memory {self.language}", "goal": "Read attributed Home memory / 出典付き記憶を参照"}))["id"]
        self.ok(self.api(0, f"/api/workspaces/{workspace}/semantic/index", {"expected_revision": 0, "spec": {
            "embedding": self.embedding, "vector": {"provider": "postgres", "endpoint": "local", "credential_env": None}, "enabled": True,
            "auto_context": False, "max_sources": 100, "max_results": 4, "max_result_tokens": 32768, "max_input_bytes": 32768}}))
        participant = self.ok(self.user(0, f"/api/workspaces/{workspace}/memory/participants", {"agent": ref(self.entries[0]["research"])}))
        other = self.ok(self.user(0, f"/api/workspaces/{workspace}/memory/participants", {"agent": ref(self.entries[0]["research"])}))
        shared = {**participant["bank"], "participant": None}
        self.ok(self.user(0, f"/api/workspaces/{workspace}/memory/operate", {"operation_id": str(uuid.uuid4()), "provider": self.memory, "bank": shared, "action": {"action": "configure_bank", "expected_revision": 0}}))
        self.selection = {"participant": participant["id"], "expected_revision": participant["revision"], "provider": self.memory}
        self.unit_ids = {}
        source = None
        for key, bank, text in [("private", participant["bank"], "Private marker: silver fern / 銀のシダ"),
                              ("shared", shared, "Iridium archive marker: ochre falcon / 黄土色のハヤブサ"),
                              ("other", other["bank"], "DO NOT DISCLOSE: separate logical Agent")]:
            unit_id = str(uuid.uuid4())
            self.unit_ids[key] = unit_id
            value = self.mutate(workspace, bank, [{"operation": "add", "id": unit_id, "content": {
                "text": text, "kind": "world", "learning": "fact", "verification": "unverified", "occurred": None, "entities": [], "evidence": [], "links": []}}])
            if key == "shared":
                source = value[0]
        # Start Home maintenance only to finish these operator-owned projections.
        self.scale(0, "worker", 1)
        self.until(lambda: len([entry for entry in self.ok(self.user(0, f"/api/workspaces/{workspace}/semantic/entries")) if entry["state"] == "READY"]) == 3, "native index ready")
        self.scale(0, "worker", 0)
        return workspace, source

    def activate(self, task, agent):
        grant = str(uuid.uuid4())
        self.ok(self.user(0, f"/api/tasks/{task}/remote-grants", {"id": grant, "node_id": "aidash://tx-01", "agent": agent, "ttl_seconds": 1800,
            "semantic": {"mode": "required_home", "embedding": ref(self.entries[0]["home-embedding"]), "native": self.selection}}))
        activated = self.ok(self.user(0, f"/api/tasks/{task}/remote-grants/{grant}/activate", {}))
        assert self.ok(self.user(0, f"/api/tasks/{task}/remote-grants/{grant}/activate", {}))["admission_id"] == activated["admission_id"]
        return grant, activated["admission_id"]

    def case(self, generated, language):
        self.language = language
        record = {"name": "generated" if generated else "ordinary", "language": language, "started_at": now(), "result": "failed"}
        self.evidence["cases"].append(record)
        workspace, source = self.workspace()
        if generated:
            child, agent, parent = self.generated_task(workspace)
            task = child["id"]
            record["parent"] = parent
        else:
            task = self.ok(self.user(0, f"/api/workspaces/{workspace}/tasks", {"title": "Remote research", "description": "Find attributed knowledge / 出典付き知識を探す"}))["id"]
            agent = ref(self.entries[1]["research"])
        grant, run = self.activate(task, agent)
        record.update(task=task, run=run, grant=grant)
        self.provider({"hold_inference": True})
        self.scale(1, "worker", 1)
        self.until(lambda: len(self.captured("inference", task)) == 1, "native context reached real inference")
        cut = self.snapshot(task, run)
        receipts = cut["1"]["receipts"]
        assert len(receipts) == 1, receipts
        banks = receipts[0]["receipt"]["memory"]["banks"]
        assert banks[0]["recall"]["status"] == ("empty" if generated else "ready"), banks
        assert banks[1]["recall"]["units"][0]["id"] == source["id"], banks
        if generated:
            assert banks[0]["bank"]["participant"] != self.selection["participant"], banks
            for node in ("0", "1"):
                usage = [item for item in cut[node]["usage"] if item["purpose"] == "memory"]
                assert usage and all(item["state"] == "SETTLED" and item["reported_tokens"] == 1 for item in usage), usage
        record["cut"] = cut
        record["killed_worker"] = self.kill(1, "worker")
        if generated:
            record["killed_home"] = self.kill(0, "server")
            record["killed_receiver"] = self.kill(1, "server")
            self.kube("delete", "pod", "postgres-0", "--wait=true")
            self.rollout("postgres", "statefulset")
            self.scale(0, "server", 1)
            self.scale(1, "server", 1)
            self.peers_ready()
        self.provider({"hold_inference": False})
        self.scale(1, "worker", 1)
        self.finish(task, run)
        assert len(self.captured("inference", task)) == 2, self.captured("inference", task)
        record["recovered"] = self.snapshot(task, run)
        self.mutate(workspace, source["bank"], [{"operation": "delete", "id": source["id"], "expected_revision": source["revision"]}])
        assert self.user(0, f"/api/tasks/{task}/remote-grants/{grant}/semantic")[0] == 403
        assert self.user(1, f"/api/runs/{run}/semantic")[0] == 403
        control = self.ok(self.user(1, f"/api/runs/{run}/management"))
        assert not {"context", "pending", "error"}.intersection(control), control
        assert control["memory_cleanup"]["state"] == "purged" and control["memory_cleanup"]["invalidated"], control
        record["content_free_control"] = control
        receiver = self.snapshot(task, run)["1"]
        assert not receiver["receipts"], "terminal receiver quotation bodies survived Home withdrawal"
        # Diagnostic row serialization omits SQL NULL fields. Missing receipt
        # and explicit null both represent an erased quotation in this projection.
        assert receiver["operations"] and all(item.get("receipt") is None and item["state"] == "INVALIDATED" for item in receiver["operations"]), receiver
        record["receiver_cleanup"] = receiver
        record["result"] = "passed"

    def evaluation(self):
        workspace, _ = self.workspace()
        self.scale(0, "worker", 1)
        specification = importlib.util.spec_from_file_location("native_evaluation", ROOT / "scripts/evaluate-native-memory.py")
        module = importlib.util.module_from_spec(specification)
        specification.loader.exec_module(module)
        self.evidence["fixed_label_evaluation"] = {}
        module.evaluate(
            lambda path, body: self.user(0, path, body), workspace, ref(self.entries[0]["research"]), self.memory,
            {"kind": "synthetic-contract-provider", "server_image": self.args.image, "provider": self.entries[0]["native-memory"],
             "agent": self.entries[0]["research"], "declared_prices": "zero-price synthetic model", "model_acceptance": "unapproved"},
            report=self.evidence["fixed_label_evaluation"])
        for case in self.evidence["fixed_label_evaluation"]["cases"]:
            metrics = case["metrics"]
            assert metrics["useful_recall"] >= 0.75, case
            assert metrics["extraction_label_recall"] == 1 and metrics["missing_support"] == 0 and metrics["duplicate_labels"] == 0, case
            assert metrics["stale_units_after_correction"] == 0 and metrics["charged_calls"] > 0 and metrics["charged_tokens"] > 0, case
        self.scale(0, "worker", 0)

    def learning_case(self, generated, language):
        """Registry, real inference/journal, opted-in extraction and human CAS review."""
        record = {"language": language, "generated": generated, "home": "aidash://tx-00", "started_at": now()}
        self.evidence.setdefault("run_learning", []).append(record)
        workspace = self.ok(self.user(0, "/api/workspaces", {"title": f"Learning {language}", "goal": "Review attributed failure observations"}))["id"]
        self.ok(self.api(0, f"/api/workspaces/{workspace}/semantic/index", {"expected_revision": 0, "spec": {
            "embedding": self.embedding, "vector": {"provider": "postgres", "endpoint": "local", "credential_env": None}, "enabled": True,
            "auto_context": False, "max_sources": 100, "max_results": 4, "max_result_tokens": 32768, "max_input_bytes": 32768}}))
        capability = "native-learning-generated-" + language.lower() if generated else "native-learning-ordinary"
        task = self.ok(self.user(0, f"/api/workspaces/{workspace}/tasks", {"title": f"Learning {'generated' if generated else 'ordinary'} {language}",
            "description": "Synthetic rollback check failed / 合成データのロールバック検証は失敗", "requirements": {"capability": capability}}))
        if generated:
            name = "native-learning-" + language.lower()
            template = copy.deepcopy(self.entries[0]["learning"])
            template["capabilities"] = [capability]
            spec = {"enabled": True, "template": template, "approval_required": False, "permissions": {"roles": [], "groups": [], "attributes": {}},
                "limits": {"max_agents": 4, "max_concurrent": 4, "max_depth": 4, "token_budget": 4000000, "tokens_per_agent": 800000, "lifetime_seconds": 3600},
                "embedding": {"provider": ref(self.entries[0]["home-embedding"]), "calls_per_agent": 10, "call_budget": 40}}
            self.ok(super().api(0, f"/api/generation/acme/policies/{name}", {"expected_revision": 0, "spec": spec}))
            assigned = self.ok(self.user(0, f"/api/generation/acme/tasks/{task['id']}/assign", {"policy_id": name, "reason": "Bounded post-Run learning acceptance"}))
            assert assigned["kind"] == "generated", assigned
            record["generation"] = assigned["generation"]
        else:
            self.ok(self.user(0, f"/api/tasks/{task['id']}/claim", {"revision": task["revision"], "agent": ref(self.entries[0]["learning"])}))
        self.scale(0, "worker", 1)
        self.until(lambda: bool(self.db(0, "runs", task["id"]) and self.db(0, "runs", task["id"])[0]["phase"] == "COMPLETED"), "learning Run completed")
        run = self.db(0, "runs", task["id"])[0]
        assignment = self.ok(self.user(0, f"/api/workspaces/{workspace}/tasks/{task['id']}/memory-participant"))
        # This fresh task has no manual participant assignment. The real Run
        # admits its own logical identity; use that authorized host-bound bank.
        assert assignment["participant"] is None, assignment
        participants = self.ok(self.user(0, f"/api/workspaces/{workspace}/memory/participants"))
        matching = [p for p in participants["items"] if p["agent"] == {"id": run["agent_id"], "version": run["agent_version"]}]
        assert len(matching) == 1, participants
        bank = matching[0]["bank"]

        def operate(action, operation=None):
            return self.ok(self.user(0, f"/api/workspaces/{workspace}/memory/operate", {
                "operation_id": operation or str(uuid.uuid4()), "provider": self.learning_memory, "bank": bank, "action": action}))

        self.until(lambda: len(operate({"action": "candidates"})["value"]) == 2, "opted-in canonical Run candidates")
        pending = operate({"action": "candidates"})["value"]
        candidate = next(c for c in pending if c["content"]["learning"] == "failure")
        rejected = next(c for c in pending if c["content"]["learning"] == "procedure")
        assert candidate["state"] == "pending" and candidate["content"]["verification"] == "unverified" and candidate["content"]["learning"] == "failure", candidate
        assert candidate["run"]["id"] == run["id"] and candidate["content"]["evidence"], candidate
        def current_request():
            return next(item for item in self.ok(self.user(0, "/api/generation/acme/requests")) if item["id"] == record["generation"]["id"])

        if generated:
            current = current_request()
            assert current["status"] == "ACTIVE" and not current["quota_released"], current
            record["origin_while_awaiting_review"] = current
        operation = str(uuid.uuid4())
        action = {"action": "review", "id": candidate["id"], "expected_revision": candidate["revision"],
            "mutation": {"operation_id": operation, "provider": self.learning_memory, "bank": bank,
                         "changes": [{"operation": "add", "id": candidate["id"], "content": candidate["content"]}]}}
        if self.args.live_ui:
            browser = self.review_ui(workspace, bank, candidate, language, generated)
            action, operation = browser["request"]["action"], browser["request"]["operation_id"]
            reviewed = browser["result"]["value"]
            record["browser_review"] = browser
        else:
            reviewed = operate(action, operation)["value"]
        assert operate(action, operation)["value"] == reviewed, "human review replay duplicated admission"
        assert reviewed["content"]["verification"] == "unverified", reviewed
        self.until(lambda: any(row["id"] == reviewed["id"] and row["state"] == "READY" for row in self.ok(self.user(0, f"/api/workspaces/{workspace}/semantic/entries"))), "reviewed unit indexed under original allowance")
        # The second pending candidate keeps natural retirement from racing
        # these authorized reads. Human review remains explicit and bounded.
        history_action = {"action": "history", "id": reviewed["id"], "expected_revision": reviewed["revision"], "before": None}
        history = operate(history_action)
        assert history["value"], history
        charged_usage = operate({"action": "usage", "after": None})
        if generated:
            assert current_request()["status"] == "ACTIVE", current_request()
        rejection = {"action": "review", "id": rejected["id"], "expected_revision": rejected["revision"], "mutation": None}
        reject_key = str(uuid.uuid4())
        assert operate(rejection, reject_key)["value"] is None
        replay_status, replay = self.user(0, f"/api/workspaces/{workspace}/memory/operate", {
            "operation_id": reject_key, "provider": self.learning_memory, "bank": bank, "action": rejection})
        assert replay_status in (200, 403), (replay_status, replay)
        if replay_status == 200:
            assert replay["value"] is None
        record["rejection_replay_status"] = replay_status
        if generated:
            self.until(lambda: current_request()["status"] == "COMPLETED", "origin retired after reviewed indexing")
            current = current_request()
            assert current["quota_released"], current
            record["retired_origin"] = current
            record["origin_usage"] = self.ok(self.user(0, f"/api/generation/acme/requests/{record['generation']['id']}/usage"))
            assert self.user(0, f"/api/workspaces/{workspace}/memory/operate", {
                "operation_id": str(uuid.uuid4()), "provider": self.learning_memory, "bank": bank, "action": history_action})[0] == 403
            record["retired_catalogue_disclosure"] = "forbidden under current authority"
        record.update(task=task, run=run, bank=bank, candidate=candidate, reviewed=reviewed, history=history,
                      rejected_candidate=rejected, charged_usage=charged_usage, result="passed")
        self.scale(0, "worker", 0)

    def review_ui(self, workspace, bank, candidate, language, generated):
        """A real browser posts human review to the live subject-authorized API."""
        name = f"review-{language}-{'generated' if generated else 'ordinary'}"
        with tempfile.TemporaryDirectory(prefix="aidash-memory-browser-") as temporary:
            directory = Path(temporary)
            source, result = directory / "input.json", directory / "output.json"
            source.write_text(json.dumps({"workspace": workspace, "bank": bank, "candidate": candidate,
                "language": language, "token": self.tokens[0]}))
            source.chmod(0o600)
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", 0))
                port = listener.getsockname()[1]
            run = subprocess.run(["npx", "playwright", "test", "--config", "playwright.native-memory-live.config.ts"],
                cwd=ROOT / "web", env={**os.environ, "AIDASH_BACKEND": self.forward(0), "AIDASH_UI_PORT": str(port),
                    "AIDASH_MEMORY_LIVE_INPUT": str(source), "AIDASH_MEMORY_LIVE_OUTPUT": str(result)},
                capture_output=True, text=True, timeout=120)
            (self.directory / f"{name}.log").write_text(run.stdout + run.stderr)
            assert run.returncode == 0, f"Live browser review failed; inspect {name}.log"
            shutil.copyfile(str(result) + ".png", self.directory / f"{name}.png")
            reviewed = json.loads(result.read_text())
            files = [ROOT / "web" / "tests" / "native-memory-live.spec.ts", ROOT / "web" / "playwright.native-memory-live.config.ts"]
            files += [path for path in (ROOT / "web" / "dist").rglob("*") if path.is_file()]
            reviewed["browser_files"] = {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted(files)}
            return reviewed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("kubeconfig", "image", "postgres-image", "queries"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--distribution", choices=["kubernetes", "k3s"], required=True)
    parser.add_argument("--phase", dest="native_phase", choices=["all", "remote", "evaluation", "learning"], default="all")
    parser.add_argument("--live-ui", action="store_true", help="Use a local Playwright browser for human review; requires installed web dependencies and Chromium")
    parser.add_argument("--keep", action="store_true")
    args = parser.parse_args()
    args.nodes, args.repetitions, args.phase, args.lifecycle = [2], 1, None, ["native-hindsight-memory"]
    cluster = NativeMemory(args)
    cluster.evidence["selected_phase"] = args.native_phase
    print(f"Evidence: {cluster.directory}", flush=True)
    try:
        cluster.provision(2)
        cluster.providers()
        cluster.configure()
        if args.native_phase in ("all", "remote"):
            for language in ("en-US", "ja-JP"):
                for generated in (False, True):
                    cluster.case(generated, language)
                    print(f"Native {language} {'generated' if generated else 'ordinary'} passed", flush=True)
        if args.native_phase in ("all", "evaluation"):
            cluster.evaluation()
            print("Fixed English/Japanese labels and charged usage recorded", flush=True)
        if args.native_phase in ("all", "learning"):
            for language in ("en-US", "ja-JP"):
                for generated in (False, True):
                    cluster.learning_case(generated, language)
                    print(f"Run learning {language} {'generated' if generated else 'ordinary'} passed", flush=True)
    except Exception as error:
        cluster.evidence["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        cluster.save()
        cluster.close()


if __name__ == "__main__":
    main()
