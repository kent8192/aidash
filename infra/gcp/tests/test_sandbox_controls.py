"""Applied-output and generated-command contracts; no cloud acceptance claimed."""
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

SANDBOX = Path(__file__).resolve().parents[1] / 'sandbox'
sys.path.insert(0, str(SANDBOX))
import controls
import recipes
sys.path.pop(0)


class ControlContracts(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        self.m = dict(schema=1, run_id='a' * 32, source_sha='b' * 40, broker_digest='sha256:' + 'c' * 64,
            billing_account='ABCDEF-ABCDEF-ABCDEF', parent='none', environment='test', status='created',
            projects={'app': {'id': 'aidash-sbx-app-one', 'number': '1000'}, 'byok': {'id': 'aidash-sbx-byok-one', 'number': '1001'}})
        self.app = self.m['projects']['app']['id']
        sa = lambda name: name + '@' + self.app + '.iam.gserviceaccount.com'
        self.bootstrap = {'configuration': {'value': {'project_id': self.app, 'deploy_service_account': sa('aidash-deploy'), 'broker_service_accounts': {'test': sa('aidash-test-broker')}}}}
        self.environments = {'environments': {'value': {'test': {'server_service_account': sa('adactualhash-server'), 'worker_service_account': sa('adactualhash-worker'),
            'node_pool': 'aidash-test-123456789abc', 'byok_project_id': self.m['projects']['byok']['id'],
            'gcip': {'tenant_bindings': {'tenant-A': 'sandbox-own', 'tenant-B': 'sandbox-other'}}}}}}
        self.pool = {'config': {'serviceAccount': sa('actual-configured-node')}}
        self.service = {'status': {'url': 'https://aidash-test-credential-broker-abc-uc.a.run.app', 'latestReadyRevisionName': 'aidash-test-credential-broker-00001-abc'}}
        self.r = controls.select_resources(self.m, self.bootstrap, self.environments, self.pool, self.service)
        self.path = self.directory / 'manifest.json'
        self.path.write_text(json.dumps(self.m))
        (self.directory / 'resources.json').write_text(json.dumps(self.r))

    def instance(self):
        with mock.patch.object(controls, 'owned', return_value={'lifecycleState': 'ACTIVE'}):
            return controls.Controls(self.path)

    def test_actual_node_and_root_identities_flow_into_payload_probe(self):
        cases = recipes.build(self.m, self.r)
        node = next(case for case in cases if case['id'] == 'payload-node-own')
        self.assertEqual(node['identity'], 'actual-configured-node@aidash-sbx-app-one.iam.gserviceaccount.com')
        self.assertEqual(next(case for case in cases if case['id'] == 'gcip-lookup-own')['assert'], {'users.0.localId': 'sandbox-aaaaaaaaaaaa'})
        self.assertEqual(self.r['server'], 'adactualhash-server@aidash-sbx-app-one.iam.gserviceaccount.com')
        self.assertEqual(self.r['own_tenant'], 'tenant-A')
        self.r['node'] = self.r['control']
        with self.assertRaises(controls.Refused):
            recipes.build(self.m, self.r)

    def test_wrong_project_outputs_fail_before_generating_commands(self):
        self.bootstrap['configuration']['value']['project_id'] = 'production-project'
        with self.assertRaises(controls.Refused):
            controls.select_resources(self.m, self.bootstrap, self.environments, self.pool, self.service)

    def test_alert_commands_use_real_service_and_explicit_owned_project(self):
        control = self.instance()
        control.alert_plan(None)
        plan = json.loads((self.directory / 'alert-commands.json').read_text())
        revision = plan['revision']['trigger']
        self.assertIn('aidash-test-credential-broker', revision)
        self.assertIn('--image=us-central1-docker.pkg.dev/aidash-sbx-app-one/aidash/broker@sha256:' + 'c' * 64, revision)
        for case in plan.values():
            for key in ('trigger', 'cleanup'):
                if case[key]:
                    self.assertIn('--project=aidash-sbx-app-one', case[key])
        self.assertIn('--num-nodes=0', plan['node-pool']['trigger'])
        self.assertIn('--service-account=aidash-test-broker@aidash-sbx-app-one.iam.gserviceaccount.com', plan['run-job']['trigger'])

    def test_failed_alert_create_never_deletes_unowned_resource(self):
        control = self.instance()
        args = type('Args', (), {'case': 'run-job'})()
        with mock.patch.object(controls, 'execute', side_effect=controls.Refused('not created')) as command:
            with self.assertRaises(controls.Refused):
                control.alert(args)
        self.assertEqual(command.call_count, 1)
        self.assertNotIn('delete', command.call_args.args[0])

    def test_pod_probes_use_real_route_and_distinguish_application_rejection(self):
        control = self.instance()
        args = type('Args', (), {'probe_image': 'curlimage@sha256:' + 'd' * 64})()
        def inventory(*words):
            namespace = words[2]
            if namespace == 'aidash-test':
                definitions = [('server', {'app.kubernetes.io/component': 'server', 'aidash.run/metadata-server': 'allowed'}),
                               ('worker', {'app.kubernetes.io/component': 'worker', 'aidash.run/metadata-server': 'allowed'}),
                               ('runner', {'aidash.run/runner': 'app'})]
            else:
                definitions = [('actual-workload', {})]
            return json.dumps({'items': [{'metadata': {'name': name, 'uid': namespace + name, 'labels': labels}, 'status': {'phase': 'Running'}} for name, labels in definitions]}).encode()
        urls = []
        def pod_command(argv, **kwargs):
            url = argv[-1]
            urls.append(url)
            if 'computeMetadata' in url:
                identity = next((self.r[name] for name in ('server', 'worker') if name in argv), None)
                raw = (identity.encode() + b'\n200') if identity else b'\n000'
            else:
                raw = b'{"error":{"code":"capability_invalid_signature"}}\n401'
            return type('Result', (), {'stdout': raw, 'returncode': 0})()
        def logs(project, *words):
            self.assertEqual(words[:2], ('logging', 'read'))
            self.assertIn('httpRequest.requestUrl:', words[2])
            return [{'insertId': 'audit-1', 'resource': {'labels': {'revision_name': self.r['broker_revision']}}, 'httpRequest': {'status': 401}}]
        with mock.patch.object(control, 'kube', side_effect=inventory), mock.patch.object(controls.subprocess, 'run', side_effect=pod_command), mock.patch.object(controls, 'cloud', side_effect=logs), mock.patch.object(controls, 'request', return_value=(403, {})):
            control.pods(args)
        broker_urls = [url for url in urls if '.run.app' in url]
        self.assertEqual(len(broker_urls), 2)
        self.assertTrue(all('/api/v1/chat/completions?aidash_sandbox_probe=' in url for url in broker_urls))
        rows = json.loads((self.directory / 'controls-evidence.json').read_text())
        self.assertEqual(sum(row['case'] == 'broker-request-log' and row['observed'] == 'matched' for row in rows), 2)
        self.assertEqual(sum(row['case'] == 'metadata' and row['observed'] == 'blocked' for row in rows), 4)

    def test_restore_requires_observed_removal_of_temporary_grants(self):
        control = self.instance()
        policy = {'bindings': [{'role': 'roles/iam.serviceAccountUser', 'members': ['serviceAccount:' + self.r['deploy']]}], 'etag': 'old'}
        baseline = {identity: policy for identity in (self.r['broker'], self.r['control'], self.r['delegate_target'])}
        file = self.directory / 'deny-policies-baseline.json'
        file.write_text(json.dumps(baseline))
        def cloud(project, *words):
            if 'get-iam-policy' in words:
                return {'etag': 'fresh', 'bindings': policy['bindings'] + [{'role': 'roles/iam.serviceAccountTokenCreator', 'members': ['serviceAccount:' + self.r['deploy']]}]}
            return {}
        with mock.patch.object(controls, 'cloud', side_effect=cloud):
            with self.assertRaises(controls.Refused):
                control.restore_deny(None)
        self.assertTrue(file.exists())

    def test_alert_collection_is_bounded_to_recorded_trigger(self):
        control = self.instance()
        start = '2026-10-23T12:00:00+00:00'
        control.record('alert-trigger-revision', self.app, 'triggered_receipt_pending', started_at=start)
        def cloud(project, *words):
            self.assertEqual(project, self.app)
            if 'policies' in words:
                return [{'name': 'projects/' + self.app + '/alertPolicies/123', 'conditions': [{'name': 'revision', 'conditionMatchedLog': {'filter': 'resource.type="cloud_run_revision"'}}]}]
            self.assertIn('timestamp>=', words[2])
            self.assertIn(start, words[2])
            return []
        with mock.patch.object(controls, 'cloud', side_effect=cloud):
            control.alert_collect(None)
        row = json.loads((self.directory / 'controls-evidence.json').read_text())[-1]
        self.assertEqual(row['trigger'], 'alert-trigger-revision')
        self.assertEqual(row['window_start'], start)
        self.assertEqual(row['observed'], 'pending')

    def test_kms_failure_cleans_private_signature_without_emitting_contents(self):
        control = self.instance()
        signature = self.directory / 'kms-signature.bin'
        signature.write_bytes(b'PRIVATE_CANARY')
        with mock.patch.object(controls, 'execute', side_effect=controls.Refused('signing failed')):
            with self.assertRaises(controls.Refused):
                control.kms(None)
        self.assertFalse(signature.exists())
        self.assertFalse((self.directory / 'kms-message.bin').exists())


if __name__ == '__main__':
    unittest.main()
