"""Regression tests for the cluster driver's transport and recovery oracles."""
import os
import pathlib
import socket
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest.mock import Mock, patch
from urllib.parse import urlsplit

from remote_memory_cluster_acceptance import RemoteMemory
from transaction_cluster_acceptance import Cluster


class PortForwardTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.cluster = object.__new__(Cluster)
        self.cluster.directory = pathlib.Path(self.directory.name)
        self.cluster.namespace = "test"
        self.cluster.env = os.environ.copy()
        self.cluster.token = "test-token"
        self.cluster.forwards = {}
        self.cluster.forward_lock = threading.RLock()
        self.children = []
        self.logs = []

    def tearDown(self):
        for process in self.children:
            if process.poll() is None:
                process.terminate()
            process.wait(timeout=10)
        for log in self.logs:
            log.close()
        self.directory.cleanup()

    def child(self, script):
        # Exercise real subprocess startup and TCP readiness without kubectl.
        popen = subprocess.Popen

        def start(command, **kwargs):
            process = popen([sys.executable, "-c", script, command[-1].split(":")[0]], **kwargs)
            self.children.append(process)
            self.logs.append(kwargs["stdout"])
            return process

        return patch("transaction_cluster_acceptance.subprocess.Popen", side_effect=start)

    @staticmethod
    def listener(delay):
        return f"""
import socket, sys, time
time.sleep({delay})
with socket.socket() as listener:
    listener.bind(("127.0.0.1", int(sys.argv[1])))
    listener.listen()
    while True:
        connection, _ = listener.accept()
        connection.close()
"""

    def test_waits_for_listener_instead_of_fixed_startup_sleep(self):
        with self.child(self.listener(0.6)):
            endpoint = urlsplit(self.cluster.forward(0))
            try:
                with socket.create_connection((endpoint.hostname, endpoint.port), timeout=1):
                    pass
            except OSError as error:
                self.fail(f"port-forward returned before its listener was ready: {error}")
            self.assertEqual(self.cluster.forward(0), endpoint.geturl())
            self.assertEqual(len(self.children), 1)

    def test_exited_startup_is_not_cached_and_closes_its_log(self):
        with self.child("import sys; sys.exit(7)"):
            with self.assertRaisesRegex(OSError, "port-forward"):
                self.cluster.forward(0)
        self.assertNotIn(0, self.cluster.forwards)
        self.assertTrue(self.logs[0].closed)

    def test_startup_timeout_cleans_up_process_and_cache(self):
        with self.child("import time; time.sleep(60)"), patch(
            "transaction_cluster_acceptance.time.monotonic", side_effect=[0, 0, 11]
        ):
            with self.assertRaisesRegex(TimeoutError, "port-forward"):
                self.cluster.forward(0)
        self.assertNotIn(0, self.cluster.forwards)
        self.assertIsNotNone(self.children[0].poll())
        self.assertTrue(self.logs[0].closed)

    def test_replacing_dead_cached_forward_closes_old_log(self):
        process = Mock()
        process.poll.return_value = 1
        log = (self.cluster.directory / "old.log").open("a")
        self.logs.append(log)
        self.cluster.forwards[0] = (process, "http://127.0.0.1:1", log)
        with self.child(self.listener(0)):
            endpoint = self.cluster.forward(0)
        self.assertNotEqual(endpoint, "http://127.0.0.1:1")
        self.assertTrue(log.closed)

    def test_transport_failure_does_not_replay_mutation(self):
        self.cluster.forward = Mock(return_value="http://127.0.0.1:1")
        with patch("transaction_cluster_acceptance.urllib.request.urlopen", side_effect=ConnectionResetError) as request:
            with self.assertRaises(ConnectionResetError):
                self.cluster.api(0, "/api/workspaces", {"title": "create once"})
        self.assertEqual(request.call_count, 1)
        self.assertEqual(request.call_args.args[0].get_method(), "POST")


class VisibilityOracleTests(unittest.TestCase):
    def cluster(self, responses):
        cluster = object.__new__(Cluster)
        cluster.api = Mock(side_effect=responses)
        cluster.inspect = Mock(return_value=[{"revision": 1}])
        return cluster

    @staticmethod
    def response(revision):
        return 200, {"workspace": {"revision": revision}}

    def test_rejects_old_response_after_new_on_another_node(self):
        cluster = self.cluster([self.response(1), self.response(0)])
        trace = []
        with self.assertRaisesRegex(AssertionError, "old state after publication"):
            cluster.observe(2, "transaction", ["a", "b"], trace)
        self.assertEqual(trace[-1]["revision"], 0)

    def test_remembers_publication_across_observation_rounds(self):
        cluster = self.cluster([self.response(0), self.response(1), self.response(0), self.response(1)])
        trace = []
        cluster.observe(2, "transaction", ["a", "b"], trace)
        with self.assertRaisesRegex(AssertionError, "old state after publication"):
            cluster.observe(2, "transaction", ["a", "b"], trace)

    def test_allows_unavailable_and_new_responses_after_publication(self):
        cluster = self.cluster([self.response(0), self.response(1), (503, {}), self.response(1)])
        trace = []
        cluster.observe(2, "transaction", ["a", "b"], trace)
        cluster.observe(2, "transaction", ["a", "b"], trace)
        self.assertEqual([entry["status"] for entry in trace], [200, 200, 503, 200])

    def test_checks_durable_state_before_accepting_publication(self):
        cluster = self.cluster([self.response(1)])
        cluster.inspect.return_value = [{"revision": 0}]
        with self.assertRaisesRegex(AssertionError, "partial visibility"):
            cluster.observe(2, "transaction", ["a", "b"], [])


class EmbeddingOracleTests(unittest.TestCase):
    query = "Generated remote research\nFind the relevant non-keyword archive marker"

    def cluster(self, inputs):
        cluster = object.__new__(RemoteMemory)
        cluster.provider = Mock(return_value={"errors": [], "requests": [
            {"kind": "embedding", "body": {"input": value, "model": "home-vector"}}
            for value in inputs
        ]})
        return cluster

    def test_recovery_query_count_ignores_background_reindexing(self):
        cluster = self.cluster([self.query, "Iridium archive marker: ochre falcon.",
                                "Exact executor marker: silver fern.",
                                "DO NOT DISCLOSE: Home Agent scope.", self.query])
        self.assertEqual(len(cluster.captured("embedding")), 5)
        self.assertEqual(len(cluster.captured("embedding", embedding_input=self.query)), 2)

    def test_same_query_resend_remains_visible(self):
        cluster = self.cluster([self.query, "Iridium archive marker: ochre falcon.",
                                self.query, self.query])
        self.assertEqual(len(cluster.captured("embedding", embedding_input=self.query)), 3)


if __name__ == "__main__":
    unittest.main()
