import json
import os
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch


@unittest.skipUnless(sys.platform == 'linux', 'requires Linux subreaper support')
class ForegroundAdoptionTest(unittest.TestCase):
    def test_foreground_result_returns_while_descendant_continues_and_can_be_stopped(self):
        import execution_pod
        import sandbox_v2
        operation = '00000000-0000-4000-8000-000000000001'
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory) / 'operations'
            output = Path(directory) / 'output'
            def command(*args):
                return ['/bin/sh', '-c', args[-1]]
            with patch.dict(os.environ, {'AIDASH_POD_UID': 'pod'}), \
                 patch.object(sandbox_v2, 'OPERATION_DIRECTORY', str(state)), \
                 patch.object(sandbox_v2, 'OUTPUT_DIRECTORY', str(output)), \
                 patch.object(execution_pod, 'helper_command', side_effect=command):
                started = time.monotonic()
                result = sandbox_v2.supervise_foreground(operation, 'run', '.',
                    '(sleep 3; echo later; sleep 20) & echo ready', 10, False,
                    expected_pod_uid='pod', record_changes=False, left_auth=b'secret')
                self.assertLess(time.monotonic() - started, 6)
                self.assertEqual((result['status'], result['exit_code'], result['left_running']),
                                 ('completed', 0, operation + '.left'))
                left = operation + '.left'
                self.assertTrue((state / (left + '.running')).exists())
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline and b'later' not in (output / (left + '.stdout')).read_bytes():
                    time.sleep(.05)
                self.assertIn(b'later', (output / (left + '.stdout')).read_bytes())
                sandbox_v2.stop_operation(left)
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline and not (state / (left + '.json')).exists():
                    time.sleep(.05)
                result = json.loads((state / (left + '.json')).read_text())
                self.assertEqual(result['status'], 'stopped')
                self.assertTrue(execution_pod.verify_adopted_result(result, b'secret'))

    def test_a_finished_command_without_descendants_returns_its_exit_code(self):
        import execution_pod
        import sandbox_v2
        operation = '00000000-0000-4000-8000-000000000002'
        with tempfile.TemporaryDirectory() as directory:
            with patch.dict(os.environ, {'AIDASH_POD_UID': 'pod'}), \
                 patch.object(sandbox_v2, 'OPERATION_DIRECTORY', str(Path(directory) / 'operations')), \
                 patch.object(sandbox_v2, 'OUTPUT_DIRECTORY', str(Path(directory) / 'output')), \
                 patch.object(execution_pod, 'helper_command', side_effect=lambda *args: ['/bin/sh', '-c', args[-1]]):
                result = sandbox_v2.supervise_foreground(operation, 'run', '.', 'exit 7', 10, False,
                                                         expected_pod_uid='pod', record_changes=False)
                self.assertEqual((result['status'], result['exit_code']), ('completed', 7))
                self.assertNotIn('left_running', result)


if __name__ == '__main__':
    unittest.main()
