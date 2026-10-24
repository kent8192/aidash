"""Provider allocation receipts outlive cells, authority and operation acknowledgement.

This is the primary owner of the durable provider journal and delete barrier;
Kubernetes here is a fake API, not proof of a real gVisor deployment.
"""
import copy
import contextlib
import io
import json
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
                               'annotations':{'aidash/digest':'request-digest'},
                               'finalizers':['aidash.dev/allocation-receipt','another.example/retention']},
                    'spec':{'containers':[{'name':'execution'}, {'name':'collector'}]},
                    'status':{'startTime':'2026-10-01T23:59:00Z', 'containerStatuses':[
                        {'name':'execution', 'state':{'running':{'startedAt':'2026-10-01T23:59:01Z'}}},
                        {'name':'collector', 'state':{'running':{'startedAt':'2026-10-01T23:59:01Z'}}}]}}

    def restart(self):
        runner = object.__new__(Runner)
        runner.root = self.root
        runner.lock = threading.RLock()
        runner.config = {'namespace':'test-runner'}
        runner.instance = '00000000-0000-0000-0000-000000000001'
        runner.kube = Mock()
        runner.aggregate = ActivityAggregate()
        runner.initialize_allocations()
        return runner

    def finish(self):
        for index, status in enumerate(self.pod['status']['containerStatuses']):
            status['state'] = {'terminated':{'finishedAt':f'2026-10-02T00:02:0{index}Z'}}
        self.runner.observe_allocation(self.record, self.pod)

    def test_one_allocation_includes_reused_cells_and_survives_restart(self):
        self.runner.observe_allocation(self.record, self.pod)
        reused = dict(self.record, operation_id=str(uuid.uuid4()), epoch=2)
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

    def test_missing_or_contradictory_provider_times_remain_visible_without_receipt(self):
        for broken in ('missing_start', 'missing_finish', 'backwards'):
            with self.subTest(broken=broken):
                pod = copy.deepcopy(self.pod)
                pod['metadata']['uid'] = str(uuid.uuid4())
                if broken == 'missing_start':
                    pod['status'].pop('startTime')
                if broken == 'backwards':
                    pod['status']['startTime'] = '2027-01-01T00:00:00Z'
                for status in pod['status']['containerStatuses']:
                    status['state'] = {'terminated':{'finishedAt':'2026-10-02T00:02:00Z'}}
                if broken == 'missing_finish':
                    pod['status']['containerStatuses'][0]['state']['terminated'] = {}
                self.runner.observe_allocation(self.record, pod)
        page = self.runner.allocation_receipts()
        self.assertEqual(page['receipts'], [])
        self.assertEqual(page['pending_evidence'], 3)

    def test_bounded_pages_retain_each_unacknowledged_receipt(self):
        for _ in range(20):
            self.pod['metadata']['uid'] = str(uuid.uuid4())
            self.runner.observe_allocation(self.record, self.pod)
            self.finish()
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

    def test_changed_provider_identity_cannot_overwrite_an_immutable_receipt(self):
        self.runner.observe_allocation(self.record, self.pod)
        self.finish()
        before = self.runner.allocation_receipts()['receipts'][0]
        self.pod['status']['containerStatuses'][0]['state']['terminated']['finishedAt'] = '2028-01-01T00:00:00Z'
        with self.assertRaises(Rejected):
            self.runner.observe_allocation(self.record, self.pod)
        self.assertEqual(self.runner.allocation_receipts()['receipts'], [before])

    def test_delete_barrier_keeps_finalizer_until_provider_stop_is_durable(self):
        self.runner.observe_allocation(self.record, self.pod)
        self.runner.close_allocation(self.record)
        self.assertFalse(any(call.args[0][0] == 'patch' for call in self.runner.kube.call_args_list))
        self.finish()
        self.runner.close_allocation(self.record)
        self.assertTrue(any(call.args[0][0] == 'patch' for call in self.runner.kube.call_args_list))

    def test_cleanup_preconditions_preserve_a_same_name_replacement_and_expose_the_blocker(self):
        with patch('allocation_usage.time.time', return_value=1000):
            self.runner.observe_allocation(self.record, self.pod)
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

    def test_marker_loss_between_listing_and_ack_retains_the_receipt_until_operator_recovery(self):
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
