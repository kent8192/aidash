"""Regression tests for the cluster driver's externally observed visibility oracle."""
import unittest
from unittest.mock import Mock

from transaction_cluster_acceptance import Cluster


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


if __name__ == "__main__":
    unittest.main()
