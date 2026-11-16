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


def test_process_proxy_also_reaches_node_fetch(tmp_path, monkeypatch):
    # Node's built-in fetch, which corepack uses, ignores the proxy variables
    # unless asked to read them (#845).
    from sandbox_v2 import image_environment, save_image_environment
    root = tmp_path / 'rootfs'
    root.mkdir()
    save_image_environment(tmp_path, {'PATH': '/bin', 'NODE_USE_ENV_PROXY': '0'})
    monkeypatch.setenv('HTTPS_PROXY', 'http://aidash:run@proxy:8888')
    environment = image_environment(str(root))
    assert environment['HTTPS_PROXY'] == 'http://aidash:run@proxy:8888'
    assert environment['NODE_USE_ENV_PROXY'] == '1'
    monkeypatch.delenv('HTTPS_PROXY')
    assert 'NODE_USE_ENV_PROXY' not in image_environment(str(root))


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


def test_stop_run_waits_until_signalled_process_finishes(tmp_path):
    import threading
    import time
    from types import SimpleNamespace
    operation = '00000000-0000-4000-8000-000000000001'
    controller = ExecutionPodController(SimpleNamespace(root=tmp_path), {}, admitted=True)
    controller._command_path(operation).write_text(json.dumps({'operation_id': operation,
        'scratch_space_id': 's', 'run_id': 'r', 'status': 'running', 'background': True}))
    def stop(op):
        def finish():
            time.sleep(.1)
            controller._update_command(op, status='stopped')
        threading.Thread(target=finish, daemon=True).start()
        return {'status': 'stopping'}
    controller._stop_command = stop
    assert controller.stop_run('s', 'r') == {'stopped': [operation]}
    assert controller._read_command(operation)['status'] == 'stopped'


def test_stop_run_also_stops_an_interrupted_unconfirmed_supervisor(tmp_path):
    from types import SimpleNamespace
    operation = '00000000-0000-4000-8000-000000000001'
    alive = [True]
    def kube(args, **kwargs):
        if 'stop' in args:
            alive[0] = False
            return SimpleNamespace(returncode=0, stdout=b'')
        if 'running' in args:
            return SimpleNamespace(returncode=0, stdout=json.dumps({'running': alive[0]}).encode())
        raise AssertionError(args)
    controller = ExecutionPodController(SimpleNamespace(root=tmp_path, kube=kube), {}, admitted=True)
    controller._get = Mock(return_value={'metadata': {'uid': 'uid'}, 'status': {'phase': 'Running'}})
    controller._command_path(operation).write_text(json.dumps({'operation_id': operation,
        'scratch_space_id': 's', 'run_id': 'r', 'status': 'interrupted', 'background': True,
        'docker_enabled': False, 'pod': 'pod', 'pod_uid': 'uid', 'stop_pending': True, 'terminal_at': 1}))
    assert controller.stop_run('s', 'r') == {'stopped': [operation]}
    assert alive[0] is False


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


def test_preview_waits_for_port_and_returns_pod_address(tmp_path):
    from types import SimpleNamespace
    operation = '00000000-0000-4000-8000-000000000001'
    backend = SimpleNamespace(root=tmp_path, kube=Mock(return_value=SimpleNamespace(returncode=0,
        stdout=b'{"ready":true}')))
    controller = ExecutionPodController(backend, {'preview_port': 8080}, admitted=True)
    controller._get = Mock(return_value={'metadata': {'uid': 'uid'}, 'status': {'phase': 'Running',
                              'podIP': '10.2.3.4'}})
    controller.submit_command = Mock(return_value={'status': 'queued'})
    controller.command_result = Mock(return_value={'status': 'running'})
    controller._read_command = Mock(return_value={'pod_uid': 'uid'})
    request = {'operation_id': operation, 'scratch_space_id': 's', 'run_id': 'r',
               'code': 'serve', 'port': 8080, 'proxy_credential': 'run-token'}
    assert controller.start_preview(request) == {'operation_id': operation, 'address': '10.2.3.4:8080'}
    command = controller.submit_command.call_args.args[0]
    assert command['background'] is True
    assert command['proxy_credential'] == 'run-token'


def test_mcp_routes_start_call_and_stop_a_run_owned_session(tmp_path):
    from types import SimpleNamespace
    operation = '00000000-0000-4000-8000-000000000001'
    def kube(args, data=None, **kwargs):
        if 'ping' in args:
            return SimpleNamespace(returncode=0, stdout=b'{"ready":true}')
        if 'call' in args:
            message = json.loads(data)
            return SimpleNamespace(returncode=0, stdout=json.dumps({'message': {
                'jsonrpc': '2.0', 'id': message['id'], 'result': 'ok'}}).encode())
        raise AssertionError(args)
    controller = ExecutionPodController(SimpleNamespace(root=tmp_path, kube=kube), {}, admitted=True)
    controller.submit_command = Mock(return_value={'status': 'queued'})
    controller.command_result = Mock(return_value={'status': 'running'})
    controller._get = Mock(return_value={'metadata': {'uid': 'uid'}})
    controller._read_command = Mock(return_value={'pod_uid': 'uid', 'pod': 'pod', 'docker_enabled': False,
                                                   'kind': 'mcp_session', 'background': True, 'status': 'running'})
    request = {'operation_id': operation, 'scratch_space_id': 's', 'run_id': 'r', 'install': '',
               'command': ['example'], 'env': {}, 'plugin_data': '.', 'proxy_credential': 'token'}
    assert controller.start_mcp(request) == {'operation_id': operation}
    result = controller.call_mcp({'operation_id': operation, 'timeout_seconds': 3,
                                  'message': {'jsonrpc': '2.0', 'id': 1, 'method': 'ping'}})
    assert result == {'message': {'jsonrpc': '2.0', 'id': 1, 'result': 'ok'}}
    assert controller.submit_command.call_args.args[0]['proxy_credential'] == 'token'


def test_left_running_record_is_listed_and_its_log_is_readable(tmp_path):
    import base64
    import http.client
    import threading
    from http.server import ThreadingHTTPServer
    from types import SimpleNamespace
    from control import Handler
    from sandbox_v2 import operation_output
    operation = '00000000-0000-4000-8000-000000000001.left'
    (tmp_path / (operation + '.stdout')).write_bytes(b'left output')
    def kube(args, **kwargs):
        chunk = operation_output(operation, args[-3], None if args[-2] == 'tail' else int(args[-2]),
                                 int(args[-1]), directory=tmp_path)
        return SimpleNamespace(returncode=0, stdout=json.dumps(chunk).encode())
    controller = ExecutionPodController(SimpleNamespace(root=tmp_path, kube=kube), {}, admitted=True)
    controller._command_path(operation).write_text(json.dumps({'operation_id': operation,
        'scratch_space_id': 's', 'run_id': 'r', 'status': 'running', 'background': True, 'pod': 'pod'}))
    assert controller.background_processes('s')['processes'][0]['operation_id'] == operation
    chunk = controller.background_log({'operation_id': operation, 'stream': 'stdout', 'limit': 6})
    assert base64.b64decode(chunk['data']) == b'output'
    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.runner = SimpleNamespace(token='x' * 32, execution_pods=controller)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        connection = http.client.HTTPConnection('127.0.0.1', server.server_port)
        connection.request('POST', '/v2/background/log', json.dumps({'operation_id': operation,
                           'stream': 'stdout', 'limit': 6}), {'Authorization': 'Bearer ' + 'x' * 32})
        response = connection.getresponse()
        assert response.status == 200
        assert base64.b64decode(json.loads(response.read())['data']) == b'output'
        connection.close()
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def test_foreground_completion_registers_adopted_process_for_its_run(tmp_path):
    from concurrent.futures import Future
    from types import SimpleNamespace
    operation = '00000000-0000-4000-8000-000000000001'
    controller = ExecutionPodController(SimpleNamespace(root=tmp_path, kube=Mock()), {}, admitted=True)
    record = {'operation_id': operation, 'scratch_space_id': 's', 'run_id': 'r', 'pod': 'pod',
              'pod_uid': 'uid', 'status': 'running', 'background': False, 'docker_enabled': False}
    controller._command_path(operation).write_text(json.dumps(record))
    controller.queues['s'] = SimpleNamespace(submit=Mock(return_value=Future()))
    controller._finish_command(record, {'status': 'completed', 'exit_code': 0,
        'left_running': operation + '.left'}, {'stdout': '', 'stderr': '', 'truncated': False})
    result = controller.command_result(operation)
    assert result['left_running'] == operation + '.left'
    processes = controller.background_processes('s')['processes']
    assert [(item['operation_id'], item['run_id'], item['status']) for item in processes] == [
        (operation + '.left', 'r', 'running')]


def test_docker_adopted_result_authentication_rejects_tampering():
    from sandbox_v2 import sign_result
    from execution_pod import verify_adopted_result
    result = {'operation_id': '00000000-0000-4000-8000-000000000001.left',
              'run_id': 'r', 'status': 'stopped'}
    result['auth'] = sign_result(result, b'secret')
    assert verify_adopted_result(result, b'secret')
    assert not verify_adopted_result(dict(result, status='completed'), b'secret')
    assert not verify_adopted_result(result, b'other')


def test_file_routes_write_search_patch_and_manifest(tmp_path):
    import base64
    import http.client
    import threading
    from http.server import ThreadingHTTPServer
    from types import SimpleNamespace
    from control import Handler
    from files_v2 import FileService
    root = tmp_path / 'scratch'
    root.mkdir()
    service = FileService(root, tmp_path / 'state')
    def kube(args, data=None, **kwargs):
        result = getattr(service, args[-2])(json.loads(data))
        return SimpleNamespace(returncode=0, stdout=json.dumps({'result': result}).encode())
    controller = ExecutionPodController(SimpleNamespace(root=tmp_path, kube=kube), {}, admitted=True)
    controller._get = Mock(return_value={'metadata': {'uid': 'uid'}, 'status': {'phase': 'Running'}})
    controller._admitted_uid = Mock(return_value='uid')
    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.runner = SimpleNamespace(token='x' * 32, execution_pods=controller)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    def post(path, body):
        connection = http.client.HTTPConnection('127.0.0.1', server.server_port)
        connection.request('POST', '/v2/files/' + path, json.dumps(dict(body, scratch_space_id='s')),
                           {'Authorization': 'Bearer ' + 'x' * 32})
        response = connection.getresponse()
        result = json.loads(response.read())
        assert response.status == 200, result
        connection.close()
        return result
    try:
        assert post('write', {'run_id': 'r', 'path': 'x', 'offset': 0, 'final': True,
                              'data': base64.b64encode(b'hello\n').decode()}) == {'size': 6}
        assert post('search', {'pattern': 'hello', 'regex': False, 'max_results': 5})['matches'] == [
            {'path': 'x', 'line': 1, 'text': 'hello'}]
        patch = '*** Begin Patch\n*** Update File: x\n@@\n-hello\n+world\n*** End Patch\n'
        assert post('patch', {'run_id': 'r', 'patch': patch}) == {'changed': ['x']}
        result = post('manifest', {'root': '.', 'ignore_files': [], 'exclude': [], 'include': [], 'max_files': 5})
        assert [(entry['path'], entry['size']) for entry in result['files']] == [('x', 6)]
        assert (root / 'x').read_bytes() == b'world\n'
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def test_preview_and_mcp_http_routes(tmp_path):
    import http.client
    import threading
    from http.server import ThreadingHTTPServer
    from types import SimpleNamespace
    from control import Handler
    preview = '00000000-0000-4000-8000-000000000001'
    mcp = '00000000-0000-4000-8000-000000000002'
    def kube(args, data=None, **kwargs):
        if 'port-check' in args or 'ping' in args:
            return SimpleNamespace(returncode=0, stdout=b'{"ready":true}')
        if 'call' in args:
            return SimpleNamespace(returncode=0, stdout=b'{"message":{"jsonrpc":"2.0","id":1,"result":"ok"}}')
        raise AssertionError(args)
    controller = ExecutionPodController(SimpleNamespace(root=tmp_path, kube=kube), {'preview_port': 1234}, admitted=True)
    controller._get = Mock(return_value={'metadata': {'uid': 'uid'}, 'status': {'phase': 'Running', 'podIP': '10.1.2.3'}})
    controller.submit_command = Mock(return_value={'status': 'queued'})
    controller.command_result = Mock(return_value={'status': 'running'})
    controller._read_command = lambda op: {'operation_id': op, 'pod': 'pod', 'pod_uid': 'uid',
        'status': 'running', 'background': True, 'docker_enabled': False,
        'kind': 'mcp_session' if op == mcp else 'preview'}
    controller._stop_command = lambda op: {'operation_id': op, 'status': 'stopping'}
    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.runner = SimpleNamespace(token='x' * 32, execution_pods=controller)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    def post(path, body):
        connection = http.client.HTTPConnection('127.0.0.1', server.server_port)
        connection.request('POST', '/v2/' + path, json.dumps(body), {'Authorization': 'Bearer ' + 'x' * 32})
        response = connection.getresponse()
        result = json.loads(response.read())
        assert response.status == 200, result
        connection.close()
        return result
    try:
        result = post('previews/start', {'operation_id': preview, 'scratch_space_id': 's',
                                        'run_id': 'r', 'code': 'serve', 'port': 1234})
        assert result == {'operation_id': preview, 'address': '10.1.2.3:1234'}
        assert post('previews/stop', {'operation_id': preview})['status'] == 'stopping'
        result = post('mcp/start', {'operation_id': mcp, 'scratch_space_id': 's', 'run_id': 'r',
                                    'install': '', 'command': ['server'], 'env': {}, 'plugin_data': '.'})
        assert result == {'operation_id': mcp}
        result = post('mcp/call', {'operation_id': mcp, 'timeout_seconds': 5,
                                   'message': {'jsonrpc': '2.0', 'id': 1, 'method': 'ping'}})
        assert result['message']['result'] == 'ok'
        assert post('mcp/stop', {'operation_id': mcp})['status'] == 'stopping'
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def test_run_stop_route_is_scoped_and_idempotent(tmp_path):
    import http.client
    import threading
    from http.server import ThreadingHTTPServer
    from types import SimpleNamespace
    from control import Handler
    operation = '00000000-0000-4000-8000-000000000001'
    controller = ExecutionPodController(SimpleNamespace(root=tmp_path), {}, admitted=True)
    controller._command_path(operation).write_text(json.dumps({'operation_id': operation, 'scratch_space_id': 's',
        'run_id': 'r', 'status': 'running', 'background': True}))
    controller._stop_command = lambda op: controller._update_command(op, status='stopped')
    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.runner = SimpleNamespace(token='x' * 32, execution_pods=controller)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    def stop(run):
        connection = http.client.HTTPConnection('127.0.0.1', server.server_port)
        connection.request('POST', '/v2/runs/stop', json.dumps({'scratch_space_id': 's', 'run_id': run}),
                           {'Authorization': 'Bearer ' + 'x' * 32})
        response = connection.getresponse()
        result = json.loads(response.read())
        assert response.status == 200
        connection.close()
        return result
    try:
        assert stop('other') == {'stopped': []}
        assert stop('r') == {'stopped': [operation]}
        assert stop('r') == {'stopped': []}
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def test_python_start_http_route_passes_credential_without_persisting_it(tmp_path):
    import http.client
    import threading
    from http.server import ThreadingHTTPServer
    from types import SimpleNamespace
    from control import Handler
    backend = SimpleNamespace(root=tmp_path, kube=Mock(return_value=SimpleNamespace(returncode=0,
                              stdout=b'{"available":true}')))
    controller = ExecutionPodController(backend, {}, admitted=True)
    controller._get = Mock(return_value={'metadata': {'uid': 'uid'}, 'status': {'phase': 'Running'},
        'spec': {'containers': [{'name': 'execution', 'securityContext': {}}]}})
    controller.submit_command = Mock(return_value={'status': 'queued'})
    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.runner = SimpleNamespace(token='x' * 32, execution_pods=controller)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        connection = http.client.HTTPConnection('127.0.0.1', server.server_port)
        connection.request('POST', '/v2/python/start', json.dumps({'scratch_space_id': 's', 'run_id': 'r',
                           'proxy_credential': 'canary-token'}), {'Authorization': 'Bearer ' + 'x' * 32})
        response = connection.getresponse()
        result = json.loads(response.read())
        assert response.status == 200
        assert result['run_id'] == 'r'
        assert controller.submit_command.call_args.args[0]['proxy_credential'] == 'canary-token'
        assert 'canary-token' not in controller._python_session_path('s', 'r').read_text()
        connection.close()
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def test_command_submit_http_keeps_proxy_credential_out_of_journal(tmp_path):
    import http.client
    import threading
    from concurrent.futures import Future
    from http.server import ThreadingHTTPServer
    from types import SimpleNamespace
    from control import Handler
    operation = '00000000-0000-4000-8000-000000000001'
    controller = ExecutionPodController(SimpleNamespace(root=tmp_path),
                                        {'proxy_endpoint': 'http://proxy:8080'}, admitted=True)
    controller._get = Mock(return_value={'metadata': {'uid': 'uid'}, 'status': {'phase': 'Running'},
        'spec': {'containers': [{'name': 'execution', 'securityContext': {}}]}})
    controller._old_helper_image = Mock(return_value=False)
    controller._admitted_uid = Mock(return_value='uid')
    controller.queues['s'] = SimpleNamespace(submit=Mock(return_value=Future()))
    server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    server.runner = SimpleNamespace(token='x' * 32, execution_pods=controller)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        connection = http.client.HTTPConnection('127.0.0.1', server.server_port)
        request = {'operation_id': operation, 'scratch_space_id': 's', 'run_id': 'r', 'code': 'true',
                   'proxy_credential': 'canary-token'}
        connection.request('POST', '/v2/commands/submit', json.dumps(request),
                           {'Authorization': 'Bearer ' + 'x' * 32})
        response = connection.getresponse()
        assert response.status == 200
        assert json.loads(response.read())['status'] == 'queued'
        journal = json.loads(controller._command_path(operation).read_text())
        assert journal['proxy_credential_given'] is True
        assert 'canary-token' not in json.dumps(journal)
        assert controller.process_proxies[operation] == 'http://aidash:canary-token@proxy:8080'
        connection.close()
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
