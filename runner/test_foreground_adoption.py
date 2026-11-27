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
                from types import SimpleNamespace

                def kube(args, data=None, **kwargs):
                    helper = args[args.index('/.aidash-helpers/sandbox_v2.py') + 1]
                    if helper == 'stop':
                        sandbox_v2.stop_operation(args[-1])
                        return SimpleNamespace(returncode=0, stdout=b'')
                    if helper == 'results':
                        operations = json.loads(data)
                        receipt = sandbox_v2.read_background_results(operations, 'pod')
                    else:
                        raise AssertionError(args)
                    return SimpleNamespace(returncode=0, stdout=json.dumps(receipt).encode())

                controller = execution_pod.ExecutionPodController(SimpleNamespace(root=Path(directory), kube=kube), {})
                with patch.object(controller, '_get', return_value={'metadata': {'uid': 'pod'}}), \
                     patch('execution_pod.Thread'):
                    controller._adopt_left({'run_id': 'run', 'scratch_space_id': 'space',
                                            'pod': 'execution-pod', 'pod_uid': 'pod',
                                            'docker_enabled': True}, left, b'secret')
                    self.assertEqual(controller.command_result(left)['status'], 'running')
                    self.assertEqual(controller.stop_background(left)['status'], 'stopping')
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
    def test_start_returns_before_in_pod_deadline_and_tail_are_persisted(self):
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
    import json
    receipt = sandbox_v2.supervise_background(operation, 'run', '.', 'unused', 2, False,
                                             expected_pod_uid='pod', record_changes=False,
                                             output_limit=32, left_auth=b'secret', capture_tail=True)
    print(json.dumps(receipt))
"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            reply = subprocess.run(
                [sys.executable, '-c', code, directory, operation],
                env=dict(os.environ, AIDASH_POD_UID='pod'), cwd=Path(__file__).resolve().parent,
                capture_output=True, check=True, timeout=5,
            )
            self.assertEqual(json.loads(reply.stdout), {'started': True})
            result_path = root / 'state' / (operation + '.json')
            self.assertFalse(result_path.exists())
            deadline = time.monotonic() + 8
            while time.monotonic() < deadline and not result_path.exists():
                time.sleep(.02)
            result = json.loads(result_path.read_text())
            self.assertEqual(result['status'], 'timed_out')
            self.assertTrue(verify_adopted_result(result, b'secret'))
            self.assertEqual(base64.b64decode(result['output_tail']), b'x' * 4091 + b'tail\n')
            self.assertEqual((root / 'output' / (operation + '.stdout')).stat().st_size, 32)



if __name__ == '__main__':
    unittest.main()
