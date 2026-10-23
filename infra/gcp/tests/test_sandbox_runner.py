"""Runner contracts at its CLI/cloud subprocess and HTTP response boundaries.

The fake cloud models resources and failures, never IAM decisions. These tests
are local lifecycle/redaction proof, not evidence that Google's IAM works.
"""
import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

RUNNER = Path(__file__).resolve().parents[1] / 'sandbox/runner.py'
SPEC = importlib.util.spec_from_file_location('sandbox_runner', RUNNER)
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)

FAKE = '''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
path = Path(os.environ['CLOUD_DB'])
db = json.loads(path.read_text())
a = sys.argv[1:]
p = next((x.split('=', 1)[1] for x in a if x.startswith('--project=')), None)
assert p, 'explicit project mandatory'
db.setdefault('calls', []).append(a)
path.write_text(json.dumps(db))
if db.get('fail') and db['fail'] in ' '.join(a):
    print('SECRET_TOKEN_REFLECTED', file=sys.stderr)
    sys.exit(1)
if a[:2] == ['auth', 'print-access-token']:
    print('SECRET_TOKEN_REFLECTED')
    sys.exit(0)
if a[:2] == ['projects', 'create']:
    if p in db['projects']:
        sys.exit(1)
    label = next(x.split('=', 2)[2] for x in a if x.startswith('--labels='))
    db['projects'][p] = {'projectId': p, 'projectNumber': str(1000 + len(db['projects'])), 'labels': {'aidash-sandbox-run': label}, 'parent': ({'type': 'folder', 'id': '123'} if '--folder=123' in a else None), 'lifecycleState': 'ACTIVE'}
    result = {}
elif a[:2] == ['projects', 'describe']:
    result = db['projects'].get(p)
    if result is None: sys.exit(1)
elif a[:2] == ['projects', 'delete']:
    db['projects'][p]['lifecycleState'] = 'DELETE_REQUESTED'
    result = {}
elif a[:3] == ['billing', 'projects', 'link']:
    db.setdefault('billing', {})[p] = {'billingEnabled': True, 'billingAccountName': 'billingAccounts/' + next(x.split('=',1)[1] for x in a if x.startswith('--billing-account='))}
    result = {}
elif a[:3] == ['billing', 'projects', 'unlink']:
    db.setdefault('billing', {})[p] = {'billingEnabled': False, 'billingAccountName': ''}
    result = {}
elif a[:3] == ['billing', 'projects', 'describe']:
    result = db.get('billing', {}).get(p, {'billingEnabled': False})
else:
    sys.exit(2)
path.write_text(json.dumps(db))
print(json.dumps(result))
'''


class LifecycleTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.db = self.root / 'cloud.json'
        self.db.write_text(json.dumps({'projects': {}}))
        executable = self.root / 'gcloud'
        executable.write_text(FAKE)
        executable.chmod(0o700)
        self.env = {**os.environ, 'PATH': str(self.root) + os.pathsep + os.environ['PATH'], 'CLOUD_DB': str(self.db)}
        self.directory = self.root / 'run'
        self.manifest = self.directory / 'manifest.json'
        self.call('prepare', '--directory', str(self.directory), '--source-sha', 'a' * 40,
                  '--broker-digest', 'sha256:' + 'b' * 64, '--billing-account', 'ABCDEF-ABCDEF-ABCDEF',
                  '--parent', 'folders/123', '--app-project', 'aidash-sbx-app-one', '--byok-project', 'aidash-sbx-byok-one')

    def call(self, phase, *args, ok=True):
        command = [sys.executable, str(RUNNER), phase]
        if phase != 'prepare':
            command += ['--manifest', str(self.manifest)]
        result = subprocess.run(command + list(args), env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0 if ok else 1, result.stdout + result.stderr)
        self.assertNotIn('SECRET_TOKEN_REFLECTED', result.stdout + result.stderr)
        return json.loads(result.stdout)

    def change_db(self, change):
        db = json.loads(self.db.read_text())
        change(db)
        self.db.write_text(json.dumps(db))

    def test_create_never_adopts_existing_and_redacts_failures(self):
        self.change_db(lambda db: db['projects'].update({'aidash-sbx-app-one': {'labels': {}}}))
        self.call('create', ok=False)
        self.call('create', ok=False)
        calls = json.loads(self.db.read_text())['calls']
        self.assertFalse(any('link' in call or 'delete' in call for call in calls))
        self.assertEqual(json.loads(self.manifest.read_text())['status'], 'creating')

    def test_explicit_no_organization_project_lifecycle(self):
        m = json.loads(self.manifest.read_text())
        m['parent'] = 'none'
        self.manifest.write_text(json.dumps(m))
        self.call('create')
        self.call('cleanup')
        self.assertEqual(json.loads(self.manifest.read_text())['status'], 'shutdown_verified')
        self.assertFalse(any(any(arg.startswith(('--folder=', '--organization=')) for arg in call) for call in json.loads(self.db.read_text())['calls']))

    def test_explicit_routing_and_verified_shutdown(self):
        self.call('create')
        result = self.call('cleanup')
        self.assertEqual(result['permanent_deletion'], 'pending_google_recovery_period')
        self.call('cleanup')
        m = json.loads(self.manifest.read_text())
        self.assertEqual(m['status'], 'shutdown_verified')
        self.assertTrue(all(p['number'].isdigit() for p in m['projects'].values()))
        for call in json.loads(self.db.read_text())['calls']:
            self.assertIn('--project=' + call[2 if call[0] == 'projects' else 3], call)

    def test_interrupted_cleanup_resumes_and_attempts_both_projects(self):
        self.call('create')
        self.change_db(lambda db: db.update(fail='projects delete aidash-sbx-app-one'))
        self.call('cleanup', ok=False)
        m = json.loads(self.manifest.read_text())
        self.assertEqual(m['status'], 'cleanup_pending')
        self.assertEqual(m['projects']['byok']['status'], 'shutdown_verified')
        self.change_db(lambda db: db.pop('fail'))
        self.call('cleanup')
        self.assertEqual(json.loads(self.manifest.read_text())['status'], 'shutdown_verified')

    def test_wrong_number_or_project_cannot_be_deleted(self):
        self.call('create')
        self.change_db(lambda db: db['projects']['aidash-sbx-app-one'].update(projectNumber='9999'))
        self.call('cleanup', ok=False)
        db = json.loads(self.db.read_text())
        self.assertEqual(db['projects']['aidash-sbx-app-one']['lifecycleState'], 'ACTIVE')
        self.assertFalse(any(call[:3] == ['billing', 'projects', 'unlink'] and 'aidash-sbx-app-one' in call for call in db['calls']))

    def test_probe_records_only_metadata_and_keeps_previous_attempts(self):
        self.call('create')
        requests = self.root / 'requests.json'
        requests.write_text(json.dumps([dict(id='own-secret', assumption=3, project='byok',
            identity='aidash-test-broker@aidash-sbx-app-one.iam.gserviceaccount.com', method='GET',
            url='https://secretmanager.googleapis.com/v1/projects/1001/secrets/aidash-test-cred-dummy/versions/1:access',
            expected='allow', **{'assert': {'name': 'projects/1001/secrets/aidash-test-cred-dummy/versions/1'}})]))
        body = json.dumps({'name': 'projects/1001/secrets/aidash-test-cred-dummy/versions/1', 'payload': {'data': 'SECRET_TOKEN_REFLECTED'}}).encode()
        class Response(io.BytesIO):
            status = 200
        args = type('Args', (), {'requests': str(requests)})()
        with mock.patch.dict(os.environ, self.env), mock.patch('urllib.request.OpenerDirector.open', side_effect=lambda *a, **k: Response(body)):
            for _ in range(2):
                runner.probe(json.loads(self.manifest.read_text()), self.manifest, args)
        recorded = (self.directory / 'probe-results.json').read_text()
        self.assertNotIn('SECRET_TOKEN_REFLECTED', recorded)
        self.assertNotIn('payload', recorded)
        self.assertEqual(len(json.loads(recorded)), 2)
        self.assertTrue(all(item['result'] == 'passed' for item in json.loads(recorded)))

    def test_generated_requests_are_executable_with_owned_resource_contract(self):
        self.call('create')
        resources = self.root / 'resources.json'
        resources.write_text(json.dumps({**{key: 'dummy-' + key + '@aidash-sbx-app-one.iam.gserviceaccount.com' for key in ('server', 'worker', 'node', 'control')},
            'node_pool_service_account': 'dummy-node@aidash-sbx-app-one.iam.gserviceaccount.com',
            'delegate_target': 'aidash-sandbox-target@aidash-sbx-app-one.iam.gserviceaccount.com',
            'broker': 'aidash-test-broker@aidash-sbx-app-one.iam.gserviceaccount.com', 'deploy': 'aidash-deploy@aidash-sbx-app-one.iam.gserviceaccount.com',
            'own_tenant': 'own-tenant', 'other_tenant': 'other-tenant', 'dummy_user': 'fixture-user',
            'byok_policy': {'etag': 'fresh-policy', 'version': 3, 'bindings': []}}))
        output = self.root / 'requests'
        result = subprocess.run([sys.executable, str(RUNNER.with_name('recipes.py')), '--manifest', str(self.manifest),
            '--resources', str(resources), '--output', str(output)], capture_output=True, text=True, env=self.env)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        cases = [case for path in output.glob('*.json') for case in json.loads(path.read_text())]
        batch = self.root / 'batch.json'
        batch.write_text(json.dumps(cases))
        args = type('Args', (), {'requests': str(batch)})()
        class Response(io.BytesIO):
            status = 503
        # All requests are validated before any transport. Unavailability must
        # yield non-passing observations, never claim real IAM proof.
        with mock.patch.dict(os.environ, self.env), mock.patch('urllib.request.OpenerDirector.open', side_effect=lambda *a, **k: Response(b'{}')):
            observed = runner.probe(json.loads(self.manifest.read_text()), self.manifest, args)
        self.assertFalse(observed['all_passed'])
        self.assertEqual(observed['passed'], 0)

    def test_every_successful_denial_stops_batch_and_preserves_target(self):
        self.call('create')
        batch = self.root / 'requests.json'
        base = dict(id='same-id', assumption=3, project='byok',
            identity='aidash-test-broker@aidash-sbx-app-one.iam.gserviceaccount.com', method='GET',
            expected='deny', permission='secretmanager.versions.access', denial_reason='IAM_PERMISSION_DENIED')
        args = type('Args', (), {'requests': str(batch)})()
        class Response(io.BytesIO):
            def __init__(self, status, body):
                super().__init__(body)
                self.status = status
        for number, (status, body) in enumerate([(204, b''), (200, b'[]'), (200, b'"scalar"'), (200, b'not-json')]):
            resource = f'projects/1001/secrets/aidash-test-cred-dummy-{number}/versions/1'
            first = dict(base, url='https://secretmanager.googleapis.com/v1/' + resource + ':access', resource=resource)
            batch.write_text(json.dumps([first, dict(first, id='must-not-run')]))
            with mock.patch.dict(os.environ, self.env), mock.patch('urllib.request.OpenerDirector.open', side_effect=lambda *a, **k: Response(status, body)) as transport:
                result = runner.probe(json.loads(self.manifest.read_text()), self.manifest, args)
                self.assertEqual(transport.call_count, 1)
                self.assertFalse(result['all_passed'])
        rows = json.loads((self.directory / 'probe-results.json').read_text())
        self.assertTrue(all(row['result'] == 'failed' for row in rows))
        self.assertEqual(len({row['target'] for row in rows}), 4)
        self.assertEqual(len({row['request_sha256'] for row in rows}), 4)

    def test_plan_isolated_real_roots_single_unpublished_environment(self):
        m = json.loads(self.manifest.read_text())
        m['source_sha'] = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=RUNNER.parent).decode().strip()
        self.manifest.write_text(json.dumps(m))
        self.call('create')
        original = (RUNNER.parent.parent / 'environments/broker-image.json').read_bytes()
        self.call('plan', '--alert-email', 'sandbox@example.com')
        source = self.directory / 'source/infra/gcp'
        env = json.loads((source / 'environments/sandbox.auto.tfvars.json').read_text())
        self.assertEqual(list(env['environments']), ['test'])
        self.assertFalse(env['environments']['test']['published'])
        self.assertEqual(env['project_id'], 'aidash-sbx-app-one')
        self.assertEqual(env['byok_project_id'], 'aidash-sbx-byok-one')
        self.assertEqual((RUNNER.parent.parent / 'environments/broker-image.json').read_bytes(), original)
        commands = json.loads((self.directory / 'plan-commands.json').read_text())
        self.assertFalse(any('apply' in command for command in commands))
        self.assertTrue(all(str(self.directory / 'source') in command[1] for command in commands))

    def test_wildcard_iam_project_requires_owned_account(self):
        self.call('create')
        requests = self.root / 'requests.json'
        spec = dict(id='mint', assumption=8, project='app',
            identity='aidash-deploy@aidash-sbx-app-one.iam.gserviceaccount.com',
            method='POST', url='https://iamcredentials.googleapis.com/v1/projects/-/serviceAccounts/foreign@production-project.iam.gserviceaccount.com:generateAccessToken',
            expected='deny', permission='iam.serviceAccounts.getAccessToken',
            resource='projects/-/serviceAccounts/foreign@production-project.iam.gserviceaccount.com', denial_reason='IAM_PERMISSION_DENIED')
        requests.write_text(json.dumps([spec]))
        self.call('probe', '--requests', str(requests), ok=False)
        self.assertFalse(any('print-access-token' in call for call in json.loads(self.db.read_text())['calls']))

    def test_wrong_project_request_rejected_before_token_or_http(self):
        self.call('create')
        requests = self.root / 'requests.json'
        requests.write_text(json.dumps([dict(id='bad', assumption=3, project='byok',
            identity='aidash-test-broker@aidash-sbx-app-one.iam.gserviceaccount.com',
            method='GET', url='https://secretmanager.googleapis.com/v1/projects/production-project/secrets/secret/versions/1:access', expected='allow')]))
        self.call('probe', '--requests', str(requests), ok=False)
        self.assertFalse(any('print-access-token' in call for call in json.loads(self.db.read_text())['calls']))


class ResponseTests(unittest.TestCase):
    def test_denial_requires_exact_iam_boundary(self):
        spec = dict(expected='deny', permission='secretmanager.versions.access',
                    resource='projects/1001/secrets/dummy/versions/1', denial_reason='IAM_PERMISSION_DENIED')
        detail = {'@type': 'type.googleapis.com/google.rpc.ErrorInfo', 'domain': 'iam.googleapis.com',
                  'reason': spec['denial_reason'], 'metadata': {'permission': spec['permission'], 'resource': spec['resource']}}
        body = {'error': {'status': 'PERMISSION_DENIED', 'details': [detail], 'message': 'SECRET_TOKEN_REFLECTED'}}
        self.assertEqual(runner.classify(403, body, spec), 'passed')
        for field, value in [('permission', 'iam.serviceAccounts.getAccessToken'), ('resource', 'projects/9999/secrets/dummy/versions/1')]:
            with self.subTest(field=field):
                modified = json.loads(json.dumps(body))
                modified['error']['details'][0]['metadata'][field] = value
                self.assertEqual(runner.classify(403, modified, spec), 'inconclusive')
        for status, response in [(0, {}), (404, body), (403, {'error': {'status': 'PERMISSION_DENIED'}}), (401, body)]:
            self.assertEqual(runner.classify(status, response, spec), 'inconclusive')
        detail['reason'] = 'SERVICE_DISABLED'
        self.assertEqual(runner.classify(403, body, spec), 'inconclusive')

    def test_successful_forbidden_operation_is_failure(self):
        self.assertEqual(runner.classify(200, {'accessToken': 'SECRET_TOKEN_REFLECTED'}, {'expected': 'deny'}), 'failed')

    def test_malformed_denial_never_crashes_or_passes(self):
        for body in (["SECRET_TOKEN_REFLECTED"], {"error": "SECRET_TOKEN_REFLECTED"}, {"error": {"status": "PERMISSION_DENIED", "details": ["SECRET_TOKEN_REFLECTED"]}}):
            self.assertEqual(runner.classify(403, body, {"expected": "deny"}), "inconclusive")

    def test_http_success_requires_semantic_assertion(self):
        spec = {'expected': 'allow', 'assert': {'name': 'projects/1001/secrets/dummy'}}
        self.assertEqual(runner.classify(200, {'name': 'projects/1001/secrets/other'}, spec), 'failed')
        self.assertEqual(runner.classify(200, {'name': 'projects/1001/secrets/dummy'}, spec), 'passed')
        self.assertEqual(runner.classify(200, {}, {'expected': 'allow'}), 'observation_only')

    def test_resource_requests_do_not_add_quota_project_permission(self):
        class Response(io.BytesIO):
            status = 204
        with mock.patch('urllib.request.OpenerDirector.open', return_value=Response(b'')) as transport:
            status, _ = runner.request({'url': 'https://secretmanager.googleapis.com/v1/projects/1001/secrets/dummy', 'method': 'DELETE'}, 'private-token', 'aidash-sbx-app-one')
        self.assertEqual(status, 204)
        self.assertFalse(transport.call_args.args[0].has_header('X-goog-user-project'))

    def test_transport_error_and_redirect_never_become_denial(self):
        with mock.patch('urllib.request.OpenerDirector.open', side_effect=OSError('SECRET_TOKEN_REFLECTED')):
            self.assertEqual(runner.request({'url': 'https://secretmanager.googleapis.com/v1/projects/1001/secrets/dummy', 'method': 'GET'}, 'private-token', 'aidash-sbx-app-one'), (0, {}))
        self.assertIsNone(runner.NoRedirect().redirect_request(None, None, None, None, None, None))


if __name__ == '__main__':
    unittest.main()
