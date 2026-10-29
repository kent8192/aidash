"""Provider allocation receipts outlive cells, authority and operation acknowledgement.

This is the primary owner of the durable provider journal and delete barrier;
Kubernetes here is a fake API, not proof of a real gVisor deployment.
"""
import copy
import base64
import contextlib
import io
import hashlib
import json
import shutil
import http.client
from http.server import ThreadingHTTPServer
from pathlib import Path
import tempfile
import threading
import unittest
import uuid
from unittest.mock import Mock, patch
from control import Runner, Rejected, save, ActivityAggregate, Handler


class AllocationUsageTest(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.runner = self.restart()
        self.operation = str(uuid.uuid4())
        self.uid = str(uuid.uuid4())
        self.record = dict(operation_id=self.operation, area_id=str(uuid.uuid4()), epoch=1,
                           wire_digest='request-digest', pod_uid=self.uid, request={'usage_tenant':'acme'})
        self.pod = {'metadata':{'uid':self.uid, 'resourceVersion':'101', 'name':'operation-' + self.operation,
                               'annotations':{'aidash/digest':'request-digest', 'aidash/area':self.record['area_id'], 'aidash/epoch':'1'},
                               'finalizers':['aidash.dev/allocation-receipt','another.example/retention']},
                    'spec':{'containers':[{'name':'execution'}, {'name':'collector'}]},
                    'status':{'startTime':'2026-10-01T23:59:00Z', 'containerStatuses':[
                        {'name':'execution', 'state':{'running':{'startedAt':'2026-10-01T23:59:01Z'}}},
                        {'name':'collector', 'state':{'running':{'startedAt':'2026-10-01T23:59:01Z'}}}]}}
        save(self.runner.path(self.operation), self.record)

    def restart(self):
        runner = object.__new__(Runner)
        runner.root = self.root
        runner.lock = threading.RLock()
        runner.config = {'namespace':'test-runner'}
        runner.instance = '00000000-0000-0000-0000-000000000001'
        fresh_instance = not (self.root / 'instance').exists()
        if fresh_instance:
            save(self.root / 'instance', runner.instance)
        runner.kube = Mock()
        runner.aggregate = ActivityAggregate()
        runner.active = set()
        runner.initialize_allocations(fresh_instance=fresh_instance)
        return runner

    def finish(self, record=None, pod=None):
        record, pod = record or self.record, pod or self.pod
        for index, status in enumerate(pod['status']['containerStatuses']):
            status['state'] = {'terminated':{'finishedAt':f'2026-10-02T00:02:0{index}Z'}}
        self.runner.observe_allocation(record, pod)

    def distinct_origin(self, pod):
        operation = str(uuid.uuid4())
        pod['metadata']['name'] = 'operation-'+operation
        record = dict(self.record, operation_id=operation, pod_uid=pod['metadata']['uid'])
        save(self.runner.path(operation), record)
        return record

    def test_one_allocation_includes_reused_cells_and_survives_restart(self):
        self.runner.observe_allocation(self.record, self.pod)
        reused = dict(self.record, operation_id=str(uuid.uuid4()), epoch=2, wire_digest='next-cell-wire')
        reused['pod_operation'] = self.operation
        self.runner.observe_allocation(reused, self.pod)
        self.assertEqual(self.runner.allocation_receipts()['receipts'], [])
        self.finish()
        runner = self.restart()
        page = runner.allocation_receipts()
        [receipt] = page['receipts']
        self.assertEqual(receipt['allocation_id'], self.uid)
        self.assertEqual(receipt['operation_id'], self.operation)
        self.assertEqual(receipt['epoch'], 1)
        self.assertEqual(receipt['tenant'], 'acme')
        self.assertEqual(receipt['started_at'], '2026-10-01T23:59:00Z')
        self.assertEqual(receipt['ended_at'], '2026-10-02T00:02:01Z')
        with self.assertRaises(Rejected):
            runner.acknowledge_allocation(self.uid, 'wrong-instance', receipt['digest'])
        with self.assertRaises(Rejected):
            runner.acknowledge_allocation(self.uid, runner.instance, 'wrong-digest')
        self.assertEqual(len(runner.allocation_receipts()['receipts']), 1)
        runner.acknowledge_allocation(self.uid, runner.instance, receipt['digest'])
        runner.acknowledge_allocation(self.uid, runner.instance, receipt['digest'])
        self.assertEqual(self.restart().allocation_receipts()['receipts'], [])

    def test_initial_observation_rejects_replacement_uid_or_untrusted_origin_metadata(self):
        unbound = dict(self.record, status='starting')
        unbound.pop('pod_uid')
        save(self.runner.path(self.operation), unbound)
        self.runner.kube = Mock(return_value=Mock(stdout=json.dumps(self.pod).encode()))
        self.runner.pod(self.operation)
        self.assertEqual(self.runner.get(self.operation)['pod_uid'], self.uid)
        for mismatch in ('uid', 'digest', 'area', 'epoch'):
            with self.subTest(mismatch=mismatch):
                pod = copy.deepcopy(self.pod)
                pod['metadata']['uid'] = str(uuid.uuid4())
                record = dict(self.record)
                if mismatch != 'uid':
                    record['pod_uid'] = pod['metadata']['uid']
                    pod['metadata']['annotations']['aidash/'+mismatch] = 'replacement-value'
                save(self.runner.path(self.operation), record)
                self.runner.kube = Mock(return_value=Mock(stdout=json.dumps(pod).encode()))
                with self.assertRaises(Rejected):
                    self.runner.pod(self.operation)
                self.assertFalse((self.root / 'allocations' / (pod['metadata']['uid']+'.json')).exists())
                self.runner.allocation_housekeeping()
                self.assertFalse(any(call.args[0][0] in ('delete', 'patch') for call in self.runner.kube.call_args_list))
        self.assertEqual(self.runner.allocation_receipts()['receipts'], [])

    def test_housekeeping_polls_the_provider_without_the_runner_lock(self):
        self.runner.observe_allocation(self.record, self.pod)
        path = self.runner.allocations / (self.uid + '.json')
        held = []
        def kube(arguments, data=None, **_):
            if arguments[0] == 'get':
                # Another thread must be able to take the Runner-wide lock meanwhile.
                acquired = []
                def contend():
                    acquired.append(self.runner.lock.acquire(timeout=1))
                    if acquired[0]:
                        self.runner.lock.release()
                thread = threading.Thread(target=contend)
                thread.start()
                thread.join()
                held.append(not acquired[0])
                return Mock(stdout=json.dumps(self.pod).encode())
            return Mock(stdout=b'')
        self.runner.kube = kube
        self.runner.allocation_housekeeping()
        self.assertEqual(held, [False])
        # A concurrent observation that advanced the journal discards the stale snapshot.
        def advancing(arguments, data=None, **_):
            if arguments[0] == 'get':
                allocation = json.loads(path.read_bytes())
                allocation['resource_version'] = '102'
                save(path, allocation)
                stale = copy.deepcopy(self.pod)
                stale['metadata']['resourceVersion'] = '100'
                return Mock(stdout=json.dumps(stale).encode())
            return Mock(stdout=b'')
        self.runner.kube = advancing
        self.runner.allocation_housekeeping()
        self.assertEqual(json.loads(path.read_bytes())['resource_version'], '102')

    def test_housekeeping_waits_for_durable_shell_output_before_cleanup(self):
        content = b'output still being harvested'
        file = dict(path='saved.txt', object_id=hashlib.sha256(content).hexdigest(),
                    digest=hashlib.sha256(content).hexdigest(), size=len(content))
        self.record.update(status='running', digest='command', container_id='a'*64,
                           cancel_requested=False, executed=True, started_at=1790000000,
                           request={'usage_tenant':'acme', 'kind':'shell', 'files':[]})
        save(self.runner.path(self.operation), self.record)
        self.pod['status']['containerStatuses'][0]['state'] = {
            'terminated':{'finishedAt':'2026-10-02T00:02:00Z', 'exitCode':0}}
        self.runner.config.update(output_bytes=1024, working_bytes=1<<20)
        self.runner.guard = Mock(return_value={'termination_confirmed':True})
        entered, release = threading.Event(), threading.Event()
        deleted = []
        def kube(arguments, data=None, **_):
            if arguments[0] == 'get':
                return Mock(stdout=json.dumps(self.pod).encode())
            if arguments[0] == 'delete':
                deleted.append(True)
                self.pod['metadata']['deletionTimestamp'] = '2026-10-02T00:02:01Z'
                self.pod['status']['containerStatuses'][1]['state'] = {
                    'terminated':{'finishedAt':'2026-10-02T00:02:01Z'}}
            return Mock(stdout=b'')
        def collector(operation, action, *_args, **_kwargs):
            if action[0] == 'logs':
                return Mock(stdout=json.dumps({'stdout':'saved logs', 'truncated':False}).encode())
            if action[0] == 'export':
                return Mock(stdout=json.dumps({'files':[file]}).encode())
            if action[0] == 'read':
                entered.set()
                if not release.wait(5):
                    raise RuntimeError('test harvest was not released')
                if deleted:
                    raise RuntimeError('collector was deleted during output harvest')
                return Mock(stdout=content)
            raise AssertionError(action)
        self.runner.kube, self.runner.exec_collector = kube, collector
        worker = threading.Thread(target=self.runner.reconcile, args=(self.operation,), daemon=True)
        worker.start()
        try:
            self.assertTrue(entered.wait(5), 'actual reconcile owner reached file harvesting')
            self.runner.allocation_housekeeping()
            self.assertEqual(deleted, [], 'execution stop alone cannot authorize collector deletion')
        finally:
            release.set()
            worker.join(5)
        self.assertFalse(worker.is_alive())
        self.assertEqual(self.runner.get(self.operation)['status'], 'completed')
        self.assertEqual(self.runner.get(self.operation)['stdout'], 'saved logs')
        self.assertTrue(self.runner.get(self.operation)['output_harvested'])
        self.assertEqual((self.root / (self.operation+'.files') / file['object_id']).read_bytes(), content)
        self.assertEqual(deleted, [True])
        self.runner.acknowledge(self.operation, 'command')
        runner = self.restart()
        allocation = json.loads((self.root / 'allocations' / (self.uid+'.json')).read_bytes())
        self.assertTrue(allocation['cleanup_ready'])
        runner.kube = kube
        runner.allocation_housekeeping()
        self.assertEqual(len(runner.allocation_receipts()['receipts']), 1)

    def test_stopped_uncertain_ack_preserves_outcome_and_independent_allocation(self):
        # Shared with the Rust digest owner, including Unicode and omitted displays.
        self.assertEqual(self.runner.output_digest({'operation_id': '00000000-0000-0000-0000-000000000002', 'digest': 'intent', 'stdout': 'aGk=', 'files': [{'path': '結果.txt', 'size': 2, 'object_id': 'file', 'digest': 'hash'}], 'truncated': True}),
                         '0b9c0a8283390da0eb7572f59944f0a7dff0f8503cf29dc25bb74dc2d5d62503')
        self.runner.observe_allocation(self.record, self.pod)
        self.finish()
        before = self.runner.allocation_receipts()
        self.record.update(status='uncertain', digest='command', termination_confirmed=True,
                           stdout='saved partial output', files=[], displays=[], truncated=True, output_harvested=True, effects_may_have_occurred=True,
                           error='output harvesting failed')
        save(self.runner.path(self.operation), self.record)
        for proof in (False, None, 'true', 1):
            with self.subTest(proof=proof):
                self.runner.update(self.operation, termination_confirmed=proof)
                with self.assertRaises(Rejected):
                    self.runner.acknowledge(self.operation, 'command')
                self.assertEqual(self.runner.get(self.operation)['stdout'], 'saved partial output')
        self.runner.update(self.operation, termination_confirmed=True)
        with self.assertRaises(Rejected):
            self.runner.acknowledge(self.operation, 'wrong-digest')
        self.assertEqual(self.runner.cancel(self.operation)['status'], 'uncertain')
        proof = self.runner.output_digest(self.runner.get(self.operation))
        for runner in (self.runner, self.restart()):
            outcome = runner.acknowledge(self.operation, 'command', proof)
            self.assertEqual(outcome['status'], 'uncertain')
            self.assertTrue(outcome['acknowledged'])
            self.assertTrue(outcome['effects_may_have_occurred'])
            self.assertEqual(outcome['error'], 'output harvesting failed')
            self.assertEqual(outcome['stdout'], '')
            self.assertEqual(runner.allocation_receipts(), before)

    def test_actual_stopped_uncertain_harvest_retries_before_adopted_ack(self):
        content = b'recovered file'
        file = dict(path='saved.txt', object_id=hashlib.sha256(content).hexdigest(),
                    digest=hashlib.sha256(content).hexdigest(), size=len(content))
        self.record.update(status='running', digest='command', container_id='a'*64,
                           cancel_requested=False, executed=True, started_at=1790000000,
                           stdout='', displays=[], files=[], truncated=False,
                           request={'usage_tenant':'acme', 'kind':'shell', 'files':[]})
        save(self.runner.path(self.operation), self.record)
        self.pod['status']['containerStatuses'][0]['state'] = {
            'terminated':{'finishedAt':'2026-10-02T00:02:00Z', 'exitCode':0}}
        self.runner.config.update(output_bytes=1024, working_bytes=1<<20)
        self.runner.kube = Mock(return_value=Mock(stdout=json.dumps(self.pod).encode()))
        self.runner.exec_collector = Mock(side_effect=RuntimeError('collector harvest failed'))
        def failed_guard(record, action, **_):
            if action == 'status':
                return {'termination_confirmed':True}
            raise RuntimeError('guard harvest failed')
        self.runner.guard = failed_guard
        self.runner.reconcile(self.operation)
        saved = self.runner.get(self.operation)
        self.assertEqual(saved['status'], 'uncertain')
        self.assertIs(saved['termination_confirmed'], True)
        error = saved['error']
        with self.assertRaises(RuntimeError):
            self.runner.acknowledge(self.operation, 'command')
        self.assertFalse(self.runner.get(self.operation).get('output_harvested', False))
        self.assertFalse(self.operation in self.runner.active)
        allocation = json.loads((self.root / 'allocations' / (self.uid+'.json')).read_bytes())
        self.assertFalse(allocation.get('cleanup_ready', False))
        entered, release = threading.Event(), threading.Event()
        def guard(record, action, **_):
            if action == 'status':
                return {'termination_confirmed':True}
            if action == 'export':
                return {'files':[file]}
            if action == 'logs':
                return {'stdout':base64.b64encode(b'recovered stdout').decode()}
            if action == 'read':
                entered.set()
                if not release.wait(5):
                    raise RuntimeError('test harvest not released')
                return {'data':base64.b64encode(content).decode()}
            raise AssertionError(action)
        self.runner.guard = guard
        failures = []
        def ack():
            try:
                self.runner.acknowledge(self.operation, 'command')
            except Exception as error:
                failures.append(error)
        worker = threading.Thread(target=ack)
        worker.start()
        try:
            self.assertTrue(entered.wait(5))
            with self.assertRaises(Rejected):
                self.runner.acknowledge(self.operation, 'command')
            self.assertFalse(self.runner.get(self.operation).get('output_harvested', False))
        finally:
            release.set()
            worker.join(5)
        self.assertFalse(worker.is_alive())
        self.assertEqual(len(failures), 1)
        self.assertIsInstance(failures[0], Rejected)
        harvested = self.runner.get(self.operation)
        self.assertEqual(harvested['status'], 'uncertain')
        self.assertEqual(harvested['error'], error)
        self.assertIs(harvested['output_harvested'], True)
        self.assertEqual((self.root / (self.operation+'.files') / file['object_id']).read_bytes(), content)
        # Crash before server adoption: the runner still retains every payload.
        runner = self.restart()
        public = runner.public(runner.get(self.operation))
        self.assertEqual(public['stdout'], base64.b64encode(b'recovered stdout').decode())
        for proof in (None, 'wrong-output-digest'):
            with self.assertRaises(Rejected):
                runner.acknowledge(self.operation, 'command', proof)
        self.assertEqual(base64.b64decode(runner.output_chunk(self.operation, file['object_id'], 0)['data']), content)
        self.finish()
        before = runner.allocation_receipts()
        self.assertEqual(len(before['receipts']), 1)
        # A crash after the saved ACK but before payload unlink retries drain.
        with patch('pathlib.Path.unlink', side_effect=OSError('interrupted unlink')):
            with self.assertRaises(OSError):
                runner.acknowledge(self.operation, 'command', public['output_digest'])
        self.assertTrue((self.root / (self.operation+'.files')).exists())
        runner = self.restart()
        acknowledged = runner.acknowledge(self.operation, 'command', public['output_digest'])
        self.assertEqual(acknowledged['status'], 'uncertain')
        self.assertEqual(acknowledged['error'], error)
        runner = self.restart()
        runner.acknowledge(self.operation, 'command', public['output_digest'])
        self.assertEqual(runner.allocation_receipts()['receipts'], before['receipts'])
        self.assertFalse((self.root / (self.operation+'.files')).exists())

    def test_stopped_python_harvest_preserves_cell_stdout_and_displays(self):
        self.record.update(status='uncertain', digest='cell', termination_confirmed=True,
                           stdout='cHl0aG9u', displays=[{'mime':'image/png','data':'saved'}], files=[], truncated=True,
                           error='cell harvest failed', request={'usage_tenant':'acme','kind':'python','files':[]})
        save(self.runner.path(self.operation), self.record)
        self.runner.config.update(working_bytes=1<<20)
        def guard(record, action, **_):
            if action == 'status':
                return {'termination_confirmed':True}
            if action == 'export':
                return {'files':[]}
            raise AssertionError('Python must not read shell logs: '+action)
        self.runner.guard = guard
        self.runner.observe_allocation(self.record, self.pod)
        with self.assertRaises(Rejected):
            self.runner.acknowledge(self.operation, 'cell')
        saved = self.runner.get(self.operation)
        self.assertEqual(saved['stdout'], self.record['stdout'])
        self.assertEqual(saved['displays'], self.record['displays'])
        self.assertEqual(saved['status'], 'uncertain')
        self.assertEqual(saved['error'], 'cell harvest failed')

    def test_restart_replays_a_durable_harvest_handoff_after_operation_ack(self):
        self.record.update(status='completed', digest='cell', container_id='a'*64,
                           pod_operation=self.operation, writer_frozen=True, termination_confirmed=False,
                           output_harvested=True, request={'usage_tenant':'acme', 'kind':'python', 'files':[]})
        save(self.runner.path(self.operation), self.record)
        self.runner.observe_allocation(self.record, self.pod)
        self.runner.acknowledge(self.operation, 'cell')
        runner = self.restart()
        runner.recover()
        allocation = json.loads((self.root / 'allocations' / (self.uid+'.json')).read_bytes())
        self.assertTrue(allocation['cleanup_ready'])
        self.assertTrue(allocation['idle_cleanup_ready'])
        runner.guard = Mock(return_value={'termination_confirmed':True})
        runner.kube = Mock(return_value=Mock(stdout=json.dumps(self.pod).encode()))
        runner.allocation_housekeeping()
        self.assertTrue(any(call.args[0][0] == 'delete' for call in runner.kube.call_args_list))

    def test_restart_stops_non_idle_shell_after_durable_harvest_before_delete(self):
        self.record.update(status='completed', digest='command', container_id='a'*64,
                           writer_frozen=False, termination_confirmed=True, output_harvested=True,
                           request={'usage_tenant':'acme', 'kind':'shell', 'files':[]})
        save(self.runner.path(self.operation), self.record)
        self.pod['status']['containerStatuses'][0]['state'] = {
            'terminated':{'finishedAt':'2026-10-02T00:02:00Z', 'exitCode':0}}
        self.runner.observe_allocation(self.record, self.pod)
        allocation_path = self.root / 'allocations' / (self.uid+'.json')
        allocation = json.loads(allocation_path.read_bytes())
        self.assertFalse(allocation.get('stop_requested'))
        self.assertFalse(allocation.get('receipt'))
        runner = self.restart()
        runner.recover()
        allocation = json.loads(allocation_path.read_bytes())
        self.assertTrue(allocation['cleanup_ready'])
        self.assertFalse(allocation['idle_cleanup_ready'])
        self.assertFalse(allocation.get('stop_requested'))
        self.assertFalse(allocation.get('receipt'))
        runner.guard = Mock(side_effect=AssertionError('non-idle cleanup cannot require an idle guard'))
        deleted = []
        def kube(arguments, data=None, **_):
            if arguments[0] == 'get':
                return Mock(stdout=json.dumps(self.pod).encode())
            if arguments[0] == 'delete':
                deleted.append(True)
                self.pod['metadata']['deletionTimestamp'] = '2026-10-02T00:02:01Z'
                self.pod['status']['containerStatuses'][1]['state'] = {
                    'terminated':{'finishedAt':'2026-10-02T00:02:01Z'}}
            if arguments[0] == 'patch':
                self.pod['metadata']['finalizers'].remove('aidash.dev/allocation-receipt')
            return Mock(stdout=b'')
        runner.kube = kube
        runner.allocation_housekeeping()
        self.assertEqual(deleted, [True], 'replayed non-idle harvest must resume deletion')
        self.assertEqual(runner.allocation_receipts()['receipts'], [])
        runner.allocation_housekeeping()
        [receipt] = runner.allocation_receipts()['receipts']
        self.assertEqual(receipt['allocation_id'], self.uid)
        self.assertEqual(receipt['started_at'], '2026-10-01T23:59:00Z')
        self.assertEqual(receipt['ended_at'], '2026-10-02T00:02:01Z')
        self.assertEqual(self.pod['metadata']['finalizers'], ['another.example/retention'])
        self.assertTrue(json.loads(allocation_path.read_bytes())['released'])

    def test_reused_python_admission_revokes_the_old_cleanup_handoff_before_dispatch(self):
        self.record.update(status='completed', termination_confirmed=False, writer_frozen=True,
                           container_id='a'*64, pod_operation=self.operation, output_harvested=True)
        save(self.runner.path(self.operation), self.record)
        self.runner.observe_allocation(self.record, self.pod)
        self.runner.allocation_harvested(self.record, idle=True)
        allocation_path = self.root / 'allocations' / (self.uid+'.json')
        settled_snapshot = json.loads(allocation_path.read_bytes())
        session = str(uuid.uuid4())
        save(self.root / ('session-'+session+'.json'), dict(
            status='frozen', area_id=self.record['area_id'], manifest=self.runner.file_identity([]),
            pod_operation=self.operation, pod_uid=self.uid, container_id='a'*64))
        save(self.root / ('area-'+self.record['area_id']+'.json'),
             dict(epoch=1, operation_id=self.operation))
        self.runner.config.update(node_guard=True, maximum_seconds=60, working_bytes=1<<20, image='fixture')
        self.runner.kube_json = Mock(return_value={'items':[self.pod]})
        self.runner.kube = Mock(return_value=Mock(stdout=json.dumps(self.pod).encode()))
        self.runner.guard = Mock(return_value={'termination_confirmed':True})
        # Run housekeeping at the dispatch boundary while only the old guard is known.
        self.runner.start = lambda _operation: self.runner.allocation_housekeeping()
        operation = str(uuid.uuid4())
        self.runner.accept(dict(operation_id=operation, area_id=self.record['area_id'], epoch=2,
                                session_id=session, usage_tenant='acme', digest='cell',
                                kind='python', code='print(2)', seconds=10, files=[]))
        self.assertFalse(any(call.args[0][0] in ('delete', 'patch') for call in self.runner.kube.call_args_list))
        allocation = json.loads((self.root / 'allocations' / (self.uid+'.json')).read_bytes())
        self.assertEqual(allocation['work_operation'], operation)
        self.assertFalse(allocation['cleanup_ready'])
        # Restart between the accepted operation save and handoff revocation.
        save(allocation_path, settled_snapshot)
        runner = self.restart()
        runner.kube = Mock(return_value=Mock(stdout=json.dumps(self.pod).encode()))
        runner.guard = Mock(return_value={'termination_confirmed':True})
        with patch('control.threading.Thread') as thread:
            thread.return_value.start.side_effect = runner.allocation_housekeeping
            runner.recover()
        self.assertFalse(any(call.args[0][0] in ('delete', 'patch') for call in runner.kube.call_args_list))
        self.assertFalse(json.loads(allocation_path.read_bytes())['cleanup_ready'])

    def test_missing_or_contradictory_provider_times_remain_visible_without_receipt(self):
        for broken in ('missing_start', 'missing_finish', 'backwards'):
            with self.subTest(broken=broken):
                pod = copy.deepcopy(self.pod)
                pod['metadata']['uid'] = str(uuid.uuid4())
                record = self.distinct_origin(pod)
                if broken == 'missing_start':
                    pod['status'].pop('startTime')
                if broken == 'backwards':
                    pod['status']['startTime'] = '2027-01-01T00:00:00Z'
                for status in pod['status']['containerStatuses']:
                    status['state'] = {'terminated':{'finishedAt':'2026-10-02T00:02:00Z'}}
                if broken == 'missing_finish':
                    pod['status']['containerStatuses'][0]['state']['terminated'] = {}
                self.runner.observe_allocation(record, pod)
        page = self.runner.allocation_receipts()
        self.assertEqual(page['receipts'], [])
        self.assertEqual(page['pending_evidence'], 3)

    def test_bounded_pages_retain_each_unacknowledged_receipt(self):
        for _ in range(20):
            pod = copy.deepcopy(self.pod)
            pod['metadata']['uid'] = str(uuid.uuid4())
            record = self.distinct_origin(pod)
            self.runner.observe_allocation(record, pod)
            self.finish(record, pod)
        page = self.runner.allocation_receipts(limit=7)
        self.assertEqual(len(page['receipts']), 7)
        second = self.runner.allocation_receipts(after=page['next_after'], limit=7)
        self.assertEqual(len(second['receipts']), 7)
        self.assertFalse({r['allocation_id'] for r in page['receipts']} &
                         {r['allocation_id'] for r in second['receipts']})

    def test_untrusted_probe_flag_cannot_exclude_a_tenant_allocation(self):
        self.record['request']['admission_probe'] = True
        self.runner.observe_allocation(self.record, self.pod)
        self.finish()
        self.assertEqual(len(self.runner.allocation_receipts()['receipts']), 1)

    def test_contradictory_provider_evidence_quarantines_immutable_receipts_across_restart(self):
        self.runner.observe_allocation(self.record, self.pod)
        self.finish()
        before = self.runner.allocation_receipts()['receipts'][0]
        self.runner.allocation_harvested(self.record)
        path = self.root / 'allocations' / (self.uid+'.json')
        verified_snapshot = json.loads(path.read_bytes())
        for mutation in ('start', 'finish', 'missing', 'backwards', 'running'):
            with self.subTest(mutation=mutation):
                save(path, verified_snapshot)
                self.runner = self.restart()
                pod = copy.deepcopy(self.pod)
                terminal = pod['status']['containerStatuses'][0]['state']['terminated']
                if mutation == 'start':
                    pod['status']['startTime'] = '2026-10-01T23:59:01Z'
                elif mutation == 'finish':
                    terminal['finishedAt'] = '2028-01-01T00:00:00Z'
                elif mutation == 'missing':
                    terminal.pop('finishedAt')
                elif mutation == 'backwards':
                    terminal['finishedAt'] = '2025-01-01T00:00:00Z'
                else:
                    pod['status']['containerStatuses'][0]['state'] = {'running':{'startedAt':'2026-10-01T23:59:01Z'}}
                logs = io.StringIO()
                with contextlib.redirect_stdout(logs):
                    try:
                        self.runner.observe_allocation(self.record, pod)
                    except Rejected:
                        pass
                self.assertEqual(self.runner.allocation_receipts()['receipts'], [])
                self.assertEqual(self.runner.allocation_receipts()['pending_evidence'], 1)
                self.assertEqual(json.loads(path.read_bytes())['receipt'], before)
                with self.assertRaises(Rejected):
                    self.runner.acknowledge_allocation(self.uid, self.runner.instance, before['digest'])
                self.runner = self.restart()
                self.assertEqual(self.runner.allocation_receipts()['receipts'], [])
                with contextlib.redirect_stdout(logs):
                    self.runner.close_allocation(self.record)
                self.assertFalse(any(call.args[0][0] in ('delete', 'patch') for call in self.runner.kube.call_args_list))
                diagnostic = json.loads(logs.getvalue().splitlines()[-1])
                self.assertEqual(diagnostic['status'], 'evidence_unavailable')
                self.assertTrue(diagnostic['pending_reason'])

    def test_delete_barrier_keeps_finalizer_until_provider_stop_is_durable(self):
        self.runner.observe_allocation(self.record, self.pod)
        self.runner.allocation_harvested(self.record)
        self.runner.close_allocation(self.record)
        self.assertFalse(any(call.args[0][0] == 'patch' for call in self.runner.kube.call_args_list))
        self.finish()
        self.runner.close_allocation(self.record)
        self.assertTrue(any(call.args[0][0] == 'patch' for call in self.runner.kube.call_args_list))

    def test_stopped_serve_hands_its_allocation_to_finalizer_cleanup(self):
        self.record.update(container_id='a'*64, serve={'status':'running', 'pod_host':'10.0.0.9'},
                           request={'usage_tenant':'acme', 'kind':'serve'})
        save(self.runner.path(self.operation), self.record)
        self.runner.observe_allocation(self.record, self.pod)
        self.runner.guard = Mock(return_value={'termination_confirmed':True})
        self.runner.startup_log = Mock(return_value='listening')
        self.runner.pod = Mock(return_value=None)
        self.runner.stop_serve(self.runner.get(self.operation), 'cancelled', 'cancelled')
        allocation_path = self.root / 'allocations' / (self.uid+'.json')
        self.assertTrue(json.loads(allocation_path.read_bytes())['cleanup_ready'])
        self.finish()
        self.runner.close_allocation(self.record)
        self.assertTrue(any(call.args[0][0] == 'patch' for call in self.runner.kube.call_args_list))
        allocation = json.loads(allocation_path.read_bytes())
        self.assertTrue(allocation['released'])
        self.assertNotIn('cleanup_blocker', allocation)

    def test_restart_replays_the_serve_cleanup_handoff_lost_after_the_terminal_save(self):
        self.record.update(container_id='a'*64, serve={'status':'running', 'pod_host':'10.0.0.9'},
                           request={'usage_tenant':'acme', 'kind':'serve'})
        save(self.runner.path(self.operation), self.record)
        self.runner.observe_allocation(self.record, self.pod)
        self.runner.guard = Mock(return_value={'termination_confirmed':True})
        self.runner.startup_log = Mock(return_value='listening')
        self.runner.pod = Mock(return_value=None)
        # Crash after the terminal save, before the handoff call.
        self.runner.allocation_harvested = Mock()
        self.runner.stop_serve(self.runner.get(self.operation), 'cancelled', 'cancelled')
        allocation_path = self.root / 'allocations' / (self.uid+'.json')
        self.assertFalse(json.loads(allocation_path.read_bytes()).get('cleanup_ready', False))
        runner = self.restart()
        runner.recover()
        self.assertTrue(json.loads(allocation_path.read_bytes())['cleanup_ready'])

    def test_cleanup_preconditions_preserve_a_same_name_replacement_and_expose_the_blocker(self):
        with patch('allocation_usage.time.time', return_value=1000):
            self.runner.observe_allocation(self.record, self.pod)
        self.runner.allocation_harvested(self.record)
        replacement = copy.deepcopy(self.pod)
        replacement['metadata'].update(uid=str(uuid.uuid4()), resourceVersion='202')
        changed = []
        sent_preconditions = []
        def kube(arguments, data=None, **_):
            if arguments[0] == 'delete':
                options = json.loads(data) if '--raw' in arguments else {}
                expected = options.get('preconditions', {})
                sent_preconditions.append(expected)
                if any(replacement['metadata'][key] != value for key, value in expected.items()):
                    raise RuntimeError('409 Conflict: delete precondition failed')
                changed.append('deleted')
            if arguments[0] == 'patch':
                changed.append('patched')
            return Mock(stdout=b'')
        self.runner.kube = kube
        logs = io.StringIO()
        with patch('allocation_usage.time.time', return_value=1015), contextlib.redirect_stdout(logs):
            self.runner.close_allocation(self.record)
        self.assertEqual(changed, [], 'cleanup must never mutate the replacement Pod')
        self.assertEqual(sent_preconditions, [{'uid':self.uid, 'resourceVersion':'101'}])
        allocation = json.loads((self.root / 'allocations' / (self.uid + '.json')).read_bytes())
        self.assertNotIn('receipt', allocation)
        self.assertNotIn('released', allocation)
        self.assertEqual(self.runner.allocation_receipts()['pending_evidence'], 1)
        [diagnostic] = [json.loads(line) for line in logs.getvalue().splitlines()]
        self.assertEqual(diagnostic['allocation_id'], self.uid)
        self.assertEqual(diagnostic['runner_instance'], self.runner.instance)
        self.assertEqual(diagnostic['status'], 'pending_evidence')
        self.assertEqual(diagnostic['age_seconds'], 15)
        self.assertEqual(diagnostic['cleanup_attempts'], 1)
        self.assertIn('precondition', diagnostic['cleanup_blocker'])

    def test_same_uid_resource_version_change_retries_without_deleting_updated_metadata(self):
        self.runner.observe_allocation(self.record, self.pod)
        self.runner.allocation_harvested(self.record)
        deleted = []
        def kube(arguments, data=None, **_):
            if arguments[0] == 'delete':
                options = json.loads(data) if '--raw' in arguments else {}
                if options.get('preconditions', {}).get('resourceVersion') == '101':
                    raise RuntimeError('409 Conflict: resource version changed')
                deleted.append(True)
            return Mock(stdout=b'')
        self.runner.kube = kube
        with contextlib.redirect_stdout(io.StringIO()):
            self.runner.close_allocation(self.record)
        self.assertEqual(deleted, [])
        self.pod['metadata']['resourceVersion'] = '202'
        self.runner.observe_allocation(self.record, self.pod)
        with contextlib.redirect_stdout(io.StringIO()):
            self.runner.close_allocation(self.record)
        self.assertEqual(deleted, [True])

    def test_idle_physical_stop_is_delivered_after_operation_ack_without_operation_scan(self):
        self.record.update(container_id='a'*64, pod_operation=self.operation,
                           digest='digest', status='completed', termination_confirmed=False, writer_frozen=True)
        save(self.runner.path(self.operation), self.record)
        self.runner.observe_allocation(self.record, self.pod)
        self.runner.allocation_harvested(self.record, idle=True)
        self.runner.acknowledge(self.operation, 'digest')
        self.assertEqual(self.runner.get(self.operation)['request'], {})
        self.runner.guard = Mock(return_value={'termination_confirmed':True})
        def kube(arguments, data=None, **_):
            if arguments[0] == 'get':
                return Mock(stdout=json.dumps(self.pod).encode())
            if arguments[0] == 'delete':
                self.pod['metadata']['deletionTimestamp'] = '2026-10-02T00:02:00Z'
                for status in self.pod['status']['containerStatuses']:
                    status['state'] = {'terminated':{'finishedAt':'2026-10-02T00:02:01Z'}}
            if arguments[0] == 'patch':
                patch = json.loads(arguments[-1])
                self.assertEqual(patch[-1]['value'], ['another.example/retention'])
                self.pod['metadata']['finalizers'] = patch[-1]['value']
            return Mock(stdout=b'')
        self.runner.kube = kube
        self.runner.allocation_housekeeping()
        self.assertEqual(self.runner.allocation_receipts()['receipts'], [])
        self.runner.allocation_housekeeping()
        [receipt] = self.runner.allocation_receipts()['receipts']
        self.assertEqual(receipt['allocation_id'], self.uid)
        self.assertEqual(receipt['ended_at'], '2026-10-02T00:02:01Z')
        self.assertEqual(self.pod['metadata']['finalizers'], ['another.example/retention'])

    def test_receipt_ack_does_not_skip_the_delete_barrier_after_a_restart(self):
        self.runner.observe_allocation(self.record, self.pod)
        self.finish()
        self.runner.allocation_harvested(self.record)
        receipt = self.runner.allocation_receipts()['receipts'][0]
        self.runner.acknowledge_allocation(self.uid, self.runner.instance, receipt['digest'])
        runner = self.restart()
        runner.kube = Mock(return_value=Mock(stdout=json.dumps(self.pod).encode()))
        runner.allocation_housekeeping()
        self.assertTrue(any(call.args[0][0] == 'patch' for call in runner.kube.call_args_list))

    def test_allocation_journal_identity_loss_is_unavailable(self):
        self.runner.observe_allocation(self.record, self.pod)
        (self.root / 'allocations' / 'instance').unlink()
        with self.assertRaises((Rejected, FileNotFoundError)):
            self.runner.allocation_receipts()
        with self.assertRaises(RuntimeError):
            self.restart()

    def test_whole_allocation_journal_loss_is_unavailable_after_restart_with_the_same_root_instance(self):
        self.runner.observe_allocation(self.record, self.pod)
        self.finish()
        root_instance = (self.root / 'instance').read_bytes()
        for restored_empty_directory, lost_anchor in ((False, False), (True, False), (False, True)):
            with self.subTest(restored_empty_directory=restored_empty_directory, lost_anchor=lost_anchor):
                if (self.root / 'allocations').exists():
                    shutil.rmtree(self.root / 'allocations')
                if restored_empty_directory:
                    (self.root / 'allocations').mkdir()
                if lost_anchor:
                    (self.root / 'allocation-journal').unlink()
                with self.assertRaisesRegex(RuntimeError, 'allocation journal.*unavailable'):
                    self.restart()
                self.assertEqual((self.root / 'instance').read_bytes(), root_instance)
                with self.assertRaises(Rejected):
                    self.runner.allocation_receipts()

    def test_marker_loss_between_listing_and_ack_retains_the_receipt_until_manual_recovery(self):
        self.runner.observe_allocation(self.record, self.pod)
        self.finish()
        [receipt] = self.runner.allocation_receipts()['receipts']
        marker = self.root / 'allocations' / 'instance'
        for replacement in (None, str(uuid.uuid4())):
            with self.subTest(replacement=replacement):
                if replacement is None:
                    marker.unlink()
                else:
                    save(marker, replacement)
                try:
                    with self.assertRaises(Rejected) as unavailable:
                        self.runner.acknowledge_allocation(self.uid, self.runner.instance, receipt['digest'])
                finally:
                    save(marker, self.runner.instance)
                self.assertEqual(unavailable.exception.code, 503)
                persisted = json.loads((self.root / 'allocations' / (self.uid + '.json')).read_bytes())
                self.assertFalse(persisted.get('acknowledged', False))
                self.assertEqual(self.runner.allocation_receipts()['receipts'], [receipt])
        runner = self.restart()
        runner.acknowledge_allocation(self.uid, runner.instance, receipt['digest'])
        self.assertEqual(runner.allocation_receipts()['receipts'], [])

    def test_receipt_transport_authenticates_and_acknowledges_only_the_exact_receipt(self):
        self.runner.observe_allocation(self.record, self.pod)
        self.finish()
        self.runner.token = 'x' * 32
        server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        server.runner = self.runner
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(thread.join)
        self.addCleanup(server.shutdown)
        connection = http.client.HTTPConnection(*server.server_address)
        self.addCleanup(connection.close)
        connection.request('GET', '/v1/allocations/receipts')
        response = connection.getresponse()
        self.assertEqual(response.status, 401)
        response.read()
        headers = {'Authorization':'Bearer ' + self.runner.token}
        connection.request('GET', '/v1/allocations/receipts?limit=1', headers=headers)
        response = connection.getresponse()
        self.assertEqual(response.status, 200)
        [receipt] = json.loads(response.read())['receipts']
        connection.request('POST', '/v1/allocations/' + self.uid + '/ack',
                           json.dumps({'instance':self.runner.instance, 'digest':receipt['digest']}), headers)
        response = connection.getresponse()
        self.assertEqual(response.status, 200)
        ack = json.loads(response.read())
        self.assertEqual(ack, {'allocation_id':self.uid, 'instance':self.runner.instance,
                               'digest':receipt['digest'], 'acknowledged':True})

    def test_origin_operation_polling_cannot_replace_a_reused_python_guard_epoch(self):
        self.record.update(container_id='a'*64, pod_operation=self.operation)
        self.runner.observe_allocation(self.record, self.pod)
        reused = dict(self.record, operation_id=str(uuid.uuid4()), epoch=2)
        self.runner.observe_allocation(reused, self.pod)
        self.runner.allocation_harvested(reused, idle=True)
        self.runner.observe_allocation(self.record, self.pod)
        self.runner.kube = Mock(return_value=Mock(stdout=json.dumps(self.pod).encode()))
        epochs = []
        def guard(record, *_):
            epochs.append(record['epoch'])
            return {'termination_confirmed':False}
        self.runner.guard = guard
        self.runner.allocation_housekeeping()
        self.assertEqual(epochs, [2])


if __name__ == '__main__':
    unittest.main()
