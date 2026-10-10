"""Chart contract tests for scheduling, privileges and retained single writers."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import yaml

ROOT = Path(__file__).resolve().parents[1]
DIGEST = '@sha256:' + 'b' * 64
# The shipped PostgreSQL value is the locally built image's tag; it must be pinned.
POSTGRES = {'existingSecret': 'db', 'image': 'aidash-postgres' + DIGEST}


def render(chart, values, release='fixture', kube_version=None, namespace='environment'):
    command = ['helm', 'template', release, str(ROOT / chart), '--namespace', namespace, '-f', '-']
    if kube_version:
        command += ['--kube-version', kube_version]
    output = subprocess.check_output(command, input=json.dumps(values).encode())
    return [value for value in yaml.safe_load_all(output) if value]


def select(objects, kind, suffix):
    return next(value for value in objects if value['kind'] == kind and value['metadata']['name'].endswith(suffix))


class ChartsTest(unittest.TestCase):
    aidash = 'deploy/helm/aidash'
    environment = 'infra/gcp/helm/environment'
    # The Runner requires the application's capability object claim.
    base = {'node': {'id': 'aidash://fixture'}, 'existingSecret': 'fixture',
            'capabilities': {'storage': {'existingClaim': 'objects'}}}

    def test_execution_is_opt_in(self):
        objects = render(self.aidash, self.base)
        self.assertFalse(any(value['kind'] in ('DaemonSet', 'RuntimeClass', 'Namespace', 'PersistentVolumeClaim') for value in objects))
        # Without the Runner the application keeps its admission-disabled default profile.
        self.assertFalse(any(value['kind'] == 'ConfigMap' and 'profile.json' in value.get('data', {}) for value in objects))
        for role in ('server', 'worker'):
            env = select(objects, 'Deployment', '-aidash-' + role)['spec']['template']['spec']['containers'][0]['env']
            self.assertNotIn('AIDASH_CAPABILITY_PROFILE', [variable['name'] for variable in env])

    def test_shared_rwo_claims_co_locate_roles_and_avoid_update_surge(self):
        affinity = {'nodeAffinity': {'preferredDuringSchedulingIgnoredDuringExecution': []}}
        ledger = dict(self.base, capabilities={'storage': {'existingClaim': ''}}, memoryRecovery={'existingClaim': 'ledger'})
        objects_only = dict(self.base)
        for values, mounts in ((ledger, {'memory-recovery': ('ledger', '/var/lib/aidash/memory-recovery')}),
                               (objects_only, {'capabilities': ('objects', '/var/lib/aidash/capabilities')})):
            objects = render(self.aidash, dict(values, affinity=affinity))
            for role in ('server', 'worker'):
                deployment = select(objects, 'Deployment', '-aidash-' + role)
                self.assertEqual(deployment['spec']['strategy'], {'type': 'Recreate'})
                pod = deployment['spec']['template']
                term = pod['spec']['affinity']['podAffinity']['requiredDuringSchedulingIgnoredDuringExecution'][0]
                self.assertEqual(term['topologyKey'], 'kubernetes.io/hostname')
                for key, value in term['labelSelector']['matchLabels'].items():
                    self.assertEqual(pod['metadata']['labels'][key], value)
                self.assertIn('nodeAffinity', pod['spec']['affinity'])
                self.assertEqual(pod['spec']['securityContext']['fsGroup'], 10001)
                claims = {volume['name']: volume['persistentVolumeClaim']['claimName'] for volume in pod['spec']['volumes']}
                paths = {mount['name']: mount['mountPath'] for mount in pod['spec']['containers'][0]['volumeMounts']}
                self.assertEqual({name: (claims[name], paths[name]) for name in claims}, mounts)
        # Without a shared claim the roles roll independently.
        unshared = render(self.aidash, dict(self.base, capabilities={'storage': {'existingClaim': ''}}))
        self.assertEqual(select(unshared, 'Deployment', '-aidash-server')['spec']['strategy']['type'], 'RollingUpdate')

    def test_server_worker_google_identities_are_separate(self):
        objects = render(self.aidash, dict(self.base,
            server={'serviceAccount': {'annotations': {'iam.gke.io/gcp-service-account': 'server@example'}}},
            worker={'serviceAccount': {'annotations': {'iam.gke.io/gcp-service-account': 'worker@example'}}}))
        for role in ('server', 'worker'):
            account = select(objects, 'ServiceAccount', '-aidash-' + role)
            self.assertEqual(account['metadata']['annotations']['iam.gke.io/gcp-service-account'], role + '@example')
            pod = select(objects, 'Deployment', '-aidash-' + role)['spec']['template']['spec']
            self.assertEqual(pod['serviceAccountName'], account['metadata']['name'])

    def test_gke_release_image_annotation_frontend_and_backend_ingress(self):
        sha = '0123456789abcdef' * 2 + '01234567'
        values = dict(self.base, image={'repository': 'registry/app', 'digest': 'sha256:' + 'c' * 64},
                      release={'sourceSha': sha}, frontend={'enabled': False},
                      trustedProxy={'cidrs': ['10.4.0.0/14', '127.0.0.1']},
                      backendIngress={'podSelectors': [{'aidash.run/edge': 'env'}]})
        objects = render(self.aidash, values, 'app')
        for role in ('server', 'worker'):
            deployment = select(objects, 'Deployment', '-aidash-' + role)
            self.assertEqual(deployment['metadata']['annotations'], {'aidash.run/source-sha': sha})
            container = deployment['spec']['template']['spec']['containers'][0]
            # The digest wins over the default tag.
            self.assertEqual(container['image'], 'registry/app@sha256:' + 'c' * 64)
            proxies = [variable.get('value') for variable in container['env'] if variable['name'] == 'AIDASH_AUTH_TRUSTED_PROXY_IPS']
            self.assertEqual(proxies, ['10.4.0.0/14,127.0.0.1'] if role == 'server' else [])
        # The runtime image serves the web bundle; nothing selects frontend Pods.
        self.assertEqual(sorted(value['metadata']['name'] for value in objects if value['kind'] in ('Deployment', 'Service')),
                         ['app-aidash-server', 'app-aidash-worker', 'app-backend'])
        policy = select(objects, 'NetworkPolicy', '-backend')['spec']
        backend = select(objects, 'Service', '-backend')['spec']['selector']
        self.assertEqual(policy['podSelector']['matchLabels'], backend)
        self.assertEqual(policy['ingress'], [{'from': [{'podSelector': {'matchLabels': {'aidash.run/edge': 'env'}}}],
                                              'ports': [{'protocol': 'TCP', 'port': 8080}]}])
        # An enabled frontend proxies to the backend, so it stays admitted.
        policy = select(render(self.aidash, dict(values, frontend={'enabled': True}), 'app'), 'NetworkPolicy', '-backend')['spec']
        self.assertIn({'podSelector': {'matchLabels': {'app.kubernetes.io/instance': 'app', 'app.kubernetes.io/component': 'frontend'}}},
                      policy['ingress'][0]['from'])
        defaults = render(self.aidash, self.base, 'app')
        self.assertFalse(any(value['kind'] == 'NetworkPolicy' for value in defaults))
        self.assertEqual(select(defaults, 'Deployment', '-aidash-server')['spec']['template']['spec']['containers'][0]['image'], 'aidash:0.1.0')
        self.assertNotIn('annotations', select(defaults, 'Deployment', '-aidash-server')['metadata'])
        self.assertEqual(select(defaults, 'Deployment', '-frontend')['spec']['replicas'], 1)
        for invalid in ({'release': {'sourceSha': sha[:39]}}, {'release': {'sourceSha': sha.upper()}},
                        {'image': {'repository': 'registry/app', 'digest': 'sha256:short'}}):
            with self.subTest(values=invalid), self.assertRaises(subprocess.CalledProcessError):
                render(self.aidash, dict(self.base, **invalid))

    @staticmethod
    def mounted(objects, role, variable):
        """Content of the file an env variable names, through the Pod's mounts."""
        pod = select(objects, 'Deployment', '-aidash-' + role)['spec']['template']['spec']
        path = Path(next(item['value'] for item in pod['containers'][0]['env'] if item['name'] == variable))
        mount = next(item for item in pod['containers'][0]['volumeMounts'] if Path(item['mountPath']) == path.parent)
        volume = next(item for item in pod['volumes'] if item['name'] == mount['name'])
        assert mount.get('readOnly'), mount
        return select(objects, 'ConfigMap', volume['configMap']['name'])['data'][path.name]

    def test_application_capability_profile_matches_the_runner_it_names(self):
        execution = {'createNamespaces': True, 'sandboxImage': 'sandbox' + DIGEST,
                     'sandboxNamespace': 'aidash-pr-1-sandbox', 'trustedNamespace': 'aidash-pr-1-trusted',
                     'runtimeClass': {'create': True, 'name': 'aidash-gvisor-pr-1'},
                     'runner': {'enabled': True, 'existingSecret': 'app-runner', 'image': 'control' + DIGEST},
                     'guard': {'enabled': True, 'image': 'control' + DIGEST}}
        values = dict(self.base, execution=execution)
        objects = render(self.aidash, values, 'app', namespace='aidash-pr-1')
        runner_profile = json.loads(select(objects, 'ConfigMap', '-execution-profile')['data']['profile.json'])
        runner_container = select(objects, 'Deployment', '-execution-runner')['spec']['template']['spec']['containers'][0]
        runner_token = next(item['valueFrom'] for item in runner_container['env'] if item['name'] == 'AIDASH_CORE_RUNNER_TOKEN')
        service = select(objects, 'Service', '-execution-runner')
        profiles = []
        for role in ('server', 'worker'):
            profile = json.loads(self.mounted(objects, role, 'AIDASH_CAPABILITY_PROFILE'))
            profiles.append(profile)
            container = select(objects, 'Deployment', '-aidash-' + role)['spec']['template']['spec']['containers'][0]
            # The token the application presents is the one the Runner verifies.
            token = next(item['valueFrom'] for item in container['env'] if item['name'] == profile['runner']['token_env'])
            self.assertEqual(token, runner_token)
            # Capability objects live in a child of the retained claim: the store
            # chmods its directory, which a root-owned volume root would refuse.
            mounts = {item['mountPath']: item['name'] for item in container['volumeMounts']}
            self.assertEqual(os.path.dirname(profile['storage']), '/var/lib/aidash/capabilities')
            self.assertEqual(mounts[os.path.dirname(profile['storage'])], 'capabilities')
        self.assertEqual(profiles[0], profiles[1])
        profile = profiles[0]
        # Exactly the Rust `Profile` fields (deny_unknown_fields): no Runner-only host_tasks.
        self.assertEqual(set(profile), {'admission', 'storage', 'cpu', 'memory_bytes', 'processes', 'working_bytes',
                                        'temporary_bytes', 'output_bytes', 'maximum_seconds', 'idle_seconds',
                                        'operation_seconds', 'install_seconds', 'runner'})
        self.assertIs(profile['admission'], True)
        for key in set(profile) & set(runner_profile) - {'runner'}:
            self.assertEqual(profile[key], runner_profile[key], key)
        self.assertEqual(profile['runner'], {
            'endpoint': 'http://%s:%d' % (service['metadata']['name'], service['spec']['ports'][0]['port']),
            'token_env': 'AIDASH_CORE_RUNNER_TOKEN', 'image': runner_profile['runner']['image'],
            'runtime_class': select(objects, 'RuntimeClass', '')['metadata']['name'],
            'namespace': select(objects, 'Namespace', '-sandbox')['metadata']['name']})
        # Operation and install limits never exceed the operation time limit.
        short = render(self.aidash, dict(values, execution=dict(execution, limits={'maximum_seconds': 30})))
        profile = json.loads(self.mounted(short, 'server', 'AIDASH_CAPABILITY_PROFILE'))
        self.assertEqual((profile['operation_seconds'], profile['install_seconds'], profile['maximum_seconds']), (30, 30, 30))
        # Admission without a retained object store would write to a read-only root.
        with self.assertRaises(subprocess.CalledProcessError):
            render(self.aidash, dict(values, capabilities={'storage': {'existingClaim': ''}}))
        # Cluster-scoped objects of same-named releases in other Environments stay distinct.
        other = dict(execution, sandboxNamespace='aidash-test-sandbox', trustedNamespace='aidash-test-trusted',
                     runtimeClass={'create': True, 'name': 'aidash-gvisor-test'}, installer={'enabled': True, 'image': 'control' + DIGEST})
        names = []
        for namespace, environment in (('aidash-pr-1', dict(execution, installer=other['installer'])), ('aidash-test', other)):
            rendered = render(self.aidash, dict(values, execution=environment, environment={'nodeSelector': {'pool': namespace}}),
                              'app', namespace=namespace)
            names.append({(value['kind'], value['metadata']['name']) for value in rendered
                          if value['kind'] in ('ClusterRole', 'ClusterRoleBinding', 'RuntimeClass', 'Namespace')})
        # Two ClusterRoles, two bindings, the RuntimeClass and two Namespaces.
        self.assertEqual(len(names[0]), 7)
        self.assertEqual(names[0] & names[1], set())

    def test_managed_provider_and_gcip_settings_are_validated_and_mounted(self):
        store = {'kind': 'secret_manager', 'byok_project_id': 'aidash-byok-1', 'environment_id': 'pr-1'}
        broker = {'endpoint': 'https://broker.run.app/api/v1', 'issuer': 'aidash-worker', 'audience': 'pr-1', 'kid': 'k1'}
        descriptor = {'fingerprint_key': {'env': 'AIDASH_PROVIDER_FINGERPRINT_KEY'}, 'store': store, 'broker': broker}
        gcip = {'project_id': 'fixture', 'web_api_key': 'public', 'public_origin': 'https://pr-1.aidash.run',
                'tenant_bindings': {'Pool-X': 'acme'}, 'session_idle_seconds': 3600}
        values = dict(self.base, node={'id': 'aidash://fixture', 'endpoint': 'https://pr-1.aidash.run'},
                      providerCredentials={'settings': descriptor}, gcip={'settings': {'dashboard': {'gcip': gcip}}})
        objects = render(self.aidash, values)
        for role in ('server', 'worker'):
            self.assertEqual(json.loads(self.mounted(objects, role, 'AIDASH_PROVIDER_CREDENTIAL_SETTINGS')),
                             {'provider_credentials': descriptor})
            self.assertEqual(json.loads(self.mounted(objects, role, 'AIDASH_GCIP_SETTINGS')), {'dashboard': {'gcip': gcip}})
        # A descriptor change restarts the Pods that read it at startup.
        rotated = render(self.aidash, dict(values, providerCredentials={'settings': dict(descriptor, broker=dict(broker, kid='k2'))}))

        def checksum(rendered):
            return select(rendered, 'Deployment', '-aidash-server')['spec']['template']['metadata']['annotations']['checksum/settings']

        self.assertNotEqual(checksum(objects), checksum(rotated))
        # The all-null descriptor is rendered as an explicit "no managed Store".
        disabled = {'fingerprint_key': None, 'store': None, 'broker': None}
        rendered = render(self.aidash, dict(self.base, providerCredentials={'settings': disabled}))
        self.assertEqual(json.loads(self.mounted(rendered, 'server', 'AIDASH_PROVIDER_CREDENTIAL_SETTINGS')),
                         {'provider_credentials': disabled})
        # Unset settings add nothing.
        names = [item['name'] for item in select(render(self.aidash, self.base), 'Deployment', '-aidash-server')
                 ['spec']['template']['spec']['containers'][0]['env']]
        self.assertFalse({'AIDASH_PROVIDER_CREDENTIAL_SETTINGS', 'AIDASH_GCIP_SETTINGS'} & set(names))
        invalid_descriptors = [
            dict(descriptor, extra=None),
            dict(descriptor, store=dict(store, kind='postgres')),
            dict(descriptor, store=dict(store, environment_id='production')),
            dict(descriptor, store=dict(store, byok_project_id='Bad_Project')),
            dict(descriptor, store=dict(store, master_key={'env': 'KEY'})),
            dict(descriptor, fingerprint_key={'file': '/key'}),
            dict(descriptor, broker=dict(broker, audience='develop')),
            dict(descriptor, broker=dict(broker, kid='')),
            dict(disabled, broker=broker),
            dict(disabled, fingerprint_key={'env': 'AIDASH_PROVIDER_FINGERPRINT_KEY'}),
        ]
        for invalid in invalid_descriptors:
            with self.subTest(descriptor=invalid), self.assertRaises(subprocess.CalledProcessError):
                render(self.aidash, dict(values, providerCredentials={'settings': invalid}))
        invalid_gcip = [
            {'dashboard': {'gcip': dict(gcip, public_origin='https://other.aidash.run')}},
            {'dashboard': {'gcip': dict(gcip, arbitrary='value')}},
            {'dashboard': {'gcip': dict(gcip, web_api_key=' ')}},
            {'dashboard': {'gcip': gcip, 'oidc': {}}},
            {'dashboard': {'gcip': gcip}, 'node': {'api_token': 'override'}},
        ]
        for invalid in invalid_gcip:
            with self.subTest(gcip=invalid), self.assertRaises(subprocess.CalledProcessError):
                render(self.aidash, dict(values, gcip={'settings': invalid}))
        # The public origin is the node's own endpoint, so an unset endpoint is refused.
        with self.assertRaises(subprocess.CalledProcessError):
            render(self.aidash, dict(values, node={'id': 'aidash://fixture'}))

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
        volumes = {value['name']: value for value in guard['volumes']}
        for name in ('gvisor-bin', 'ctr'):
            self.assertEqual(mounts[name]['mountPath'], volumes[name]['hostPath']['path'])
        # The installer replaces runtime files atomically; only a directory mount
        # shows the guard the new inode, so no runtime file is bind mounted by itself.
        self.assertEqual(volumes['runsc-dir']['hostPath'], {'path': '/usr/local/bin', 'type': 'Directory'})
        self.assertEqual(volumes['gvisor-bin']['hostPath']['type'], 'Directory')
        self.assertEqual(mounts['runsc-dir'], {'name': 'runsc-dir', 'mountPath': '/run/aidash-host-runtime', 'readOnly': True})
        guard_env = {value['name']: value['value'] for value in guard['containers'][0]['env']}
        self.assertEqual(guard_env['AIDASH_RUNSC_BINARY'], '/run/aidash-host-runtime/runsc')
        self.assertEqual(json.loads(guard_env['AIDASH_SENTRY_BINARIES']),
                         ['/run/aidash-host-runtime/runsc', '/usr/local/bin/gvisor-bin/gvisor_sentry'])
        # A Sentry path the guard cannot see would silently disable its identity check.
        unseen = dict(execution, paths={'sentryBinaries': ['/opt/elsewhere/runsc']})
        with self.assertRaises(subprocess.CalledProcessError):
            render(self.aidash, dict(self.base, execution=unseen, environment={'nodeSelector': {'pool': 'execution'}}))
        # The pinned runtime splits the Sentry; without its path no kill could be verified.
        runsc_only = dict(execution, paths={'sentryBinaries': ['/usr/local/bin/runsc']})
        with self.assertRaises(subprocess.CalledProcessError):
            render(self.aidash, dict(self.base, execution=runsc_only, environment={'nodeSelector': {'pool': 'execution'}}))
        # The Runner's isolation probe goes through this release's guard.
        unguarded = dict(execution, guard={'enabled': False})
        with self.assertRaises(subprocess.CalledProcessError):
            render(self.aidash, dict(self.base, execution=unguarded, environment={'nodeSelector': {'pool': 'execution'}}))
        # Namespace and RuntimeClass overrides that YAML reads as other scalars stay strings.
        quoted = dict(execution, sandboxNamespace='true', trustedNamespace='null',
                      runtimeClass={'create': True, 'name': 'true'})
        names = render(self.aidash, dict(self.base, execution=quoted, environment={'nodeSelector': {'pool': 'execution'}}))
        self.assertEqual(sorted(value['metadata']['name'] for value in names if value['kind'] == 'Namespace'), ['null', 'true'])
        self.assertEqual([value['metadata']['name'] for value in names if value['kind'] == 'RuntimeClass'], ['true'])
        runtime_rule = next(rule for rule in select(names, 'ClusterRole', '-runtime')['rules']
                            if rule['resources'] == ['runtimeclasses'])
        self.assertEqual(runtime_rule['resourceNames'], ['true'])
        for value in names:
            if 'namespace' in value['metadata']:
                self.assertIsInstance(value['metadata']['namespace'], str, value['kind'])
            for subject in value.get('subjects', []):
                self.assertIsInstance(subject['namespace'], str, value['kind'])
        runner = select(objects, 'Deployment', '-execution-runner')
        self.assertEqual(runner['spec']['replicas'], 1)
        self.assertEqual(runner['spec']['strategy']['type'], 'Recreate')
        self.assertEqual(runner['spec']['template']['spec']['containers'][0]['image'], 'trusted-runner' + DIGEST)
        profile = json.loads(select(objects, 'ConfigMap', '-execution-profile')['data']['profile.json'])
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
        # The application capability profile accepts 16..=4096 guest processes.
        for processes, accepted in ((15, False), (16, True), (4096, True), (4097, False)):
            limits = dict(self.base, execution={'limits': {'processes': processes, 'host_tasks': 4 * processes}})
            if accepted:
                render(self.aidash, limits)
            else:
                with self.assertRaises(subprocess.CalledProcessError):
                    render(self.aidash, limits)

    def test_enabled_trusted_images_must_be_pinned_by_digest(self):
        for component in ('installer', 'guard', 'runner'):
            execution = {'createNamespaces': True, 'sandboxImage': 'sandbox' + DIGEST,
                         'runner': {'existingSecret': 'runner'}}
            if component == 'runner':
                # The Runner always requires its release's guard.
                execution['guard'] = {'enabled': True, 'image': 'guard' + DIGEST}
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
        values = {'postgres': POSTGRES,
                  'edge': {'hostname': 'fixture.example', 'admissionImage': 'admission' + DIGEST},
                  'activity': {'existingSecret': 'observer', 'observerImage': 'observer' + DIGEST,
                               'collectorImage': 'collector' + DIGEST}}
        objects = render(self.environment, values)
        # Shipped Caddy and NATS defaults are digest pinned too: one receives all public
        # traffic, the other runs with the retained JetStream volume.
        for kind, suffix, index in (('Deployment', '-edge', 0), ('StatefulSet', '-nats', 0)):
            container = select(objects, kind, suffix)['spec']['template']['spec']['containers'][index]
            self.assertRegex(container['image'], r'@sha256:[0-9a-f]{64}$')
        values = dict(values, nats={'image': 'nats' + DIGEST})
        render(self.environment, values)
        for section, key in (('edge', 'admissionImage'), ('edge', 'caddyImage'),
                             ('activity', 'observerImage'), ('activity', 'collectorImage'),
                             ('postgres', 'image'), ('nats', 'image')):
            with self.subTest(section=section, image=key):
                mutable = dict(values, **{section: dict(values[section], **{key: 'image:latest'})})
                with self.assertRaises(subprocess.CalledProcessError):
                    render(self.environment, mutable)

    def test_execution_limits_names_paths_and_time_are_consistent_with_the_guard(self):
        runner = {'enabled': True, 'existingSecret': 'runner', 'image': 'runner' + DIGEST}
        guard = {'enabled': True, 'image': 'guard' + DIGEST}
        base = {'sandboxImage': 'sandbox' + DIGEST, 'runner': runner, 'guard': guard}

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
                                          (16, 399, False), (16, 400, True), (256, 1023, False), (256, 1024, True)):
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
                                 ('output_bytes', 8 << 20, True), ('output_bytes', (8 << 20) + 1, False),
                                 ('working_bytes', 1 << 30, True), ('working_bytes', (1 << 30) + 1, False)):
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
        values = {'postgres': POSTGRES,
            'edge': {'hostname': 'fixture.example', 'admissionImage': 'admission' + DIGEST},
            'activity': {'existingSecret': 'observer', 'observerImage': 'observer' + DIGEST,
                         'collectorImage': 'collector' + DIGEST}}
        objects = render(self.environment, values)
        # Releases share one cluster; StorageClass and the preview TLS PV are controller-owned.
        cluster_scoped = ('StorageClass', 'PersistentVolume', 'Namespace', 'ClusterRole', 'ClusterRoleBinding')
        self.assertEqual([value['kind'] for value in objects if value['kind'] in cluster_scoped], [])
        for role in ('postgres', 'nats'):
            state = select(objects, 'StatefulSet', '-' + role)['spec']
            self.assertEqual(state['replicas'], 1)
            self.assertEqual(state['persistentVolumeClaimRetentionPolicy'], {'whenDeleted': 'Retain', 'whenScaled': 'Retain'})
            self.assertEqual(state['volumeClaimTemplates'][0]['spec']['accessModes'], ['ReadWriteOnce'])
            self.assertEqual(state['volumeClaimTemplates'][0]['spec']['storageClassName'], 'aidash-retain')
        # The controller's migration Job reaches the dependencies alongside server and worker.
        sources = select(objects, 'NetworkPolicy', '-dependencies')['spec']['ingress'][0]['from']
        components = next(source['podSelector']['matchExpressions'][0]['values'] for source in sources
                          if 'matchExpressions' in source['podSelector'])
        self.assertEqual(components, ['server', 'worker', 'migration'])
        # A preview edge mounts the controller-bound shared TLS claim instead of its own.
        own = select(objects, 'PersistentVolumeClaim', '-tls')
        self.assertEqual(own['metadata']['annotations'], {'helm.sh/resource-policy': 'keep'})
        preview = render(self.environment, dict(values, edge=dict(values['edge'], existingTlsClaim='preview-tls')))
        self.assertFalse(any(value['kind'] == 'PersistentVolumeClaim' for value in preview))
        tls = next(volume for volume in select(preview, 'Deployment', '-edge')['spec']['template']['spec']['volumes']
                   if volume['name'] == 'tls')
        self.assertEqual(tls['persistentVolumeClaim']['claimName'], 'preview-tls')
        service = select(objects, 'Service', '-edge')
        self.assertEqual(service['spec']['externalTrafficPolicy'], 'Local')
        self.assertEqual([port['port'] for port in service['spec']['ports']], [80, 443])
        # Desired admission is controller-owned (kube.ADMISSION): mounted optionally and
        # read-only, never rendered, so neither install nor upgrade resets it.
        named = render(self.environment, values, 'env')
        self.assertFalse(any(value['kind'] == 'ConfigMap' and value['metadata']['name'].endswith('-admission')
                             for value in named))
        pod = select(named, 'Deployment', '-edge')['spec']['template']['spec']
        state = next(volume for volume in pod['volumes'] if volume['name'] == 'admission-state')
        self.assertEqual(state['configMap'], {'name': 'env-environment-admission', 'optional': True})
        admission = next(container for container in pod['containers'] if container['name'] == 'admission')
        self.assertIn({'name': 'admission-state', 'mountPath': '/etc/aidash-admission', 'readOnly': True},
                      admission['volumeMounts'])
        caddy = next(container for container in pod['containers'] if container['name'] == 'caddy')
        self.assertNotIn('admission-state', [mount['name'] for mount in caddy['volumeMounts']])
        conf = select(named, 'ConfigMap', '-edge')['data']['admission.conf']
        self.assertIn('io.open("/etc/aidash-admission/state", "r")', conf)
        self.assertIn('ngx.shared.aidash_activity:set("closed", not open)', conf)
        job = select(objects, 'CronJob', '-activity')['spec']
        self.assertEqual(job['schedule'], '* * * * *')
        self.assertEqual(job['concurrencyPolicy'], 'Forbid')
        for value in objects:
            if value['kind'] in ('Deployment', 'StatefulSet'):
                pod = value['spec']['template']['spec']
                self.assertFalse(pod['hostNetwork'])
                self.assertTrue(all(term.get('tolerationSeconds', 30) < 300 for term in pod['tolerations']))

    def test_environment_labels_stay_strings_and_edge_keeps_no_file_access_log(self):
        values = {'postgres': POSTGRES,
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
        # A release namespace that YAML would read as a boolean stays a string too.
        objects = render(self.environment, values, namespace='true')
        binding = select(objects, 'RoleBinding', '-activity')
        self.assertEqual([subject['namespace'] for subject in binding['subjects']], ['true'])
        # Configurable Secret and StorageClass names stay strings too.
        names = dict(values, postgres=dict(POSTGRES, existingSecret='true'),
                     activity=dict(values['activity'], existingSecret='null'),
                     storage={'className': 'true'})
        objects = render(self.environment, names)
        postgres = select(objects, 'StatefulSet', '-postgres')['spec']
        self.assertEqual(postgres['template']['spec']['containers'][0]['envFrom'], [{'secretRef': {'name': 'true'}}])
        self.assertEqual(postgres['volumeClaimTemplates'][0]['spec']['storageClassName'], 'true')
        self.assertEqual(select(objects, 'PersistentVolumeClaim', '-tls')['spec']['storageClassName'], 'true')
        collector = select(objects, 'CronJob', '-activity')['spec']['jobTemplate']['spec']['template']['spec']
        references = [variable['valueFrom']['secretKeyRef']['name']
                      for container in collector['initContainers'] + collector['containers']
                      for variable in container.get('env', []) if 'secretKeyRef' in variable.get('valueFrom', {})]
        self.assertEqual(set(references), {'null'})
        admission = next(value['data']['admission.conf'] for value in render(self.environment, values)
                         if value['kind'] == 'ConfigMap' and 'admission.conf' in value.get('data', {}))
        logs = [line.strip() for line in admission.splitlines() if line.strip().startswith('access_log')]
        self.assertEqual(logs, ['access_log off;', 'access_log off;'])

    def test_environment_names_fit_kubernetes_limits(self):
        values = {'postgres': POSTGRES,
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
        values = {'postgres': POSTGRES,
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
        # A failed observer must not fail the Pod: the collector still runs, finds no
        # database evidence and publishes its busy snapshot. Success is renamed whole.
        script = observer['command'][2]
        self.assertEqual(observer['command'][:2], ['sh', '-c'])
        for status, published in ((1, False), (0, True)):
            with self.subTest(status=status), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                (root / 'bin').mkdir()
                (root / 'snapshot').mkdir()
                fake = root / 'bin/aidash-infra-observer'
                fake.write_text(f'#!/bin/sh\necho partial\nexit {status}\n')
                fake.chmod(0o755)
                result = subprocess.run(['sh', '-c', script.replace('/snapshot', str(root / 'snapshot'))],
                                        env=dict(os.environ, PATH=f'{root / "bin"}:{os.environ["PATH"]}'))
                self.assertEqual(result.returncode, 0)
                self.assertEqual((root / 'snapshot/database.json').exists(), published)


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

    def test_missing_database_evidence_publishes_a_busy_snapshot(self):
        idle = {'protocol': 'aidash-infra-activity/1', 'busy': False, 'observed_at': 9990, 'last_active': 5000}
        patches = []

        def request(url, token=None, *, data=None, method=None, context=None):
            if method == 'PATCH':
                patches.append(json.loads(data))
                return {}
            if url.endswith('/configmaps/activity'):
                return {'metadata': {'resourceVersion': '7'}, 'data': {'snapshot.json': json.dumps(idle)}}
            return {'inflight': 0, 'last_active': 0, 'closed': False}

        environment = {'KUBERNETES_SERVICE_HOST': 'api', 'KUBERNETES_SERVICE_PORT': '443',
                       'ACTIVITY_CONFIGMAP': 'activity', 'EDGE_ACTIVITY_ENDPOINT': 'http://edge',
                       'RUNNER_ENDPOINT': 'http://runner', 'AIDASH_CORE_RUNNER_TOKEN': 'x' * 32}
        # The observer init container exited without writing /snapshot/database.json.
        with (patch.dict(os.environ, environment),
              patch.object(self.activity, 'request', request),
              patch.object(self.activity.ssl, 'create_default_context'),
              patch.object(Path, 'read_text', return_value='value'),
              patch.object(Path, 'read_bytes', side_effect=FileNotFoundError('database.json'))):
            self.activity.main()
        (published,) = patches
        self.assertEqual(published['metadata'], {'resourceVersion': '7'})
        snapshot = json.loads(published['data']['snapshot.json'])
        self.assertTrue(snapshot['busy'])
        self.assertTrue(snapshot['observation_gap'])

    def test_concurrent_publication_recombines_against_the_winner(self):
        # The minute CronJob and the controller's seal Job race on one resourceVersion.
        database = {'protocol': 'aidash-infra-activity/1', 'counts': {'runs': 0}, 'completed_transfers': [],
                    'last_work_completed': 0}
        snapshots = [('7', {'protocol': 'aidash-infra-activity/1', 'busy': True, 'observed_at': 9900}),
                     ('8', {'protocol': 'aidash-infra-activity/1', 'busy': False, 'observed_at': 9990,
                            'last_active': 5000, 'completed_transfers_digest': self.activity.digest([])})]
        environment = {'KUBERNETES_SERVICE_HOST': 'api', 'KUBERNETES_SERVICE_PORT': '443',
                       'ACTIVITY_CONFIGMAP': 'activity', 'EDGE_ACTIVITY_ENDPOINT': 'http://edge',
                       'RUNNER_ENDPOINT': 'http://runner', 'AIDASH_CORE_RUNNER_TOKEN': 'x' * 32}
        for conflicts, code in ((1, 409), (3, 409), (1, 500)):
            with self.subTest(conflicts=conflicts, code=code):
                reads, patches = [], []

                def request(url, token=None, *, data=None, method=None, context=None):
                    if method == 'PATCH':
                        patches.append(json.loads(data))
                        if len(patches) <= conflicts:
                            raise self.activity.urllib.error.HTTPError(url, code, 'conflict', {}, None)
                        return {}
                    if url.endswith('/configmaps/activity'):
                        version, snapshot = snapshots[min(len(reads), 1)]
                        reads.append(version)
                        return {'metadata': {'resourceVersion': version}, 'data': {'snapshot.json': json.dumps(snapshot)}}
                    if url.endswith('/v1/activity'):
                        return {'protocol': 'aidash-runner-activity/1', 'counts': {'runner': 0}}
                    return {'inflight': 0, 'last_active': 0, 'closed': True}

                with (patch.dict(os.environ, environment),
                      patch.object(self.activity, 'request', request),
                      patch.object(self.activity.ssl, 'create_default_context'),
                      patch.object(self.activity.time, 'time', return_value=10000),
                      patch.object(Path, 'read_text', return_value='value'),
                      patch.object(Path, 'read_bytes', return_value=json.dumps(database).encode())):
                    if conflicts == 1 and code == 409:
                        self.activity.main()
                    else:
                        with self.assertRaises(self.activity.urllib.error.HTTPError):
                            self.activity.main()
                if code != 409:
                    self.assertEqual(len(patches), 1)
                    continue
                if conflicts == 3:
                    self.assertEqual(len(patches), self.activity.PUBLISH_ATTEMPTS)
                    continue
                self.assertEqual([item['metadata']['resourceVersion'] for item in patches], ['7', '8'])
                published = json.loads(patches[-1]['data']['snapshot.json'])
                # Combined against the winner's idle snapshot, not the stale busy one.
                self.assertFalse(published['busy'])
                self.assertFalse(published['observation_gap'])
                self.assertEqual(published['last_active'], 5000)
                self.assertEqual(published['observed_at'], 10000)

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
