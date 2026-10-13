import fcntl
import json
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
            area = '00000000-0000-0000-0000-000000000001'
            binding = {'container_id': 'a' * 64, 'pod_uid': '00000000-0000-0000-0000-000000000002',
                       'area_id': area, 'epoch': 1, 'image': 'image'}
            stopped = dict(binding, terminated=True, deadline=None, frozen=False)
            exited = dict(binding, area_id='00000000-0000-0000-0000-000000000003', terminated=False,
                          deadline=1, frozen=True, sentry_pid=4242, sentry_start='1')
            (legacy / (area + '.json')).write_text(json.dumps(stopped))
            (legacy / (exited['area_id'] + '.json')).write_text(json.dumps(exited))

            with patch.object(node_guard, 'ROOT', current), patch.object(node_guard, 'LEGACY_ROOT', legacy), \
                    patch.object(node_guard, 'validate', side_effect=FileNotFoundError('container removed')) as validate, \
                    patch.object(node_guard, 'process_identity', side_effect=FileNotFoundError):
                node_guard.migrate_legacy()
                status = node_guard.handle(dict(binding, action='status'))

            validate.assert_not_called()
            self.assertEqual(status, {'termination_confirmed': True, 'status': 'stopped'})
            self.assertEqual(json.loads((current / (area + '.json')).read_text()), stopped)
            self.assertEqual(json.loads((current / (exited['area_id'] + '.json')).read_text()),
                             dict(exited, terminated=True, deadline=None, frozen=False))
            # Each release adopts its own copy of the proof.
            self.assertTrue((legacy / (area + '.json')).exists())


if __name__ == '__main__':
    unittest.main()
