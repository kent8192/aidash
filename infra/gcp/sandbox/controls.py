#!/usr/bin/env python3
"""Executable sandbox controls. Each phase is separate; no phase runs implicitly.

Review the source and the generated alert command bundle before authorizing any
live invocation. All captured credentials/bodies stay in memory or the private
run directory, never stdout. Uses the operator's ADC/gcloud login for fixture
administration; tested workload calls explicitly impersonate the discovered SA.
"""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

from runner import Refused, NoRedirect, cloud, command, owned, request, require, save, stamp, validate


def execute(argv, stdin=None):
    try:
        p = subprocess.run(argv, input=stdin, capture_output=True, timeout=120)
    except (OSError, subprocess.TimeoutExpired):
        raise Refused('control command interrupted; inspect lifecycle before retry') from None
    require(p.returncode == 0, 'control command failed; raw output suppressed')
    return p.stdout


def select_resources(m, bootstrap, environments, pool, service):
    """Translate applied root and actual node-pool outputs, never guessed SAs."""
    app = m['projects']['app']['id']
    env = environments['environments']['value']['test']
    config = bootstrap['configuration']['value']
    require(config['project_id'] == app and env['byok_project_id'] == m['projects']['byok']['id'], 'output project mismatch')
    bindings = env['gcip']['tenant_bindings']
    tenant = lambda label: next(key for key, value in bindings.items() if value == label)
    return dict(server=env['server_service_account'], worker=env['worker_service_account'],
        node=pool['config']['serviceAccount'], node_pool_service_account=pool['config']['serviceAccount'],
        node_pool=env['node_pool'], broker=config['broker_service_accounts']['test'], deploy=config['deploy_service_account'],
        control=f'aidash-sandbox-control@{app}.iam.gserviceaccount.com', delegate_target=f'aidash-sandbox-target@{app}.iam.gserviceaccount.com', own_tenant=tenant('sandbox-own'),
        other_tenant=tenant('sandbox-other'), dummy_user='sandbox-' + m['run_id'][:12],
        dummy_email='sandbox-' + m['run_id'][:12] + '@example.invalid', broker_url=service['status']['url'],
        broker_revision=service['status']['latestReadyRevisionName'], origin='terraform-output-and-current-node-pool',
        source_sha=m['source_sha'])


class Controls:
    def __init__(self, path):
        self.path = Path(path).resolve()
        self.directory = self.path.parent
        self.m = json.loads(self.path.read_text())
        validate(self.m)
        require(self.m['status'] == 'created', 'active created sandbox required')
        for p in self.m['projects'].values():
            require(owned(self.m, p)['lifecycleState'] == 'ACTIVE', 'sandbox not active')
        self.app = self.m['projects']['app']['id']
        self.byok = self.m['projects']['byok']['id']
        self.r = json.loads((self.directory / 'resources.json').read_text()) if (self.directory / 'resources.json').exists() else {}
        if self.r:
            for key in ('server', 'worker', 'node', 'broker', 'deploy', 'control'):
                require(re.fullmatch(r'[a-z][a-z0-9-]+@' + re.escape(self.app) + r'\.iam\.gserviceaccount\.com', self.r[key]), 'foreign principal')
            for key in ('own_tenant', 'other_tenant', 'dummy_user'):
                require(re.fullmatch(r'[A-Za-z0-9_-]+', self.r[key]), 'invalid fixture id')

    def record(self, case, target, observed, **metadata):
        path = self.directory / 'controls-evidence.json'
        rows = json.loads(path.read_text()) if path.exists() else []
        require(len(rows) < 1000, 'control journal limit')
        # Callers select scalars/booleans only; never pass a raw response here.
        rows.append(dict(case=case, target=target, observed=observed, source_sha=self.m['source_sha'],
                         run_id=self.m['run_id'], timestamp=stamp(),
                         control_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                         resources_sha256=hashlib.sha256(json.dumps(self.r, sort_keys=True).encode()).hexdigest(), **metadata))
        save(path, rows)

    def token(self, identity=None):
        args = ['gcloud', 'auth', 'print-access-token', '--project=' + self.app, '--quiet']
        if identity:
            args.append('--impersonate-service-account=' + identity)
        return command(args).decode().strip()

    def api(self, host, resource, method='POST', body=None, identity=None):
        require(host in ('identitytoolkit.googleapis.com', 'cloudresourcemanager.googleapis.com', 'iam.googleapis.com'), 'unsupported control API')
        require(resource.startswith(('v1/projects/' + self.app + '/', 'admin/v2/projects/' + self.app + '/',
            'v1/projects/' + self.byok + ':')), 'foreign API project')
        spec = dict(method=method, url='https://' + host + '/' + resource)
        if body is not None:
            spec['body'] = body
        status, result = request(spec, self.token(identity), self.app)
        require(200 <= status < 300 and isinstance(result, dict), 'control API failed')
        return result

    def discover(self, args):
        root = self.directory / 'source/infra/gcp'
        read = lambda name: json.loads(command(['terraform', '-chdir=' + str(root / name), 'output', '-json']))
        bootstrap, environments = read('bootstrap'), read('environments')
        pool_name = environments['environments']['value']['test']['node_pool']
        pool = cloud(self.app, 'container', 'node-pools', 'describe', pool_name, '--cluster=aidash', '--zone=us-central1-a')
        service = cloud(self.app, 'run', 'services', 'describe', 'aidash-test-credential-broker', '--region=us-central1')
        self.r = select_resources(self.m, bootstrap, environments, pool, service)
        save(self.directory / 'resources.json', self.r)
        self.record('discovery', self.app, 'mapped', node_pool=pool_name, node_identity=self.r['node'])

    def kubeconfig(self, args):
        previous = os.environ.get('KUBECONFIG')
        os.environ['KUBECONFIG'] = str(self.directory / 'kubeconfig')
        try:
            command(['gcloud', 'container', 'clusters', 'get-credentials', 'aidash', '--zone=us-central1-a', '--dns-endpoint', '--project=' + self.app])
        finally:
            if previous is None:
                os.environ.pop('KUBECONFIG', None)
            else:
                os.environ['KUBECONFIG'] = previous
        self.kube(args, 'get', 'namespace', 'aidash-test', '-o', 'name')
        self.record('kubeconfig', self.app + '/us-central1-a/aidash', 'verified')

    def fixtures(self, args):
        require(re.fullmatch(r'(user|serviceAccount):[^\s]+@[^\s]+', args.operator_member or ''), 'explicit operator IAM member required')
        for account in ('aidash-sandbox-control', 'aidash-sandbox-target'):
            cloud(self.app, 'iam', 'service-accounts', 'create', account, '--display-name=Disposable sandbox control')
        # Grant only the operator the ability to exercise the existing identities.
        # Deploy is never made a broker token creator by this fixture setup.
        for key in ('server', 'worker', 'node', 'broker', 'deploy', 'control'):
            cloud(self.app, 'iam', 'service-accounts', 'add-iam-policy-binding', self.r[key],
                  '--member=' + args.operator_member, '--role=roles/iam.serviceAccountTokenCreator')
        for secret in ('aidash-test-cred-dummy', 'aidash-other-cred-dummy', 'aidash-test-cred-management'):
            cloud(self.byok, 'secrets', 'create', secret, '--replication-policy=automatic', '--labels=aidash-sandbox-run=' + self.m['run_id'])
            execute(['gcloud', 'secrets', 'versions', 'add', secret, '--data-file=-', '--project=' + self.byok, '--quiet'], b'aidash-sandbox-dummy')
        password = secrets.token_urlsafe(32)
        save(self.directory / 'dummy-login.json', {'email': self.r['dummy_email'], 'password': password})
        for key in ('own_tenant', 'other_tenant'):
            resource = f'v1/projects/{self.app}/tenants/{self.r[key]}/accounts:'
            result = self.api('identitytoolkit.googleapis.com', resource + 'batchCreate', body={'users': [{'localId': self.r['dummy_user'], 'email': self.r['dummy_email'], 'emailVerified': True}]})
            require(not result.get('error'), 'dummy import failed')
            self.api('identitytoolkit.googleapis.com', resource + 'update', body={'localId': self.r['dummy_user'], 'password': password})
        resource = f'admin/v2/projects/{self.app}/tenants/{self.r["own_tenant"]}'
        policy = self.api('identitytoolkit.googleapis.com', resource + ':getIamPolicy', body={})
        save(self.directory / 'tenant-policy-baseline.json', policy)
        policy.setdefault('bindings', []).append({'role': 'roles/identityplatform.viewer', 'members': ['serviceAccount:' + self.r['server']]})
        self.api('identitytoolkit.googleapis.com', resource + ':setIamPolicy', body={'policy': policy})
        self.record('fixtures', self.app, 'created', operator_grant_cleanup='project_shutdown')

    def identities(self, args):
        for name in ('server', 'worker', 'node', 'broker', 'deploy', 'control'):
            identity = self.r[name]
            # Tokeninfo is called privately over TLS. Token-containing URL and
            # response are never included in diagnostics or evidence.
            token = self.token(identity)
            req = urllib.request.Request('https://oauth2.googleapis.com/tokeninfo?' + urllib.parse.urlencode({'access_token': token}))
            with urllib.request.build_opener(NoRedirect()).open(req, timeout=30) as result:
                info = json.loads(result.read(65536))
            require(info.get('email') == identity and int(info.get('expires_in', 0)) > 0, 'token identity mismatch or expiry')
            self.record('identity-' + name, identity, 'verified', expires_in_positive=int(info.get('expires_in', 0)) > 0)

    def manage(self, args):
        identity = '--impersonate-service-account=' + self.r['server']
        secret = 'aidash-test-cred-management'
        raw = execute(['gcloud', 'secrets', 'versions', 'add', secret, '--data-file=-', '--project=' + self.byok, identity, '--format=json', '--quiet'], b'aidash-sandbox-dummy')
        version = json.loads(raw)['name'].rsplit('/', 1)[1]
        for verb, expected in [('disable', 'DISABLED'), ('destroy', 'DESTROYED')]:
            cloud(self.byok, 'secrets', 'versions', verb, version, '--secret=' + secret, identity)
            actual = cloud(self.byok, 'secrets', 'versions', 'describe', version, '--secret=' + secret, identity)
            self.record('manage-' + verb, actual['name'], actual.get('state'))
            require(actual.get('state') == expected, 'version state transition failed')
        cloud(self.byok, 'secrets', 'delete', secret, identity)
        self.record('manage-delete', 'projects/' + self.byok + '/secrets/' + secret, 'delete_requested')

    def kms(self, args):
        message, signature, public = (self.directory / name for name in ('kms-message.bin', 'kms-signature.bin', 'kms-public.pem'))
        message.write_bytes(b'aidash-sandbox-dummy')
        common = ['--project=' + self.app, '--location=us-central1', '--keyring=aidash-test-capability', '--key=capability']
        try:
            execute(['gcloud', 'kms', 'asymmetric-sign', '--version=1', *common, '--input-file=' + str(message), '--signature-file=' + str(signature), '--impersonate-service-account=' + self.r['worker'], '--quiet'])
            public.write_bytes(execute(['gcloud', 'kms', 'keys', 'versions', 'get-public-key', '1', *common]))
            execute(['openssl', 'pkeyutl', '-verify', '-pubin', '-inkey', str(public), '-rawin', '-in', str(message), '-sigfile', str(signature)])
            self.record('kms-verify', f'projects/{self.app}/locations/us-central1/keyRings/aidash-test-capability/cryptoKeys/capability/cryptoKeyVersions/1', 'signature_verified', identity=self.r['worker'])
        finally:
            signature.unlink(missing_ok=True)
            message.unlink(missing_ok=True)

    def kube(self, args, *words):
        context = f'gke_{self.app}_us-central1-a_aidash'
        return execute(['kubectl', '--kubeconfig=' + str(self.directory / 'kubeconfig'), '--context=' + context, '--request-timeout=30s', *words])

    def pods(self, args):
        require(re.fullmatch(r'[^\s]+@sha256:[a-f0-9]{64}', args.probe_image or ''), 'reviewed curl image digest required')
        require(urllib.parse.urlsplit(self.r['broker_url']).hostname.endswith('.run.app'), 'unexpected broker host')
        pods = []
        for namespace in ('aidash-test', 'aidash-test-sandbox', 'aidash-test-trusted', 'aidash-test-installer'):
            data = json.loads(self.kube(args, '-n', namespace, 'get', 'pods', '-o', 'json'))
            pods += [(namespace, pod) for pod in data['items'] if pod['status'].get('phase') == 'Running']
        require(any(pod['metadata'].get('labels', {}).get('aidash.run/runner') for _, pod in pods), 'current Runner control missing')
        require(pods and len(pods) <= 30, 'bounded running workload inventory required')
        components = {pod['metadata'].get('labels', {}).get('app.kubernetes.io/component') for _, pod in pods}
        require({'server', 'worker'} <= components, 'real server and worker positive controls required')
        require(all(any(ns == expected for ns, _ in pods) for expected in ('aidash-test-sandbox', 'aidash-test-trusted', 'aidash-test-installer')), 'execution/guard/installer workload controls missing')
        for namespace, pod in pods:
            labels = pod['metadata'].get('labels', {})
            allowed = labels.get('aidash.run/metadata-server') == 'allowed'
            uri = 'http://169.254.169.254/computeMetadata/v1/instance/service-accounts/default/email'
            def fetch(url, metadata=False):
                name = 'iam-probe-' + secrets.token_hex(4)
                curl = ['curl', '--silent', '--max-time', '15', '--write-out', '\n%{http_code}']
                if metadata:
                    curl += ['--header', 'Metadata-Flavor: Google']
                argv = ['kubectl', '--kubeconfig=' + str(self.directory / 'kubeconfig'), '--context=' + f'gke_{self.app}_us-central1-a_aidash', '-n', namespace,
                        'debug', pod['metadata']['name'], '--container=' + name, '--image=' + args.probe_image, '--profile=restricted', '--quiet', '--attach=true', '--', *curl, url]
                try:
                    result = subprocess.run(argv, capture_output=True, timeout=45)
                except subprocess.TimeoutExpired:
                    raise Refused('debug attachment timeout is not metadata denial') from None
                # Missing attach/image/exec support is never a network denial.
                data = result.stdout.decode(errors='replace').rstrip('\r\n').rsplit('\n', 1)
                require(len(data) == 2 and re.fullmatch('[0-9]{3}', data[-1]), 'probe did not execute curl')
                return int(data[-1]), data[0]
            status, body = fetch(uri, True)
            self.record('metadata', namespace + '/' + pod['metadata']['name'], 'allowed' if status == 200 else 'blocked',
                pod_uid=pod['metadata']['uid'], expected_allowed=allowed, http_status=status)
            require((status == 200) == allowed and status in (0, 200), 'metadata policy mismatch or unrelated rejection')
            component = labels.get('app.kubernetes.io/component')
            if allowed and component in ('server', 'worker'):
                require(body.strip() == self.r[component], 'metadata returned the wrong workload identity')
            if labels.get('app.kubernetes.io/component') in ('server', 'worker'):
                marker = secrets.token_hex(12)
                url = self.r['broker_url'] + '/api/v1/chat/completions?aidash_sandbox_probe=' + marker
                status, body = fetch(url)
                try:
                    application_rejection = json.loads(body)['error']['code'] == 'capability_invalid_signature'
                except (ValueError, KeyError, TypeError):
                    application_rejection = False
                require(status == 401 and application_rejection, 'Pod did not reach broker capability boundary')
                self.record('broker-internal', namespace + '/' + pod['metadata']['name'], 'capability_rejection', http_status=status, marker=marker, revision=self.r['broker_revision'])
                self.collect_request_log(marker)
        marker = secrets.token_hex(12)
        spec = {'method': 'GET', 'url': self.r['broker_url'] + '/api/v1/chat/completions?aidash_sandbox_probe=' + marker}
        # No capability or provider key is sent; broker exits before any upstream.
        status, body = request(spec, '', self.app)
        require(status in (403, 404) and body.get('error', {}).get('code') != 'capability_invalid_signature', 'internet ingress boundary not established')
        self.record('broker-internet', self.r['broker_url'] + '/api/v1/chat/completions', 'ingress_rejected', http_status=status, marker=marker)

    def collect_request_log(self, marker):
        query = 'resource.type="cloud_run_revision" AND resource.labels.service_name="aidash-test-credential-broker" AND httpRequest.requestUrl:"' + marker + '"'
        logs = cloud(self.app, 'logging', 'read', query, '--limit=10', '--freshness=15m')
        matches = [row for row in logs if row.get('resource', {}).get('labels', {}).get('revision_name') == self.r['broker_revision'] and row.get('httpRequest', {}).get('status') == 401]
        self.record('broker-request-log', 'aidash-test-credential-broker', 'matched' if matches else 'pending_log', marker=marker, insert_ids=[row.get('insertId') for row in matches])
        require(matches, 'request log pending; rerun log collection with marker')

    def deny_fixtures(self, args):
        baseline_path = self.directory / 'deny-policies-baseline.json'
        require(not baseline_path.exists(), 'restore previous deny fixtures before setup')
        identities = [self.r['broker'], self.r['control'], self.r['delegate_target']]
        baseline = {identity: cloud(self.app, 'iam', 'service-accounts', 'get-iam-policy', identity) for identity in identities}
        save(baseline_path, baseline)
        save(self.directory / 'deny-keys-baseline.json', cloud(self.app, 'iam', 'service-accounts', 'keys', 'list', '--iam-account=' + self.r['control'], '--managed-by=user'))
        for identity in (self.r['broker'], self.r['control']):
            for role in ('roles/iam.serviceAccountTokenCreator', 'roles/iam.serviceAccountKeyAdmin'):
                cloud(self.app, 'iam', 'service-accounts', 'add-iam-policy-binding', identity,
                      '--member=serviceAccount:' + self.r['deploy'], '--role=' + role)
        for identity in (self.r['broker'], self.r['control']):
            cloud(self.app, 'iam', 'service-accounts', 'add-iam-policy-binding', self.r['delegate_target'],
                  '--member=serviceAccount:' + identity, '--role=roles/iam.serviceAccountTokenCreator')
        self.record('deny-fixtures', self.app, 'temporary_allow_substrate_restore_required')

    def restore_deny(self, args):
        file = self.directory / 'deny-policies-baseline.json'
        baseline = json.loads(file.read_text())
        require(set(baseline) == {self.r['broker'], self.r['control'], self.r['delegate_target']}, 'unexpected restore targets')
        for identity, policy in baseline.items():
            latest = cloud(self.app, 'iam', 'service-accounts', 'get-iam-policy', identity)
            target = self.directory / 'restore-policy.json'
            save(target, {**policy, 'etag': latest['etag']})
            cloud(self.app, 'iam', 'service-accounts', 'set-iam-policy', identity, str(target))
            restored = cloud(self.app, 'iam', 'service-accounts', 'get-iam-policy', identity)
            def bindings(value):
                return sorted(json.dumps({**binding, 'members': sorted(binding.get('members', []))}, sort_keys=True) for binding in value.get('bindings', []))
            require(bindings(restored) == bindings(policy), 'temporary grant removal not verified')
            self.record('deny-policy-restored', identity, 'exact_bindings_verified', baseline_sha256=hashlib.sha256(json.dumps(policy, sort_keys=True).encode()).hexdigest())
        original_keys = {key['name'] for key in json.loads((self.directory / 'deny-keys-baseline.json').read_text())}
        for key in cloud(self.app, 'iam', 'service-accounts', 'keys', 'list', '--iam-account=' + self.r['control'], '--managed-by=user'):
            if key['name'] not in original_keys:
                cloud(self.app, 'iam', 'service-accounts', 'keys', 'delete', key['name'].rsplit('/', 1)[1], '--iam-account=' + self.r['control'])
        file.rename(self.directory / ('deny-policies-restored-' + str(int(time.time())) + '.json'))
        self.record('deny-fixtures-restored', self.app, 'baseline_restored')

    def lifecycle(self, args):
        from runner import classify
        identity = self.r['control']
        account = cloud(self.app, 'iam', 'service-accounts', 'describe', identity)
        uid = account['uniqueId']
        require(str(uid).isdigit(), 'control unique id required')
        cloud(self.app, 'iam', 'service-accounts', 'disable', identity, '--impersonate-service-account=' + self.r['deploy'])
        try:
            require(cloud(self.app, 'iam', 'service-accounts', 'describe', identity).get('disabled') is True, 'disable control failed')
            self.record('control-disable', identity, 'disabled')
        finally:
            cloud(self.app, 'iam', 'service-accounts', 'enable', identity)
        cloud(self.app, 'iam', 'service-accounts', 'delete', identity, '--impersonate-service-account=' + self.r['deploy'])
        # Persist UID before restore, allowing manual recovery after interruption.
        save(self.directory / 'control-deleted.json', {'project': self.app, 'identity': identity, 'unique_id': uid})
        cloud(self.app, 'iam', 'service-accounts', 'undelete', str(uid), '--impersonate-service-account=' + self.r['deploy'])
        restored = cloud(self.app, 'iam', 'service-accounts', 'describe', identity)
        require(restored['uniqueId'] == uid, 'control restoration changed identity')
        self.record('control-delete-undelete', identity, 'same_identity_restored')
        broker = cloud(self.app, 'iam', 'service-accounts', 'describe', self.r['broker'])
        parent = f'//iam.googleapis.com/projects/{self.app}/serviceAccounts/{broker["uniqueId"]}'
        tags = cloud(self.app, 'resource-manager', 'tags', 'bindings', 'list', '--parent=' + parent, '--location=global')
        require(tags, 'broker tag missing')
        target = f'projects/{self.app}/serviceAccounts/{self.r["broker"]}'
        spec = {'url': 'https://iam.googleapis.com/v1/' + target, 'method': 'DELETE', 'expected': 'deny',
                'permission': 'iam.serviceAccounts.delete', 'resource': target, 'denial_reason': 'IAM_PERMISSION_DENIED'}
        status, body = request(spec, self.token(self.r['deploy']), self.app)
        result = classify(status, body, spec)
        self.record('broker-delete', target, result, http_status=status)
        if 200 <= status < 300:
            cloud(self.app, 'iam', 'service-accounts', 'undelete', broker['uniqueId'])
        require(result == 'passed', 'broker deletion boundary failed or inconclusive')
        permissions = self.api('iam.googleapis.com', 'v1/' + target + ':testIamPermissions',
            body={'permissions': ['iam.serviceAccounts.actAs']}, identity=self.r['deploy'])
        require('iam.serviceAccounts.actAs' in permissions.get('permissions', []), 'broker actAs lost')
        self.record('broker-lifecycle', target, 'deletion_denied_and_actas_retained', tag_count=len(tags))

    def tags(self, args):
        from runner import classify
        broker = cloud(self.app, 'iam', 'service-accounts', 'describe', self.r['broker'])
        control = cloud(self.app, 'iam', 'service-accounts', 'describe', self.r['control'])
        broker_parent = f'//iam.googleapis.com/projects/{self.app}/serviceAccounts/{broker["uniqueId"]}'
        control_parent = f'//iam.googleapis.com/projects/{self.app}/serviceAccounts/{control["uniqueId"]}'
        bindings = cloud(self.app, 'resource-manager', 'tags', 'bindings', 'list', '--parent=' + broker_parent, '--location=global')
        require(len(bindings) == 1, 'one actual broker tag binding required')
        binding = bindings[0]
        tag = binding['tagValue']
        policy = cloud(self.app, 'iam', 'service-accounts', 'get-iam-policy', self.r['control'])
        # No-op policy write isolates the tag guard without broadening authority.
        target = f'projects/{self.app}/serviceAccounts/{self.r["control"]}'
        spec = {'url': 'https://iam.googleapis.com/v1/' + target + ':setIamPolicy', 'method': 'POST',
                'body': {'policy': policy}, 'expected': 'deny', 'permission': 'iam.serviceAccounts.setIamPolicy',
                'resource': target, 'denial_reason': 'IAM_PERMISSION_DENIED'}
        status, _ = request(spec, self.token(self.r['deploy']), self.app)
        require(200 <= status < 300, 'untagged control lacks positive authority')
        self.record('tag-untagged-policy', target, 'allowed', http_status=status)
        created = cloud(self.app, 'resource-manager', 'tags', 'bindings', 'create', '--tag-value=' + tag, '--parent=' + control_parent, '--location=global')
        try:
            spec['body']['policy'] = cloud(self.app, 'iam', 'service-accounts', 'get-iam-policy', self.r['control'])
            status, body = request(spec, self.token(self.r['deploy']), self.app)
            result = classify(status, body, spec)
            self.record('tag-tagged-policy', target, result, permission=spec['permission'], http_status=status)
            require(result == 'passed', 'tag deny boundary failed or inconclusive')
            detach = {'url': 'https://cloudresourcemanager.googleapis.com/v3/' + binding['name'], 'method': 'DELETE',
                      'expected': 'deny', 'permission': 'resourcemanager.tagValueBindings.delete',
                      'resource': binding['tagValue'], 'denial_reason': 'IAM_PERMISSION_DENIED'}
            status, body = request(detach, self.token(self.r['deploy']), self.app)
            result = classify(status, body, detach)
            self.record('tag-detach', broker_parent, result, http_status=status, tag_value=tag)
            require(result == 'passed', 'tag detach boundary failed or inconclusive')
        finally:
            # Only our disposable control binding is removed, never broker's.
            cloud(self.app, 'resource-manager', 'tags', 'bindings', 'delete', '--tag-value=' + tag, '--parent=' + control_parent, '--location=global')
        spec['body']['policy'] = cloud(self.app, 'iam', 'service-accounts', 'get-iam-policy', self.r['control'])
        status, _ = request(spec, self.token(self.r['deploy']), self.app)
        require(200 <= status < 300, 'untagged restoration failed')
        self.record('tag-restored-policy', target, 'allowed', http_status=status)

    def alert_plan(self, args):
        name = 'aidash-sbx-' + self.m['run_id'][:12]
        broker = self.r['broker']
        image = f'us-central1-docker.pkg.dev/{self.app}/aidash/broker@{self.m["broker_digest"]}'
        g = ['gcloud', '--project=' + self.app, '--quiet', '--format=json']
        cases = {
            'revision': {'trigger': g + ['run', 'services', 'update', 'aidash-test-credential-broker', '--region=us-central1', '--image=' + image, '--revision-suffix=probe-' + self.m['run_id'][:8], '--impersonate-service-account=' + self.r['deploy']], 'cleanup': []},
            'run-service': {'trigger': g + ['run', 'deploy', name, '--region=us-central1', '--image=' + image, '--service-account=' + broker, '--ingress=internal', '--no-allow-unauthenticated', '--async'], 'cleanup': g + ['run', 'services', 'delete', name, '--region=us-central1']},
            'run-job': {'trigger': g + ['run', 'jobs', 'create', name, '--region=us-central1', '--image=' + image, '--service-account=' + broker, '--max-retries=0'], 'cleanup': g + ['run', 'jobs', 'delete', name, '--region=us-central1']},
            'compute': {'trigger': g + ['compute', 'instances', 'create', name, '--zone=us-central1-a', '--machine-type=e2-micro', '--subnet=aidash', '--no-address', '--service-account=' + broker, '--scopes=cloud-platform', '--image=projects/debian-cloud/global/images/family/debian-12', '--labels=aidash-sandbox-run=' + self.m['run_id']], 'cleanup': g + ['compute', 'instances', 'delete', name, '--zone=us-central1-a']},
            'node-pool': {'trigger': g + ['container', 'node-pools', 'create', name, '--cluster=aidash', '--zone=us-central1-a', '--machine-type=e2-standard-2', '--num-nodes=0', '--service-account=' + broker, '--node-taints=aidash.run/probe=true:NoSchedule'], 'cleanup': g + ['container', 'node-pools', 'delete', name, '--cluster=aidash', '--zone=us-central1-a']},
        }
        save(self.directory / 'alert-commands.json', cases)
        self.record('alert-plan', self.app, 'prepared_not_executed')

    def alert(self, args):
        require(args.case in ('revision', 'run-service', 'run-job', 'compute', 'node-pool', 'service-iam', 'account-iam', 'audit-config', 'nonbroker-read', 'broker-read'), 'named alert case required')
        started = stamp()
        if args.case in ('service-iam', 'account-iam'):
            words = ['run', 'services'] if args.case == 'service-iam' else ['iam', 'service-accounts']
            target = 'aidash-test-credential-broker' if args.case == 'service-iam' else self.r['broker']
            flags = ['--region=us-central1'] if args.case == 'service-iam' else []
            policy = cloud(self.app, *words, 'get-iam-policy', target, *flags)
            file = self.directory / (args.case + '-baseline.json')
            save(file, policy)
            cloud(self.app, *words, 'set-iam-policy', target, str(file), *flags)
        elif args.case == 'audit-config':
            resource = 'v1/projects/' + self.byok
            baseline = self.api('cloudresourcemanager.googleapis.com', resource + ':getIamPolicy', body={'options': {'requestedPolicyVersion': 3}})
            save(self.directory / 'byok-policy-baseline.json', baseline)
            try:
                self.api('cloudresourcemanager.googleapis.com', resource + ':setIamPolicy', body={'policy': {**baseline, 'auditConfigs': []}, 'updateMask': 'auditConfigs,etag'})
            finally:
                latest = self.api('cloudresourcemanager.googleapis.com', resource + ':getIamPolicy', body={'options': {'requestedPolicyVersion': 3}})
                self.api('cloudresourcemanager.googleapis.com', resource + ':setIamPolicy', body={'policy': {**baseline, 'etag': latest['etag']}, 'updateMask': 'auditConfigs,etag'})
        elif args.case in ('nonbroker-read', 'broker-read'):
            identity = self.r['server'] if args.case == 'nonbroker-read' else self.r['broker']
            spec = {'url': f'https://secretmanager.googleapis.com/v1/projects/{self.m["projects"]["byok"]["number"]}/secrets/aidash-test-cred-dummy/versions/1:access', 'method': 'GET'}
            status, _ = request(spec, self.token(identity), self.byok)
            require(status == (403 if args.case == 'nonbroker-read' else 200), 'read control failed')
        else:
            self.alert_plan(args)
            case = json.loads((self.directory / 'alert-commands.json').read_text())[args.case]
            created = False
            try:
                execute(case['trigger'])
                created = True
            finally:
                if created and case['cleanup']:
                    execute(case['cleanup'])
        self.record('alert-trigger-' + args.case, self.app if args.case not in ('audit-config', 'broker-read', 'nonbroker-read') else self.byok,
                    'triggered_receipt_pending', started_at=started)

    def request_log(self, args):
        require(re.fullmatch('[a-f0-9]{24}', args.marker or ''), 'recorded probe marker required')
        self.collect_request_log(args.marker)

    def alert_collect(self, args):
        # Read the actual Terraform-managed policy filters, not an approximate
        # grep. Every query is bounded; no raw audit request/body is persisted.
        for project in (self.app, self.byok):
            policies = cloud(project, 'alpha', 'monitoring', 'policies', 'list', '--filter=displayName:Credential')
            for policy in policies:
                for condition in policy.get('conditions', []):
                    log_filter = condition.get('conditionMatchedLog', {}).get('filter')
                    if not log_filter:
                        continue
                    rows = cloud(project, 'logging', 'read', log_filter, '--freshness=1h', '--limit=50')
                    self.record('alert-log', policy['name'], 'observed' if rows else 'pending', condition=condition['name'],
                                insert_ids=[row.get('insertId') for row in rows], receipt='pending_owner_sink_check')

    def gcip_reenable(self, args):
        resource = f'v1/projects/{self.app}/tenants/{self.r["own_tenant"]}/accounts:update'
        self.api('identitytoolkit.googleapis.com', resource, body={'localId': self.r['dummy_user'], 'disableUser': False})
        self.record('gcip-reenable', f'projects/{self.app}/tenants/{self.r["own_tenant"]}', 'dummy_account_reenabled')

    def gcip(self, args):
        require(args.change in ('password', 'reset', 'revoke', 'disable', 'delete'), 'one account change required')
        session = json.loads(Path(args.session_file).read_text())
        origin = urllib.parse.urlsplit(session['origin'])
        require(origin.hostname in ('127.0.0.1', 'localhost') and origin.scheme == 'http' and not origin.path, 'session origin must be sandbox port-forward loopback')
        def check_session():
            req = urllib.request.Request(session['origin'] + '/auth/session', headers={'Cookie': session['cookie']})
            try:
                with urllib.request.build_opener(NoRedirect()).open(req, timeout=20) as response:
                    return response.status, json.loads(response.read(65536))
            except urllib.error.HTTPError as error:
                return error.code, {}
        status, before_session = check_session()
        # /auth/session returns SessionView.id only for an authenticated session.
        require(status == 200 and before_session.get('id'), 'fresh authenticated application session required')
        base = f'v1/projects/{self.app}/tenants/{self.r["own_tenant"]}/accounts:'
        lookup = lambda: self.api('identitytoolkit.googleapis.com', base + 'lookup', body={'localId': [self.r['dummy_user']]})
        before = lookup()['users'][0].get('validSince')
        password = secrets.token_urlsafe(32)
        if args.change == 'reset':
            response = self.api('identitytoolkit.googleapis.com', base + 'sendOobCode', body={'requestType': 'PASSWORD_RESET', 'email': self.r['dummy_email'], 'returnOobLink': True})
            code = urllib.parse.parse_qs(urllib.parse.urlsplit(response['oobLink']).query)['oobCode'][0]
            # API key is read only from private bootstrap output, never printed.
            bootstrap = json.loads(command(['terraform', '-chdir=' + str(self.directory / 'source/infra/gcp/bootstrap'), 'output', '-json']))
            api_key = bootstrap['gcip_web_api_key']['value']
            url = 'https://identitytoolkit.googleapis.com/v1/accounts:resetPassword?' + urllib.parse.urlencode({'key': api_key})
            status, _ = request({'method': 'POST', 'url': url, 'body': {'oobCode': code, 'newPassword': password, 'tenantId': self.r['own_tenant']}}, '', self.app)
            require(status == 200, 'password reset failed')
        elif args.change == 'delete':
            self.api('identitytoolkit.googleapis.com', base + 'delete', body={'localId': self.r['dummy_user']})
        else:
            changes = {'password': {'password': password}, 'revoke': {'validSince': str(int(time.time()))}, 'disable': {'disableUser': True}}
            self.api('identitytoolkit.googleapis.com', base + 'update', body={'localId': self.r['dummy_user'], **changes[args.change]})
        users = lookup().get('users', []) if args.change != 'delete' else []
        after = users[0].get('validSince') if users else None
        # Single bounded check; callers may rerun session-check with the original
        # cookie after the documented application freshness/cache interval.
        status, after_session = check_session()
        denied = status in (401, 403) or (status == 200 and not after_session.get('id'))
        self.record('gcip-' + args.change, f'projects/{self.app}/tenants/{self.r["own_tenant"]}', 'session_denied' if denied else 'session_still_valid', valid_since_changed=before != after, http_status=status)
        if args.change in ('password', 'reset'):
            save(self.directory / 'dummy-login.json', {'email': self.r['dummy_email'], 'password': password})
        require(denied, 'old session still valid; acceptance failed')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('phase', choices=['discover', 'fixtures', 'identities', 'manage', 'kms', 'pods', 'gcip', 'tags', 'alert-plan', 'alert', 'alert-collect', 'kubeconfig', 'request-log', 'deny-fixtures', 'restore-deny', 'lifecycle', 'gcip-reenable'])
    parser.add_argument('--manifest', required=True)
    parser.add_argument('--operator-member')
    parser.add_argument('--probe-image')
    parser.add_argument('--change')
    parser.add_argument('--case')
    parser.add_argument('--marker')
    parser.add_argument('--session-file')
    args = parser.parse_args()
    try:
        with open(Path(args.manifest).with_suffix('.lock'), 'w') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            control = Controls(args.manifest)
            getattr(control, args.phase.replace('-', '_'))(args)
        print(json.dumps({'phase': args.phase, 'status': 'observations_recorded'}))
    except (Refused, OSError, ValueError, KeyError, TypeError, AttributeError, StopIteration):
        print(json.dumps({'phase': args.phase, 'status': 'incomplete', 'detail': 'check private journal and prerequisites; raw output suppressed'}))
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
