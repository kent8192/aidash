"""A Completion Gate Check ("verify") runs like shell but never exports /work."""
import base64
import contextlib
import hashlib
import io
import json
import os
import tempfile
import threading
import time
import unittest
import uuid
from pathlib import Path
from unittest.mock import Mock, patch

os.environ.setdefault("AIDASH_OUTPUT_BYTES", "1024")
import collector
from control import Rejected, Runner, SandboxGone, save, termination_time


def request(kind):
    return {"operation_id": str(uuid.uuid4()), "area_id": str(uuid.uuid4()), "epoch": 1,
            "usage_tenant": "acme", "digest": "fixture", "kind": kind, "code": "make test", "seconds": 60, "files": []}


class Admitted(Exception):
    """Raised once a request passed contract validation."""


class VerificationTest(unittest.TestCase):
    def test_starting_absent_pod_is_created_again_before_dispatch(self):
        for kind in ('shell', 'python'):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as root:
                runner = self.runner(root)
                operation = str(uuid.uuid4())
                record = {'operation_id':operation, 'request':request(kind), 'status':'starting',
                          'executed':False, 'startup_at':time.time(), 'wire_digest':'digest'}
                runner.get = Mock(return_value=record)
                runner.update = Mock(side_effect=lambda _, record=record, **changes: record.update(changes) or record.copy())
                runner.pod = Mock(side_effect=[None, RuntimeError('observation failed')])
                runner.manifest = Mock(return_value={})
                runner.kube.return_value = Mock(stderr=b'')
                with self.assertRaisesRegex(RuntimeError, 'observation failed'):
                    (runner._reconcile_python_once if kind == 'python' else runner._reconcile_shell_once)(operation)
                self.assertEqual(runner.kube.call_args.args[0][:2], ['create', '-f'])

    def test_startup_identity_mismatch_fails_without_waiting_or_deleting_replacement(self):
        for kind in ('shell', 'python'):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as root:
                runner = self.runner(root)
                operation = str(uuid.uuid4())
                record = {'operation_id':operation, 'request':request(kind), 'status':'starting',
                          'executed':False, 'startup_at':time.time(), 'wire_digest':'expected'}
                runner.get = Mock(side_effect=lambda _, record=record: record.copy())
                runner.update = Mock(side_effect=lambda _, record=record, **changes: record.update(changes) or record.copy())
                runner.pod = Mock(return_value={'metadata':{'uid':'replacement', 'annotations':{'aidash/digest':'other'}}})
                runner.active = {operation}
                with patch('time.sleep', side_effect=AssertionError('must not retry')):
                    (runner.reconcile_python if kind == 'python' else runner.reconcile)(operation)
                self.assertEqual(record['status'], 'failed')
                self.assertIn('identity', record['error']['message'])
                runner.close_allocation.assert_not_called()

    def test_python_checks_identity_of_pod_seen_after_create(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            operation = str(uuid.uuid4())
            record = {'operation_id':operation, 'request':request('python'), 'status':'starting',
                      'executed':False, 'startup_at':time.time(), 'wire_digest':'expected'}
            runner.get = Mock(side_effect=lambda _: record.copy())
            runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
            replacement = {'metadata':{'uid':'replacement', 'annotations':{'aidash/digest':'other'}}}
            runner.pod = Mock(side_effect=[None, replacement, replacement])
            runner.manifest = Mock(return_value={})
            runner.active = {operation}
            runner.reconcile_python(operation)
            self.assertEqual(record['status'], 'failed')
            self.assertIn('identity', record['error']['message'])
            runner.close_allocation.assert_not_called()

    def test_startup_outage_retries_then_fails_with_cleanup_at_persisted_deadline(self):
        for kind in ('shell', 'python'):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as root:
                runner = self.runner(root)
                operation = str(uuid.uuid4())
                record = {'operation_id':operation, 'request':request(kind), 'status':'starting',
                          'executed':False, 'startup_at':10, 'pod_uid':'uid'}
                runner.get = Mock(side_effect=lambda _, record=record: record.copy())
                runner.update = Mock(side_effect=lambda _, record=record, **changes: record.update(changes) or record.copy())
                runner.active = {operation}
                once = Mock(side_effect=RuntimeError('API unavailable'))
                setattr(runner, '_reconcile_python_once' if kind == 'python' else '_reconcile_shell_once', once)
                now = [128]
                with patch('time.time', side_effect=lambda now=now: now[0]), \
                     patch('time.sleep', side_effect=lambda _, now=now: now.__setitem__(0, now[0] + 1)):
                    (runner.reconcile_python if kind == 'python' else runner.reconcile)(operation)
                self.assertGreaterEqual(once.call_count, 2)
                self.assertEqual(record['status'], 'failed')
                self.assertTrue(record['termination_confirmed'])
                self.assertFalse(record['executed'])
                runner.allocation_harvested.assert_called_once()
                runner.close_allocation.assert_called_once()

    def starting(self, root, kind, *, executed, uploads=0, started=None):
        """A command whose Pod is running, seen at `started` seconds after its start limit began."""
        runner = self.runner(root)
        runner.config.update(processes=128, output_bytes=1024)
        operation, uid = str(uuid.uuid4()), str(uuid.uuid4())
        files = [{'file_id': str(uuid.uuid4()), 'path': f'input-{index}', 'scope': 'working', 'size': 3,
                  'digest': hashlib.sha256(b'abc').hexdigest()} for index in range(uploads)]
        (Path(root) / (operation + '.inputs')).mkdir()
        for file in files:
            (Path(root) / (operation + '.inputs') / file['file_id']).write_bytes(b'abc')
        clock = [1000.0]
        record = {'operation_id': operation, 'request': dict(request(kind), files=files), 'digest': 'fixture',
                  'wire_digest': 'digest', 'status': 'starting', 'executed': executed, 'cancel_requested': False,
                  'startup_at': clock[0] - (started if started is not None else 5), 'pod_uid': uid}
        if executed:
            record.update(dispatch_at=record['startup_at'] + 1)
        runner.get = Mock(side_effect=lambda _: record.copy())
        runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
        runner.active = {operation}
        runner.execution_limits = Mock(return_value={})
        runner.guard = Mock(return_value={})
        runner.allocations = Path(root) / 'allocations'
        runner.allocations.mkdir()
        save(runner.allocations / (uid + '.json'), {'resource_version': '101', 'receipt': 'present'})
        runner.pod = Mock(return_value={'metadata': {'uid': uid, 'resourceVersion': '101', 'annotations': {'aidash/digest': 'digest'}},
            'spec': {'nodeName': 'node', 'containers': [{'image': 'image'}]}, 'status': {'containerStatuses': [
                {'name': 'execution', 'containerID': 'containerd://cid', 'state': {'running': {}}},
                {'name': 'collector', 'state': {'running': {}}}]}})
        return runner, operation, record, clock

    def test_a_long_input_upload_is_not_charged_to_the_start_limit(self):
        # The start limit bounds the wait for the Pod; uploading inputs is not part of it (#709, decision 9).
        for kind in ('shell', 'python'):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as root:
                self._long_upload_is_not_charged(kind, root)

    def _long_upload_is_not_charged(self, kind, root):
        class Dispatched(Exception):
            pass

        runner, operation, record, clock = self.starting(root, kind, executed=False, uploads=4)
        seen = []

        def collector(_, action, data=None, **kwargs):
            seen.append(action[0])
            if action[0] == 'prepared':
                return Mock(stdout=b'{"digest":null}')
            if action[0] == 'write':
                clock[0] += 40
                return Mock(stdout=b'')
            # A shell command starts at its hydrate; a Python cell is sent to the node guard after it.
            if kind == 'shell':
                raise Dispatched
            return Mock(stdout=b'{}')

        def guard(_, action, **kwargs):
            if action == 'cell':
                raise Dispatched
            return {}

        runner.exec_collector = Mock(side_effect=collector)
        runner.guard = Mock(side_effect=guard)
        with patch('time.time', side_effect=lambda: clock[0]), patch('time.sleep'), self.assertRaises(Dispatched):
            (runner._reconcile_python_once if kind == 'python' else runner._reconcile_shell_once)(operation)
        self.assertEqual(seen, ['prepared'] + ['write'] * 4 + ['hydrate'])
        self.assertGreater(clock[0] - record['startup_at'], 120)
        self.assertTrue(record['executed'])

    def test_a_dispatched_command_gets_its_hydrate_and_its_cancel_after_the_start_limit(self):
        # Hydrate started the command but its reply was lost: the retry must send it again, and must reach the
        # cancel handling, however long ago the start limit began.
        class Delivered(Exception):
            pass

        with tempfile.TemporaryDirectory() as root:
            runner, operation, record, clock = self.starting(root, 'shell', executed=True, started=500)
            record['cancel_requested'] = True
            seen = []

            def collector(_, action, data=None, **kwargs):
                seen.append(action[0])
                if action[0] == 'prepared':
                    return Mock(stdout=json.dumps({'digest': record['request']['digest'], 'started_at': 990}).encode())
                if action[0] == 'hydrate':
                    return Mock(stdout=b'{"started_at":990}')
                raise Delivered

            runner.exec_collector = Mock(side_effect=collector)
            with patch('time.time', side_effect=lambda: clock[0]), patch('time.sleep'), self.assertRaises(Delivered):
                runner._reconcile_shell_once(operation)
            self.assertEqual(seen, ['prepared', 'hydrate', 'cancel'])
            self.assertEqual(record['status'], 'running')

    def test_uncertain_export_refusal_discards_immediately(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            runner.active = set()
            operation = str(uuid.uuid4())
            record = {'operation_id':operation, 'request':request('shell'), 'status':'uncertain',
                      'termination_confirmed':True, 'uncertain_at':100, 'stdout':'', 'files':[]}
            path = runner.path(operation)
            save(path, record)
            runner.operation_files = Mock(return_value=[path])
            runner.get = Mock(side_effect=lambda _: record.copy())
            runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
            runner.guard = Mock(side_effect=lambda _, action, **kwargs: {
                'status':{'termination_confirmed':True}, 'export':{'refused':{'reason':'symlink','path':'bad','limit':None}},
                'logs':{'stdout':'', 'truncated':True}}[action])
            with patch('time.time', return_value=101):
                runner.sweep_uncertain_output()
            self.assertTrue(record['output_discarded'])
            self.assertIn('symlink', record['error']['message'])
            runner.close_allocation.assert_called_once()
            self.assertNotIn('logs', [call.args[1] for call in runner.guard.call_args_list])

    def test_failed_start_cleanup_survives_an_outage_before_acknowledgement(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            runner.active = set()
            operation = str(uuid.uuid4())
            record = {'operation_id':operation, 'request':request('shell'), 'status':'failed',
                      'termination_confirmed':True, 'startup_cleanup_pending':True,
                      'wire_digest':'wire', 'digest':'digest'}
            path = runner.path(operation)
            save(path, record)
            runner.operation_files = Mock(return_value=[path])
            runner.get = Mock(side_effect=lambda _: record.copy())
            runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
            runner.pod = Mock(side_effect=[RuntimeError('API unavailable'), None])
            runner.sweep_uncertain_output()
            self.assertTrue(record['startup_cleanup_pending'])
            with self.assertRaises(Rejected):
                runner.acknowledge(operation, 'digest')
            runner.sweep_uncertain_output()
            self.assertFalse(record['startup_cleanup_pending'])
            runner.close_allocation.assert_called_once()

    def test_finishing_preview_is_bounded_without_losing_captured_output(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            output = b'a' * 20000 + b'failure at the end'
            record = {'status':'finishing', 'request':request('verify'),
                      'stdout':base64.b64encode(output).decode(), 'truncated':False, 'output_fetched_at':123}
            public = runner.public(record)
            preview = base64.b64decode(public['stdout'])
            self.assertEqual(len(preview), 16384)
            self.assertTrue(preview.endswith(b'failure at the end'))
            self.assertTrue(public['truncated'])
            self.assertEqual(public['output_fetched_at'], 123)
            self.assertEqual(base64.b64decode(record['stdout']), output)

    def test_confirmed_missing_pod_settles_with_unsaved_changes_error(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            operation = str(uuid.uuid4())
            record = {'operation_id': operation, 'request': request('shell'), 'status': 'running',
                      'executed': True, 'pod_uid': 'uid'}
            runner.get = Mock(side_effect=lambda _: record.copy())
            runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
            runner.pod = Mock(return_value=None)
            runner.active = {operation}
            runner.reconcile(operation)
            self.assertEqual(record['status'], 'uncertain')
            self.assertTrue(record['termination_confirmed'])
            self.assertEqual(record['error']['code'], 'SANDBOX_LOST')
            self.assertIn('not saved', record['error']['message'])

    def test_expired_command_retries_cancel_and_settles_after_exit(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            operation = str(uuid.uuid4())
            record = {'operation_id': operation, 'request': request('shell'), 'wire_digest': 'digest',
                      'status': 'running', 'executed': True, 'cancel_requested': False,
                      'pod_uid': 'uid', 'running_at': 1, 'started_at': 1}
            runner.get = Mock(side_effect=lambda _: record.copy())
            runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
            runner.active, runner.admission_operation = {operation}, None
            runner.config['output_bytes'] = 1024
            metadata = {'uid':'uid', 'annotations': {'aidash/digest':'digest'}}
            running = {'metadata':metadata, 'status': {'containerStatuses': [
                {'name':'execution','state':{'running':{}}}, {'name':'collector','state':{'running':{}}}]}}
            stopped = {'metadata':metadata, 'status': {'containerStatuses': [
                {'name':'execution','state':{'terminated':{'exitCode':137}}}, {'name':'collector','state':{'running':{}}}]}}
            runner.pod = Mock(side_effect=[running, running, running, running, running, running, running, stopped])
            runner.exec_collector = Mock(side_effect=[Mock(stdout=b'{"stdout":"","truncated":false,"offset":0}'),
                RuntimeError('cancel unavailable'), Mock(stdout=b''),
                Mock(stdout=b'{"stdout":"","truncated":false,"offset":0}'),
                Mock(stdout=b'{"stdout":"","truncated":false,"offset":0}'), Mock(stdout=b'{"files":[]}')])
            with patch('time.time', return_value=100), patch('time.sleep'):
                runner.reconcile(operation)
            # Nobody cancelled: the watchdog's stop settles from the exit code, and the collector's flag
            # decides whether the command timed out.
            self.assertEqual(record['status'], 'failed')
            self.assertEqual(record['exit_code'], 137)
            self.assertTrue(record['termination_confirmed'])
            self.assertEqual(sum(1 for call in runner.exec_collector.call_args_list if call.args[1] == ['cancel']), 2)

    def outage_shell(self, root, *, exit_code, expired):
        """A shell command that finished while Kubernetes was out of reach past its limit + 30 s."""
        runner = self.runner(root)
        operation = str(uuid.uuid4())
        record = {'operation_id': operation, 'request': request('shell'), 'wire_digest': 'digest',
                  'status': 'running', 'executed': True, 'cancel_requested': False, 'pod_uid': 'uid',
                  'running_at': 1, 'started_at': 1}
        runner.get = Mock(side_effect=lambda _: record.copy())
        runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
        runner.active, runner.admission_operation = {operation}, None
        runner.config['output_bytes'] = 1024
        runner.pod = Mock(return_value={'metadata': {'uid': 'uid', 'annotations': {'aidash/digest': 'digest'}},
            'status': {'containerStatuses': [{'name': 'execution', 'state': {'terminated': {'exitCode': exit_code}}},
                                             {'name': 'collector', 'state': {'running': {}}}]}})
        captured = {'stdout': '', 'truncated': False, 'deadline_expired': expired}
        runner.exec_collector = Mock(side_effect=[Mock(stdout=json.dumps(captured).encode()),
                                                  Mock(stdout=b'{"files":[]}')])
        runner.kube = Mock(return_value=Mock(stdout=b''))
        # The first pass fails: the API server is down until well past the limit.
        settle, failed = runner._reconcile_shell_once, []

        def once(name):
            if not failed:
                failed.append(name)
                raise RuntimeError('API unavailable')
            settle(name)

        runner._reconcile_shell_once = once
        with patch('time.time', return_value=100), patch('time.sleep'), contextlib.redirect_stderr(io.StringIO()):
            runner.reconcile(operation)
        return record

    def test_a_command_that_finished_during_an_outage_past_its_limit_is_not_cancelled(self):
        # The watchdog asks for a stop, but nobody cancelled: the command settles from how it ended.
        with tempfile.TemporaryDirectory() as root:
            record = self.outage_shell(root, exit_code=0, expired=False)
            self.assertEqual(record['status'], 'completed')
            self.assertFalse(record['cancel_requested'])
            self.assertFalse(record['deadline_expired'])
        with tempfile.TemporaryDirectory() as root:
            record = self.outage_shell(root, exit_code=2, expired=False)
            self.assertEqual(record['status'], 'failed')

    def test_a_command_still_running_at_its_limit_stays_timed_out_when_the_watchdog_stops_it(self):
        with tempfile.TemporaryDirectory() as root:
            record = self.outage_shell(root, exit_code=0, expired=True)
            self.assertEqual(record['status'], 'completed')
            self.assertTrue(record['deadline_expired'])

    def test_expired_cancel_uses_guard_if_collector_cannot_be_reached(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            operation = str(uuid.uuid4())
            record = {'operation_id':operation, 'request':request('shell'), 'wire_digest':'digest',
                      'status':'cancelling', 'executed':True, 'cancel_requested':True,
                      'pod_uid':'uid', 'running_at':1, 'started_at':1}
            runner.get = Mock(side_effect=lambda _: record.copy())
            runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
            runner.active = {operation}
            runner.pod = Mock(return_value={'metadata':{'uid':'uid', 'annotations':{'aidash/digest':'digest'}},
                'status':{'containerStatuses':[{'name':'execution', 'state':{'running':{}}},
                                               {'name':'collector', 'state':{'running':{}}}]}})
            runner.exec_collector = Mock(side_effect=RuntimeError('collector unavailable'))
            runner.guard = Mock(return_value={'termination_confirmed':True})
            runner.recover_stopped_shell = Mock(side_effect=lambda current: record.update(status='failed'))
            with patch('time.time', return_value=100):
                runner.reconcile(operation)
            runner.guard.assert_called_once_with(unittest.mock.ANY, 'terminate')
            runner.recover_stopped_shell.assert_called_once()
            self.assertTrue(record['termination_confirmed'])

    def test_cancel_during_outage_is_retried_before_deadline(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            operation = str(uuid.uuid4())
            record = {'operation_id':operation, 'request':request('shell'), 'wire_digest':'digest',
                      'status':'running', 'executed':True, 'cancel_requested':True,
                      'pod_uid':'uid', 'running_at':70, 'started_at':70}
            runner.get = Mock(side_effect=lambda _: record.copy())
            runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
            runner.active, runner.admission_operation = {operation}, None
            runner.config['output_bytes'] = 1024
            metadata = {'uid':'uid', 'annotations':{'aidash/digest':'digest'}}
            running = {'metadata':metadata, 'status':{'containerStatuses':[
                {'name':'execution','state':{'running':{}}}, {'name':'collector','state':{'running':{}}}]}}
            stopped = {'metadata':metadata, 'status':{'containerStatuses':[
                {'name':'execution','state':{'terminated':{'exitCode':137}}}, {'name':'collector','state':{'running':{}}}]}}
            runner.pod = Mock(side_effect=[running, running, running, running, stopped])
            runner.exec_collector = Mock(side_effect=[RuntimeError('unreachable'), Mock(stdout=b''),
                Mock(stdout=b'{"stdout":"","truncated":false,"offset":0}'),
                Mock(stdout=b'{"stdout":"","truncated":false,"offset":0}'), Mock(stdout=b'{"files":[]}')])
            with patch('time.time', return_value=100), patch('time.sleep'):
                runner.reconcile(operation)
            self.assertEqual(record['status'], 'cancelled')
            self.assertEqual(sum(1 for call in runner.exec_collector.call_args_list if call.args[1] == ['cancel']), 2)

    def test_past_stop_retention_requests_deletion_before_confirming_absence(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            operation = str(uuid.uuid4())
            record = {'operation_id':operation, 'request':request('shell'), 'status':'running',
                      'executed':True, 'cancel_requested':False, 'pod_uid':'uid', 'running_at':1}
            runner.get = Mock(side_effect=lambda _: record.copy())
            runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
            runner.active = {operation}
            runner._reconcile_shell_once = Mock(side_effect=[RuntimeError('API unavailable'), SandboxGone()])
            with patch('time.time', return_value=4000), patch('time.sleep'):
                runner.reconcile(operation)
            runner.close_allocation.assert_called_once()
            self.assertEqual(record['status'], 'uncertain')
            self.assertTrue(record['termination_confirmed'])

    def test_past_stop_retention_asks_for_deletion_once_a_minute(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            operation = str(uuid.uuid4())
            record = {'operation_id':operation, 'request':request('shell'), 'status':'running',
                      'executed':True, 'cancel_requested':False, 'pod_uid':'uid', 'running_at':1}
            runner.get = Mock(side_effect=lambda _: record.copy())
            runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
            runner.active = {operation}
            runner._reconcile_shell_once = Mock(side_effect=[RuntimeError('stuck')] * 5 + [SandboxGone()])
            with patch('time.time', return_value=4000), patch('time.sleep'), contextlib.redirect_stderr(io.StringIO()):
                runner.reconcile(operation)
            runner.close_allocation.assert_called_once()

    def test_late_harvest_reads_through_the_collector_of_a_finished_command(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            operation = str(uuid.uuid4())
            record = {'operation_id':operation, 'request':request('shell'), 'status':'uncertain',
                      'termination_confirmed':True, 'pod_uid':'uid', 'stdout':'', 'files':[]}
            data = b'kept'
            file = {'path':'out.txt', 'object_id':'object', 'size':len(data), 'digest':hashlib.sha256(data).hexdigest()}
            runner.config['output_bytes'] = 1024
            runner.get = Mock(side_effect=lambda _: record.copy())
            runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
            runner.pod = Mock(return_value={'metadata':{'uid':'uid'}, 'status':{'containerStatuses':[
                {'name':'execution', 'state':{'terminated':{'exitCode':0}}}, {'name':'collector', 'state':{'running':{}}}]}})
            # The guard cannot see a container that already finished.
            runner.guard = Mock(side_effect=RuntimeError('runsc state failed'))
            replies = {'logs':json.dumps({'stdout':'b3V0', 'truncated':False}).encode(),
                       'export':json.dumps({'files':[file]}).encode(), 'read':data}
            runner.exec_collector = Mock(side_effect=lambda _, action, **kwargs: Mock(stdout=replies[action[0]]))
            runner.harvest_stopped_output(record)
            self.assertTrue(record['output_harvested'])
            self.assertEqual(record['files'], [file])
            self.assertEqual((Path(root) / (operation + '.files') / 'object').read_bytes(), data)
            runner.guard.assert_not_called()
            runner.close_allocation.assert_called_once()

    def test_stopped_unreadable_output_waits_then_reopens_as_uncertain(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            operation = str(uuid.uuid4())
            now = [100.0]
            record = {'operation_id': operation, 'request': request('shell'), 'wire_digest': 'digest',
                      'status': 'running', 'executed': True, 'cancel_requested': False,
                      'pod_uid': 'uid', 'running_at': 90, 'started_at': 90}
            runner.get = Mock(side_effect=lambda _: record.copy())
            runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
            runner.active = {operation}
            runner.pod = Mock(return_value={'metadata': {'uid': 'uid', 'annotations': {'aidash/digest':'digest'}},
                'status': {'containerStatuses': [{'name':'execution', 'state': {'terminated': {'exitCode':0}}},
                                                {'name':'collector', 'state': {'running':{}}}]}})
            runner.exec_collector = Mock(side_effect=RuntimeError('collector unavailable'))
            runner.config['output_bytes'] = 1024
            with patch('control.STOP_WATCH_SECONDS', 3), patch('time.time', side_effect=lambda: now[0]), \
                 patch('time.sleep', side_effect=lambda seconds: now.__setitem__(0, now[0] + seconds)):
                runner.reconcile(operation)
            self.assertGreaterEqual(now[0], 103)
            self.assertEqual(record['status'], 'uncertain')
            self.assertTrue(record['termination_confirmed'])
            self.assertEqual(record['error']['code'], 'OUTPUT_LOST')
            runner.close_allocation.assert_not_called()

    def test_uncertain_retention_discards_only_after_pod_disappears(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            runner.active = set()
            runner.instance = 'instance'
            operation = str(uuid.uuid4())
            record = {'operation_id': operation, 'request': request('shell'), 'status': 'uncertain',
                      'termination_confirmed': True, 'uncertain_at': 1, 'pod_uid': 'uid',
                      'stdout': '', 'files': [], 'displays': [], 'digest': 'fixture'}
            path = runner.path(operation)
            save(path, record)
            runner.operation_files = Mock(return_value=[path])
            runner.get = Mock(side_effect=lambda _: record.copy())
            runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
            runner.pod = Mock(side_effect=[{'metadata': {'uid': 'uid'}}, None])
            runner.config['uncertain_retention_seconds'] = 10
            with patch('time.time', return_value=12):
                runner.sweep_uncertain_output()
                self.assertFalse(record.get('acknowledged', False))
                runner.sweep_uncertain_output()
            self.assertTrue(record['output_discarded'])
            self.assertTrue(record['acknowledged'])
            self.assertEqual(record['acknowledged_output_digest'], runner.output_digest(record))

    def test_uncertain_output_can_be_retained_later_and_closes_allocation(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            runner.active = set()
            operation = str(uuid.uuid4())
            record = {'operation_id':operation, 'request':request('shell'), 'status':'uncertain',
                      'termination_confirmed':True, 'uncertain_at':100, 'stdout':'', 'files':[]}
            path = runner.path(operation)
            save(path, record)
            runner.operation_files = Mock(return_value=[path])
            runner.get = Mock(side_effect=lambda _: record.copy())
            runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
            runner.guard = Mock(side_effect=lambda _, action, **kwargs: {
                'status':{'termination_confirmed':True}, 'export':{'files':[]},
                'logs':{'stdout':'cmVjb3ZlcmVk', 'truncated':True}}[action])
            with patch('time.time', return_value=101):
                runner.sweep_uncertain_output()
            self.assertEqual(record['status'], 'uncertain')
            self.assertTrue(record['output_harvested'])
            self.assertEqual(record['stdout'], 'cmVjb3ZlcmVk')
            runner.close_allocation.assert_called_once()

    def test_shell_retries_pod_and_preview_outages_before_completion(self):
        import itertools
        import subprocess
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            operation = str(uuid.uuid4())
            record = {'operation_id': operation, 'request': request('shell'), 'wire_digest': 'digest',
                      'status': 'running', 'executed': True, 'cancel_requested': False,
                      'pod_uid': 'uid', 'running_at': time.time(), 'started_at': time.time()}
            runner.get = Mock(side_effect=lambda _: record.copy())
            runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
            runner.active = {operation}
            runner.admission_operation = None
            runner.config['output_bytes'] = 1024
            running = {'metadata': {'uid': 'uid', 'annotations': {'aidash/digest': 'digest'}},
                'status': {'containerStatuses': [{'name': 'execution', 'state': {'running': {}}},
                                                {'name': 'collector', 'state': {'running': {}}}]}}
            stopped = {'metadata': running['metadata'], 'status': {'containerStatuses': [
                {'name': 'execution', 'state': {'terminated': {'exitCode': 0}}},
                {'name': 'collector', 'state': {'running': {}}}]}}
            timeout = subprocess.TimeoutExpired('kubectl', 45)
            runner.pod = Mock(side_effect=[timeout, running, running, running, running, running, stopped])
            content = b'new file contents'
            digest = hashlib.sha256(content).hexdigest()
            file = {'path':'output.txt', 'object_id':digest, 'digest':digest, 'size':len(content)}
            runner.exec_collector = Mock(side_effect=[Mock(stdout=b'{"stdout":"cHJldmlldw==","truncated":false,"offset":0}'),
                                                     timeout, Mock(stdout=b'{"stdout":"","truncated":false,"offset":0}'),
                                                     Mock(stdout=json.dumps({'files':[file]}).encode()),
                                                     Mock(stdout=content)])
            ticks = itertools.count(step=2)
            with patch('time.sleep'), patch('time.monotonic', side_effect=lambda: next(ticks)):
                runner.reconcile(operation)
            self.assertEqual(record['status'], 'completed')
            self.assertEqual(record['files'], [file])
            self.assertEqual((Path(root) / (operation + '.files') / digest).read_bytes(), content)
            self.assertTrue(record['termination_confirmed'])
            self.assertIn('output_fetched_at', runner.public(record))

    def runner(self, root):
        runner = object.__new__(Runner)
        runner.proven = True
        runner.root = Path(root)
        runner.lock = threading.RLock()
        runner.config = {"maximum_seconds": 600, "working_bytes": 1 << 20, "limits": {}}
        runner.kube = Mock()
        runner.writable_bytes = Mock(return_value=1 << 20)
        runner.update = Mock()
        runner.allocation_harvested = Mock()
        runner.close_allocation = Mock()
        return runner

    def test_only_known_command_kinds_are_accepted(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            runner.lock = threading.RLock()
            runner.kube_json = Mock(side_effect=Admitted)
            for kind in ("shell", "verify"):
                with self.assertRaises(Admitted):
                    runner.accept(request(kind))
            with self.assertRaises(Rejected) as rejected:
                runner.accept(request("publish"))
            self.assertEqual(rejected.exception.args[0], 400)

    def stopped(self, kind):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            calls = []

            def guard(record, action, **_):
                calls.append(action)
                if action == "status":
                    return {"termination_confirmed": True}
                if action == "export":
                    return {"files": []}
                return {"stdout": "", "truncated": False}

            runner.guard = guard
            record = {"operation_id": str(uuid.uuid4()), "request": request(kind)}
            save(runner.path(record["operation_id"]), record)
            runner.recover_stopped_shell(record)
            return calls, runner.update.call_args.kwargs

    def test_stopped_shell_export_refusal_is_final(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            refusal = {'reason': 'symlink', 'path': 'node_modules/.bin/tool', 'limit': None}
            runner.guard = Mock(side_effect=lambda record, action, **kw: {
                'status': {'termination_confirmed': True},
                'export': {'refused': refusal},
                'logs': {'stdout': '', 'truncated': True},
            }[action])
            record = {'operation_id': str(uuid.uuid4()), 'request': request('shell')}
            runner.recover_stopped_shell(record)
            recorded = runner.update.call_args.kwargs
            self.assertEqual(recorded['status'], 'failed')
            self.assertEqual(recorded['error']['code'], 'WORKING_EXPORT_REFUSED')
            self.assertEqual(recorded['error']['details'], refusal)
            self.assertTrue(recorded['termination_confirmed'])
            self.assertNotIn('files', recorded)

    def test_shell_refusal_finishes_without_recovery(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            operation = str(uuid.uuid4())
            record = {'operation_id': operation, 'request': request('shell'), 'wire_digest': 'digest',
                      'status': 'running', 'cancel_requested': False, 'started_at': time.time() - 1}
            runner.get = Mock(return_value=record)
            runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
            runner.lock = threading.RLock()
            runner.active = {operation}
            runner.config['output_bytes'] = 1024
            pod = {'metadata': {'uid': 'uid', 'annotations': {'aidash/digest': 'digest'}},
                   'status': {'containerStatuses': [
                       {'name': 'execution', 'state': {'terminated': {'exitCode': 0}}},
                       {'name': 'collector', 'state': {'running': {}}}]}}
            runner.pod = Mock(return_value=pod)
            refusal = {'reason': 'symlink', 'path': 'node_modules/.bin/tool', 'limit': None}
            def invoke(_, action, **kwargs):
                body = {'logs': {'stdout': '', 'truncated': False}, 'export': {'refused': refusal}}[action[0]]
                return Mock(stdout=json.dumps(body).encode())
            runner.exec_collector = Mock(side_effect=invoke)
            runner.recover_stopped_shell = Mock(side_effect=AssertionError('must not retry'))
            runner.reconcile(operation)
            self.assertEqual(record['status'], 'failed')
            self.assertEqual(record['error']['details'], refusal)
            self.assertTrue(record['termination_confirmed'])
            runner.recover_stopped_shell.assert_not_called()

    def test_python_refusal_stops_the_frozen_session(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            operation = str(uuid.uuid4())
            pod_uid = str(uuid.uuid4())
            record = {'operation_id': operation, 'area_id': str(uuid.uuid4()), 'session_id': str(uuid.uuid4()),
                      'pod_operation': 'existing', 'pod_uid': pod_uid, 'request': request('python'),
                      'digest': 'fixture', 'cancel_requested': False}
            runner.get = Mock(return_value=record)
            runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
            runner.lock = threading.RLock()
            runner.active = {operation}
            runner.config['idle_seconds'] = 60
            runner.execution_limits = Mock(return_value={})
            runner.allocations = Path(root) / 'allocations'
            runner.allocations.mkdir()
            save(runner.allocations / (pod_uid + '.json'), {'resource_version': '101'})
            runner.observe_allocation = Mock()
            runner.pod = Mock(return_value={'metadata': {'uid': pod_uid, 'resourceVersion': '101'},
                'spec': {'nodeName': 'node', 'containers': [{'image': 'image'}]},
                'status': {'containerStatuses': [
                    {'name': 'execution', 'containerID': 'containerd://cid', 'state': {'running': {}}},
                    {'name': 'collector', 'state': {'running': {}}}]}})
            refusal = {'reason': 'hard_link', 'path': 'a', 'limit': None}
            def guard(_, action, **kwargs):
                return {'limits': {}, 'cell': {}, 'cell_status': {'status': 'completed', 'exit_code': 0},
                        'freeze': {'writer_frozen': True}, 'export': {'refused': refusal},
                        'terminate': {'termination_confirmed': True}}[action]
            runner.guard = Mock(side_effect=guard)
            runner.reconcile_python(operation)
            self.assertEqual(record['status'], 'failed')
            self.assertEqual(record['error']['details'], refusal)
            self.assertTrue(record['termination_confirmed'])
            self.assertFalse(record['writer_frozen'])
            runner.observe_allocation.assert_called_once()
            runner.allocation_harvested.assert_called_once()
            runner.close_allocation.assert_called_once()
            self.assertIn('terminate', [call.args[1] for call in runner.guard.call_args_list])
            session = json.loads((Path(root) / ('session-' + record['session_id'] + '.json')).read_text())
            self.assertEqual(session['status'], 'stopped')

    def test_python_cell_status_timeout_recovers_without_replaying_code(self):
        import subprocess
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            operation, pod_uid = str(uuid.uuid4()), str(uuid.uuid4())
            record = {'operation_id':operation, 'area_id':str(uuid.uuid4()), 'session_id':str(uuid.uuid4()),
                      'pod_operation':'existing', 'pod_uid':pod_uid, 'request':request('python'),
                      'digest':'fixture', 'cancel_requested':False, 'status':'running', 'hydrated':True,
                      'executed':False, 'running_at':time.time()}
            runner.get = Mock(side_effect=lambda _: record.copy())
            runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
            runner.active = {operation}
            runner.config['idle_seconds'] = 60
            runner.execution_limits = Mock(return_value={})
            runner.allocations = Path(root) / 'allocations'
            runner.allocations.mkdir()
            save(runner.allocations / (pod_uid + '.json'), {'resource_version':'101', 'receipt':'present'})
            runner.pod = Mock(return_value={'metadata':{'uid':pod_uid, 'resourceVersion':'101'},
                'spec':{'nodeName':'node', 'containers':[{'image':'image'}]}, 'status':{'containerStatuses':[
                    {'name':'execution','containerID':'containerd://cid','state':{'running':{}}},
                    {'name':'collector','state':{'running':{}}}]}})
            calls = []
            def guard(_, action, **kwargs):
                calls.append(action)
                if action == 'cell_status' and calls.count(action) == 1:
                    raise subprocess.TimeoutExpired('kubectl', 45)
                return {'limits':{}, 'cell':{}, 'cell_status':{'status':'completed', 'exit_code':0, 'stdout':''},
                        'freeze':{'writer_frozen':True}, 'export':{'files':[]}}[action]
            runner.guard = Mock(side_effect=guard)
            with patch('time.sleep'):
                runner.reconcile_python(operation)
            self.assertEqual(record['status'], 'completed')
            self.assertTrue(record['writer_frozen'])
            self.assertEqual(calls.count('cell_status'), 2)
            self.assertEqual(calls.count('limits'), 1)

    def test_python_execution_stopped_during_outage_is_harvested(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            operation, pod_uid = str(uuid.uuid4()), str(uuid.uuid4())
            record = {'operation_id':operation, 'request':request('python'), 'status':'running',
                      'pod_operation':'existing', 'pod_uid':pod_uid, 'executed':True,
                      'cancel_requested':False, 'running_at':time.time(), 'stdout':''}
            runner.get = Mock(side_effect=lambda _: record.copy())
            runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
            runner.active = {operation}
            runner.pod = Mock(return_value={'metadata':{'uid':pod_uid}, 'status':{'containerStatuses':[
                {'name':'execution','state':{'terminated':{'exitCode':137}}},
                {'name':'collector','state':{'running':{}}}]}})
            runner.guard = Mock(return_value={'files':[]})
            runner.reconcile_python(operation)
            self.assertEqual(record['status'], 'failed')
            self.assertTrue(record['termination_confirmed'])
            self.assertTrue(record['output_harvested'])
            self.assertEqual(record['error']['code'], 'SESSION_LOST')

    def test_outage_does_not_turn_an_early_python_stop_into_a_timeout(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            operation, pod_uid = str(uuid.uuid4()), str(uuid.uuid4())
            now = time.time()
            from datetime import datetime, timezone
            finished = datetime.fromtimestamp(now - 100, timezone.utc).isoformat()
            record = {'operation_id': operation, 'request': request('python'), 'status': 'running',
                      'pod_operation': 'existing', 'pod_uid': pod_uid, 'executed': True,
                      'cancel_requested': False, 'running_at': now - 120, 'stdout': ''}
            runner.get = Mock(side_effect=lambda _: record.copy())
            runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
            runner.active = {operation}
            runner.pod = Mock(return_value={'metadata': {'uid': pod_uid}, 'status': {'containerStatuses': [
                {'name': 'execution', 'state': {'terminated': {'exitCode': 137, 'finishedAt': finished}}},
                {'name': 'collector', 'state': {'running': {}}}]}})
            runner.guard = Mock(return_value={'files': []})
            runner.reconcile_python(operation)
            self.assertFalse(record['deadline_expired'])
            self.assertEqual(record['error']['code'], 'SESSION_LOST')
    def frozen_cell(self, root, **changes):
        """A Python cell that froze its interpreter, then lost the way to its output."""
        runner = self.runner(root)
        operation, pod_uid = str(uuid.uuid4()), str(uuid.uuid4())
        record = {'operation_id': operation, 'request': request('python'), 'status': 'finishing',
                  'pod_operation': 'existing', 'pod_uid': pod_uid, 'executed': True, 'cancel_requested': False,
                  'writer_frozen': True, 'termination_confirmed': False, 'running_at': time.time(), 'stdout': ''}
        record.update(changes)
        runner.get = Mock(side_effect=lambda _: record.copy())
        runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
        runner.active = {operation}
        runner.guard = Mock(return_value={'termination_confirmed': True, 'files': []})
        runner.pod = Mock(return_value={'metadata': {'uid': pod_uid}, 'status': {'containerStatuses': [
            {'name': 'execution', 'state': {'terminated': {'exitCode': 137}}},
            {'name': 'collector', 'state': {'running': {}}}]}})
        return runner, operation, record

    def test_a_confirmed_stop_ends_the_frozen_session_at_every_recovery_step(self):
        # `python.rs::completed` keeps a reusable session for as long as `writer_frozen` is set, and its Pod is
        # deleted once the stop is confirmed. Each step that confirms the stop must clear the flag.
        with self.subTest(step='stop seen'), tempfile.TemporaryDirectory() as root:
            runner, operation, record = self.frozen_cell(root)
            with self.assertRaisesRegex(RuntimeError, 'collecting its output'):
                runner._reconcile_python_once(operation)
            self.assertTrue(record['termination_confirmed'])
            self.assertFalse(record['writer_frozen'])
        with self.subTest(step='stop delivered'), tempfile.TemporaryDirectory() as root:
            runner, operation, record = self.frozen_cell(root, cancel_requested=True)
            with self.assertRaisesRegex(RuntimeError, 'collecting its output'):
                runner._reconcile_python_once(operation)
            self.assertTrue(record['termination_confirmed'])
            self.assertFalse(record['writer_frozen'])
        with self.subTest(step='output harvested'), tempfile.TemporaryDirectory() as root:
            runner, operation, record = self.frozen_cell(root, stop_confirmed_at=time.time(), termination_confirmed=True)
            runner._reconcile_python_once = Mock(side_effect=RuntimeError('API unavailable'))
            runner.reconcile_python(operation)
            self.assertTrue(record['output_harvested'])
            self.assertFalse(record['writer_frozen'])
        with self.subTest(step='pod gone'), tempfile.TemporaryDirectory() as root:
            runner, operation, record = self.frozen_cell(root)
            runner.pod = Mock(return_value=None)
            runner.reconcile_python(operation)
            self.assertEqual(record['status'], 'uncertain')
            self.assertFalse(record['writer_frozen'])


    def test_recovered_python_pod_uses_guard_deadline_evidence(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            operation, pod_uid = str(uuid.uuid4()), str(uuid.uuid4())
            record = {'operation_id': operation, 'request': request('python'), 'status': 'running',
                      'pod_operation': 'existing', 'pod_uid': pod_uid, 'executed': True,
                      'cancel_requested': False, 'running_at': time.time() - 120, 'stdout': ''}
            runner.get = Mock(side_effect=lambda _: record.copy())
            runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
            runner.active = {operation}
            runner.pod = Mock(return_value={'metadata': {'uid': pod_uid}, 'status': {'containerStatuses': [
                {'name': 'execution', 'state': {'terminated': {'exitCode': 137}}},
                {'name': 'collector', 'state': {'running': {}}}]}})
            runner.guard = Mock(side_effect=lambda _, action, **kwargs: {'status': 'stopped', 'termination_confirmed': True, 'deadline_expired': True} if action == 'cell_status' else {'files': []})
            runner.reconcile_python(operation)
            self.assertTrue(record['deadline_expired'])

    def finished_shell(self, root, log, *, exported=None, expired=False):
        """A shell command that exited with no output, seen by the running controller."""
        runner = self.runner(root)
        runner.config['output_bytes'] = 1024
        operation = str(uuid.uuid4())
        record = {'operation_id': operation, 'request': request('shell'), 'wire_digest': 'digest',
                  'status': 'running', 'executed': True, 'cancel_requested': False,
                  'pod_uid': 'uid', 'running_at': 1, 'started_at': 1}
        runner.get = Mock(side_effect=lambda _: record.copy())
        runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
        runner.pod = Mock(return_value={'metadata': {'uid': 'uid', 'annotations': {'aidash/digest': 'digest'}},
            'status': {'containerStatuses': [
                {'name': 'execution', 'state': {'terminated': {'exitCode': 143}}},
                {'name': 'collector', 'state': {'running': {}}}]}})
        runner.kube = Mock(return_value=Mock(stdout=log))
        captured = {'stdout': '', 'truncated': False, 'deadline_expired': expired}
        runner.exec_collector = Mock(side_effect=[Mock(stdout=json.dumps(captured).encode()),
                                                  Mock(stdout=json.dumps(exported or {'files': []}).encode())])
        runner._reconcile_shell_once(operation)
        return record

    def test_an_empty_execution_log_is_no_error(self):
        for log in (b'', b' \n'):
            with tempfile.TemporaryDirectory() as root:
                record = self.finished_shell(root, log, expired=True)
                self.assertEqual(record['status'], 'failed')
                self.assertIsNone(record['error'])
                self.assertTrue(record['deadline_expired'])

    def test_an_execution_log_is_reported_under_its_own_code(self):
        with tempfile.TemporaryDirectory() as root:
            record = self.finished_shell(root, b'exec format error')
            self.assertEqual(record['error'], {'code': 'COMMAND_FAILED', 'message': 'exec format error'})
            self.assertFalse(record['deadline_expired'])

    def test_a_refused_save_keeps_the_word_on_the_deadline(self):
        refusal = {'reason': 'symlink', 'path': 'a', 'limit': None}
        with tempfile.TemporaryDirectory() as root:
            record = self.finished_shell(root, b'', exported={'refused': refusal}, expired=True)
            self.assertEqual(record['error']['code'], 'WORKING_EXPORT_REFUSED')
            self.assertTrue(record['deadline_expired'])

    def stopped_cell(self, root, *, running_at, expired=False, stopped_while_running=True, cancel_requested=False):
        """A Python cell whose sandbox stopped under the running controller."""
        runner = self.runner(root)
        operation, pod_uid = str(uuid.uuid4()), str(uuid.uuid4())
        record = {'operation_id': operation, 'area_id': str(uuid.uuid4()), 'session_id': str(uuid.uuid4()),
                  'pod_operation': 'existing', 'pod_uid': pod_uid, 'request': request('python'),
                  'digest': 'fixture', 'cancel_requested': cancel_requested, 'status': 'running', 'hydrated': True,
                  'executed': False, 'running_at': running_at, 'stdout': ''}
        runner.get = Mock(side_effect=lambda _: record.copy())
        runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
        runner.active = {operation}
        runner.config['idle_seconds'] = 60
        runner.execution_limits = Mock(return_value={})
        runner.allocations = Path(root) / 'allocations'
        runner.allocations.mkdir()
        save(runner.allocations / (pod_uid + '.json'), {'resource_version': '101', 'receipt': 'present'})
        runner.pod = Mock(return_value={'metadata': {'uid': pod_uid, 'resourceVersion': '101'},
            'spec': {'nodeName': 'node', 'containers': [{'image': 'image'}]}, 'status': {'containerStatuses': [
                {'name': 'execution', 'containerID': 'containerd://cid', 'state': {'running': {}}},
                {'name': 'collector', 'state': {'running': {}}}]}})
        answers = {'limits': {}, 'cell': {}, 'export': {'files': []}, 'terminate': {'termination_confirmed': True, 'deadline_expired': expired},
                   'cell_status': {'status': 'stopped', 'termination_confirmed': True, 'deadline_expired': expired}}
        runner.guard = Mock(side_effect=lambda _, action, **kwargs: answers[action])
        runner.reconcile_python(operation)
        return record

    def watched_cell(self, root, answers, *, running_at, clock=None, **changes):
        """A dispatched Python cell under the running controller; `answers` are the node guard's replies."""
        runner = self.runner(root)
        operation, pod_uid = str(uuid.uuid4()), str(uuid.uuid4())
        record = {'operation_id': operation, 'area_id': str(uuid.uuid4()), 'session_id': str(uuid.uuid4()),
                  'pod_operation': 'existing', 'pod_uid': pod_uid, 'request': request('python'),
                  'digest': 'fixture', 'cancel_requested': False, 'status': 'running', 'hydrated': True,
                  'executed': True, 'running_at': running_at, 'stdout': ''}
        record.update(changes)
        runner.get = Mock(side_effect=lambda _: record.copy())
        statuses = []

        def update(_, **fields):
            statuses.append(fields.get('status'))
            return record.update(fields) or record.copy()

        runner.update = Mock(side_effect=update)
        runner.active = {operation}
        runner.config['idle_seconds'] = 60
        runner.execution_limits = Mock(return_value={})
        runner.allocations = Path(root) / 'allocations'
        runner.allocations.mkdir()
        save(runner.allocations / (pod_uid + '.json'), {'resource_version': '101', 'receipt': 'present'})
        runner.pod = Mock(return_value={'metadata': {'uid': pod_uid, 'resourceVersion': '101'},
            'spec': {'nodeName': 'node', 'containers': [{'image': 'image'}]}, 'status': {'containerStatuses': [
                {'name': 'execution', 'containerID': 'containerd://cid', 'state': {'running': {}}},
                {'name': 'collector', 'state': {'running': {}}}]}})
        replies = dict({'cell': {}, 'export': {'files': []}, 'freeze': {'writer_frozen': True},
                        'terminate': {'termination_confirmed': True}}, **answers)
        runner.guard = Mock(side_effect=lambda _, action, **kwargs: replies[action])
        return runner, operation, record, statuses

    def after_outage(self, runner, operation):
        """Run the cell's controller through an outage that outlasts the cell's limit + 30 s."""
        settle, failed = runner._reconcile_python_once, []

        def once(name):
            if not failed:
                failed.append(name)
                raise RuntimeError('API unavailable')
            settle(name)

        runner._reconcile_python_once = once
        with patch('time.time', return_value=100), patch('time.sleep'), contextlib.redirect_stderr(io.StringIO()):
            runner.reconcile_python(operation)

    def test_a_python_cell_that_finished_during_an_outage_past_its_limit_keeps_its_result(self):
        with tempfile.TemporaryDirectory() as root:
            runner, operation, record, _ = self.watched_cell(
                root, {'cell_status': {'status': 'completed', 'exit_code': 0, 'stdout': 'done'}}, running_at=1)
            self.after_outage(runner, operation)
            self.assertTrue(record['limit_stop_requested'])
            self.assertFalse(record['cancel_requested'])
            self.assertEqual(record['status'], 'completed')
            self.assertTrue(record['writer_frozen'])
            self.assertNotIn('terminate', [call.args[1] for call in runner.guard.call_args_list])

    def test_guard_reports_stop_cause_seen_after_outage(self):
        for expired in (False, True):
            with self.subTest(expired=expired), tempfile.TemporaryDirectory() as root:
                runner, operation, record, _ = self.watched_cell(
                    root, {'cell_status': {'status': 'stopped', 'termination_confirmed': True,
                                           'deadline_expired': expired}}, running_at=1)
                self.after_outage(runner, operation)
                self.assertEqual(record['deadline_expired'], expired)
                self.assertEqual(record['error']['code'], 'SESSION_LOST')

    def test_unknown_stop_time_does_not_become_a_timeout_during_an_outage(self):
        with tempfile.TemporaryDirectory() as root:
            runner, operation, record, _ = self.watched_cell(
                root, {'cell_status': {'status': 'stopped', 'termination_confirmed': True}}, running_at=1)
            self.after_outage(runner, operation)
            self.assertFalse(record['deadline_expired'])
            self.assertEqual(record['error']['code'], 'SESSION_LOST')

    def test_a_python_cell_still_running_past_its_limit_is_stopped_and_timed_out(self):
        with tempfile.TemporaryDirectory() as root:
            runner, operation, record, _ = self.watched_cell(root, {'cell_status': {'status': 'running'}}, running_at=1)
            self.after_outage(runner, operation)
            self.assertFalse(record['cancel_requested'])
            self.assertEqual(record['status'], 'failed')
            self.assertTrue(record['termination_confirmed'])
            self.assertFalse(record['writer_frozen'])
            self.assertTrue(record['deadline_expired'])

    def test_a_python_cell_cancelled_by_someone_stays_cancelled_past_its_limit(self):
        with tempfile.TemporaryDirectory() as root:
            runner, operation, record, _ = self.watched_cell(
                root, {'cell_status': {'status': 'running'}}, running_at=1, cancel_requested=True, status='cancelling')
            with patch('time.time', return_value=100), patch('time.sleep'):
                runner.reconcile_python(operation)
            self.assertEqual(record['status'], 'cancelled')

    def test_a_python_reply_that_is_lost_keeps_the_cell_busy_until_its_stop_and_then_ends_its_pod(self):
        with tempfile.TemporaryDirectory() as root:
            clock = [1000.0]
            runner, operation, record, statuses = self.watched_cell(
                root, {'cell_status': {'status': 'uncertain'}}, running_at=clock[0])
            with patch('time.time', side_effect=lambda: clock[0]), \
                 patch('time.sleep', side_effect=lambda seconds: clock.__setitem__(0, clock[0] + 10)):
                runner.reconcile_python(operation)
            # Busy and unconfirmed while the cell is watched (the Working Area stays busy, not unavailable),
            # and uncertain only once the interpreter is known to have stopped.
            seen = [status for status in statuses if status]
            self.assertEqual(seen[-1], 'uncertain')
            self.assertEqual(seen.count('uncertain'), 1)
            self.assertLess(seen.index('cancelling'), seen.index('finishing'))
            self.assertEqual(record['status'], 'uncertain')
            self.assertTrue(record['termination_confirmed'])
            self.assertFalse(record['writer_frozen'])
            self.assertTrue(record['output_harvested'])
            self.assertGreaterEqual(clock[0], 1000 + 60 + 30)
            runner.close_allocation.assert_called_once()

    def test_a_python_reply_that_is_lost_waits_for_a_stop_that_is_not_confirmed(self):
        with tempfile.TemporaryDirectory() as root:
            clock = [1000.0]
            runner, operation, record, _ = self.watched_cell(
                root, {'cell_status': {'status': 'uncertain'}, 'terminate': {'termination_confirmed': False}},
                running_at=clock[0])
            asleep = []

            def sleep(seconds):
                asleep.append(seconds)
                clock[0] += 200
                if len(asleep) > 20:
                    raise SystemExit('still waiting')

            with patch('time.time', side_effect=lambda: clock[0]), patch('time.sleep', side_effect=sleep), \
                 contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                runner.reconcile_python(operation)
            self.assertEqual(record['status'], 'cancelling')
            self.assertFalse(record.get('termination_confirmed'))
            # Past the limit + 30 minutes the Pod gets a normal delete, as for any command whose stop stays unconfirmed.
            runner.close_allocation.assert_called()

    def test_a_python_cell_stopped_past_its_limit_timed_out(self):
        with tempfile.TemporaryDirectory() as root:
            record = self.stopped_cell(root, running_at=time.time() - 120, expired=True)
            self.assertTrue(record['deadline_expired'])
            self.assertEqual(record['status'], 'failed')

    def test_a_python_cell_stopped_before_its_limit_lost_its_session(self):
        with tempfile.TemporaryDirectory() as root:
            record = self.stopped_cell(root, running_at=time.time())
            self.assertFalse(record['deadline_expired'])
            self.assertEqual(record['error']['code'], 'SESSION_LOST')

    def test_a_python_cell_cancelled_past_its_limit_timed_out(self):
        with tempfile.TemporaryDirectory() as root:
            record = self.stopped_cell(root, running_at=time.time() - 120, expired=True, cancel_requested=True)
            self.assertTrue(record['deadline_expired'])

    def test_a_python_cell_cancelled_before_its_limit_stays_cancelled(self):
        with tempfile.TemporaryDirectory() as root:
            record = self.stopped_cell(root, running_at=time.time(), cancel_requested=True)
            self.assertFalse(record['deadline_expired'])
            self.assertEqual(record['status'], 'cancelled')
            self.assertEqual(record['error']['code'], 'CANCELLED')

    def test_python_execution_stopped_past_its_limit_during_outage_timed_out(self):
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            operation, pod_uid = str(uuid.uuid4()), str(uuid.uuid4())
            record = {'operation_id': operation, 'request': request('python'), 'status': 'running',
                      'pod_operation': 'existing', 'pod_uid': pod_uid, 'executed': True,
                      'cancel_requested': False, 'running_at': time.time() - 120, 'stdout': ''}
            runner.get = Mock(side_effect=lambda _: record.copy())
            runner.update = Mock(side_effect=lambda _, **changes: record.update(changes) or record.copy())
            runner.active = {operation}
            from datetime import datetime, timezone
            runner.pod = Mock(return_value={'metadata': {'uid': pod_uid}, 'status': {'containerStatuses': [
                {'name': 'execution', 'state': {'terminated': {'exitCode': 137, 'finishedAt': datetime.fromtimestamp(time.time(), timezone.utc).isoformat()}}},
                {'name': 'collector', 'state': {'running': {}}}]}})
            runner.guard = Mock(return_value={'files': []})
            runner.reconcile_python(operation)
            self.assertEqual(record['status'], 'failed')
            self.assertTrue(record['deadline_expired'])

    def test_a_python_cell_past_its_limit_whose_save_was_refused_says_so(self):
        refusal = {'reason': 'symlink', 'path': 'a', 'limit': None}
        with tempfile.TemporaryDirectory() as root:
            runner = self.runner(root)
            record = {'operation_id': str(uuid.uuid4()), 'request': request('python'), 'stdout': '',
                      'running_at': time.time() - 120, 'stop_confirmed_at': time.time(), 'cell_deadline_expired': True}
            runner.guard = Mock(return_value={'refused': refusal})
            self.assertIsNone(runner.collect_stopped_output(record))
            recorded = runner.update.call_args.kwargs
            self.assertEqual(recorded['error']['code'], 'WORKING_EXPORT_REFUSED')
            self.assertTrue(recorded['deadline_expired'])

    def test_a_stopped_check_records_its_output_without_exporting_files(self):
        calls, recorded = self.stopped("verify")
        self.assertNotIn("export", calls)
        self.assertEqual(recorded["files"], [])
        self.assertEqual(recorded["status"], "failed")

    def test_a_stopped_shell_still_exports_its_files(self):
        calls, _ = self.stopped("shell")
        self.assertIn("export", calls)


class OutputTailTest(unittest.TestCase):
    def logs(self, *arguments):
        stdout = io.StringIO()
        with patch.object(collector.sys, "argv", ["collector.py", "logs", "1000", "final", *arguments]), \
                contextlib.redirect_stdout(stdout):
            collector.logs(1000)
        captured = json.loads(stdout.getvalue())
        return base64.b64decode(captured["stdout"]), captured["truncated"]

    def test_a_check_reads_the_end_of_output_the_stored_prefix_dropped(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            with patch.multiple(collector, OUTPUT=root / "output.bin", OUTPUT_STATE=root / "output.state",
                                OUTPUT_TAIL=root / "output.tail", OUTPUT_LIMIT=8, TAIL_LIMIT=12):
                collector.append_output(b"compiling crates\n")
                collector.append_output(b"test result: FAILED")
                self.assertEqual(self.logs("tail"), (b"sult: FAILED", True))
                self.assertEqual(self.logs(), (b"compilin", True))

    def test_a_running_preview_returns_the_end_of_the_stored_output_with_its_offset(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            with patch.multiple(collector, OUTPUT=root / "output.bin", OUTPUT_STATE=root / "output.state",
                                OUTPUT_TAIL=root / "output.tail", OUTPUT_LIMIT=64, TAIL_LIMIT=64):
                collector.append_output(b"abcdef")
                stdout = io.StringIO()
                with patch.object(collector.sys, "argv", ["collector.py", "logs", "3", "window"]), \
                        contextlib.redirect_stdout(stdout):
                    collector.logs(3)
                captured = json.loads(stdout.getvalue())
                self.assertEqual(base64.b64decode(captured["stdout"]), b"def")
                self.assertEqual(captured["offset"], 3)

    def test_a_running_preview_starts_at_a_utf8_boundary(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            with patch.multiple(collector, OUTPUT=root / "output.bin", OUTPUT_STATE=root / "output.state",
                                OUTPUT_TAIL=root / "output.tail", OUTPUT_LIMIT=64, TAIL_LIMIT=64):
                collector.append_output("aébc".encode())
                stdout = io.StringIO()
                with patch.object(collector.sys, "argv", ["collector.py", "logs", "3", "window"]), \
                        contextlib.redirect_stdout(stdout):
                    collector.logs(3)
                captured = json.loads(stdout.getvalue())
                self.assertEqual((captured["offset"], base64.b64decode(captured["stdout"]).decode()), (3, "bc"))

    def test_a_running_preview_keeps_standalone_invalid_bytes(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            with patch.multiple(collector, OUTPUT=root / "output.bin", OUTPUT_STATE=root / "output.state",
                                OUTPUT_TAIL=root / "output.tail", OUTPUT_LIMIT=64, TAIL_LIMIT=64):
                collector.append_output(b"a\x80b")
                stdout = io.StringIO()
                with patch.object(collector.sys, "argv", ["collector.py", "logs", "2", "window"]), \
                        contextlib.redirect_stdout(stdout):
                    collector.logs(2)
                captured = json.loads(stdout.getvalue())
                self.assertEqual((captured["offset"], base64.b64decode(captured["stdout"])), (1, b"\x80b"))

    def test_python_preview_keeps_a_complete_utf8_tail_and_absolute_offset(self):
        import importlib
        with patch.dict("sys.modules", {"jupyter_client": Mock()}):
            python_cells = importlib.import_module("python_cells")
        offset, data = python_cells.output_window("aébc".encode(), 3)
        self.assertEqual((offset, data.decode()), (3, "bc"))

    def test_a_short_check_output_is_returned_whole(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            with patch.multiple(collector, OUTPUT=root / "output.bin", OUTPUT_STATE=root / "output.state",
                                OUTPUT_TAIL=root / "output.tail", OUTPUT_LIMIT=64, TAIL_LIMIT=64):
                collector.append_output(b"ok\n")
                self.assertEqual(self.logs("tail"), (b"ok\n", False))


class DeadlineTest(unittest.TestCase):
    """Whether a Check timed out is what its liveness deadline did, not its runtime."""

    def probe(self, root, control):
        handler = object.__new__(collector.Health)
        handler.server = Mock(startup_deadline=0)
        handler.send_response = Mock()
        handler.end_headers = Mock()
        with patch.multiple(collector, CONTROL=root / "control.json", EXPIRED=root / "deadline.expired"):
            collector.save(collector.CONTROL, control)
            handler.do_GET()
        return handler.send_response.call_args.args[0]

    def test_a_deadline_that_expires_while_running_is_recorded(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            self.assertEqual(self.probe(root, {"deadline": time.monotonic() + 60, "cancelled": False}), 200)
            self.assertFalse((root / "deadline.expired").exists())
            self.assertEqual(self.probe(root, {"deadline": time.monotonic() - 1, "cancelled": False}), 503)
            self.assertTrue((root / "deadline.expired").exists())

    def test_a_cancellation_is_not_a_timeout(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            self.assertEqual(self.probe(root, {"cancelled": True, "deadline": 0}), 503)
            self.assertFalse((root / "deadline.expired").exists())

    def watch(self, root, control, kind="verify", connections=0):
        with patch.multiple(collector, CONTROL=root / "control.json", REQUEST=root / "request.json",
                            EXPIRED=root / "deadline.expired", OUTPUT=root / "output.bin",
                            OUTPUT_STATE=root / "output.state", OUTPUT_TAIL=root / "output.tail"):
            collector.save(collector.CONTROL, control)
            collector.save(collector.REQUEST, {"kind": kind})
            collector.save(collector.OUTPUT_STATE, {"total": 0, "stored": 0, "connections": connections})
            return collector.expire_if_running()

    def test_a_deadline_passed_with_the_command_running_is_recorded_without_a_probe(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            self.assertFalse(self.watch(root, {"deadline": time.monotonic() + 60, "cancelled": False}, connections=1))
            self.assertFalse((root / "deadline.expired").exists())
            self.assertTrue(self.watch(root, {"deadline": time.monotonic() - 1, "cancelled": False}, connections=1))
            self.assertTrue((root / "deadline.expired").exists())

    def test_a_stream_closing_after_the_deadline_records_it(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            with patch.multiple(collector, CONTROL=root / "control.json", REQUEST=root / "request.json",
                                EXPIRED=root / "deadline.expired"):
                collector.save(collector.REQUEST, {"kind": "verify"})
                collector.save(collector.CONTROL, {"deadline": time.monotonic() + 60, "cancelled": False})
                collector.expire_if_late()
                self.assertFalse((root / "deadline.expired").exists())
                collector.save(collector.CONTROL, {"deadline": time.monotonic() - 1, "cancelled": True})
                collector.expire_if_late()
                self.assertFalse((root / "deadline.expired").exists())
                collector.save(collector.CONTROL, {"deadline": time.monotonic() - 1, "cancelled": False})
                collector.expire_if_late()
                self.assertTrue((root / "deadline.expired").exists())

    def test_a_command_that_exited_before_its_deadline_did_not_time_out(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            self.assertTrue(self.watch(root, {"deadline": time.monotonic() - 1, "cancelled": False}))
            self.assertFalse((root / "deadline.expired").exists())

    def test_the_watcher_records_no_timeout_for_cancellations_or_python_sessions(self):
        for control, kind in (({"deadline": 0, "cancelled": True}, "verify"),
                              ({"deadline": time.monotonic() - 1, "cancelled": False}, "python")):
            with tempfile.TemporaryDirectory() as root:
                root = Path(root)
                self.assertTrue(self.watch(root, control, kind=kind, connections=1))
                self.assertFalse((root / "deadline.expired").exists())

    def test_final_logs_report_the_deadline(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            with patch.multiple(collector, OUTPUT=root / "output.bin", OUTPUT_STATE=root / "output.state",
                                OUTPUT_TAIL=root / "output.tail", EXPIRED=root / "deadline.expired"):
                collector.append_output(b"slow\n")
                for expired in (False, True):
                    if expired:
                        (root / "deadline.expired").touch()
                    stdout = io.StringIO()
                    with patch.object(collector.sys, "argv", ["collector.py", "logs", "1000", "final", "tail"]), \
                            contextlib.redirect_stdout(stdout):
                        collector.logs(1000)
                    self.assertEqual(json.loads(stdout.getvalue())["deadline_expired"], expired)


if __name__ == "__main__":
    unittest.main()


class TerminationTimeTest(unittest.TestCase):
    """A Check is charged until its command stopped, not until the controller saw it."""

    STARTED = 1_800_000_000.4  # 2027-01-15T08:00:00.4Z

    def test_a_prompt_observation_keeps_its_sub_second_precision(self):
        stopped = termination_time({"finishedAt": "2027-01-15T08:00:02Z"}, self.STARTED, self.STARTED + 2.3)
        self.assertEqual(stopped, self.STARTED + 2.3)

    def test_a_late_observation_is_capped_by_the_containers_own_stop(self):
        # The controller restarted and saw the stop a minute late.
        stopped = termination_time({"finishedAt": "2027-01-15T08:00:02Z"}, self.STARTED, self.STARTED + 60)
        self.assertEqual(stopped, 1_800_000_003.0)

    def test_without_a_usable_stop_time_the_observation_stands(self):
        for terminated in ({}, {"finishedAt": None}, {"finishedAt": "not a time"},
                           # The node's clock is far behind the controller's.
                           {"finishedAt": "2027-01-15T07:59:00Z"}):
            with self.subTest(terminated=terminated):
                self.assertEqual(termination_time(terminated, self.STARTED, self.STARTED + 5), self.STARTED + 5)



class StartStampTest(unittest.TestCase):
    """A Check is charged from when its command was published, also after a restart."""

    def hydrate(self, root, incoming):
        stdout = io.StringIO()
        with patch.multiple(collector, CONTROL=root / "control.json", REQUEST=root / "request.json",
                            ROOTS={"working": root / "work", "references": root, "received": root}), \
                patch.object(collector.sys, "stdin", io.StringIO(json.dumps(incoming))), \
                contextlib.redirect_stdout(stdout):
            collector.hydrate()
        return json.loads(stdout.getvalue())

    def test_a_repeated_hydrate_returns_the_original_start(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            incoming = {"digest": "d", "kind": "verify", "code": "make test", "processes": 16,
                        "files": [], "working_bytes": 1 << 20, "seconds": 60}
            first = self.hydrate(root, incoming)
            self.assertTrue(first["accepted"])
            self.assertIsInstance(first["started_at"], float)
            self.assertEqual(self.hydrate(root, incoming)["started_at"], first["started_at"])
            # A controller that restarts after the command ended reads it back.
            stdout = io.StringIO()
            with patch.multiple(collector, CONTROL=root / "control.json", REQUEST=root / "request.json"), \
                    contextlib.redirect_stdout(stdout):
                collector.prepared()
            self.assertEqual(json.loads(stdout.getvalue()), {"digest": "d", "started_at": first["started_at"]})
