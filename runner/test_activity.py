"""/v1/activity must not cost more as completed operations accumulate in the journal."""
import json
import os
import random
import tempfile
import threading
import unittest
import uuid
from pathlib import Path
from unittest.mock import Mock, patch

from control import TERMINAL, ActivityAggregate, Runner, save


def record(status, finished_at=None, **fields):
    value = {'operation_id': str(uuid.uuid4()), 'status': status}
    if finished_at is not None:
        value['finished_at'] = finished_at
    return dict(value, **fields)


def full_scan(root):
    """What /v1/activity used to compute on every request."""
    busy, completed = 0, 0
    for path in Path(root).glob('*.json'):
        if path.name.startswith(('area-', 'session-')):
            continue
        value = json.loads(path.read_bytes())
        if value['status'] not in TERMINAL or not (value.get('termination_confirmed') or value.get('writer_frozen')):
            busy += 1
        completed = max(completed, value.get('finished_at', 0))
    return busy, completed


class ActivityTest(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.runner = object.__new__(Runner)
        self.runner.root = self.root
        self.runner.lock = threading.RLock()
        self.runner.aggregate = ActivityAggregate()
        self.runner.start = Mock()

    def write(self, value):
        save(self.root / (value['operation_id'] + '.json'), value)
        return value['operation_id']

    def activity(self):
        result = self.runner.activity()
        self.assertEqual(result['protocol'], 'aidash-runner-activity/1')
        self.assertEqual(result['busy'], result['counts']['runner'] > 0)
        return result['counts']['runner'], result['last_work_completed']

    def test_request_cost_does_not_grow_with_retained_completed_operations(self):
        reads, calls = [], []
        for retained in (10, 5000):
            with self.subTest(retained=retained), tempfile.TemporaryDirectory() as directory:
                self.root = self.runner.root = Path(directory)
                self.runner.aggregate = ActivityAggregate()
                for number in range(retained):
                    value = record('completed', 1000 + number, termination_confirmed=True, acknowledged=True)
                    (self.root / (value['operation_id'] + '.json')).write_text(json.dumps(value))
                self.write(record('running'))
                self.runner.recover()
                original = (Path.glob, Path.read_bytes, Path.open, os.scandir, os.listdir)
                calls.clear()

                def count(function):
                    def counted(*arguments, **keywords):
                        calls.append(function.__name__)
                        return function(*arguments, **keywords)
                    return counted

                with (patch.object(Path, 'glob', count(original[0])), patch.object(Path, 'read_bytes', count(original[1])),
                      patch.object(Path, 'open', count(original[2])), patch('os.scandir', count(original[3])),
                      patch('os.listdir', count(original[4]))):
                    self.assertEqual(self.activity(), (1, 1000 + retained - 1))
                reads.append(len(calls))
        # Not one journal file is listed, opened or parsed, however many are retained.
        self.assertEqual(reads, [0, 0])

    def test_the_global_lock_is_never_held_across_a_journal_scan(self):
        for number in range(50):
            self.write(record('completed', number, termination_confirmed=True))
        self.runner.recover()
        held = []

        def scan(*_, **__):
            held.append(self.runner.lock._is_owned())
            return iter(())

        with patch.object(Path, 'glob', scan), patch.object(Path, 'read_bytes', side_effect=scan):
            self.activity()
        self.assertEqual(held, [])
        # Starting the Runner is the only full scan, and it needs no lock either.
        with patch.object(Path, 'glob', scan):
            self.runner.recover()
        self.assertEqual(held, [False])

    def test_startup_seeds_the_aggregate_in_the_recovery_scan(self):
        # Startup already lists and parses the journal once to resume interrupted
        # work; seeding the aggregate must reuse that scan, not add another.
        values = [record('running'), record('awaiting_files'), record('accepted'),
                  record('uncertain', 900), record('completed', 1000, termination_confirmed=True)]
        for value in values:
            self.write(value)
        save(self.root / 'area-x.json', {'epoch': 1, 'operation_id': values[0]['operation_id']})
        listings, parsed = [], []
        glob, read_bytes = Path.glob, Path.read_bytes

        def listed(path, pattern):
            listings.append(pattern)
            return glob(path, pattern)

        def read(path):
            parsed.append(path.stem)
            return read_bytes(path)

        with patch.object(Path, 'glob', listed), patch.object(Path, 'read_bytes', read):
            self.runner.recover()
        self.assertEqual(listings, ['*.json'])
        self.assertCountEqual(parsed, [value['operation_id'] for value in values])
        self.assertEqual(self.activity(), full_scan(self.root))
        # Interrupted work resumes; files still awaiting upload and terminal work do not.
        self.assertCountEqual([call.args[0] for call in self.runner.start.call_args_list],
                              [values[0]['operation_id'], values[2]['operation_id']])

    def test_transitions_keep_the_aggregate_equal_to_a_full_scan(self):
        operation = self.write(record('accepted', termination_confirmed=False))
        self.runner.aggregate.track(operation, json.loads((self.root / (operation + '.json')).read_bytes()))
        self.assertEqual(self.activity(), (1, 0))
        # Terminal without proof that the writer stopped is still busy.
        self.runner.update(operation, status='uncertain', finished_at=1500.5)
        self.assertEqual(self.activity(), (1, 1500.5))
        self.runner.update(operation, termination_confirmed=True)
        self.assertEqual(self.activity(), (0, 1500.5))
        # Acknowledgement clears the payload but neither renews nor loses completion.
        self.runner.update(operation, acknowledged=True, request={}, stdout='', files=[], displays=[])
        self.assertEqual(self.activity(), (0, 1500.5))
        frozen = self.write(record('failed', 1400, writer_frozen=True))
        self.runner.aggregate.track(frozen, json.loads((self.root / (frozen + '.json')).read_bytes()))
        self.assertEqual(self.activity(), (0, 1500.5))
        self.assertEqual(self.activity(), full_scan(self.root))
        # The aggregate rebuilt after a restart matches the live one.
        restarted = ActivityAggregate()
        self.runner.aggregate, live = restarted, self.runner.aggregate
        self.runner.recover()
        self.assertEqual((restarted.unsettled, restarted.last_completed), (live.unsettled, live.last_completed))

    def test_aggregate_matches_the_full_scan_for_any_sequence_of_states(self):
        generator = random.Random(183)
        statuses = ['awaiting_files', 'accepted', 'starting', 'running', 'finishing', 'cancelling', 'completed',
                    'cancelled', 'failed', 'uncertain']
        operations = []
        for _ in range(60):
            operations.append(self.write(record(generator.choice(statuses))))
        self.runner.recover()
        for step in range(400):
            operation = generator.choice(operations)
            changes = {'status': generator.choice(statuses)}
            if generator.random() < .5:
                changes['termination_confirmed'] = generator.random() < .5
            if generator.random() < .2:
                changes['writer_frozen'] = generator.random() < .5
            if changes['status'] in TERMINAL:
                # The Runner stamps time.time(), so a later completion is never earlier.
                changes['finished_at'] = 1000 + step
            self.runner.update(operation, **changes)
            if step % 25 == 0:
                self.assertEqual(self.activity(), full_scan(self.root))
        self.assertEqual(self.activity(), full_scan(self.root))
        # Area and session files are not operations.
        save(self.root / 'area-x.json', {'epoch': 1, 'operation_id': operations[0]})
        save(self.root / 'session-x.json', {'status': 'frozen'})
        restarted = ActivityAggregate()
        self.runner.aggregate = restarted
        self.runner.recover()
        self.assertEqual(self.activity(), full_scan(self.root))


if __name__ == '__main__':
    unittest.main()
