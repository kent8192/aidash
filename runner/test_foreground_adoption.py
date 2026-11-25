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
                 patch.object(execution_pod, 'chroot_helper_command', side_effect=command):
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
                 patch.object(execution_pod, 'chroot_helper_command', side_effect=lambda *args: ['/bin/sh', '-c', args[-1]]):
                result = sandbox_v2.supervise_foreground(operation, 'run', '.', 'exit 7', 7000, False,
                                                         expected_pod_uid='pod', record_changes=False)
                self.assertEqual((result['status'], result['exit_code']), ('completed', 7))
                self.assertNotIn('left_running', result)


@unittest.skipUnless(sys.platform == 'linux', 'requires Linux subreaper support')
class DetachedBackgroundTest(unittest.TestCase):
    def test_deadline_and_tail_survive_loss_of_the_exec_parent(self):
        import base64
        import subprocess
        from execution_pod import verify_adopted_result

        operation = '00000000-0000-4000-8000-000000000003'
        code = """
import sys
from pathlib import Path
from unittest.mock import patch
import execution_pod
import sandbox_v2
root, operation = sys.argv[1:]
command = [sys.executable, '-c', "import time; print('x' * 4096 + 'tail', flush=True); time.sleep(30)"]
with patch.object(execution_pod, 'chroot_helper_command', return_value=command), \
     patch.object(sandbox_v2, 'OPERATION_DIRECTORY', str(Path(root) / 'state')), \
     patch.object(sandbox_v2, 'OUTPUT_DIRECTORY', str(Path(root) / 'output')):
    sandbox_v2.supervise_detached(operation, 'run', '.', 'unused', 2, False,
                                  expected_pod_uid='pod', record_changes=False,
                                  output_limit=32, left_auth=b'secret', capture_tail=True)
"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with subprocess.Popen(
                [sys.executable, '-c', code, directory, operation],
                env=dict(os.environ, AIDASH_POD_UID='pod'),
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            ) as runner_exec:
                output = root / 'output' / (operation + '.stdout')
                deadline = time.monotonic() + 3
                while time.monotonic() < deadline and (not output.exists() or not output.stat().st_size):
                    time.sleep(.02)
                self.assertGreater(output.stat().st_size, 0)
                runner_exec.kill()
                runner_exec.wait(timeout=3)
                result_path = root / 'state' / (operation + '.json')
                deadline = time.monotonic() + 8
                while time.monotonic() < deadline and not result_path.exists():
                    time.sleep(.02)
                result = json.loads(result_path.read_text())
                self.assertEqual(result['status'], 'timed_out')
                self.assertTrue(verify_adopted_result(result, b'secret'))
                self.assertEqual(base64.b64decode(result['output_tail']), b'x' * 4091 + b'tail\n')
                self.assertEqual(output.stat().st_size, 32)


if __name__ == '__main__':
    unittest.main()
