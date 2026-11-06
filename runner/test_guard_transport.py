import contextlib
import io
import json
import base64
from pathlib import Path
import subprocess
import tempfile
import threading
import unittest
from unittest.mock import Mock, patch

import control
from control import ActivityAggregate, Handler, Rejected, Runner, save


class GuardTransportTest(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.runner = object.__new__(Runner)
        self.runner.root = Path(directory.name)
        self.runner.lock = threading.RLock()
        self.runner.aggregate = ActivityAggregate()
        self.runner.start = Mock()
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
            (self.runner.root / f'{index}.json').write_text(json.dumps(dict(record, operation_id=str(index))))
        for name in ('area-fixture', 'session-fixture'):
            (self.runner.root / (name + '.json')).write_text('{}')
        self.runner.recover()
        activity = self.runner.activity()
        self.assertEqual(activity['counts'], {'runner': 3})
        self.assertTrue(activity['busy'])
        self.assertEqual(activity['last_work_completed'], 60)

    def test_corrupt_journal_cannot_supply_idle_activity(self):
        # The journal is indexed once, by the startup recovery scan. As before, a
        # corrupt record cannot be read, so the Runner does not start and activity
        # never reports idle.
        (self.runner.root / 'operation.json').write_text('{')
        with self.assertRaises(json.JSONDecodeError):
            self.runner.recover()

    def test_failed_admission_probe_stops_counting_once_its_pod_is_gone(self):
        # A probe that failed before dispatch (e.g. scheduling) is terminal `uncertain`.
        probe = {'operation_id': '1e0c5bd2-3f6a-4f5f-9b55-3c4f3ad3c0aa', 'status': 'uncertain',
                 'termination_confirmed': False, 'admission_probe': True}
        # Ordinary work cannot claim the probe's internal marker through its digest.
        work = {'operation_id': '2f1d6ce3-4a7b-4060-8c66-4d5a4be4d1bb', 'status': 'uncertain',
                'termination_confirmed': False, 'digest': 'immutable-admission-probe/3'}
        for record in (probe, work):
            (self.runner.root / (record['operation_id'] + '.json')).write_text(json.dumps(record))
        self.runner.recover()
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


class StartupTest(unittest.TestCase):
    """A failed isolation proof refuses new work; it must not stop the Runner watching its commands (#722, decision 14)."""

    def construct(self, isolation, freeze=lambda runner: None):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        config = {'journal': directory.name, 'token_env': 'AIDASH_TEST_RUNNER_TOKEN', 'namespace': 'runner',
                  'image': 'sandbox@sha256:' + 'a' * 64, 'kubectl': 'kubectl', 'runtime_class': 'runsc',
                  'maximum_seconds': 600,
                  'node_guard': {'namespace': 'trusted', 'selector': 'app=guard', 'command': ['guard']}}
        steps = []

        def prove(runner):
            isolation(runner)
            runner.verified, runner.probe = True, {'protocol': 'aidash-admission-probe/2'}

        def freeze_writer(runner):
            steps.append('freeze')
            freeze(runner)
            runner.python_verified = True

        with (patch.dict('os.environ', {'AIDASH_TEST_RUNNER_TOKEN': 't' * 32}),
              patch.object(Runner, 'kube_json', return_value={'handler': 'runsc'}),
              patch.object(Runner, 'ensure_network_policy'), patch.object(Runner, 'ensure_v2_network_policy'),
              patch.object(Runner, 'verify_isolation', autospec=True, side_effect=prove),
              patch.object(Runner, 'verify_freeze', autospec=True, side_effect=freeze_writer),
              patch.object(Runner, 'retire_admission_probes', side_effect=lambda: steps.append('retire')),
              patch.object(Runner, 'recover', side_effect=lambda resume=True: steps.append(('recover', resume))),
              patch('control.ExecutionPodController.recover_commands', side_effect=lambda: steps.append('watch')),
              patch.object(Runner, 'activate_execution_pods', side_effect=lambda: steps.append('admit')),
              patch('control.threading.Thread'), contextlib.redirect_stderr(io.StringIO()) as log):
            runner = Runner(config)
        return runner, steps, log.getvalue()

    @staticmethod
    def health(runner):
        handler = object.__new__(Handler)
        handler.server, handler.command, handler.path = Mock(runner=runner), 'GET', '/v1/health'
        handler.headers = {'Authorization': 'Bearer ' + runner.token}
        report = handler.handle_request()
        return {key: report[key] for key in ('verified', 'python_verified', 'probe')}

    @staticmethod
    def refuse(message):
        def fail(runner):
            raise RuntimeError(message)
        return fail

    def test_a_failed_isolation_proof_still_watches_commands_and_admits_no_new_work(self):
        runner, steps, log = self.construct(self.refuse('sandbox image admission probe is not aidash-admission-probe/2'))
        self.assertEqual(steps, ['retire', ('recover', False), 'watch'])
        # The health endpoint is how the server learns to refuse new work.
        self.assertFalse(runner.proven)
        self.assertFalse(runner.verified)
        self.assertFalse(runner.python_verified)
        self.assertIsNone(runner.probe)
        # Execution Pods stay unadmitted, so none accepts a new command while recovered ones are watched.
        self.assertFalse(runner.execution_pods.admitted)
        self.assertIn('aidash-admission-probe/2', runner.execution_pods.admission_error)
        self.assertIn('Deployment Administrator', log)

    def test_a_proof_that_stops_half_way_does_not_report_the_runner_verified(self):
        runner, steps, _ = self.construct(lambda runner: None, freeze=self.refuse('gVisor freeze did not stop the writer'))
        self.assertEqual(steps, ['freeze', 'retire', ('recover', False), 'watch'])
        self.assertFalse(runner.proven)
        self.assertFalse(runner.verified)
        self.assertFalse(runner.python_verified)
        self.assertIsNone(runner.probe)
        # The health report is what the server reads before it sends work.
        self.assertEqual(self.health(runner), {'verified': False, 'python_verified': False, 'probe': None})

    def test_a_proven_boundary_resumes_recovered_work_and_admits_execution_pods(self):
        runner, steps, _ = self.construct(lambda runner: None)
        self.assertEqual(steps, ['freeze', 'retire', ('recover', True), 'watch', 'admit'])
        self.assertTrue(runner.proven)
        self.assertEqual(self.health(runner), {'verified': True, 'python_verified': True,
                                               'probe': {'protocol': 'aidash-admission-probe/2'}})

    def test_an_unproven_runner_takes_no_new_work_but_still_delivers_stops(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        runner = object.__new__(Runner)
        runner.root = Path(directory.name)
        runner.lock = threading.RLock()
        runner.aggregate = ActivityAggregate()
        runner.active = set()
        runner.start = Mock()
        self.assertFalse(runner.proven)
        for refused in (lambda: runner.accept({}), lambda: runner.commit_inputs('operation')):
            with self.assertRaises(Rejected) as error:
                refused()
            self.assertEqual(error.exception.args[0], 503)
        # Only the Runner's own admission probe runs without the proof.
        with self.assertRaises(Rejected) as error:
            runner.accept({}, admission_probe=True)
        self.assertEqual(error.exception.args[0], 400)
        # A stop, a status read and an acknowledgement stay available.
        operation = '5a7b1cb4-4c5e-4c44-9c05-0d3b4f6d3a10'
        save(runner.path(operation), {'operation_id': operation, 'status': 'running', 'request': {'kind': 'shell'},
                                      'cancel_requested': False, 'termination_confirmed': False, 'digest': 'd'})
        self.assertTrue(runner.cancel(operation)['cancel_requested'])
        self.assertEqual(runner.get(operation)['status'], 'running')

    def test_recovery_without_a_proof_seeds_activity_but_dispatches_nothing(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        runner = object.__new__(Runner)
        runner.root = Path(directory.name)
        runner.lock = threading.RLock()
        runner.aggregate = ActivityAggregate()
        runner.start = Mock()
        (runner.root / 'running.json').write_text(json.dumps({'operation_id': 'running', 'status': 'running'}))
        runner.recover(resume=False)
        runner.start.assert_not_called()
        # An idle stop must not take the Runner down with that work in flight.
        self.assertTrue(runner.activity()['busy'])
        runner.recover()
        runner.start.assert_called_once_with('running')

    def test_a_failure_after_commands_resume_cannot_leave_a_process_that_serves_nothing(self):
        # The resume workers are not daemon threads, so an exception that ends startup would hang the exit.
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        profile = Path(directory.name) / 'profile.json'
        profile.write_text(json.dumps({'runner': {'listen_host': '127.0.0.1', 'listen_port': 1}}))
        with (patch.dict('os.environ', {'AIDASH_CAPABILITY_PROFILE': str(profile)}), patch('control.Runner'),
              patch('control.ThreadingHTTPServer', side_effect=OSError('address already in use')),
              patch('control.os._exit') as leave, contextlib.redirect_stderr(io.StringIO()) as log):
            control.main()
        leave.assert_called_once_with(1)
        self.assertIn('address already in use', log.getvalue())


if __name__ == '__main__':
    unittest.main()
