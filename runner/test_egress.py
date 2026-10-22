"""Runner admission and NetworkPolicy exercise the real manifest constructors."""
import json
import unittest
import uuid
from unittest.mock import Mock

from control import Rejected, Runner


class EgressIsolationTest(unittest.TestCase):
    def runner(self):
        runner = object.__new__(Runner)
        runner.config = {
            'namespace': 'test-sandbox', 'working_bytes': 1 << 20,
            'maximum_seconds': 600,
            'egress': {'endpoint': 'http://10.96.0.90:8950', 'namespace': 'test-app',
                       'pod_selector': {'app.kubernetes.io/component': 'server'}},
        }
        runner.kube = Mock()
        return runner

    def test_only_gateway_pods_and_its_declared_tcp_port_are_reachable(self):
        runner = self.runner()
        runner.ensure_network_policy()
        policy = json.loads(runner.kube.call_args.args[1])['spec']
        self.assertEqual(policy['ingress'], [])
        self.assertEqual(policy['policyTypes'], ['Ingress', 'Egress'])
        self.assertEqual(policy['egress'], [{
            'to': [{'namespaceSelector': {'matchLabels': {'kubernetes.io/metadata.name': 'test-app'}},
                    'podSelector': {'matchLabels': {'app.kubernetes.io/component': 'server'}}}],
            'ports': [{'protocol': 'TCP', 'port': 8950}],
        }])
        runner.config.pop('egress')
        runner.ensure_network_policy()
        self.assertEqual(json.loads(runner.kube.call_args.args[1])['spec']['egress'], [])

    def test_a_caller_cannot_retarget_the_gateway_or_reuse_another_operation_token(self):
        runner = self.runner()
        operation = str(uuid.uuid4())
        request = {'operation_id': operation, 'area_id': str(uuid.uuid4()), 'epoch': 1,
                   'digest': 'fixture', 'kind': 'shell', 'code': 'true', 'seconds': 30, 'files': []}
        for lease in [
            {'url': 'http://another-service:8950', 'token': operation + '.' + 'a' * 43},
            {'url': runner.config['egress']['endpoint'], 'token': str(uuid.uuid4()) + '.' + 'a' * 43},
            {'url': runner.config['egress']['endpoint'], 'token': operation + '.' + 'a' * 43, 'secret': 'forbidden'},
        ]:
            with self.subTest(lease=lease), self.assertRaises(Rejected) as rejected:
                runner.accept(dict(request, egress=lease))
            self.assertEqual(rejected.exception.code, 400)
        runner.kube.assert_not_called()
