"""Applied-output and generated-command contracts; no cloud acceptance claimed."""
import importlib
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock

SANDBOX = Path(__file__).resolve().parents[1] / 'sandbox'
with mock.patch.object(sys, 'path', [str(SANDBOX), *sys.path]):
    controls = importlib.import_module('controls')
    recipes = importlib.import_module('recipes')


class ControlContracts(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.directory = Path(self.temp.name)
        self.m = dict(schema=1, run_id='a' * 32, source_sha='b' * 40, broker_digest='sha256:' + 'c' * 64,
            billing_account='ABCDEF-ABCDEF-ABCDEF', parent='none', environment='test', status='created',
            projects={'app': {'id': 'aidash-sbx-app-one', 'number': '1000'}, 'byok': {'id': 'aidash-sbx-byok-one', 'number': '1001'}})
        self.app = self.m['projects']['app']['id']
        def sa(name):
            return name + '@' + self.app + '.iam.gserviceaccount.com'
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
        mislabeled_runner = [False]
        runner_metadata_status = [200]
        def inventory(*words):
            namespace = words[2]
            if namespace == 'aidash-test':
                definitions = [('server', {'app.kubernetes.io/component': 'server', 'aidash.run/metadata-server': 'allowed'}),
                               ('worker', {'app.kubernetes.io/component': 'worker', 'aidash.run/metadata-server': 'allowed'}),
                               ('runner', {'aidash.run/runner': 'app', **({'aidash.run/metadata-server': 'allowed'} if mislabeled_runner[0] else {})})]
            else:
                definitions = [('actual-workload', {})]
            return json.dumps({'items': [{'metadata': {'name': name, 'uid': namespace + name, 'labels': labels}, 'spec': {'serviceAccountName': 'app-aidash-' + name}, 'status': {'phase': 'Running'}} for name, labels in definitions]}).encode()
        urls = []
        def pod_command(argv, **kwargs):
            url = argv[-1]
            urls.append(url)
            if 'computeMetadata' in url:
                identity = next((self.r[name] for name in ('server', 'worker') if name in argv), None)
                raw = (identity.encode() + b'\n200') if identity else b'\n000'
                if 'runner' in argv and mislabeled_runner[0] and runner_metadata_status[0] == 200:
                    raw = b'unexpected-node@project.iam.gserviceaccount.com\n200'
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
        self.assertEqual(sum(row['case'] == 'metadata' and row['expected_allowed'] for row in rows), 2)
        # The label being tested cannot authorize a Runner. A mistaken label is
        # itself a failed policy observation, even if metadata happens to block.
        mislabeled_runner[0] = True
        for observed_status in (200, 0):
            runner_metadata_status[0] = observed_status
            with self.subTest(runner_metadata_status=observed_status):
                with mock.patch.object(control, 'kube', side_effect=inventory), mock.patch.object(controls.subprocess, 'run', side_effect=pod_command), mock.patch.object(controls, 'cloud', side_effect=logs), mock.patch.object(controls, 'request', return_value=(403, {})):
                    with self.assertRaises(controls.Refused):
                        control.pods(args)

    def test_restore_requires_observed_removal_of_temporary_grants(self):
        control = self.instance()
        policy = {'bindings': [{'role': 'roles/iam.serviceAccountUser', 'members': ['serviceAccount:' + self.r['deploy']]}], 'etag': 'old'}
        baseline = {identity: policy for identity in (self.r['broker'], self.r['control'], self.r['delegate_target'])}
        file = self.directory / 'deny-policies-baseline.json'
        file.write_text(json.dumps(baseline))
        (self.directory / 'deny-keys-baseline.json').write_text(json.dumps({self.r['broker']: [], self.r['control']: []}))
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
                return [{'name': 'projects/' + self.app + '/alertPolicies/' + str(i), 'displayName': name, 'conditions': [{'name': 'revision', 'conditionMatchedLog': {'filter': 'resource.type="cloud_run_revision"'}}]} for i, name in enumerate(['Credential Broker: new Cloud Run revision', 'Credential Broker: IAM policy changed'])]
            self.assertIn('timestamp>=', words[2])
            self.assertIn(start, words[2])
            return []
        with mock.patch.object(controls, 'cloud', side_effect=cloud):
            control.alert_collect(None)
        row = json.loads((self.directory / 'controls-evidence.json').read_text())[-1]
        self.assertEqual(row['trigger'], 'alert-trigger-revision')
        self.assertEqual(row['window_start'], start)
        self.assertEqual(row['observed'], 'pending')

    def test_restore_revokes_new_broker_and_control_keys_and_retries_interruption(self):
        control = self.instance()
        identities = (self.r['broker'], self.r['control'], self.r['delegate_target'])
        baseline = {identity: {'bindings': [], 'etag': 'old'} for identity in identities}
        file = self.directory / 'deny-policies-baseline.json'
        file.write_text(json.dumps(baseline))
        def keyname(identity, suffix):
            return 'projects/' + self.app + '/serviceAccounts/' + identity + '/keys/' + suffix
        original = {identity: [{'name': keyname(identity, 'original')}] for identity in identities[:2]}
        (self.directory / 'deny-keys-baseline.json').write_text(json.dumps(original))
        current = {identity: keys + [{'name': keyname(identity, 'unexpected')}] for identity, keys in original.items()}
        deletes = []
        interrupted = [True]
        def cloud(project, *words):
            self.assertEqual(project, self.app)
            if 'get-iam-policy' in words:
                return {'bindings': [], 'etag': 'fresh'}
            if 'set-iam-policy' in words:
                self.assertEqual(json.loads(Path(words[-1]).read_text())['etag'], 'fresh')
                return {}
            identity = next(word.split('=', 1)[1] for word in words if word.startswith('--iam-account='))
            if 'list' in words:
                return list(current[identity])
            self.assertIn('delete', words)
            deletes.append((identity, words[4]))
            if identity == self.r['control'] and interrupted[0]:
                interrupted[0] = False
                raise controls.Refused('interrupted')
            current[identity] = [key for key in current[identity] if key['name'].rsplit('/', 1)[1] != words[4]]
            return {}
        with mock.patch.object(controls, 'cloud', side_effect=cloud):
            with self.assertRaises(controls.Refused):
                control.restore_deny(None)
            self.assertTrue(file.exists())
            control.restore_deny(None)
        self.assertEqual(current, original)
        self.assertIn((self.r['broker'], 'unexpected'), deletes)
        self.assertIn((self.r['control'], 'unexpected'), deletes)
        self.assertTrue(all(key != 'original' for _, key in deletes))
        self.assertFalse(file.exists())

    def test_key_snapshot_failure_prevents_all_fixture_grants(self):
        control = self.instance()
        commands = []
        def cloud(project, *words):
            commands.append(words)
            if 'get-iam-policy' in words:
                return {'bindings': [], 'etag': 'old'}
            if '--iam-account=' + self.r['broker'] in words:
                raise controls.Refused('key inventory unavailable')
            return []
        with mock.patch.object(controls, 'cloud', side_effect=cloud):
            with self.assertRaises(controls.Refused):
                control.deny_fixtures(None)
        self.assertFalse(any('add-iam-policy-binding' in words for words in commands))
        self.assertFalse((self.directory / 'deny-policies-baseline.json').exists())

    def test_lifecycle_journals_recovery_before_mutation(self):
        control = self.instance()
        mutations = []
        def cloud(project, *words):
            if 'describe' in words:
                return {'uniqueId': '12345', 'disabled': 'disable' in mutations}
            if 'disable' in words:
                pending = json.loads((self.directory / 'control-lifecycle.json').read_text())
                self.assertEqual(pending['identity'], self.r['control'])
                self.assertEqual(pending['stage'], 'disable_pending')
                mutations.append('disable')
            if 'delete' in words:
                pending = json.loads((self.directory / 'control-deleted.json').read_text())
                self.assertEqual(pending['unique_id'], '12345')
                raise controls.Refused('delete response lost')
            return {}
        with mock.patch.object(controls, 'cloud', side_effect=cloud):
            with self.assertRaises(controls.Refused):
                control.lifecycle(None)
        self.assertEqual(json.loads((self.directory / 'control-deleted.json').read_text())['stage'], 'delete_pending')

    def test_byok_alert_inventory_includes_audit_policy_and_rejects_gaps(self):
        control = self.instance()
        control.record('alert-trigger-audit-config', control.byok, 'triggered_receipt_pending', started_at='2026-10-23T12:00:00+00:00')
        names = ['Provider Credential Key Material read by a non-broker principal', 'BYOK project: audit configuration or IAM changed']
        queries = []
        policies = [{'name': 'projects/' + control.byok + '/alertPolicies/' + str(i), 'displayName': name,
                     'conditions': [{'name': str(i), 'conditionMatchedLog': {'filter': 'exact_filter_' + str(i)}}]} for i, name in enumerate(names)]
        policies.append({'name': 'unrelated', 'displayName': 'Unrelated', 'conditions': [{'conditionMatchedLog': {'filter': 'wrong_filter'}}]})
        def cloud(project, *words):
            self.assertEqual(project, control.byok)
            if 'policies' in words:
                return [p for p in policies if 'Credential' in p['displayName']] if '--filter=displayName:Credential' in words else policies
            queries.append(words[2])
            return []
        with mock.patch.object(controls, 'cloud', side_effect=cloud):
            control.alert_collect(None)
            self.assertEqual(len(queries), 2)
            self.assertTrue(any('exact_filter_1' in query for query in queries))
            self.assertFalse(any('wrong_filter' in query for query in queries))
            policies.pop(1)
            with self.assertRaises(controls.Refused):
                control.alert_collect(None)

    def test_last_broker_undelete_recovers_identity_policy_and_tags_for_each_outcome(self):
        for outcome in ('deny', 'allow', 'tags_lost', 'interrupted'):
            with self.subTest(outcome=outcome):
                self.check_broker_undelete_outcome(outcome)

    def check_broker_undelete_outcome(self, outcome):
        control = self.instance()
        control.record('control-delete-undelete', self.r['control'], 'same_identity_restored', tag_count=0)
        journal = self.directory / 'broker-undelete-recovery.json'
        journal.unlink(missing_ok=True)
        uid = '12345'
        resource = 'projects/' + self.app + '/serviceAccounts/' + uid
        parent = '//iam.googleapis.com/' + resource
        original_policy = {'etag': 'old', 'bindings': [{'role': 'roles/iam.serviceAccountUser', 'members': ['serviceAccount:' + self.r['deploy']]}]}
        tag = {'name': 'tagBindings/fixture', 'tagValue': 'tagValues/123', 'parent': parent}
        state = {'deleted': False, 'tags': [tag], 'policy': original_policy, 'restore_fails': outcome == 'interrupted'}
        events = []
        def cloud(project, *words):
            self.assertEqual(project, self.app)
            if 'bindings' in words:
                if 'create' in words:
                    state['tags'] = [tag]
                    return tag
                return state['tags']
            if 'get-iam-policy' in words:
                return {**state['policy'], 'etag': 'fresh'}
            if 'set-iam-policy' in words:
                restored = json.loads(Path(words[-1]).read_text())
                self.assertEqual(restored['etag'], 'fresh')
                state['policy'] = restored
                return restored
            if 'delete' in words:
                durable = json.loads(journal.read_text())
                self.assertEqual(durable['stage'], 'delete_pending')
                self.assertEqual(durable['unique_id'], uid)
                self.assertEqual(durable['policy'], {**original_policy, 'etag': 'fresh'})
                self.assertEqual(durable['tags'], [tag])
                events.append('admin-delete')
                state['deleted'] = True
                if outcome == 'tags_lost':
                    state['tags'] = []
                if outcome == 'interrupted':
                    raise controls.Refused('lost delete response')
                return {}
            return {'uniqueId': uid, 'disabled': False}
        def request(spec, token, project):
            if spec['method'] == 'GET':
                return (404, {}) if state['deleted'] else (200, {'uniqueId': uid, 'disabled': False})
            self.assertEqual(spec['url'], 'https://iam.googleapis.com/v1/' + resource + ':undelete')
            if token == 'deploy':
                events.append('deploy-undelete')
                self.assertTrue(state['deleted'])
                if outcome == 'allow':
                    state['deleted'] = False
                    return 200, {}
                return 403, {'error': {'status': 'PERMISSION_DENIED', 'details': [{'@type': 'type.googleapis.com/google.rpc.ErrorInfo', 'domain': 'iam.googleapis.com', 'reason': 'IAM_PERMISSION_DENIED', 'metadata': {'permission': 'iam.serviceAccounts.undelete', 'resource': resource}}]}}
            events.append('admin-undelete')
            if state['restore_fails']:
                return 503, {}
            state['deleted'] = False
            state['policy'] = {'etag': 'after', 'bindings': []}
            return 200, {'uniqueId': uid}
        with mock.patch.object(controls, 'cloud', side_effect=cloud), mock.patch.object(controls, 'request', side_effect=request), mock.patch.object(control, 'token', side_effect=lambda identity=None: 'deploy' if identity else 'admin'):
            if outcome == 'deny':
                control.broker_undelete_last(None)
            else:
                with self.assertRaises(controls.Refused):
                    control.broker_undelete_last(None)
            if outcome == 'interrupted':
                self.assertEqual(json.loads(journal.read_text())['stage'], 'restore_pending')
                state['restore_fails'] = False
                control.recover_broker(None)
        self.assertFalse(state['deleted'])
        self.assertEqual(state['policy']['bindings'], original_policy['bindings'])
        self.assertEqual(state['tags'], [tag])
        self.assertEqual(json.loads(journal.read_text())['stage'], 'restored')
        if outcome != 'interrupted':
            rows = json.loads((self.directory / 'controls-evidence.json').read_text())
            result = [row for row in rows if row['case'] == 'broker-undelete'][-1]
            self.assertEqual(result['observed'], {'deny': 'passed', 'allow': 'failed', 'tags_lost': 'inconclusive_tag_state'}[outcome])
            self.assertEqual(events[:2], ['admin-delete', 'deploy-undelete'])


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
