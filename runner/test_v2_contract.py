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


def test_python_start_reports_missing_image_interpreter(tmp_path):
    from types import SimpleNamespace
    import pytest
    from execution_pod import ExecutionPodError
    backend = SimpleNamespace(root=tmp_path, kube=Mock(return_value=SimpleNamespace(
        returncode=0, stdout=b'{"available":false}')))
    controller = ExecutionPodController(backend, {}, admitted=True)
    controller._get = Mock(return_value={'metadata': {'uid': 'uid'}, 'status': {'phase': 'Running'},
        'spec': {'containers': [{'name': 'execution', 'securityContext': {}}]}})
    controller.submit_command = Mock()
    with pytest.raises(ExecutionPodError) as failure:
        controller.start_python('s', 'r', 'secret')
    assert failure.value.code == 'python_unavailable'
    assert 'apt-get install -y python3' in failure.value.message
    controller.submit_command.assert_not_called()


def test_python_start_uses_image_interpreter_and_passes_run_credential(tmp_path):
    from types import SimpleNamespace
    backend = SimpleNamespace(root=tmp_path, kube=Mock(return_value=SimpleNamespace(
        returncode=0, stdout=b'{"available":true}')))
    controller = ExecutionPodController(backend, {}, admitted=True)
    controller._get = Mock(return_value={'metadata': {'uid': 'uid'}, 'status': {'phase': 'Running'},
        'spec': {'containers': [{'name': 'execution', 'securityContext': {}}]}})
    controller.submit_command = Mock(return_value={'status': 'queued'})
    session = controller.start_python('s', 'r', 'run-token')
    request = controller.submit_command.call_args.args[0]
    assert request['code'].startswith('exec python3 -I /.aidash-helpers/python_session.py serve ')
    assert request['proxy_credential'] == 'run-token'
    assert 'run-token' not in controller._python_session_path('s', 'r').read_text()
    assert session['run_id'] == 'r'


def test_background_log_route_reads_tail_and_preserves_finished_output(tmp_path):
    import base64
    import http.client
    import threading
    from http.server import ThreadingHTTPServer
    from types import SimpleNamespace
    from control import Handler
    from sandbox_v2 import operation_output
    operation = '00000000-0000-4000-8000-000000000001'
    (tmp_path / (operation + '.stdout')).write_bytes(b'0123456789')
    def kube(args, **kwargs):
        if 'output' in args:
            result = operation_output(operation, args[-3], None if args[-2] == 'tail' else int(args[-2]),
                                      int(args[-1]), directory=tmp_path)
            return SimpleNamespace(returncode=0, stdout=json.dumps(result).encode())
        if 'forget' in args:
            (tmp_path / (operation + '.stdout')).unlink(missing_ok=True)
        return SimpleNamespace(returncode=0, stdout=b'')
    controller = ExecutionPodController(SimpleNamespace(root=tmp_path, kube=kube), {}, admitted=True)
    record = {'operation_id': operation, 'scratch_space_id': 's', 'run_id': 'r', 'pod': 'pod',
              'status': 'running', 'background': True}
    controller._command_path(operation).write_text(json.dumps(record))
    controller._finish_command(record, {'status': 'completed', 'exit_code': 0},
                               {'stdout': '', 'stderr': '', 'truncated': False})
    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.runner = SimpleNamespace(token='x' * 32, execution_pods=controller)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        conn = http.client.HTTPConnection('127.0.0.1', server.server_port)
        conn.request('POST', '/v2/background/log', json.dumps({'operation_id': operation,
                     'stream': 'stdout', 'limit': 4}), {'Authorization': 'Bearer ' + 'x' * 32})
        response = conn.getresponse()
        body = json.loads(response.read())
        assert response.status == 200
        assert base64.b64decode(body['data']) == b'6789'
        assert (body['next_offset'], body['size'], body['eof']) == (10, 10, True)
        conn.close()
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def test_file_read_http_route_returns_bounded_bytes(tmp_path):
    import base64
    import http.client
    import threading
    from http.server import ThreadingHTTPServer
    from types import SimpleNamespace
    from control import Handler
    from files_v2 import FileService
    root = tmp_path / 'scratch'
    root.mkdir()
    (root / 'file').write_bytes(b'abcdef')
    service = FileService(root, tmp_path / 'state')
    def kube(args, data=None, **kwargs):
        result = service.read(json.loads(data))
        return SimpleNamespace(returncode=0, stdout=json.dumps({'result': result}).encode())
    backend = SimpleNamespace(root=tmp_path, kube=kube)
    controller = ExecutionPodController(backend, {}, admitted=True)
    controller._get = Mock(return_value={'metadata': {'uid': 'uid'}, 'status': {'phase': 'Running'}})
    controller._admitted_uid = Mock(return_value='uid')
    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.runner = SimpleNamespace(token='x' * 32, execution_pods=controller)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        conn = http.client.HTTPConnection('127.0.0.1', server.server_port)
        conn.request('POST', '/v2/files/read', json.dumps({'scratch_space_id': 's', 'path': 'file',
                     'offset': 2, 'limit': 3}), {'Authorization': 'Bearer ' + 'x' * 32})
        response = conn.getresponse()
        body = json.loads(response.read())
        assert response.status == 200
        assert base64.b64decode(body['data']) == b'cde'
        assert body['size'] == 6
        conn.close()
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
