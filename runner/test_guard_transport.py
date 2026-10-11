import json
import base64
from pathlib import Path
import subprocess
import tempfile
import threading
import unittest
from unittest.mock import Mock, patch

from control import ActivityAggregate, Runner


class GuardTransportTest(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.runner = object.__new__(Runner)
        self.runner.root = Path(directory.name)
        self.runner.lock = threading.RLock()
        self.runner.aggregate = ActivityAggregate()
        self.runner.config = {'node_guard': {'namespace': 'trusted', 'selector': 'app=guard',
                                             'command': ['python3', '-I', '/opt/aidash/node_guard.py']}}
        self.runner.command = ['kubectl', '--namespace', 'execution']
        self.runner.environment = {}
        self.record = dict(operation_id='9a87fdb1-e1fd-4ce9-a89f-754b64e746b0', area_id='area',
                           pod_uid='uid', container_id='cid', epoch=1, image='pinned', cluster_node='cluster-a')
        self.runner.pod = Mock()
        self.runner.kube_json = Mock(return_value={'items': [dict(metadata={'name': 'guard-a'},
            spec={'nodeName': 'cluster-a'}, status={'conditions': [{'type': 'Ready', 'status': 'True'}]})]})

    def test_exec_routes_stdin_to_the_original_cluster_node(self):
        with patch('control.subprocess.run', return_value=subprocess.CompletedProcess([], 0, b'{"termination_confirmed":true}')) as run:
            result = self.runner.guard(self.record, 'terminate')
        self.assertTrue(result['termination_confirmed'])
        self.assertIn('spec.nodeName=cluster-a', self.runner.kube_json.call_args.args[0])
        self.assertEqual(run.call_args.args[0][-9:], ['exec', '-i', 'guard-a', '-c', 'guard', '--', 'python3', '-I', '/opt/aidash/node_guard.py'])
        self.assertEqual(json.loads(run.call_args.kwargs['input'])['pod_uid'], 'uid')
        self.runner.pod.assert_not_called()

    def test_old_journal_is_bound_from_live_uid_before_guard_dispatch(self):
        for missing in ('image', 'cluster_node'):
            record = dict(self.record)
            del record[missing]
            self.runner.pod.return_value = dict(metadata={'uid': 'uid'},
                spec={'nodeName': 'cluster-a', 'containers': [{'name': 'execution', 'image': 'pinned'}]})
            with patch('control.subprocess.run', return_value=subprocess.CompletedProcess([], 0, b'{}')):
                self.runner.guard(record, 'status')
            self.runner.pod.return_value['metadata']['uid'] = 'replacement'
            with self.assertRaisesRegex(RuntimeError, 'binding unavailable'):
                self.runner.guard(record, 'status')

    def test_missing_or_ambiguous_guard_never_supplies_stop_proof(self):
        pod = self.runner.kube_json.return_value['items'][0]
        for items in ([], [pod, pod], [dict(pod, spec={'nodeName': 'cluster-b'})],
                      [dict(pod, metadata={'name': 'guard-a', 'deletionTimestamp': 'now'})]):
            self.runner.kube_json.return_value = {'items': items}
            with patch('control.subprocess.run') as run, self.assertRaisesRegex(RuntimeError, 'unconfirmed'):
                self.runner.guard(self.record, 'status')
            run.assert_not_called()

    def test_isolation_admission_requires_guest_capacity_and_separate_host_ceiling(self):
        self.runner.config.update(working_bytes=4096, temporary_bytes=4096, processes=128, host_tasks=512)
        self.runner.accept = Mock()
        report = {'protocol': 'aidash-admission-probe/2', 'kernel': '4.19.0-gvisor', 'uid': 10000, 'process_limit': 128,
                  'physical_page_size': 4096, 'egress_denied': {'metadata': True},
                  'guest_capacity_verified': 112}
        record = {'status': 'completed', 'resource_evidence': {'host_tasks': 512, 'processes': 128}}
        for capacity, host_tasks, passes in ((112, 512, True), (40, 512, False), (112, 128, False)):
            with self.subTest(capacity=capacity, host_tasks=host_tasks):
                report['guest_capacity_verified'] = capacity
                record['resource_evidence']['host_tasks'] = host_tasks
                record['stdout'] = base64.b64encode(json.dumps(report).encode()).decode()
                self.runner.get = Mock(return_value=record)
                if passes:
                    self.runner.verify_isolation()
                    self.assertTrue(self.runner.verified)
                else:
                    with self.assertRaisesRegex(RuntimeError, 'does not match'):
                        self.runner.verify_isolation()
        # A sandbox image built from a source with another probe contract (e.g. one
        # that counted exited children) is refused, never read as evidence.
        for protocol in (None, 'aidash-admission-probe/1'):
            with self.subTest(protocol=protocol):
                stale = dict(report, guest_capacity_verified=112)
                if protocol is None:
                    del stale['protocol']
                else:
                    stale['protocol'] = protocol
                record['resource_evidence']['host_tasks'] = 512
                record['stdout'] = base64.b64encode(json.dumps(stale).encode()).decode()
                self.runner.get = Mock(return_value=record)
                with self.assertRaisesRegex(RuntimeError, 'aidash-admission-probe/2'):
                    self.runner.verify_isolation()

    def test_activity_keeps_pending_and_unproven_journals_busy(self):
        records = [
            {'status': 'awaiting_files'}, {'status': 'running'},
            {'status': 'uncertain', 'termination_confirmed': False},
            {'status': 'completed', 'writer_frozen': True, 'finished_at': 50},
            {'status': 'cancelled', 'termination_confirmed': True, 'finished_at': 60},
        ]
        for index, record in enumerate(records):
            (self.runner.root / f'{index}.json').write_text(json.dumps(record))
        for name in ('area-fixture', 'session-fixture'):
            (self.runner.root / (name + '.json')).write_text('{}')
        self.runner.index_journal()
        activity = self.runner.activity()
        self.assertEqual(activity['counts'], {'runner': 3})
        self.assertTrue(activity['busy'])
        self.assertEqual(activity['last_work_completed'], 60)

    def test_corrupt_journal_cannot_supply_idle_activity(self):
        # The journal is indexed once at startup; a corrupt record stops the Runner
        # from starting rather than letting /v1/activity report idle.
        (self.runner.root / 'operation.json').write_text('{')
        with self.assertRaises(json.JSONDecodeError):
            self.runner.index_journal()

    def test_failed_admission_probe_stops_counting_once_its_pod_is_gone(self):
        # A probe that failed before dispatch (e.g. scheduling) is terminal `uncertain`.
        probe = {'operation_id': '1e0c5bd2-3f6a-4f5f-9b55-3c4f3ad3c0aa', 'status': 'uncertain',
                 'termination_confirmed': False, 'admission_probe': True}
        # Ordinary work cannot claim the probe's internal marker through its digest.
        work = {'operation_id': '2f1d6ce3-4a7b-4060-8c66-4d5a4be4d1bb', 'status': 'uncertain',
                'termination_confirmed': False, 'digest': 'immutable-admission-probe/3'}
        for record in (probe, work):
            (self.runner.root / (record['operation_id'] + '.json')).write_text(json.dumps(record))
        self.runner.index_journal()
        self.runner.kube = Mock()
        # While its Pod survives deletion, the probe stays busy (fail closed).
        self.runner.pod.return_value = {'metadata': {'uid': 'uid'}}
        self.runner.retire_admission_probes()
        self.assertEqual(self.runner.activity()['counts'], {'runner': 2})
        self.assertEqual([call.args[0][:3] for call in self.runner.kube.call_args_list],
                         [['delete', 'pod', 'operation-' + probe['operation_id']]])
        self.runner.pod.return_value = None
        self.runner.retire_admission_probes()
        self.assertEqual(self.runner.activity()['counts'], {'runner': 1})
        self.assertTrue(self.runner.get(probe['operation_id'])['termination_confirmed'])
        self.assertFalse(self.runner.get(work['operation_id'])['termination_confirmed'])


class NamespaceTest(unittest.TestCase):
    def construct(self, namespace):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        config = {'journal': directory.name, 'token_env': 'AIDASH_TEST_RUNNER_TOKEN', 'namespace': namespace,
                  'image': 'sandbox@sha256:' + 'a' * 64, 'kubectl': 'kubectl', 'runtime_class': 'runsc'}
        # Stop at the first cluster call: everything before it is local validation.
        with (patch.dict('os.environ', {'AIDASH_TEST_RUNNER_TOKEN': 't' * 32}),
              patch.object(Runner, 'kube_json', side_effect=RuntimeError('validated'))):
            Runner(config)

    def test_dedicated_namespace_is_any_rfc1123_label(self):
        # The chart accepts any Kubernetes namespace, including one character.
        for namespace in ('a', '7', 'ab', 'a-b', 'x' * 63):
            with self.subTest(namespace=namespace), self.assertRaisesRegex(RuntimeError, 'validated'):
                self.construct(namespace)
        for namespace in ('', '-a', 'a-', 'A', 'a_b', 'x' * 64):
            with self.subTest(namespace=namespace), self.assertRaisesRegex(ValueError, 'namespace'):
                self.construct(namespace)


if __name__ == '__main__':
    unittest.main()
