import json
import tempfile
from pathlib import Path
from unittest.mock import Mock, patch

from execution_pod import ExecutionPodController


def test_image_proxy_is_not_inherited_without_process_credential(tmp_path, monkeypatch):
    from sandbox_v2 import image_environment, save_image_environment
    root = tmp_path / 'rootfs'
    root.mkdir()
    save_image_environment(tmp_path, {'PATH': '/bin', 'HTTPS_PROXY': 'http://old:secret@proxy'})
    monkeypatch.delenv('HTTPS_PROXY', raising=False)
    assert 'HTTPS_PROXY' not in image_environment(str(root))


def test_credential_is_sent_on_stdin_not_command_arguments_or_journal(tmp_path):
    from types import SimpleNamespace
    operation = '00000000-0000-4000-8000-000000000001'
    backend = SimpleNamespace(root=tmp_path, command=['kubectl'], environment={}, kube=Mock())
    controller = ExecutionPodController(backend, {}, admitted=True)
    record = {'operation_id': operation, 'scratch_space_id': 's', 'run_id': 'r', 'pod': 'pod',
              'pod_uid': 'uid', 'docker_enabled': False, 'background': False, 'timeout_seconds': 5,
              'cwd': '.', 'code': 'true', 'status': 'queued', 'proxy_credential_given': True}
    controller._command_path(operation).write_text(json.dumps(record))
    controller.process_proxies[operation] = 'http://aidash:secret@proxy:8080'
    controller._await_normal_result = Mock(return_value={
        'operation_id': operation, 'run_id': 'r', 'status': 'completed', 'exit_code': 0})
    transport = {'exit_code': 0, 'transport_timed_out': False,
                 'output': {'stdout': '', 'stderr': '', 'truncated': False}}
    with patch('execution_pod.drain_process', return_value=transport) as execute:
        controller._execute_command(operation)
    assert b'secret' in execute.call_args.kwargs['input_data']
    assert 'secret' not in repr(execute.call_args.args[0])
    assert 'secret' not in controller._command_path(operation).read_text()
    assert not controller.process_proxies


def test_stop_run_stops_only_active_processes_of_that_run():
    with tempfile.TemporaryDirectory() as directory:
        backend = Mock(root=Path(directory))
        controller = ExecutionPodController(backend, {}, admitted=True)
        records = [
            ('00000000-0000-4000-8000-000000000001', 'run', False, 'running'),
            ('00000000-0000-4000-8000-000000000002', 'run', True, 'running'),
            ('00000000-0000-4000-8000-000000000003', 'other', True, 'running'),
            ('00000000-0000-4000-8000-000000000004', 'run', True, 'completed'),
        ]
        for operation, run, background, status in records:
            controller._command_path(operation).write_text(json.dumps({
                'operation_id': operation, 'scratch_space_id': 'space', 'run_id': run,
                'background': background, 'status': status,
            }))
        stopped = []
        def stop(operation):
            stopped.append(operation)
            controller._update_command(operation, status='stopped')
        controller._stop_command = stop
        assert controller.stop_run('space', 'run') == {'stopped': sorted(stopped)}
        assert set(stopped) == {records[0][0], records[1][0]}
        assert controller.stop_run('space', 'run') == {'stopped': []}
