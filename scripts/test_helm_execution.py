"""Chart contract tests for scheduling, privileges and retained single writers."""
import hashlib
import importlib.util
import ipaddress
import json
import os
import re
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
GVISOR = json.loads((ROOT / 'runner/gvisor.json').read_text())


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
    base = {'deployment': {'id': 'aidash://fixture'}, 'existingSecret': 'fixture',
            'capabilities': {'storage': {'existingClaim': 'objects'}}}

    def test_protected_memory_schedules_backup_and_independent_prune(self):
        gcip = {'dashboard': {'gcip': {'project_id': 'fixture', 'web_api_key': 'public',
                                      'public_origin': 'https://fixture.aidash.run', 'tenant_bindings': {}}}}
        values = {**self.base, 'deployment': {'id': 'aidash://fixture', 'endpoint': 'https://fixture.aidash.run'},
                  'gcip': {'settings': gcip}, 'image': {'repository': 'aidash', 'digest': 'sha256:' + 'a' * 64},
                  'memoryRecovery': {'existingClaim': 'memory', 'directory': '/var/lib/aidash/memory-recovery/home',
                                     'subPath': 'home', 'protection': {
                                         'required': True, 'historyBucket': 'fixture-history',
                                         'archiveBucket': 'fixture-archives', 'existingSecret': 'maintenance',
                                         'backupServiceAccount': {'annotations': {'iam.gke.io/gcp-service-account': 'backup@fixture.iam.gserviceaccount.com'}},
                                         'pruneServiceAccount': {'annotations': {'iam.gke.io/gcp-service-account': 'prune@fixture.iam.gserviceaccount.com'}}}}}
        objects = render(self.aidash, values)
        jobs = {value['metadata']['name']: value for value in objects if value['kind'] == 'CronJob'}
        self.assertEqual(sorted(jobs), ['fixture-aidash-memory-backup', 'fixture-aidash-memory-prune'])
        for name, action, schedule in [('backup', 'backup-all', '0 */6 * * *'), ('prune', 'prune-remote', '*/15 * * * *')]:
            job = jobs['fixture-aidash-memory-' + name]
            self.assertEqual(job['spec']['schedule'], schedule)
            self.assertTrue(job['spec']['suspend'], 'initialization must finish before maintenance is scheduled')
            self.assertEqual(job['spec']['concurrencyPolicy'], 'Forbid')
            pod = job['spec']['jobTemplate']['spec']['template']['spec']
            container = pod['containers'][0]
            self.assertIn(action, container['args'])
            self.assertEqual(container['image'], 'aidash@sha256:' + 'a' * 64)
            self.assertEqual(container['envFrom'], [{'secretRef': {'name': 'maintenance'}}])
            env = {item['name']: item.get('value') for item in container['env']}
            self.assertEqual(env['AIDASH_DEPLOYMENT_ID'], 'aidash://fixture')
            self.assertEqual(env['AIDASH_ENDPOINT'], 'https://fixture.aidash.run')
            self.assertEqual(env['AIDASH_GCIP_SETTINGS'], '/etc/aidash/gcip/settings.json')
            mount = next(item for item in container['volumeMounts'] if item['name'] == 'gcip')
            self.assertTrue(mount['readOnly'])
            self.assertEqual(next(item for item in pod['volumes'] if item['name'] == 'gcip')['configMap']['name'], 'fixture-aidash-gcip')
            self.assertEqual(pod['securityContext']['runAsUser'], 10001)
            self.assertTrue(container['securityContext']['readOnlyRootFilesystem'])
            mounted = [volume for volume in pod['volumes'] if 'persistentVolumeClaim' in volume]
            if name == 'backup':
                self.assertEqual(mounted, [{'name': 'memory-recovery', 'persistentVolumeClaim': {'claimName': 'memory'}}])
                self.assertIn('podAffinity', pod['affinity'])
            else:
                self.assertEqual(mounted, [], 'pruning must run after total ledger disk loss')
        for role in ('server', 'worker'):
            container = select(objects, 'Deployment', '-aidash-' + role)['spec']['template']['spec']['containers'][0]
            env = {item['name']: item.get('value') for item in container['env']}
            self.assertEqual(env['AIDASH_MEMORY_RECOVERY_REQUIRED'], 'true')
            self.assertEqual(env['AIDASH_MEMORY_RECOVERY_HISTORY_BUCKET'], 'fixture-history')

    def test_protected_memory_rejects_incomplete_storage_configuration(self):
        command = ['helm', 'template', 'fixture', str(ROOT / self.aidash), '-f', '-']
        values = {**self.base, 'memoryRecovery': {'protection': {'required': True}}}
        result = subprocess.run(command, input=json.dumps(values), text=True, capture_output=True)
        self.assertNotEqual(result.returncode, 0, 'a required protected Home cannot silently render a local-only profile')

    def test_common_auth_mounts_the_managed_profile_and_tls_on_both_product_workers(self):
        objects = render(self.aidash, dict(self.base, commonAuth={'existingSecret': 'cloud-auth'}))
        for role in ('server', 'worker'):
            pod = select(objects, 'Deployment', '-' + role)['spec']['template']['spec']
            container = pod['containers'][0]
            environment = {entry['name']: entry.get('value') for entry in container['env']}
            self.assertEqual(environment['REINHARDT_ENV'], 'managed-common-auth')
            self.assertNotIn('AIDASH_GCIP_SETTINGS', environment)
            mount = next(mount for mount in container['volumeMounts'] if mount.get('subPath') == 'settings.toml')
            self.assertEqual(mount['mountPath'], '/app/server/settings/managed-common-auth.toml')
            self.assertTrue(mount['readOnly'])
            self.assertIn({'name': 'common-auth', 'mountPath': '/run/auth-tls', 'readOnly': True}, container['volumeMounts'])
            volume = next(volume for volume in pod['volumes'] if volume['name'] == mount['name'])
            self.assertEqual(volume['secret']['secretName'], 'cloud-auth')
        gcip = {'dashboard': {'gcip': {'project_id': 'project', 'web_api_key': 'public',
                                     'tenant_bindings': {}, 'public_origin': 'https://cloud.example.test'}}}
        with self.assertRaises(subprocess.CalledProcessError):
            render(self.aidash, dict(self.base, deployment={'id': 'aidash://fixture', 'endpoint': 'https://cloud.example.test'},
                                    commonAuth={'existingSecret': 'cloud-auth'}, gcip={'settings': gcip}))

    def test_local_postgres_secret_store_is_mounted_without_key_material(self):
        values = yaml.safe_load((ROOT / 'deploy/local-k8s/values.yaml').read_text())
        values.update(self.base)
        objects = render(self.aidash, values)
        expected = {
            'provider_credentials': {
                'fingerprint_key': {'env': 'AIDASH_PROVIDER_FINGERPRINT_KEY'},
                'store': {'kind': 'postgres', 'master_key': {'env': 'AIDASH_PROVIDER_STORE_MASTER_KEY'}},
                'broker': None,
            }
        }
        for role in ('server', 'worker'):
            self.assertEqual(json.loads(self.mounted(objects, role, 'AIDASH_PROVIDER_CREDENTIAL_SETTINGS')), expected)
        for store in ({'kind': 'postgres', 'master_key': {'value': 'plaintext'}},
                      {'kind': 'postgres', 'master_key': {'env': 'OTHER_KEY'}}):
            values['providerCredentials']['settings']['store'] = store
            with self.subTest(store=store), self.assertRaises(subprocess.CalledProcessError):
                render(self.aidash, values)

    def test_chart_installs_the_release_the_runner_image_bundles(self):
        # The installer reads only the archive bundled in its image (#394), so a
        # chart pin that differs from runner/gvisor.json can never install.
        values = yaml.safe_load((ROOT / self.aidash / 'values.yaml').read_text())['execution']['gvisor']
        self.assertEqual(values['version'], GVISOR['version'])
        self.assertEqual(values['sha256'], GVISOR['sha256']['x86_64'])

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
                claims = {volume['name']: volume['persistentVolumeClaim']['claimName'] for volume in pod['spec']['volumes']
                          if 'persistentVolumeClaim' in volume}
                paths = {mount['name']: mount['mountPath'] for mount in pod['spec']['containers'][0]['volumeMounts']}
                self.assertEqual({name: (claims[name], paths[name]) for name in claims}, mounts)
        # Without a shared claim the roles roll independently.
        unshared = render(self.aidash, dict(self.base, capabilities={'storage': {'existingClaim': ''}}))
        self.assertEqual(select(unshared, 'Deployment', '-aidash-server')['spec']['strategy']['type'], 'RollingUpdate')

    def test_read_only_roles_have_a_writable_temp_directory(self):
        # Several migration roots are staged under the temp directory, and the
        # migration Job copies the server template.
        objects = render(self.aidash, self.base)
        for role in ('server', 'worker'):
            pod = select(objects, 'Deployment', '-aidash-' + role)['spec']['template']['spec']
            container = pod['containers'][0]
            self.assertTrue(container['securityContext']['readOnlyRootFilesystem'])
            self.assertIn({'name': 'tmp', 'mountPath': '/tmp'}, container['volumeMounts'])
            self.assertIn({'name': 'tmp', 'emptyDir': {'sizeLimit': '64Mi'}}, pod['volumes'])

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
                      keylessEndpoints={'permittedPrivateOrigins': ['http://ollama.lan:11434', 'https://models.lan']},
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
            # Workers make Keyless calls too, so both roles carry the permitted origins.
            origins = [variable.get('value') for variable in container['env'] if variable['name'] == 'AIDASH_KEYLESS_PRIVATE_ORIGINS']
            self.assertEqual(origins, ['http://ollama.lan:11434,https://models.lan'])
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
        # Only the metadata-server egress policy is on by default; backend ingress is opt-in.
        self.assertEqual([value['metadata']['name'] for value in defaults if value['kind'] == 'NetworkPolicy'],
                         ['app-aidash-metadata-egress'])
        self.assertEqual(select(defaults, 'Deployment', '-aidash-server')['spec']['template']['spec']['containers'][0]['image'], 'aidash:0.1.0')
        self.assertNotIn('annotations', select(defaults, 'Deployment', '-aidash-server')['metadata'])
        # No private origin is permitted unless a Deployment Administrator lists one.
        self.assertNotIn('AIDASH_KEYLESS_PRIVATE_ORIGINS', [variable['name'] for variable in
                         select(defaults, 'Deployment', '-aidash-worker')['spec']['template']['spec']['containers'][0]['env']])
        self.assertEqual(select(defaults, 'Deployment', '-frontend')['spec']['replicas'], 1)
        for invalid in ({'release': {'sourceSha': sha[:39]}}, {'release': {'sourceSha': sha.upper()}},
                        {'image': {'repository': 'registry/app', 'digest': 'sha256:short'}},
                        {'keylessEndpoints': {'permittedPrivateOrigins': ['http://ollama.lan/v1']}},
                        {'keylessEndpoints': {'permittedPrivateOrigins': ['ollama.lan:11434']}}):
            with self.subTest(values=invalid), self.assertRaises(subprocess.CalledProcessError):
                render(self.aidash, dict(self.base, **invalid))

    def test_browser_frontends_reject_framing_at_each_public_serving_layer(self):
        objects = render(self.aidash, dict(self.base, frontend={'enabled': True}), 'app')
        nginx = select(objects, 'ConfigMap', '-frontend-conf')['data']['default.conf']
        self.assertIn('add_header Content-Security-Policy "frame-ancestors \'none\'" always;', nginx)
        self.assertIn('add_header X-Frame-Options "DENY" always;', nginx)

        values = {'postgres': POSTGRES,
                  'edge': {'hostname': 'fixture.example', 'admissionImage': 'admission' + DIGEST},
                  'activity': {'existingSecret': 'observer', 'observerImage': 'observer' + DIGEST,
                               'collectorImage': 'collector' + DIGEST}}
        rendered_edge = render(self.environment, values)
        caddy = select(rendered_edge, 'ConfigMap', '-edge')['data']['Caddyfile']
        self.assertEqual(caddy.count('header +Content-Security-Policy "frame-ancestors \'none\'"'), 1)
        self.assertEqual(caddy.count('header >X-Frame-Options DENY'), 1)
        edge = select(rendered_edge, 'Deployment', '-edge')
        annotations = edge['spec']['template']['metadata']['annotations']
        self.assertEqual(
            annotations['checksum/caddy-security-policy'],
            hashlib.sha256(b"frame-ancestors 'none'|DENY").hexdigest(),
        )

        proxied_values = dict(values, edge=dict(values['edge'], firebaseProject='aidash-fixture'))
        proxied = select(render(self.environment, proxied_values), 'ConfigMap', '-edge')['data']['Caddyfile']
        helper, application = proxied.split('  handle {\n', 1)
        self.assertNotIn('Content-Security-Policy', helper)
        self.assertNotIn('X-Frame-Options', helper)
        self.assertIn('header +Content-Security-Policy "frame-ancestors \'none\'"', application)
        self.assertIn('header >X-Frame-Options DENY', application)

    def test_trusted_proxies_are_rendered_only_when_the_server_accepts_them(self):
        # As server/src/http.rs ProxyNetwork parses AIDASH_AUTH_TRUSTED_PROXY_IPS at startup.
        accepted = ['10.4.0.0/14', '127.0.0.1', '0.0.0.0/0', '::1', '::/0', 'fe80::/10', '2001:DB8::/32',
                    '::ffff:1.2.3.0/120', '1:2:3:4:5::1.2.3.4', '1:2:3:4:5:6:7:8/128', 'ffff:fffe::/31']
        objects = render(self.aidash, dict(self.base, trustedProxy={'cidrs': accepted}))
        container = select(objects, 'Deployment', '-aidash-server')['spec']['template']['spec']['containers'][0]
        proxies = next(variable['value'] for variable in container['env'] if variable['name'] == 'AIDASH_AUTH_TRUSTED_PROXY_IPS')
        self.assertEqual(proxies, ','.join(accepted))
        for invalid in ('10.0.0.1/24', '10.0.0.1/33', '999.0.0.1', '010.0.0.1', '10.0.0.0/', 'fe80::1/10',
                        '1:2:3:4:5:6:7::8', '1::2::3', '1.2.3.4::', '::ffff:1.2.3.4/120', '1::/15', '1:2:3:4:5:6:7:8/129'):
            with self.subTest(proxy=invalid), self.assertRaises(subprocess.CalledProcessError):
                render(self.aidash, dict(self.base, trustedProxy={'cidrs': [invalid]}))

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
                     'installerNamespace': 'aidash-pr-1-installer',
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
            'endpoint': 'http://%s.aidash-pr-1.svc.cluster.local:%d' % (service['metadata']['name'], service['spec']['ports'][0]['port']),
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
                     installerNamespace='aidash-test-installer',
                     runtimeClass={'create': True, 'name': 'aidash-gvisor-test'}, installer={'enabled': True, 'image': 'control' + DIGEST})
        names = []
        for namespace, environment in (('aidash-pr-1', dict(execution, installer=other['installer'])), ('aidash-test', other)):
            rendered = render(self.aidash, dict(values, execution=environment, environment={'nodeSelector': {'pool': namespace}}),
                              'app', namespace=namespace)
            names.append({(value['kind'], value['metadata']['name']) for value in rendered
                          if value['kind'] in ('ClusterRole', 'ClusterRoleBinding', 'RuntimeClass', 'Namespace')})
        # Two ClusterRoles, two bindings, the RuntimeClass and three Namespaces.
        self.assertEqual(len(names[0]), 8)
        self.assertEqual(names[0] & names[1], set())

    def test_egress_is_an_internal_service_shared_by_the_application_and_runner(self):
        execution = {'sandboxImage': 'sandbox' + DIGEST,
                     'runner': {'enabled': True, 'existingSecret': 'app-runner', 'image': 'control' + DIGEST},
                     'guard': {'enabled': True, 'image': 'control' + DIGEST},
                     'egress': {'enabled': True, 'clusterIP': '10.96.0.90', 'port': 8950}}
        values = dict(self.base, execution=execution)
        objects = render(self.aidash, values, 'app', namespace='tenant-app')
        service = select(objects, 'Service', '-egress')['spec']
        self.assertEqual((service['type'], service['clusterIP']), ('ClusterIP', '10.96.0.90'))
        self.assertEqual(service['ports'], [{'name': 'egress', 'port': 8950, 'targetPort': 8950}])
        profiles = [json.loads(self.mounted(objects, role, 'AIDASH_CAPABILITY_PROFILE'))['egress']
                    for role in ('server', 'worker')]
        runner = json.loads(select(objects, 'ConfigMap', '-execution-profile')['data']['profile.json'])['egress']
        self.assertEqual(profiles, [runner, runner])
        self.assertEqual(runner['endpoint'], 'http://10.96.0.90:8950')
        self.assertEqual(runner['pod_selector'], service['selector'])
        self.assertEqual(runner['limits'], {'sent_bytes': 1 << 30, 'received_bytes': 1 << 30, 'requests': 1000})
        policy = select(objects, 'NetworkPolicy', '-egress-internal')['spec']
        self.assertEqual(policy['podSelector']['matchLabels'], service['selector'])
        ingress = policy['ingress'][-1]
        self.assertEqual(ingress['ports'], [{'protocol': 'TCP', 'port': 8950}])
        self.assertEqual(ingress['from'][0], {
            'namespaceSelector': {'matchLabels': {'kubernetes.io/metadata.name': 'app-sandbox'}},
            'podSelector': {'matchLabels': {'aidash-sandbox': 'true'}},
        })

    def test_egress_refuses_an_unconfigured_address_runner_or_managed_store(self):
        execution = {'sandboxImage': 'sandbox' + DIGEST,
                     'runner': {'enabled': True, 'existingSecret': 'app-runner', 'image': 'control' + DIGEST},
                     'guard': {'enabled': True, 'image': 'control' + DIGEST},
                     'egress': {'enabled': True, 'clusterIP': '10.96.0.90'}}
        invalid = [dict(self.base, execution=dict(execution, egress={'enabled': True, 'clusterIP': ''})),
                   dict(self.base, execution=dict(execution, runner={'enabled': False})),
                   dict(self.base, execution=execution, providerCredentials={'settings': {
                       'store': {'kind': 'secret_manager', 'byok_project_id': 'aidash-byok-1', 'environment_id': 'pr-1'}}})]
        for values in invalid:
            with self.subTest(values=values), self.assertRaises(subprocess.CalledProcessError):
                render(self.aidash, values)

    def test_managed_provider_and_gcip_settings_are_validated_and_mounted(self):
        store = {'kind': 'secret_manager', 'byok_project_id': 'aidash-byok-1', 'environment_id': 'pr-1'}
        broker = {'endpoint': 'https://broker.run.app/api/v1', 'issuer': 'aidash-worker', 'audience': 'pr-1', 'kid': 'k1'}
        descriptor = {'fingerprint_key': {'env': 'AIDASH_PROVIDER_FINGERPRINT_KEY'}, 'store': store, 'broker': broker}
        gcip = {'project_id': 'fixture', 'web_api_key': 'public', 'public_origin': 'https://pr-1.aidash.run',
                'tenant_bindings': {'Pool-X': 'acme'}, 'session_idle_seconds': 3600}
        values = dict(self.base, deployment={'id': 'aidash://fixture', 'endpoint': 'https://pr-1.aidash.run'},
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
        for helper in ('firebase', 'public_origin'):
            selected = {'dashboard': {'gcip': dict(gcip, auth_helper=helper)}}
            rendered = render(self.aidash, dict(values, gcip={'settings': selected}))
            self.assertEqual(json.loads(self.mounted(rendered, 'server', 'AIDASH_GCIP_SETTINGS')), selected)
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
            {'dashboard': {'gcip': dict(gcip, auth_helper='auth.example.test')}},
            {'dashboard': {'gcip': dict(gcip, web_api_key=' ')}},
            {'dashboard': {'gcip': gcip, 'oidc': {}}},
            {'dashboard': {'gcip': gcip}, 'deployment': {'api_token': 'override'}},
        ]
        for invalid in invalid_gcip:
            with self.subTest(gcip=invalid), self.assertRaises(subprocess.CalledProcessError):
                render(self.aidash, dict(values, gcip={'settings': invalid}))
        # The public origin is the Deployment's own endpoint, so an unset endpoint is refused.
        with self.assertRaises(subprocess.CalledProcessError):
            render(self.aidash, dict(values, deployment={'id': 'aidash://fixture'}))

    def test_production_disk_holders_are_statefulsets_that_stateful_ha_fails_over(self):
        execution = {'createNamespaces': True, 'sandboxImage': 'sandbox@sha256:' + 'a' * 64,
                     'runtimeClass': {'create': True},
                     'runner': {'enabled': True, 'existingSecret': 'runner', 'image': 'trusted-runner' + DIGEST},
                     'guard': {'enabled': True, 'image': 'trusted-guard' + DIGEST},
                     'installer': {'enabled': True, 'image': 'trusted-installer' + DIGEST}}
        claims = {'memoryRecovery': {'existingClaim': 'memory-recovery', 'directory': '/var/lib/aidash/memory-recovery/home'},
                  'capabilities': {'storage': {'existingClaim': 'objects'}}}
        failover = {'workloadKind': 'StatefulSet', 'highAvailability': {'enabled': True}}
        values = dict(self.base, execution=execution, environment={'nodeSelector': {'pool': 'execution'}}, **claims)
        objects = render(self.aidash, dict(values, **failover))
        names = ['fixture-aidash-server', 'fixture-aidash-worker', 'fixture-execution-runner']
        workloads = {value['metadata']['name']: value for value in objects if value['kind'] == 'StatefulSet'}
        self.assertEqual(sorted(workloads), names)
        self.assertFalse([value for value in objects if value['kind'] == 'Deployment' and value['metadata']['name'] in names])
        for name, workload in workloads.items():
            # One Pod at a time keeps a ReadWriteOnce claim on one Cluster Node.
            self.assertEqual(workload['spec']['replicas'], 1)
            self.assertEqual(workload['spec']['serviceName'], name)
            self.assertEqual(workload['spec']['updateStrategy'], {'type': 'RollingUpdate'})
            self.assertNotIn('strategy', workload['spec'])
        # Server and worker share their claims, so they still move together.
        for role in ('server', 'worker'):
            terms = workloads[f'fixture-aidash-{role}']['spec']['template']['spec']['affinity']['podAffinity']
            self.assertEqual(terms['requiredDuringSchedulingIgnoredDuringExecution'][0]['topologyKey'], 'kubernetes.io/hostname')
        ha = {value['metadata']['name']: value['spec'] for value in objects if value['kind'] == 'HighAvailabilityApplication'}
        self.assertEqual(sorted(ha), names)
        for spec in ha.values():
            self.assertEqual(spec['resourceSelection'], {'resourceKind': 'StatefulSet'})
            self.assertEqual(spec['policy']['storageSettings'], {'requireRegionalStorage': True})
            self.assertEqual(spec['policy']['failoverSettings'], {
                'forceDeleteStrategy': 'AfterNodeUnreachable', 'afterNodeUnreachable': {'afterNodeUnreachableSeconds': 20}})
        environment = {'postgres': POSTGRES,
                       'edge': {'hostname': 'fixture.example', 'admissionImage': 'admission' + DIGEST},
                       'activity': {'existingSecret': 'observer', 'observerImage': 'observer' + DIGEST,
                                    'collectorImage': 'collector' + DIGEST}}
        objects = render(self.environment, dict(environment, **failover))
        edge = select(objects, 'StatefulSet', '-edge')
        self.assertEqual(edge['spec']['serviceName'], 'fixture-environment-edge')
        self.assertNotIn('strategy', edge['spec'])
        self.assertEqual(sorted(value['metadata']['name'] for value in objects if value['kind'] == 'HighAvailabilityApplication'),
                         ['fixture-environment-edge', 'fixture-environment-nats', 'fixture-environment-postgres'])
        # Outside production nothing changes: Deployments and no HA resources.
        for chart, plain in ((self.aidash, values), (self.environment, environment)):
            objects = render(chart, plain)
            self.assertFalse([value for value in objects if value['kind'] == 'HighAvailabilityApplication'])
        self.assertEqual(select(render(self.environment, environment), 'Deployment', '-edge')['spec']['strategy'], {'type': 'Recreate'})

    def test_high_availability_needs_statefulsets(self):
        for chart, values in ((self.aidash, self.base), (self.environment, {'postgres': POSTGRES,
                              'edge': {'hostname': 'fixture.example', 'admissionImage': 'admission' + DIGEST},
                              'activity': {'existingSecret': 'observer', 'observerImage': 'observer' + DIGEST,
                                           'collectorImage': 'collector' + DIGEST}})):
            for invalid in ({'highAvailability': {'enabled': True}}, {'workloadKind': 'DaemonSet'}):
                with self.subTest(chart=chart, invalid=invalid), self.assertRaises(subprocess.CalledProcessError):
                    render(chart, dict(values, **invalid))

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
        self.assertEqual(runtime['scheduling']['nodeSelector'], {'aidash.run/gvisor': GVISOR['version']})
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
        # The Runner may exec into every Pod of the guard namespace. The privileged
        # installer, which mounts the host root, must live where it has no rights.
        installer_namespace = 'fixture-installer'
        self.assertEqual(select(objects, 'DaemonSet', '-installer')['metadata']['namespace'], installer_namespace)
        self.assertEqual(select(objects, 'ServiceAccount', '-installer')['metadata']['namespace'], installer_namespace)
        self.assertEqual(select(objects, 'ClusterRoleBinding', '-installer')['subjects'][0]['namespace'], installer_namespace)
        self.assertEqual(select(objects, 'Namespace', '-installer')['metadata']['labels']['pod-security.kubernetes.io/enforce'], 'privileged')
        self.assertEqual(select(objects, 'DaemonSet', '-guard')['metadata']['namespace'], 'fixture-guard')
        # Unqualified Roles belong to the release namespace ('environment').
        bound = {value['metadata'].get('namespace', 'environment') for value in objects if value['kind'] in ('Role', 'RoleBinding')}
        self.assertNotIn(installer_namespace, bound)
        self.assertEqual({value['metadata']['namespace'] for value in objects
                          if value['kind'] == 'RoleBinding' and value['metadata']['name'].endswith('-runner')},
                         {'fixture-sandbox', 'fixture-guard'})
        for clash in ('fixture-guard', 'fixture-sandbox', 'environment'):
            with self.assertRaises(subprocess.CalledProcessError):
                render(self.aidash, dict(self.base, execution=dict(execution, installerNamespace=clash),
                                         environment={'nodeSelector': {'pool': 'execution'}}))
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
        quoted = dict(execution, sandboxNamespace='true', trustedNamespace='null', installerNamespace='false',
                      runtimeClass={'create': True, 'name': 'true'})
        names = render(self.aidash, dict(self.base, execution=quoted, environment={'nodeSelector': {'pool': 'execution'}}))
        self.assertEqual(sorted(value['metadata']['name'] for value in names if value['kind'] == 'Namespace'), ['false', 'null', 'true'])
        self.assertEqual([value['metadata']['name'] for value in names if value['kind'] == 'RuntimeClass'], ['true'])
        runtime_rule = next(rule for rule in select(names, 'ClusterRole', '-runtime')['rules']
                            if rule['resources'] == ['runtimeclasses'])
        self.assertEqual(runtime_rule['resourceNames'], ['true'])
        for value in names:
            if 'namespace' in value['metadata']:
                self.assertIsInstance(value['metadata']['namespace'], str, value['kind'])
            for subject in value.get('subjects', []):
                self.assertIsInstance(subject['namespace'], str, value['kind'])
        # Runner Secret selectors and the reused journal claim stay strings too.
        names = render(self.aidash, dict(self.base, environment={'nodeSelector': {'pool': 'execution'}},
                                         execution=dict(execution, runner=dict(execution['runner'], existingSecret='true',
                                                                               tokenKey='null', journal={'existingClaim': 'true'}))))
        runner_pod = select(names, 'Deployment', '-execution-runner')['spec']['template']['spec']
        token = next(variable for variable in runner_pod['containers'][0]['env'] if variable['name'] == 'AIDASH_CORE_RUNNER_TOKEN')
        self.assertEqual(token['valueFrom']['secretKeyRef'], {'name': 'true', 'key': 'null'})
        journal = next(volume for volume in runner_pod['volumes'] if volume['name'] == 'journal')
        self.assertEqual(journal['persistentVolumeClaim']['claimName'], 'true')
        # The installer writes the version to one label and `true` to the other; one key cannot hold both.
        with self.assertRaises(subprocess.CalledProcessError):
            render(self.aidash, dict(self.base, environment={'nodeSelector': {'pool': 'execution'}},
                                     execution=dict(execution, gvisor={'installedLabel': 'aidash.run/gvisor'})))
        runner = select(objects, 'Deployment', '-execution-runner')
        self.assertEqual(runner['spec']['replicas'], 1)
        self.assertEqual(runner['spec']['strategy']['type'], 'Recreate')
        self.assertEqual(runner['spec']['template']['spec']['containers'][0]['image'], 'trusted-runner' + DIGEST)
        profile = json.loads(select(objects, 'ConfigMap', '-execution-profile')['data']['profile.json'])
        self.assertEqual(profile['processes'], 128)
        self.assertEqual(profile['host_tasks'], 512)
        self.assertNotIn('kubeconfig', profile['runner'])
        # Only this release's application reaches the Runner; other Environments
        # in a shared cluster cannot hold its unauthenticated connections.
        policy = select(objects, 'NetworkPolicy', '-execution-runner')['spec']
        self.assertEqual(policy['podSelector'], runner['spec']['selector'])
        (rule,) = policy['ingress']
        self.assertEqual(rule, {'from': [{'podSelector': {
            'matchLabels': {'app.kubernetes.io/instance': 'fixture'},
            'matchExpressions': [{'key': 'app.kubernetes.io/component', 'operator': 'In',
                                  'values': ['server', 'worker', 'migration']}]}}],
            'ports': [{'protocol': 'TCP', 'port': 8949}]})
        collector = dict(execution, runner=dict(execution['runner'], ingress={'podSelectors': [{'aidash.run/activity': 'env'}]}))
        policy = select(render(self.aidash, dict(self.base, execution=collector, environment={'nodeSelector': {'pool': 'execution'}})),
                        'NetworkPolicy', '-execution-runner')['spec']
        self.assertEqual(policy['ingress'][0]['from'][1:], [{'podSelector': {'matchLabels': {'aidash.run/activity': 'env'}}}])

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
                                 ('working_bytes', 1 << 30, True), ('working_bytes', (1 << 30) + 1, False),
                                 # The application capability runtime accepts only 1..=64 CPUs and 64 MiB or more.
                                 ('cpu', 64, True), ('cpu', 65, False),
                                 ('memory_bytes', 64 << 20, True), ('memory_bytes', (64 << 20) - 1, False)):
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
        # The controller's migration Job reaches the dependencies alongside server and worker;
        # the activity observer reaches only PostgreSQL, never the unauthenticated NATS port.
        application, observer, maintenance = select(objects, 'NetworkPolicy', '-dependencies')['spec']['ingress']
        self.assertEqual(application['from'], [{'podSelector': {'matchExpressions': [
            {'key': 'app.kubernetes.io/component', 'operator': 'In', 'values': ['server', 'worker', 'migration']}]}}])
        self.assertEqual(application['ports'], [{'protocol': 'TCP', 'port': 5432}, {'protocol': 'TCP', 'port': 4222}])
        self.assertEqual(observer['from'], [{'podSelector': {'matchLabels': {'aidash.run/activity': 'fixture'}}}])
        self.assertEqual(observer['ports'], [{'protocol': 'TCP', 'port': 5432}])
        self.assertEqual(maintenance['ports'], [{'protocol': 'TCP', 'port': 5432}])
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
        # The last meaningful request survives an admission-container restart on a small
        # emptyDir that only the admission container (not Caddy) can write.
        persisted = next(volume for volume in pod['volumes'] if volume['name'] == 'activity-state')
        self.assertEqual(persisted['emptyDir'], {'sizeLimit': '1Mi'})
        self.assertIn({'name': 'activity-state', 'mountPath': '/var/lib/aidash-activity'}, admission['volumeMounts'])
        self.assertNotIn('activity-state', [mount['name'] for mount in caddy['volumeMounts']])
        conf = select(named, 'ConfigMap', '-edge')['data']['admission.conf']
        # One stamp file, restored at start and replaced only by the serialized fold.
        self.assertEqual(conf.count('/var/lib/aidash-activity/last_active'), 1)
        self.assertEqual(conf.count('require("aidash_activity").persist()'), 2)
        self.assertEqual(conf.count('os.rename('), 1)
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

    def test_edge_proxies_only_the_selected_firebase_sign_in_helper_ahead_of_admission(self):
        values = {'postgres': POSTGRES,
            'edge': {'hostname': 'fixture.example', 'admissionImage': 'admission' + DIGEST},
            'activity': {'existingSecret': 'observer', 'observerImage': 'observer' + DIGEST,
                         'collectorImage': 'collector' + DIGEST}}

        def edge(**selected):
            objects = render(self.environment, dict(values, edge=dict(values['edge'], **selected)))
            annotations = select(objects, 'Deployment', '-edge')['spec']['template']['metadata']['annotations']
            return select(objects, 'ConfigMap', '-edge')['data']['Caddyfile'], annotations['checksum/config']

        plain, plain_checksum = edge()
        self.assertIn(
            'fixture.example {\n'
            '  header >Referrer-Policy no-referrer\n'
            '  header +Content-Security-Policy "frame-ancestors \'none\'"\n'
            '  header >X-Frame-Options DENY\n'
            '  reverse_proxy 127.0.0.1:8088\n'
            '}',
            plain,
        )
        self.assertIn('@origin host fixture.example', plain)
        self.assertIn('respond 404', plain)
        self.assertEqual(edge(firebaseProject=''), (plain, plain_checksum))
        proxied, proxied_checksum = edge(firebaseProject='aidash-fixture')
        # Switching the helper restarts the edge so Caddy reads the new routes.
        self.assertNotEqual(proxied_checksum, plain_checksum)
        helper, application = proxied.split('  handle {\n')
        self.assertIn('@firebase_helper path /__/auth/* /__/firebase/init.json', helper)
        self.assertIn('handle @firebase_helper {', helper)
        self.assertIn('reverse_proxy https://aidash-fixture.firebaseapp.com {', helper)
        for directive in ('header_up Host {upstream_hostport}', 'header_up -Cookie',
                          'header_up -Authorization', 'header_down -Set-Cookie'):
            self.assertIn(directive, helper)
        self.assertNotIn('127.0.0.1:8088', helper)
        self.assertIn('reverse_proxy 127.0.0.1:8088', application)
        for invalid in ('Aidash-Fixture', 'aidash-fixture.evil.example {', 'x'):
            with self.subTest(project=invalid), self.assertRaises(subprocess.CalledProcessError):
                edge(firebaseProject=invalid)

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
                              policy['ingress'][1]['from'])
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


    def test_preview_gateway_renders_apart_with_a_wildcard_ingress_and_sandbox_policy(self):
        values = {**self.base,
                  'preview': {'enabled': True, 'domain': 'aidashusercontent.app', 'inProcess': False,
                              'port': 8960, 'publicPort': 443, 'podPort': 3000,
                              'ingress': {'enabled': True, 'className': 'nginx', 'tlsSecret': 'preview-tls'}},
                  'image': {'repository': 'aidash', 'digest': 'sha256:' + 'a' * 64},
                  'execution': {'sandboxNamespace': 'fixture-sandbox'}}
        objects = render(self.aidash, values)
        service = select(objects, 'Service', '-preview')
        self.assertEqual(service['spec']['selector']['app.kubernetes.io/component'], 'preview')
        self.assertEqual(service['spec']['ports'], [{'name': 'preview', 'port': 8960, 'targetPort': 8960}])
        deployment = select(objects, 'Deployment', '-preview')
        container = deployment['spec']['template']['spec']['containers'][0]
        self.assertEqual(container['args'], ['preview-gateway'])
        env = {item['name']: item.get('value') for item in container['env']}
        self.assertEqual(env['AIDASH_PREVIEW_DOMAIN'], 'aidashusercontent.app')
        self.assertEqual(env['AIDASH_PREVIEW_LISTEN'], '0.0.0.0:8960')
        self.assertEqual(env['AIDASH_PREVIEW_IN_PROCESS'], 'false')
        self.assertEqual(env['AIDASH_PREVIEW_PUBLIC_PORT'], '443')
        self.assertTrue(container['securityContext']['readOnlyRootFilesystem'])
        ingress = select(objects, 'Ingress', '-preview')
        self.assertEqual(ingress['spec']['rules'][0]['host'], '*.aidashusercontent.app')
        self.assertEqual(ingress['spec']['tls'][0]['secretName'], 'preview-tls')
        self.assertEqual(ingress['spec']['ingressClassName'], 'nginx')
        policy = next(value for value in objects if value['kind'] == 'NetworkPolicy'
                      and value['metadata']['name'] == 'fixture-preview-sandbox')
        self.assertEqual(policy['metadata']['namespace'], 'fixture-sandbox')
        self.assertEqual(policy['spec']['podSelector']['matchLabels'], {'aidash-sandbox': 'true'})
        self.assertEqual(policy['spec']['ingress'][0]['ports'], [{'protocol': 'TCP', 'port': 3000}])
        self.assertEqual(policy['spec']['ingress'][0]['from'][0]['podSelector']['matchLabels'],
                         {'app.kubernetes.io/instance': 'fixture',
                          'app.kubernetes.io/component': 'preview'})

    def test_in_process_preview_gateway_reaches_no_separate_workload(self):
        values = {**self.base, 'preview': {'enabled': True, 'domain': 'localhost', 'inProcess': True}}
        objects = render(self.aidash, values)
        service = select(objects, 'Service', '-preview')
        self.assertEqual(service['spec']['selector']['app.kubernetes.io/component'], 'server')
        self.assertFalse([value for value in objects if value['kind'] == 'Deployment'
                          and value['metadata']['name'] == 'fixture-preview'])
        server = select(objects, 'Deployment', '-server')
        env = {item['name']: item.get('value')
               for item in server['spec']['template']['spec']['containers'][0]['env']}
        self.assertEqual(env['AIDASH_PREVIEW_IN_PROCESS'], 'true')
        policy = next(value for value in objects if value['kind'] == 'NetworkPolicy'
                      and value['metadata']['name'] == 'fixture-preview-sandbox')
        self.assertEqual(policy['spec']['ingress'][0]['from'][0]['podSelector']['matchLabels']
                         ['app.kubernetes.io/component'], 'server')

class MetadataServerEgressTest(unittest.TestCase):
    """The GKE metadata server (169.254.169.254) is reachable only from server and worker."""
    # The metadata server and the address gke-metadata-server DNATs it to (169.254.169.252:988):
    # Dataplane V2 enforces policy before the DNAT, Calico after it, so both must be blocked.
    metadata = tuple(ipaddress.ip_address(address) for address in ('169.254.169.254', '169.254.169.252'))
    workloads = ('-aidash-server', '-aidash-worker', '-frontend', '-execution-runner', '-installer', '-guard',
                 '-environment-postgres', '-environment-nats', '-environment-edge', '-environment-activity')
    allowlist = frozenset({'server', 'worker'})
    label = 'aidash.run/metadata-server'
    aidash = ChartsTest.aidash
    environment = ChartsTest.environment

    @staticmethod
    def pod_templates(objects, namespace='environment'):
        """Yields (kind, name, namespace, pod template) for every workload that creates Pods."""
        for value in objects:
            spec = value.get('spec', {})
            if value['kind'] in ('Deployment', 'StatefulSet', 'DaemonSet', 'Job', 'ReplicaSet'):
                template = spec['template']
            elif value['kind'] == 'CronJob':
                template = spec['jobTemplate']['spec']['template']
            elif value['kind'] == 'Pod':
                template = value
            else:
                continue
            yield value['kind'], value['metadata']['name'], value['metadata'].get('namespace', namespace), template

    @staticmethod
    def selects(selector, labels):
        """Kubernetes label selector semantics, including that NotIn/DoesNotExist match absent keys."""
        if any(labels.get(key) != value for key, value in selector.get('matchLabels', {}).items()):
            return False
        for expression in selector.get('matchExpressions', []):
            key, operator, values = expression['key'], expression['operator'], expression.get('values', [])
            present = key in labels
            if not {'In': present and labels[key] in values, 'NotIn': not present or labels[key] not in values,
                    'Exists': present, 'DoesNotExist': not present}[operator]:
                return False
        return True

    def everything(self):
        digest = '@sha256:' + 'b' * 64
        execution = {'createNamespaces': True, 'sandboxImage': 'sandbox' + digest, 'runtimeClass': {'create': True},
                     'runner': {'enabled': True, 'existingSecret': 'runner', 'image': 'runner' + digest},
                     'guard': {'enabled': True, 'image': 'guard' + digest},
                     'installer': {'enabled': True, 'image': 'installer' + digest}}
        application = render(self.aidash, dict(ChartsTest.base, execution=execution,
                                               memoryRecovery={'existingClaim': 'ledger'},
                                               environment={'nodeSelector': {'pool': 'execution'}}))
        dependencies = render(self.environment, {
            'postgres': POSTGRES, 'edge': {'hostname': 'fixture.example', 'admissionImage': 'admission' + digest},
            'activity': {'existingSecret': 'observer', 'observerImage': 'observer' + digest,
                         'collectorImage': 'collector' + digest}})
        return application, dependencies

    def test_policy_selects_every_workload_except_server_and_worker(self):
        application, dependencies = self.everything()
        objects = application + dependencies
        policies = [(value['metadata'].get('namespace', 'environment'), value['spec'])
                    for value in objects if value['kind'] == 'NetworkPolicy' and 'metadata-egress' in value['metadata']['name']]
        # One in the release namespace per chart, plus the guard and installer namespaces.
        self.assertEqual(sorted(namespace for namespace, _ in policies),
                         ['environment', 'environment', 'fixture-guard', 'fixture-installer'])
        seen = set()
        for kind, name, namespace, template in self.pod_templates(objects):
            labels = template['metadata'].get('labels', {})
            covered = any(policy_namespace == namespace and self.selects(spec['podSelector'], labels)
                          for policy_namespace, spec in policies)
            # The allowlist is named by workload, not read back from the labels under test.
            allowed = kind == 'Deployment' and re.search(r'-aidash-(server|worker)$', name) is not None
            with self.subTest(workload=f'{kind}/{name}'):
                self.assertEqual(covered, not allowed)
            seen.add((kind, name))
        # Every Pod kind of both charts took part: the check cannot pass vacuously.
        for expected in self.workloads:
            self.assertTrue(any(name.endswith(expected) for _, name in seen), expected)
        self.assertEqual({name.rsplit('-aidash-', 1)[1] for _, name in seen if name.endswith(('-aidash-server', '-aidash-worker'))},
                         self.allowlist)

    def test_policy_denies_only_the_metadata_server_and_nothing_else_grants_egress(self):
        application, dependencies = self.everything()
        for value in application + dependencies:
            if value['kind'] != 'NetworkPolicy':
                continue
            spec = value['spec']
            if 'metadata-egress' not in value['metadata']['name']:
                # Egress rules are additive: any other policy allowing egress would re-open the address.
                self.assertNotIn('Egress', spec['policyTypes'], value['metadata']['name'])
                continue
            self.assertEqual(spec['policyTypes'], ['Egress'])
            reachable = set()
            for rule in spec['egress']:
                self.assertTrue(rule.get('to'), 'a rule without "to" allows every destination')
                for peer in rule['to']:
                    if 'ipBlock' not in peer:
                        # Pod selectors only name in-cluster Pods, never the metadata server.
                        self.assertEqual(peer, {'namespaceSelector': {}})
                        continue
                    block = peer['ipBlock']
                    excepted = [ipaddress.ip_network(cidr) for cidr in block.get('except', [])]
                    reachable |= {str(address) for address in self.metadata
                                  if address in ipaddress.ip_network(block['cidr']) and not any(address in n for n in excepted)}
            self.assertEqual(reachable, set(), value['metadata']['name'])

    def test_both_charts_render_the_same_policy_spec(self):
        application, dependencies = self.everything()
        specs = [next(value['spec'] for value in objects if value['kind'] == 'NetworkPolicy'
                      and value['metadata']['name'].endswith('-metadata-egress') and 'namespace' not in value['metadata'])
                 for objects in (application, dependencies)]
        # The two charts keep separate copies of the template so each installs on its own.
        self.assertEqual(specs[0], specs[1])

    def test_server_and_worker_keep_the_allowlist_label_without_the_policy(self):
        objects = render(self.aidash, dict(ChartsTest.base, metadataServerEgress={'enabled': False}))
        self.assertEqual([value for value in objects if value['kind'] == 'NetworkPolicy'], [])
        for role in ('server', 'worker'):
            labels = select(objects, 'Deployment', '-aidash-' + role)['spec']['template']['metadata']['labels']
            self.assertEqual(labels[self.label], 'allowed')
        environment = render(self.environment, {
            'metadataServerEgress': {'enabled': False}, 'postgres': POSTGRES,
            'edge': {'hostname': 'fixture.example', 'admissionImage': 'admission@sha256:' + 'b' * 64},
            'activity': {'enabled': False}})
        self.assertFalse(any('metadata-egress' in value['metadata']['name'] for value in environment))

    def test_no_aidash_pod_uses_the_host_network(self):
        application, dependencies = self.everything()
        seen = []
        for kind, name, _, template in self.pod_templates(application + dependencies):
            with self.subTest(workload=f'{kind}/{name}'):
                # Explicit false: a Pod on the node's network namespace bypasses every NetworkPolicy
                # and the Workload Identity metadata server, receiving the node's identity.
                self.assertIs(template['spec'].get('hostNetwork'), False)
            seen.append(name)
        # Every workload of both charts was checked: the loop cannot pass vacuously.
        for expected in self.workloads:
            self.assertTrue(any(name.endswith(expected) for name in seen), expected)

    def test_selector_semantics_match_kubernetes(self):
        selector = {'matchExpressions': [{'key': self.label, 'operator': 'DoesNotExist'}]}
        self.assertTrue(self.selects(selector, {}))
        self.assertTrue(self.selects(selector, {'app.kubernetes.io/component': 'edge'}))
        self.assertFalse(self.selects(selector, {self.label: 'allowed'}))
        self.assertFalse(self.selects(selector, {self.label: ''}))


class ActivityTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        spec = importlib.util.spec_from_file_location('activity', ROOT / 'infra/gcp/helm/environment/files/activity.py')
        cls.activity = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.activity)

    def summary(self, receipts):
        # Stand-in for the observer: a fixed-size count and digest of the receipt IDs.
        return {'completed_transfers_count': len(receipts), 'completed_transfers_digest': self.activity.digest(receipts)}

    def combine(self, previous, counts=None, transfers=None, completion=0):
        return self.activity.combine(
            dict(self.summary(transfers or []), protocol='aidash-infra-activity/1', counts=counts or {},
                 last_work_completed=completion), {'inflight': 0, 'last_active': 0, 'closed': False},
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
        database = dict(self.summary([]), protocol='aidash-infra-activity/1', counts={'runs': 0},
                        last_work_completed=0)
        snapshots = [('7', {'protocol': 'aidash-infra-activity/1', 'busy': True, 'observed_at': 9900}),
                     ('8', {'protocol': 'aidash-infra-activity/1', 'busy': False, 'observed_at': 9990,
                            'last_active': 5000, 'completed_transfers_digest': self.activity.digest([])})]
        environment = {'KUBERNETES_SERVICE_HOST': 'api', 'KUBERNETES_SERVICE_PORT': '443',
                       'ACTIVITY_CONFIGMAP': 'activity', 'EDGE_ACTIVITY_ENDPOINT': 'http://edge',
                       'RUNNER_ENDPOINT': 'http://runner', 'AIDASH_CORE_RUNNER_TOKEN': 'x' * 32}
        for conflicts, code in ((1, 409), (3, 409), (1, 500)):
            with self.subTest(conflicts=conflicts, code=code):
                reads, patches = [], []

                # Bind this subTest's state as defaults; a closure would read the loop's latest values.
                def request(url, token=None, *, data=None, method=None, context=None,
                            reads=reads, patches=patches, conflicts=conflicts, code=code):
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
        # The pair is the renewal key: an unchanged digest with another count renews.
        self.assertEqual(self.combine(dict(settled, completed_transfers_count=3), transfers=['a', 'b'])['last_active'], 10000)
        legacy = dict(previous, completed_transfers=['a', 'b'])
        self.assertEqual(self.combine(legacy, transfers=['b', 'a'])['last_active'], 5000)
        self.assertEqual(self.combine(legacy, transfers=['a', 'b', 'c'])['last_active'], 10000)

    def test_a_very_large_receipt_set_keeps_the_observation_and_snapshot_bounded(self):
        previous = {'observed_at': 9950, 'last_active': 5000, 'busy': False}
        receipts = [f'{n:036d}' for n in range(50000)]
        settled = dict(self.combine(previous, transfers=receipts), last_active=5000, observed_at=9950)
        self.assertLess(len(json.dumps(self.summary(receipts))), 512)
        self.assertLess(len(json.dumps(settled)), 2048)
        self.assertEqual(self.combine(settled, transfers=receipts)['last_active'], 5000)
        # One new receipt out of fifty thousand still renews the idle interval.
        renewed = self.combine(settled, transfers=receipts + ['f' * 36])
        self.assertEqual(renewed['last_active'], 10000)
        self.assertEqual(renewed['completed_transfers_count'], 50001)

    def test_an_observation_that_is_not_the_bounded_contract_is_unavailable(self):
        valid = dict(self.summary(['a']), protocol='aidash-infra-activity/1', counts={})
        edge, runner = {'inflight': 0, 'last_active': 0, 'closed': False}, {'protocol': 'aidash-runner-activity/1', 'counts': {}}
        previous = {'observed_at': 9950, 'last_active': 5000, 'busy': False}
        self.activity.combine(valid, edge, runner, previous, 10000)
        legacy = dict(valid, completed_transfers=['a'])
        for name, database in (
                ('receipt list beside a summary', legacy),
                ('receipt list without a summary', {key: value for key, value in legacy.items() if not key.startswith('completed_transfers_')}),
                ('missing digest', {key: value for key, value in valid.items() if key != 'completed_transfers_digest'}),
                ('missing count', {key: value for key, value in valid.items() if key != 'completed_transfers_count'}),
                ('negative count', dict(valid, completed_transfers_count=-1)),
                ('boolean count', dict(valid, completed_transfers_count=True)),
                ('short digest', dict(valid, completed_transfers_digest='abc')),
                ('uppercase digest', dict(valid, completed_transfers_digest='A' * 64))):
            with self.subTest(name), self.assertRaises((KeyError, ValueError)):
                self.activity.combine(database, edge, runner, previous, 10000)

    def test_an_oversized_observation_file_publishes_a_busy_snapshot(self):
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
        database = dict(self.summary([]), protocol='aidash-infra-activity/1', counts={}, padding='x' * 70000)
        with (patch.dict(os.environ, environment),
              patch.object(self.activity, 'request', request),
              patch.object(self.activity.ssl, 'create_default_context'),
              patch.object(Path, 'read_text', return_value='value'),
              patch.object(Path, 'read_bytes', return_value=json.dumps(database).encode())):
            self.activity.main()
        snapshot = json.loads(patches[0]['data']['snapshot.json'])
        self.assertTrue(snapshot['busy'])
        self.assertTrue(snapshot['observation_gap'])

if __name__ == '__main__':
    unittest.main()
