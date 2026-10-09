"""Chart contract tests for scheduling, privileges and retained single writers."""
import importlib.util
import json
from pathlib import Path
import subprocess
import unittest

import yaml

ROOT = Path(__file__).resolve().parents[1]


def render(chart, values):
    output = subprocess.check_output(['helm', 'template', 'fixture', str(ROOT / chart),
                                     '--namespace', 'environment', '-f', '-'], input=json.dumps(values).encode())
    return [value for value in yaml.safe_load_all(output) if value]


def select(objects, kind, suffix):
    return next(value for value in objects if value['kind'] == kind and value['metadata']['name'].endswith(suffix))


class ChartsTest(unittest.TestCase):
    aidash = 'deploy/helm/aidash'
    environment = 'infra/gcp/helm/environment'
    base = {'node': {'id': 'aidash://fixture'}, 'existingSecret': 'fixture'}

    def test_execution_is_opt_in(self):
        objects = render(self.aidash, self.base)
        self.assertFalse(any(value['kind'] in ('DaemonSet', 'RuntimeClass', 'Namespace', 'PersistentVolumeClaim') for value in objects))

    def test_shared_rwo_ledger_co_locates_roles_and_avoids_update_surge(self):
        objects = render(self.aidash, dict(self.base, memoryRecovery={'existingClaim': 'ledger'},
                         affinity={'nodeAffinity': {'preferredDuringSchedulingIgnoredDuringExecution': []}}))
        for role in ('server', 'worker'):
            deployment = select(objects, 'Deployment', '-aidash-' + role)
            self.assertEqual(deployment['spec']['strategy'], {'type': 'Recreate'})
            pod = deployment['spec']['template']
            term = pod['spec']['affinity']['podAffinity']['requiredDuringSchedulingIgnoredDuringExecution'][0]
            self.assertEqual(term['topologyKey'], 'kubernetes.io/hostname')
            for key, value in term['labelSelector']['matchLabels'].items():
                self.assertEqual(pod['metadata']['labels'][key], value)
            self.assertIn('nodeAffinity', pod['spec']['affinity'])
            claim = pod['spec']['volumes'][0]['persistentVolumeClaim']['claimName']
            self.assertEqual(claim, 'ledger')

    def test_server_worker_google_identities_are_separate(self):
        objects = render(self.aidash, dict(self.base,
            server={'serviceAccount': {'annotations': {'iam.gke.io/gcp-service-account': 'server@example'}}},
            worker={'serviceAccount': {'annotations': {'iam.gke.io/gcp-service-account': 'worker@example'}}}))
        for role in ('server', 'worker'):
            account = select(objects, 'ServiceAccount', '-aidash-' + role)
            self.assertEqual(account['metadata']['annotations']['iam.gke.io/gcp-service-account'], role + '@example')
            pod = select(objects, 'Deployment', '-aidash-' + role)['spec']['template']['spec']
            self.assertEqual(pod['serviceAccountName'], account['metadata']['name'])

    def test_execution_privileges_scheduling_and_independent_trusted_images(self):
        execution = {'createNamespaces': True, 'sandboxImage': 'sandbox@sha256:' + 'a' * 64,
                     'runtimeClass': {'create': True},
                     'runner': {'enabled': True, 'existingSecret': 'runner', 'image': 'trusted-runner'},
                     'guard': {'enabled': True, 'image': 'trusted-guard'},
                     'installer': {'enabled': True, 'image': 'trusted-installer'}}
        objects = render(self.aidash, dict(self.base, execution=execution))
        runtime = select(objects, 'RuntimeClass', '-runsc')
        self.assertEqual(runtime['handler'], 'runsc')
        self.assertEqual(runtime['scheduling']['nodeSelector'], {'aidash.run/gvisor': '20260921.0'})
        self.assertEqual(select(objects, 'Namespace', '-sandbox')['metadata']['labels']['pod-security.kubernetes.io/enforce'], 'restricted')
        rule = select(objects, 'ClusterRole', '-installer')['rules']
        self.assertEqual(rule, [{'apiGroups': [''], 'resources': ['nodes'], 'verbs': ['get', 'patch']}])
        guard_role = select(objects, 'Role', '-guard')['rules']
        self.assertEqual(guard_role[1], {'apiGroups': [''], 'resources': ['pods/exec'], 'verbs': ['create']})
        for role in ('installer', 'guard'):
            pod = select(objects, 'DaemonSet', '-' + role)['spec']['template']['spec']
            self.assertTrue(pod['hostPID'])
            self.assertFalse(pod['hostNetwork'])
            self.assertEqual(pod['containers'][0]['image'], 'trusted-' + role)
        guard = select(objects, 'DaemonSet', '-guard')['spec']['template']['spec']
        mounts = {mount['name']: mount for mount in guard['containers'][0]['volumeMounts']}
        self.assertEqual(mounts['kubelet-pods']['mountPropagation'], 'HostToContainer')
        for name in ('runsc', 'gvisor-bin', 'ctr'):
            volume = next(value for value in guard['volumes'] if value['name'] == name)
            self.assertEqual(mounts[name]['mountPath'], volume['hostPath']['path'])
        runner = select(objects, 'Deployment', '-execution-runner')
        self.assertEqual(runner['spec']['replicas'], 1)
        self.assertEqual(runner['spec']['strategy']['type'], 'Recreate')
        self.assertEqual(runner['spec']['template']['spec']['containers'][0]['image'], 'trusted-runner')
        profile = json.loads(select(objects, 'ConfigMap', '-profile')['data']['profile.json'])
        self.assertEqual(profile['processes'], 128)
        self.assertEqual(profile['host_tasks'], 512)
        self.assertNotIn('kubeconfig', profile['runner'])

    def test_gcp_persistence_local_lb_private_admission_and_activity(self):
        objects = render(self.environment, {'postgres': {'existingSecret': 'db'}, 'edge': {'hostname': 'fixture.example'},
            'activity': {'existingSecret': 'observer'}, 'storage': {'createClass': True},
            'previewTls': {'createVolume': True, 'volumeHandle': 'projects/fixture/zones/us-central1-a/disks/preview'}})
        self.assertEqual(select(objects, 'StorageClass', 'retain')['reclaimPolicy'], 'Retain')
        for role in ('postgres', 'nats'):
            state = select(objects, 'StatefulSet', '-' + role)['spec']
            self.assertEqual(state['replicas'], 1)
            self.assertEqual(state['persistentVolumeClaimRetentionPolicy'], {'whenDeleted': 'Retain', 'whenScaled': 'Retain'})
            self.assertEqual(state['volumeClaimTemplates'][0]['spec']['accessModes'], ['ReadWriteOnce'])
        pv = select(objects, 'PersistentVolume', 'preview-tls')
        self.assertEqual(pv['spec']['persistentVolumeReclaimPolicy'], 'Retain')
        service = select(objects, 'Service', '-edge')
        self.assertEqual(service['spec']['externalTrafficPolicy'], 'Local')
        self.assertEqual([port['port'] for port in service['spec']['ports']], [80, 443])
        job = select(objects, 'CronJob', '-activity')['spec']
        self.assertEqual(job['schedule'], '* * * * *')
        self.assertEqual(job['concurrencyPolicy'], 'Forbid')
        for value in objects:
            if value['kind'] in ('Deployment', 'StatefulSet'):
                pod = value['spec']['template']['spec']
                self.assertFalse(pod['hostNetwork'])
                self.assertTrue(all(term.get('tolerationSeconds', 30) < 300 for term in pod['tolerations']))


class ActivityTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        spec = importlib.util.spec_from_file_location('activity', ROOT / 'infra/gcp/helm/environment/files/activity.py')
        cls.activity = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.activity)

    def combine(self, previous, counts=None, transfers=None, completion=0):
        return self.activity.combine(
            {'protocol': 'aidash-infra-activity/1', 'counts': counts or {}, 'completed_transfers': transfers or [],
             'last_work_completed': completion}, {'inflight': 0, 'last_active': 0, 'closed': False},
            {'protocol': 'aidash-runner-activity/1', 'counts': {'runner': 0}}, previous, 10000)

    def test_observation_gap_blocks_stop_and_renews_full_idle_interval(self):
        for previous in ({}, {'observed_at': 9700, 'last_active': 5000, 'busy': False}):
            result = self.combine(previous)
            self.assertTrue(result['busy'])
            self.assertTrue(result['observation_gap'])
            self.assertEqual(result['last_active'], 10000)

    def test_completion_transfers_and_busy_history_keep_idle_fencing(self):
        previous = {'observed_at': 9950, 'last_active': 5000, 'busy': False}
        self.assertEqual(self.combine(previous, completion=9990)['last_active'], 9990)
        self.assertEqual(self.combine(previous, transfers=['new'])['last_active'], 10000)
        self.assertFalse(self.combine(previous, counts={'database_work': 1})['busy'])
        self.assertTrue(self.combine(previous, counts={'leases': 1})['busy'])
        self.assertEqual(self.combine(dict(previous, busy=True))['last_active'], 10000)


if __name__ == '__main__':
    unittest.main()
