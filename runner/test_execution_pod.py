"""Contracts for stable, opaque Scratch Space resource identities."""
import unittest

from control import scratch_resource_name


class ScratchResourceNameTest(unittest.TestCase):
    def test_arbitrary_ids_map_to_stable_dns_labels_without_disclosure(self):
        for scratch_id in ('customer/name', '日本語 🐈', 'x' * 10000):
            with self.subTest(scratch_id=scratch_id[:20]):
                name = scratch_resource_name(scratch_id)
                self.assertRegex(name, r'^scratch-[0-9a-f]{48}$')
                self.assertEqual(name, scratch_resource_name(scratch_id))
                self.assertNotIn(scratch_id, name)
        self.assertNotEqual(scratch_resource_name('A'), scratch_resource_name('a'))

    def test_empty_or_non_string_id_is_rejected(self):
        for scratch_id in ('', None, 1):
            with self.subTest(scratch_id=scratch_id):
                with self.assertRaises(ValueError):
                    scratch_resource_name(scratch_id)


class CommandQueueTest(unittest.TestCase):
    def test_queued_foreground_cancellation_and_concurrency_limit(self):
        import threading
        import time
        from execution_pod import CommandQueue

        started = []
        release = threading.Event()
        queue = CommandQueue()

        def run(command):
            started.append(command)
            release.wait(3)
            return command

        try:
            futures = [queue.submit('run-a', str(i), run) for i in range(8)]
            deadline = time.monotonic() + 2
            while len(started) < 8 and time.monotonic() < deadline:
                time.sleep(.01)
            self.assertEqual(len(started), 8)
            waiting = queue.submit('run-b', 'waiting', run)
            queue.cancel_waiting('run-b')
            self.assertTrue(waiting.cancelled())
            release.set()
            self.assertEqual([f.result(timeout=3) for f in futures], [str(i) for i in range(8)])
            self.assertNotIn('waiting', started)
        finally:
            release.set()
            queue.close()

    def test_cancelling_run_preserves_queued_background_work(self):
        import threading
        from execution_pod import CommandQueue

        release = threading.Event()
        queue = CommandQueue()
        try:
            running = [queue.submit('run-a', i, lambda value: (release.wait(3), value)[1])
                       for i in range(8)]
            background = queue.submit('run-b', 'background', lambda value: value, background=True)
            queue.cancel_waiting('run-b')
            self.assertFalse(background.cancelled())
            release.set()
            self.assertEqual(background.result(timeout=3), 'background')
            for future in running:
                future.result(timeout=3)
        finally:
            release.set()
            queue.close()
