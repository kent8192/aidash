"""Regression tests for the cluster driver's transport and recovery oracles."""
import contextlib
import io
import json
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
from transaction_cluster_acceptance import Cluster, main, transaction_cases


class TransactionPartitionTests(unittest.TestCase):
    def test_complete_inventory_retains_every_durable_cut_and_lifecycle(self):
        # Independent acceptance contract: both completion branches, both sides
        # of every cut, and three attempts at each original topology/action.
        cuts = [("coordinator.submit", False), ("coordinator.vote", False),
                ("coordinator.commit", False), ("coordinator.abort", True),
                ("coordinator.visible", False), ("coordinator.complete", False),
                ("coordinator.complete", True), ("participant.reserve", False),
                ("participant.prepare", False), ("participant.apply", False),
                ("participant.release", False), ("participant.abort", True)]
        expected = {(2, phase, edge, abort, repetition, None)
                    for phase, abort in cuts for edge in ("before", "after")
                    for repetition in (1, 2, 3)}
        expected.update((count, "coordinator.commit", "before", False, repetition, action)
                        for count in (2, 3, 16) for action in ("scale", "rolling")
                        for repetition in (1, 2, 3))
        expected.update((count, phase, "before", abort, repetition, "partition")
                        for count in (3, 16)
                        for phase, abort in (("coordinator.commit", False), ("coordinator.abort", True))
                        for repetition in (1, 2, 3))
        expected.update((2, "coordinator.commit", "before", False, repetition, "peer-recovery")
                        for repetition in (1, 2, 3))

        actual = transaction_cases([2, 3, 16])

        self.assertEqual(len(expected), 105)
        self.assertEqual(len(actual), 105)
        self.assertEqual(set(actual), expected)

    def test_ci_partitions_are_disjoint_complete_and_keep_case_order(self):
        complete = transaction_cases([2, 3, 16])
        selected = set()
        for partition, count in (("coordinator", 42), ("participant", 30), ("lifecycle", 33)):
            with self.subTest(partition=partition):
                cases = transaction_cases([2, 3, 16], partition=partition)
                self.assertEqual(len(cases), count)
                self.assertEqual(len(set(cases)), count)
                self.assertFalse(selected.intersection(cases))
                self.assertEqual(cases, [case for case in complete if case in set(cases)])
                selected.update(cases)
        self.assertEqual(selected, set(complete))

    def test_individual_selectors_preserve_both_completion_outcomes_and_peer_recovery(self):
        completion = transaction_cases([2, 3, 16], phase="coordinator.complete")
        self.assertEqual(len(completion), 36)
        self.assertEqual({case[0] for case in completion}, {2, 3, 16})
        self.assertEqual({case[2:4] for case in completion},
                         {("before", False), ("after", False), ("before", True), ("after", True)})
        self.assertTrue(all(case[5] is None for case in completion))
        self.assertEqual(transaction_cases([2, 3, 16], lifecycle=["peer-recovery"]),
                         [(2, "coordinator.commit", "before", False, repetition, "peer-recovery")
                          for repetition in (1, 2, 3)])

    def test_unknown_partitions_and_mixed_selectors_fail_closed(self):
        for arguments in ({"partition": "missing"},
                          {"partition": "coordinator", "phase": "coordinator.complete"},
                          {"partition": "lifecycle", "lifecycle": ["rolling"]}):
            with self.subTest(arguments=arguments), self.assertRaises(ValueError):
                transaction_cases([2, 3, 16], **arguments)

    def test_cli_rejects_empty_or_ambiguous_selection_before_creating_resources(self):
        required = ["driver", "--kubeconfig", "unused", "--distribution", "k3s", "--image", "unused", "--queries", "unused"]
        for arguments, message in ((["--nodes", "3", "--lifecycle", "peer-recovery"], "no transaction cases"),
                                   (["--partition", "lifecycle", "--phase", "coordinator.commit"], "select a partition"),
                                   (["--partition", "participant", "--lifecycle", "rolling"], "select a partition"),
                                   (["--phase", "coordinator.commit", "--lifecycle", "rolling"], "select either phase")):
            errors = io.StringIO()
            with self.subTest(arguments=arguments), patch("sys.argv", required + arguments), patch(
                "transaction_cluster_acceptance.Cluster"
            ) as cluster, contextlib.redirect_stderr(errors):
                with self.assertRaises(SystemExit) as error:
                    main()
                self.assertEqual(error.exception.code, 2)
                self.assertIn(message, errors.getvalue())
                cluster.assert_not_called()

    def test_partition_executes_and_records_all_expected_cases_with_full_topology(self):
        with tempfile.TemporaryDirectory() as directory:
            cluster = Mock(directory=pathlib.Path(directory))
            arguments = ["driver", "--kubeconfig", "unused", "--distribution", "k3s", "--image", "unused", "--queries", "unused",
                         "--partition", "participant"]
            with patch("sys.argv", arguments), patch(
                "transaction_cluster_acceptance.Cluster", return_value=cluster
            ), contextlib.redirect_stdout(io.StringIO()):
                main()
            evidence = json.loads((cluster.directory / "expected-cases.json").read_text())
            actual = [call.args for call in cluster.case.call_args_list]
            self.assertEqual(actual, transaction_cases([2, 3, 16], partition="participant"))
            self.assertEqual(evidence["partition"], "participant")
            self.assertEqual([tuple(case[key] for key in
                                    ("nodes", "phase", "edge", "abort", "repetition", "lifecycle"))
                              for case in evidence["cases"]], actual)
            cluster.provision.assert_called_once_with(16)
            cluster.close.assert_called_once_with()

    def test_evidence_write_failure_closes_resources_before_provisioning(self):
        with tempfile.TemporaryDirectory() as directory:
            cluster = Mock(directory=pathlib.Path(directory))
            arguments = ["driver", "--kubeconfig", "unused", "--distribution", "k3s", "--image", "unused", "--queries", "unused"]
            with patch("sys.argv", arguments), patch(
                "transaction_cluster_acceptance.Cluster", return_value=cluster
            ), patch.object(pathlib.Path, "write_text", side_effect=OSError("full")), contextlib.redirect_stdout(io.StringIO()):
                with self.assertRaisesRegex(OSError, "full"):
                    main()
            cluster.provision.assert_not_called()
            cluster.close.assert_called_once_with()


class ExtensionBootstrapTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.cluster = object.__new__(Cluster)
        self.cluster.directory = pathlib.Path(self.directory.name)
        self.cluster.queries = {"pgroonga_ready": "generated readiness query"}

    def test_preparing_is_retried_on_a_fresh_psql_backend(self):
        self.cluster.kube = Mock(side_effect=[
            subprocess.CompletedProcess([], 1, "", "ERROR: pgroonga_crash_safer is preparing"),
            subprocess.CompletedProcess([], 0, "CREATE EXTENSION", ""),
            subprocess.CompletedProcess([], 0, "ready", ""),
        ])
        with patch("transaction_cluster_acceptance.time.sleep") as sleep:
            self.cluster.install_extensions("tx_6")
        self.assertEqual(self.cluster.kube.call_count, 3)
        self.assertTrue(all(not call.kwargs["check"] for call in self.cluster.kube.call_args_list))
        sleep.assert_called_once_with(0.1)
        self.assertEqual(self.cluster.kube.call_args_list[-1].args[-1], "generated readiness query")
        self.assertIn("pgroonga_crash_safer is preparing",
                      (self.cluster.directory / "extension-bootstrap.log").read_text())

    def test_permanent_extension_errors_surface_with_diagnostics(self):
        self.cluster.kube = Mock(return_value=subprocess.CompletedProcess([], 1, "", "ERROR: extension is not available"))
        with patch("transaction_cluster_acceptance.time.sleep") as sleep:
            with self.assertRaisesRegex(RuntimeError, "extension is not available"):
                self.cluster.install_extensions("tx_6")
        sleep.assert_not_called()
        self.cluster.kube.assert_called_once()

    def test_preparing_retry_is_bounded(self):
        self.cluster.kube = Mock(return_value=subprocess.CompletedProcess([], 1, "", "ERROR: pgroonga_crash_safer is preparing"))
        with patch("transaction_cluster_acceptance.time.monotonic", side_effect=[0, 31]):
            with self.assertRaisesRegex(RuntimeError, "pgroonga_crash_safer is preparing"):
                self.cluster.install_extensions("tx_6")
        self.cluster.kube.assert_called_once()


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
        self.cluster.pods = Mock(return_value=[self.pod("current")])
        self.children = []
        self.logs = []
        self.commands = []

    @staticmethod
    def pod(name, *, terminating=False, ready=True):
        return {"metadata": {"name": name, **({"deletionTimestamp": "2026-10-05T00:00:00Z"} if terminating else {})},
                "status": {"phase": "Running", "conditions": [{"type": "Ready", "status": "True" if ready else "False"}]}}

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
            self.commands.append(command)
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

    def test_forward_targets_current_ready_pod_without_service_selection(self):
        self.cluster.pods.return_value = [self.pod("old", terminating=True),
                                          self.pod("starting", ready=False), self.pod("new")]
        with self.child(self.listener(0)):
            self.cluster.forward(0)
        self.assertIn("pod/new", self.commands[0])
        self.assertNotIn("service/tx-0", self.commands[0])

    def test_rollout_stops_old_recovery_before_replacing_the_forward(self):
        with self.child(self.listener(0)):
            self.cluster.forward(0)
            original = self.children[0]
            self.cluster.kube = Mock()
            self.cluster.pods.side_effect = [[self.pod("old", terminating=True), self.pod("new")],
                                             [self.pod("new")], [self.pod("new")]]
            self.cluster.rollout("tx-0-server")
            self.assertNotIn(0, self.cluster.forwards)
            self.assertIsNotNone(original.poll())
            self.assertTrue(self.logs[0].closed)
            self.cluster.forward(0)
        self.assertEqual(len(self.children), 2)
        self.assertIn("pod/new", self.commands[1])

    def test_ambiguous_ready_generations_do_not_start_a_forward(self):
        self.cluster.pods.return_value = [self.pod("old"), self.pod("new")]
        with self.child(self.listener(0)), self.assertRaisesRegex(ConnectionError, "one current ready"):
            self.cluster.forward(0)
        self.assertEqual(self.children, [])
        self.assertEqual(self.cluster.forwards, {})


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
