"""Chart contract tests for scheduling, privileges and retained single writers."""
import importlib.util
import json
from pathlib import Path
import subprocess
import unittest

import yaml

ROOT = Path(__file__).resolve().parents[1]
DIGEST = '@sha256:' + 'b' * 64


def render(chart, values, release='fixture', kube_version=None):
    command = ['helm', 'template', release, str(ROOT / chart), '--namespace', 'environment', '-f', '-']
    if kube_version:
        command += ['--kube-version', kube_version]
    output = subprocess.check_output(command, input=json.dumps(values).encode())
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
                     'runner': {'enabled': True, 'existingSecret': 'runner', 'image': 'trusted-runner' + DIGEST},
                     'guard': {'enabled': True, 'image': 'trusted-guard' + DIGEST},
                     'installer': {'enabled': True, 'image': 'trusted-installer' + DIGEST}}
        objects = render(self.aidash, dict(self.base, execution=execution,
                                           environment={'nodeSelector': {'pool': 'execution'}}))
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
            self.assertEqual(pod['containers'][0]['image'], 'trusted-' + role + DIGEST)
        guard = select(objects, 'DaemonSet', '-guard')['spec']['template']['spec']
        # The guard follows the installed label, which is never withdrawn during a
        # runtime replacement; only the RuntimeClass depends on the admission label.
        self.assertEqual(guard['nodeSelector'], {'pool': 'execution', 'aidash.run/gvisor-installed': 'true'})
        env = {value['name']: value.get('value') for value in select(objects, 'DaemonSet', '-installer')['spec']['template']['spec']['containers'][0]['env']}
        self.assertEqual((env['AIDASH_GVISOR_LABEL'], env['AIDASH_GVISOR_INSTALLED_LABEL']), ('aidash.run/gvisor', 'aidash.run/gvisor-installed'))
        mounts = {mount['name']: mount for mount in guard['containers'][0]['volumeMounts']}
        self.assertEqual(mounts['kubelet-pods']['mountPropagation'], 'HostToContainer')
        for name in ('runsc', 'gvisor-bin', 'ctr'):
            volume = next(value for value in guard['volumes'] if value['name'] == name)
            self.assertEqual(mounts[name]['mountPath'], volume['hostPath']['path'])
        runner = select(objects, 'Deployment', '-execution-runner')
        self.assertEqual(runner['spec']['replicas'], 1)
        self.assertEqual(runner['spec']['strategy']['type'], 'Recreate')
        self.assertEqual(runner['spec']['template']['spec']['containers'][0]['image'], 'trusted-runner' + DIGEST)
        profile = json.loads(select(objects, 'ConfigMap', '-profile')['data']['profile.json'])
        self.assertEqual(profile['processes'], 128)
        self.assertEqual(profile['host_tasks'], 512)
        self.assertNotIn('kubeconfig', profile['runner'])

    def test_installer_needs_a_selector_guard_state_is_per_release_and_process_floor(self):
        execution = {'createNamespaces': True, 'sandboxImage': 'sandbox@sha256:' + 'a' * 64,
                     'guard': {'enabled': True, 'image': 'g' + DIGEST}, 'installer': {'enabled': True, 'image': 'i' + DIGEST}}
        with self.assertRaises(subprocess.CalledProcessError):
            render(self.aidash, dict(self.base, execution=execution))
        values = dict(self.base, execution=execution, environment={'nodeSelector': {'pool': 'execution'}})
        installer = select(render(self.aidash, values), 'DaemonSet', '-installer')
        self.assertEqual(installer['spec']['template']['spec']['nodeSelector'], {'pool': 'execution'})
        states = []
        for release in ('alpha', 'beta'):
            guard = select(render(self.aidash, values, release), 'DaemonSet', '-guard')['spec']['template']['spec']
            container = guard['containers'][0]
            path = next(volume['hostPath']['path'] for volume in guard['volumes'] if volume['name'] == 'state')
            self.assertEqual(path, '/var/lib/aidash-node-guard/' + release)
            self.assertIn({'name': 'AIDASH_GUARD_STATE', 'value': path}, container['env'])
            self.assertIn({'name': 'state', 'mountPath': path}, container['volumeMounts'])
            self.assertIn(path + '/watch.lock', '\n'.join(container['readinessProbe']['exec']['command']))
            states.append(path)
        self.assertEqual(len(set(states)), 2)
        custom = dict(values, execution=dict(execution, paths={'guardState': '/srv/guard'}))
        guard = select(render(self.aidash, custom), 'DaemonSet', '-guard')['spec']['template']['spec']
        self.assertIn({'name': 'state', 'hostPath': {'path': '/srv/guard', 'type': 'DirectoryOrCreate'}}, guard['volumes'])
        # The admission probe lowers its own hard RLIMIT_NPROC to 8 and cannot raise it.
        for processes, accepted in ((7, False), (8, True)):
            limits = dict(self.base, execution={'limits': {'processes': processes}})
            if accepted:
                render(self.aidash, limits)
            else:
                with self.assertRaises(subprocess.CalledProcessError):
                    render(self.aidash, limits)

    def test_enabled_trusted_images_must_be_pinned_by_digest(self):
        for component in ('installer', 'guard', 'runner'):
            execution = {'createNamespaces': True, 'sandboxImage': 'sandbox' + DIGEST,
                         'runner': {'existingSecret': 'runner'}}
            execution.setdefault(component, {}).update(enabled=True, image='trusted:latest')
            values = dict(self.base, execution=execution, environment={'nodeSelector': {'pool': 'execution'}})
            with self.subTest(component=component):
                with self.assertRaises(subprocess.CalledProcessError):
                    render(self.aidash, values)
                execution[component]['image'] = 'trusted' + DIGEST
                render(self.aidash, values)
        # A disabled component is not rendered, so its default tag is not an error.
        render(self.aidash, self.base)

    def test_environment_trusted_images_must_be_pinned_by_digest(self):
        values = {'postgres': {'existingSecret': 'db'},
                  'edge': {'hostname': 'fixture.example', 'admissionImage': 'admission' + DIGEST},
                  'activity': {'existingSecret': 'observer', 'observerImage': 'observer' + DIGEST,
                               'collectorImage': 'collector' + DIGEST}}
        objects = render(self.environment, values)
        # The shipped Caddy default is digest pinned too: it receives all public traffic.
        caddy = select(objects, 'Deployment', '-edge')['spec']['template']['spec']['containers'][0]
        self.assertRegex(caddy['image'], r'@sha256:[0-9a-f]{64}$')
        for section, key in (('edge', 'admissionImage'), ('edge', 'caddyImage'),
                             ('activity', 'observerImage'), ('activity', 'collectorImage')):
            with self.subTest(image=key):
                mutable = dict(values, **{section: dict(values[section], **{key: 'image:latest'})})
                with self.assertRaises(subprocess.CalledProcessError):
                    render(self.environment, mutable)

    def test_execution_limits_names_paths_and_time_are_consistent_with_the_guard(self):
        runner = {'enabled': True, 'existingSecret': 'runner', 'image': 'runner' + DIGEST}
        base = {'sandboxImage': 'sandbox' + DIGEST, 'runner': runner}

        def accepted(release='fixture', **execution):
            render(self.aidash, dict(self.base, execution=dict(base, **execution)), release)

        accepted()
        # The Runner Service is `<release>-execution-runner`; a DNS label holds 63 characters.
        accepted(release='r' * 46)
        with self.assertRaises(subprocess.CalledProcessError):
            accepted(release='r' * 47)
        # Execution Pods set pod-level spec.resources, which needs Kubernetes 1.34.
        for version, ok in (('1.33.5', False), ('1.34.0', True), ('v1.34.1-gke.1000', True)):
            with self.subTest(kube_version=version):
                values = dict(self.base, execution=base)
                if ok:
                    render(self.aidash, values, kube_version=version)
                else:
                    with self.assertRaises(subprocess.CalledProcessError):
                        render(self.aidash, values, kube_version=version)
        # Execution stays opt-in, so the parent chart still renders on older clusters.
        render(self.aidash, self.base, kube_version='1.30.0')
        # Any RFC 1123 label is a valid sandbox namespace, including one character.
        accepted(sandboxNamespace='a')
        # The Sentry's own host threads share pids.max with the probe's guest children;
        # only the 128/512 headroom is verified, so neither ratio nor margin may shrink.
        for processes, host_tasks, ok in ((128, 129, False), (128, 511, False), (128, 512, True),
                                          (8, 391, False), (8, 392, True), (256, 1023, False), (256, 1024, True)):
            with self.subTest(processes=processes, host_tasks=host_tasks):
                limits = {'limits': {'processes': processes, 'host_tasks': host_tasks}}
                if ok:
                    accepted(**limits)
                else:
                    with self.assertRaises(subprocess.CalledProcessError):
                        accepted(**limits)
        # The admission probe requests 30 seconds; the guard rejects cells above 600
        # seconds, frozen sessions beyond 1800, and result files above 24 MiB.
        for limit, value, ok in (('maximum_seconds', 29, False), ('maximum_seconds', 30, True),
                                 ('maximum_seconds', 600, True), ('maximum_seconds', 601, False),
                                 ('idle_seconds', 1800, True), ('idle_seconds', 1801, False),
                                 ('output_bytes', 8 << 20, True), ('output_bytes', (8 << 20) + 1, False)):
            with self.subTest(limit=limit, value=value):
                if ok:
                    accepted(limits={limit: value})
                else:
                    with self.assertRaises(subprocess.CalledProcessError):
                        accepted(limits={limit: value})
        # The installer only writes the runtime under /usr/local/bin.
        installer = {'enabled': True, 'image': 'installer' + DIGEST}
        values = dict(self.base, environment={'nodeSelector': {'pool': 'execution'}})
        render(self.aidash, dict(values, execution=dict(base, installer=installer)))
        with self.assertRaises(subprocess.CalledProcessError):
            render(self.aidash, dict(values, execution=dict(base, installer=installer, paths={'runsc': '/opt/bin/runsc'})))

    def test_gcp_persistence_local_lb_private_admission_and_activity(self):
        objects = render(self.environment, {'postgres': {'existingSecret': 'db'},
            'edge': {'hostname': 'fixture.example', 'admissionImage': 'admission' + DIGEST},
            'activity': {'existingSecret': 'observer', 'observerImage': 'observer' + DIGEST,
                         'collectorImage': 'collector' + DIGEST}, 'storage': {'createClass': True},
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

    def test_environment_labels_stay_strings_and_edge_keeps_no_file_access_log(self):
        values = {'postgres': {'existingSecret': 'db'},
                  'edge': {'hostname': 'fixture.example', 'admissionImage': 'admission' + DIGEST},
                  'activity': {'existingSecret': 'observer', 'observerImage': 'observer' + DIGEST,
                               'collectorImage': 'collector' + DIGEST}}
        # Valid release names that YAML would otherwise read as booleans or null.
        for release in ('true', 'null'):
            with self.subTest(release=release):
                objects = render(self.environment, values, release)
                for value in objects:
                    for labels in (value['metadata'].get('labels', {}),
                                   value.get('spec', {}).get('selector', {}),
                                   value.get('spec', {}).get('selector', {}).get('matchLabels', {}),
                                   value.get('spec', {}).get('template', {}).get('metadata', {}).get('labels', {})):
                        if isinstance(labels, dict):
                            self.assertTrue(all(isinstance(label, str) for label in labels.values()
                                                if not isinstance(label, dict)), (value['kind'], labels))
                policy = select(objects, 'NetworkPolicy', '-dependencies')['spec']
                self.assertIn({'podSelector': {'matchLabels': {'aidash.run/activity': release}}},
                              policy['ingress'][0]['from'])
        admission = next(value['data']['admission.conf'] for value in render(self.environment, values)
                         if value['kind'] == 'ConfigMap' and 'admission.conf' in value.get('data', {}))
        logs = [line.strip() for line in admission.splitlines() if line.strip().startswith('access_log')]
        self.assertEqual(logs, ['access_log off;', 'access_log off;'])

    def test_environment_names_fit_kubernetes_limits(self):
        values = {'postgres': {'existingSecret': 'db'},
                  'edge': {'hostname': 'fixture.example', 'admissionImage': 'admission' + DIGEST},
                  'activity': {'existingSecret': 'observer', 'observerImage': 'observer' + DIGEST,
                               'collectorImage': 'collector' + DIGEST}}
        objects = render(self.environment, values, 'r' * 31)
        for value in objects:
            name = value['metadata']['name']
            # StatefulSets and CronJobs append 11 controller-generated characters.
            limit = 52 if value['kind'] in ('StatefulSet', 'CronJob') else 63
            if value['kind'] in ('Service', 'StatefulSet', 'CronJob', 'Deployment', 'ConfigMap'):
                self.assertLessEqual(len(name), limit, (value['kind'], name))
        self.assertEqual(len(select(objects, 'Service', '-environment-postgres')['metadata']['name']), 52)
        with self.assertRaises(subprocess.CalledProcessError):
            render(self.environment, values, 'r' * 32)

    def test_only_the_activity_collector_receives_the_patch_token(self):
        values = {'postgres': {'existingSecret': 'db'},
                  'edge': {'hostname': 'fixture.example', 'admissionImage': 'admission' + DIGEST},
                  'activity': {'existingSecret': 'observer', 'observerImage': 'observer' + DIGEST,
                               'collectorImage': 'collector' + DIGEST}}
        job = select(render(self.environment, values), 'CronJob', '-activity')
        pod = job['spec']['jobTemplate']['spec']['template']['spec']
        self.assertIs(pod['automountServiceAccountToken'], False)
        api = next(volume for volume in pod['volumes'] if 'projected' in volume)
        sources = api['projected']['sources']
        self.assertEqual([next(iter(source)) for source in sources],
                         ['serviceAccountToken', 'configMap', 'downwardAPI'])
        account = '/var/run/secrets/kubernetes.io/serviceaccount'
        (observer,), (collector,) = pod['initContainers'], pod['containers']
        self.assertNotIn(api['name'], [mount['name'] for mount in observer['volumeMounts']])
        self.assertIn({'name': api['name'], 'mountPath': account, 'readOnly': True}, collector['volumeMounts'])


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

    def test_persisted_snapshot_keeps_a_bounded_transfer_digest(self):
        previous = {'observed_at': 9950, 'last_active': 5000, 'busy': False}
        first = self.combine(previous, transfers=['a', 'b'])
        self.assertNotIn('completed_transfers', first)
        self.assertEqual(first['completed_transfers_count'], 2)
        settled = dict(first, last_active=5000, observed_at=9950)
        self.assertEqual(self.combine(settled, transfers=['b', 'a'])['last_active'], 5000)
        self.assertEqual(self.combine(settled, transfers=['a', 'b', 'c'])['last_active'], 10000)
        self.assertEqual(self.combine(settled, transfers=['a'])['last_active'], 10000)
        legacy = dict(previous, completed_transfers=['a', 'b'])
        self.assertEqual(self.combine(legacy, transfers=['b', 'a'])['last_active'], 5000)
        self.assertEqual(self.combine(legacy, transfers=['a', 'b', 'c'])['last_active'], 10000)
        many = self.combine(previous, transfers=[f'{n:036d}' for n in range(50000)])
        self.assertLess(len(json.dumps(many)), 2048)


if __name__ == '__main__':
    unittest.main()
