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


if __name__ == '__main__':
    unittest.main()
