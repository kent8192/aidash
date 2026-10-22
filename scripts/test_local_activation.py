"""Fresh local stacks provision activation before starting their writers."""

import json
from pathlib import Path
import subprocess
import tempfile
import tomllib
import unittest

import yaml

ROOT = Path(__file__).resolve().parents[1]


def compose_config(directory):
    return json.loads(subprocess.check_output(
        ['docker', 'compose', '--profile', 'dev', 'config', '--format', 'json'], cwd=directory,
    ))


class LocalActivationTest(unittest.TestCase):
    def test_compose_waits_for_provisioning_before_backend_start(self):
        config = compose_config(ROOT)
        services = config['services']
        provision = services['activation-provision']
        self.assertEqual(provision['command'], ['activation-provision'])
        self.assertEqual(provision['environment']['NATS_URL'], services['backend']['environment']['NATS_URL'])
        self.assertEqual(provision['environment']['AIDASH_NODE_ID'], services['backend']['environment']['AIDASH_NODE_ID'])
        self.assertEqual(provision['depends_on']['nats']['condition'], 'service_healthy')
        self.assertEqual(services['backend']['depends_on']['activation-provision']['condition'], 'service_completed_successfully')
        self.assertIn('healthz?js-enabled-only=true', ' '.join(services['nats']['healthcheck']['test']))

    def test_compose_provisioner_and_backend_share_activation_overrides(self):
        settings = {
            'AIDASH_ACTIVATION_NAMESPACE': 'local-activation-fixture',
            'AIDASH_ACTIVATION_MAX_AGE_SECONDS': '3600',
            'AIDASH_ACTIVATION_MAX_BYTES': '10485760',
            'AIDASH_ACTIVATION_REPLICAS': '3',
            'AIDASH_ACTIVATION_NATS_URL': 'nats://activation-fixture:4222',
        }
        with tempfile.TemporaryDirectory() as directory:
            fixture = Path(directory)
            (fixture / 'compose.yaml').write_bytes((ROOT / 'compose.yaml').read_bytes())
            (fixture / '.env').write_text(''.join(f'{key}={value}\n' for key, value in settings.items()))
            services = compose_config(fixture)['services']
        for key, value in settings.items():
            with self.subTest(setting=key):
                self.assertEqual(services['backend']['environment'][key], value)
                self.assertEqual(services['activation-provision']['environment'][key], value)

    def test_kind_provisions_after_nats_and_before_application_start(self):
        tasks = tomllib.loads((ROOT / 'Makefile.toml').read_text())['tasks']
        order = tasks['k8s-up']['dependencies']
        self.assertLess(order.index('k8s-wait-nats'), order.index('k8s-activation-provision'))
        self.assertLess(order.index('k8s-activation-provision'), order.index('k8s-helm'))
        script = tasks['k8s-activation-provision']['script']
        self.assertIn('job/aidash-activation-provision', script)
        job = yaml.safe_load((ROOT / 'deploy/local-k8s/activation-provision.yaml').read_text())
        self.assertEqual(job['kind'], 'Job')
        pod = job['spec']['template']['spec']
        self.assertFalse(pod['automountServiceAccountToken'])
        container = pod['containers'][0]
        self.assertEqual(container['command'], ['aidash', 'activation-provision'])
        self.assertEqual(container['envFrom'], [{'secretRef': {'name': 'aidash-local-app'}}])
        self.assertIn({'name': 'AIDASH_NODE_ID', 'value': 'aidash://node-a'}, container['env'])


if __name__ == '__main__':
    unittest.main()
