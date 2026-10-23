"""Exercise the provisioned Node addresses through the acceptance providers."""
import http.client
import json
import threading
import unittest
from http.server import ThreadingHTTPServer
from unittest.mock import Mock

from native_memory_cluster_acceptance import NativeMemory
from native_memory_fixture import NativeHandler
from remote_memory_cluster_acceptance import RemoteMemory
from remote_memory_fixture import Handler, LOCK, STATE


HOME = "aidash://tx-00/brave-otter-k3m9q2xa"
EXECUTOR = "aidash://tx-01/calm-hawk-p7r4t6yu"


class MemoryProviderFixtureTests(unittest.TestCase):
    def test_provider_deployments_receive_the_provisioned_node_addresses(self):
        for driver in (RemoteMemory, NativeMemory):
            with self.subTest(driver=driver.__name__):
                cluster = object.__new__(driver)
                cluster.nodes = {0: {"address": HOME}, 1: {"address": EXECUTOR}}
                cluster.kube = Mock()
                for method in ("service", "rollout", "ready", "peers_ready", "until"):
                    setattr(cluster, method, Mock())
                cluster.providers()
                deployment = next(call.kwargs["value"] for call in cluster.kube.call_args_list
                                  if call.kwargs.get("value", {}).get("kind") == "Deployment")
                container = deployment["spec"]["template"]["spec"]["containers"][0]
                environment = {item["name"]: item["value"] for item in container.get("env", [])}
                self.assertEqual(environment.get("AIDASH_FIXTURE_HOME_NODE"), HOME)
                self.assertEqual(environment.get("AIDASH_FIXTURE_EXECUTOR_NODE"), EXECUTOR)

    def test_providers_accept_full_node_addresses_and_reject_another_node(self):
        for handler in (Handler, NativeHandler):
            with self.subTest(handler=handler.__name__):
                with LOCK:
                    STATE.update(hold_embedding=False, hold_inference=False, requests=[], errors=[])
                server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
                server.home_node, server.executor_node = HOME, EXECUTOR
                thread = threading.Thread(target=server.serve_forever, daemon=True)
                thread.start()
                try:
                    semantic = {
                        "home_node": HOME, "executor": EXECUTOR + "/agents/research@1.0.0",
                        "sources": [{"revision": 1}],
                        "result": {"matches": [] if handler is NativeHandler else [{"text": "ochre falcon"}]},
                        "memory": {"banks": [
                            {"bank": {"participant": "11111111-1111-4111-8111-111111111111"},
                             "recall": {"units": [{"revision": 1, "content": {"text": "ochre falcon", "verification": "unverified"}}]}},
                            {"bank": {"participant": None}, "recall": {"units": []}},
                        ]},
                    }
                    for home, executor, expected in (
                        (HOME, EXECUTOR, 200),
                        ("aidash://tx-00", EXECUTOR, 400),
                        ("aidash://tx-00/calm-hawk-p7r4t6yu", EXECUTOR, 400),
                        (HOME, "aidash://tx-01/brave-otter-k3m9q2xa", 400),
                    ):
                        context = {"current": {"task": {"id": "synthetic-task", "title": "Remote work"},
                                               "semantic_memory": {**semantic, "home_node": home,
                                                                   "executor": executor + "/agents/research@1.0.0"}}}
                        body = {"messages": [{"role": "system", "content": "Synthetic fixture"},
                                             {"role": "user", "content": json.dumps(context)}]}
                        client = http.client.HTTPConnection("127.0.0.1", server.server_port, timeout=5)
                        try:
                            client.request("POST", "/v1/chat/completions", json.dumps(body),
                                           {"Content-Type": "application/json"})
                            response = client.getresponse()
                            self.assertEqual(response.status, expected, (home, executor))
                            response.read()
                        finally:
                            client.close()
                finally:
                    server.shutdown()
                    server.server_close()
                    thread.join()


if __name__ == "__main__":
    unittest.main()
