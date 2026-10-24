import base64
import fcntl
import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import node_guard


class GuardMigrationTest(unittest.TestCase):
    def test_live_legacy_deadline_is_adopted_and_unverified_record_is_retained(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            current = root / 'release'
            legacy = root / 'legacy'
            current.mkdir()
            legacy.mkdir()
            live = {'pod_uid': 'live', 'deadline': 1, 'terminated': False}
            stale = {'pod_uid': 'other-release', 'deadline': 1, 'terminated': False}
            (legacy / 'live.json').write_text(json.dumps(live))
            (legacy / 'other.json').write_text(json.dumps(stale))

            def validate(record):
                if record['pod_uid'] != 'live':
                    raise ValueError('wrong sandbox namespace')

            with patch.object(node_guard, 'ROOT', current), patch.object(node_guard, 'LEGACY_ROOT', legacy), patch.object(node_guard, 'validate', side_effect=validate):
                node_guard.migrate_legacy()

            self.assertEqual(json.loads((current / 'live.json').read_text()), live)
            self.assertFalse((legacy / 'live.json').exists())
            self.assertTrue((legacy / 'other.json').exists())

    def test_adoption_holds_the_area_lock_used_by_live_commands(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            current = root / 'release'
            legacy = root / 'legacy'
            legacy.mkdir()
            record = {'pod_uid': 'live', 'deadline': 1, 'terminated': False}
            (legacy / 'area.json').write_text(json.dumps(record))
            contended = []

            def save(target, value):
                # handle() takes this exact lock; it must be unavailable mid-adoption.
                with (current / 'area.lock').open('a') as competing:
                    try:
                        fcntl.flock(competing, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    except BlockingIOError:
                        contended.append(target.name)
                target.write_text(json.dumps(value))

            with patch.object(node_guard, 'ROOT', current), patch.object(node_guard, 'LEGACY_ROOT', legacy), patch.object(node_guard, 'validate'), patch.object(node_guard, 'save', side_effect=save):
                node_guard.migrate_legacy()

            self.assertEqual(contended, ['area.json'])
            self.assertFalse((legacy / 'area.json').exists())

    def test_terminal_tombstone_migrates_without_a_live_container(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            current = root / 'release'
            legacy = root / 'legacy'
            legacy.mkdir()
            native = root / 'native'
            native.mkdir()
            work = root / 'saved-work'
            work.mkdir()
            (work / 'saved.txt').write_bytes(b'saved before termination')
            area = '00000000-0000-0000-0000-000000000001'
            binding = {'container_id': 'a' * 64, 'pod_uid': '00000000-0000-0000-0000-000000000002',
                       'area_id': area, 'epoch': 1, 'image': 'image'}
            stopped = dict(binding, terminated=True, deadline=None, frozen=False)
            exited = dict(binding, area_id='00000000-0000-0000-0000-000000000003', terminated=False,
                          deadline=1, frozen=True, sentry_pid=4242, sentry_start='1')
            (legacy / (area + '.json')).write_text(json.dumps(stopped))
            (legacy / (exited['area_id'] + '.json')).write_text(json.dumps(exited))

            native_actions = []

            def native_command(*args, **kwargs):
                if 'list' in args:
                    return (binding['container_id'] + '\n').encode()
                native_actions.append(args)
                raise AssertionError('unverified native action')

            with patch.object(node_guard, 'ROOT', current), patch.object(node_guard, 'LEGACY_ROOT', legacy), \
                    patch.object(node_guard, 'RUNTIME', str(native)), \
                    patch.object(node_guard, 'validate', side_effect=FileNotFoundError('binding unavailable')), \
                    patch.object(node_guard, 'command', side_effect=native_command), \
                    patch.object(node_guard, 'volume', return_value=work), \
                    patch.object(node_guard, 'process_identity', side_effect=FileNotFoundError):
                node_guard.migrate_legacy()
                status = node_guard.handle(dict(binding, action='status'))
                node_guard.retry_cleanup('')
                recovered = node_guard.handle(dict(binding, action='read', path='saved.txt', offset=0))

            self.assertEqual(status, {'termination_confirmed': True, 'status': 'stopped'})
            self.assertEqual(base64.b64decode(recovered['data']), b'saved before termination')
            retained = json.loads((current / (area + '.json')).read_text())
            self.assertTrue(retained['terminated'])
            self.assertTrue(retained['native_cleanup_queued'])
            self.assertEqual({key:retained[key] for key in binding}, binding)
            pending = current / 'runtime-cleanup' / (binding['container_id'] + '.json')
            self.assertTrue(pending.exists())
            self.assertEqual(json.loads(pending.read_text())['last_error'], 'FileNotFoundError')
            self.assertEqual(native_actions, [])
            self.assertEqual(json.loads((current / (exited['area_id'] + '.json')).read_text()),
                             dict(exited, terminated=True, deadline=None, frozen=False))
            # Each release adopts its own copy of the proof.
            self.assertTrue((legacy / (area + '.json')).exists())

    def test_pending_cleanup_survives_new_heap_and_restarted_reader(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            current, native = root / 'guard', root / 'native'
            current.mkdir()
            native.mkdir()
            old = dict(container_id='a' * 64, pod_uid='00000000-0000-0000-0000-000000000002',
                       area_id='00000000-0000-0000-0000-000000000001', epoch=1, image='pinned')
            new = dict(old, container_id='b' * 64, pod_uid='00000000-0000-0000-0000-000000000003', epoch=2)
            area = current / (old['area_id'] + '.json')
            area.write_text(json.dumps(dict(old, terminated=True, frozen=False, deadline=None)))
            containers = {old['container_id']:old, new['container_id']:new}
            deleted = []
            inventory = None
            delete_fails = True

            def native_command(*args, **kwargs):
                if 'list' in args:
                    if isinstance(inventory, Exception):
                        raise inventory
                    return inventory if inventory is not None else ('\n'.join(containers) + '\n').encode()
                cid = args[-1]
                observed = containers[cid]
                if 'info' in args:
                    return json.dumps({'ID':cid, 'Runtime':{'Name':'io.containerd.runsc.v1'}, 'Labels':{
                        'io.kubernetes.pod.namespace':node_guard.NAMESPACE,
                        'io.kubernetes.pod.uid':observed['pod_uid'],
                        'io.kubernetes.container.name':'execution',
                        'io.kubernetes.pod.name':'operation-owned'}}).encode()
                if 'state' in args:
                    return json.dumps({'id':cid, 'status':'stopped' if cid == old['container_id'] else 'paused',
                        'pid':0 if cid == old['container_id'] else 4242,
                        'annotations':{'io.kubernetes.cri.image-name':observed['image']}}).encode()
                if 'delete' in args:
                    # Native cleanup must not hold the newer heap's deadline
                    # lock while the external runtime can block or fail.
                    with (current / (old['area_id'] + '.lock')).open('a') as deadline_lock:
                        fcntl.flock(deadline_lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    deleted.append(cid)
                    if delete_fails:
                        raise subprocess.TimeoutExpired('native cleanup', 15)
                    del containers[cid]
                    return b''
                raise AssertionError('unexpected native boundary')

            with patch.object(node_guard, 'ROOT', current), patch.object(node_guard, 'RUNTIME', str(native)), \
                    patch.object(node_guard, 'command', side_effect=native_command), \
                    patch.object(node_guard, 'process_identity', return_value=('2', 'S')):
                self.assertTrue(node_guard.handle(dict(old, action='status'))['termination_confirmed'])
                pending = current / 'runtime-cleanup' / (old['container_id'] + '.json')
                with self.assertRaisesRegex(ValueError, 'terminated execution cannot be reused'):
                    node_guard.handle(dict(old, action='status', epoch=2))
                # Failed, malformed, duplicate and prefix-only inventories cannot
                # be converted into completed native absence.
                for inventory in (FileNotFoundError('native command missing'), b'not-an-id\n',
                                  (old['container_id'] + 'c\n').encode(),
                                  ((old['container_id'] + '\n') * 2).encode()):
                    node_guard.retry_cleanup('')
                    self.assertTrue(pending.exists())
                    self.assertEqual(deleted, [])
                inventory = None
                node_guard.retry_cleanup('')
                self.assertTrue(pending.exists())
                self.assertTrue(node_guard.handle(dict(old, action='status'))['termination_confirmed'])

                # Ordinary admission of the next heap replaces area state, while
                # the old exact-container cleanup remains independently durable.
                self.assertTrue(node_guard.handle(dict(new, action='freeze', idle_seconds=60))['writer_frozen'])
                self.assertEqual(json.loads(area.read_text())['container_id'], new['container_id'])
                self.assertTrue(pending.exists())
                delete_fails = False
                # A fresh reader has no prior in-memory queue cursor or binding.
                node_guard.retry_cleanup('')
                self.assertFalse(pending.exists())
                self.assertEqual(deleted, [old['container_id'], old['container_id']])
                self.assertIn(new['container_id'], containers)
                self.assertFalse(node_guard.handle(dict(new, action='status'))['termination_confirmed'])

    def test_existing_target_retires_the_legacy_record_without_revalidation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            current = root / 'release'
            legacy = root / 'legacy'
            current.mkdir()
            legacy.mkdir()
            record = {'pod_uid': 'live', 'deadline': 1, 'terminated': False}
            newer = dict(record, deadline=2)
            # A live command won the area lock first and wrote these targets.
            (legacy / 'same.json').write_text(json.dumps(record))
            (current / 'same.json').write_text(json.dumps(record))
            (legacy / 'newer.json').write_text(json.dumps(record))
            (current / 'newer.json').write_text(json.dumps(newer))

            with patch.object(node_guard, 'ROOT', current), patch.object(node_guard, 'LEGACY_ROOT', legacy), \
                    patch.object(node_guard, 'gone', return_value=False), \
                    patch.object(node_guard, 'validate') as validate:
                node_guard.migrate_legacy()
                node_guard.migrate_legacy()

            validate.assert_not_called()
            # An exact copy was already migrated; a differing one is kept aside
            # and leaves the scanned set, and the live records are untouched.
            self.assertEqual(sorted(path.name for path in legacy.glob('*.json')), [])
            self.assertTrue((legacy / 'newer.superseded').exists())
            self.assertEqual(json.loads((current / 'same.json').read_text()), record)
            self.assertEqual(json.loads((current / 'newer.json').read_text()), newer)

    def test_adopted_terminal_tombstones_are_not_rescanned(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            current = root / 'release'
            legacy = root / 'legacy'
            legacy.mkdir()
            stopped = {'pod_uid': 'live', 'deadline': None, 'terminated': True}
            (legacy / 'done.json').write_text(json.dumps(stopped))
            reads = []
            original = Path.read_bytes

            def read_bytes(path):
                if path.parent == legacy:
                    reads.append(path.name)
                return original(path)

            with patch.object(node_guard, 'ROOT', current), patch.object(node_guard, 'LEGACY_ROOT', legacy), \
                    patch.object(node_guard, 'ADOPTED_TOMBSTONES', {}), \
                    patch.object(Path, 'read_bytes', read_bytes):
                node_guard.migrate_legacy()
                node_guard.migrate_legacy()

            # Adopted once; the shared proof stays for other releases.
            self.assertEqual(reads, ['done.json'])
            self.assertTrue((legacy / 'done.json').exists())
            self.assertTrue((current / 'done.json').exists())


if __name__ == '__main__':
    unittest.main()
