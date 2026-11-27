import base64
import errno
import json
from types import SimpleNamespace
from unittest.mock import Mock
from uuid import UUID

import pytest

import sandbox_v2
from helper_transport import bundle_frame, prepare_helper_exec
from execution_pod import ExecutionPodController, ExecutionPodError


def command(number):
    return {'operation_id': str(UUID(int=number)), 'scratch_space_id': 'space',
            'run_id': 'run', 'code': 'true', 'background': True, 'timeout_seconds': 10}


def response(value):
    return SimpleNamespace(stdout=json.dumps(value).encode(), returncode=0)


@pytest.fixture
def controller(tmp_path, monkeypatch):
    pod = {'metadata': {'uid': 'uid'}, 'status': {'phase': 'Running'},
           'spec': {'containers': [{'name': 'execution',
                                    'securityContext': {'capabilities': {'add': ['NET_RAW', 'NET_ADMIN']}}}]}}
    class Backend(SimpleNamespace):
        prepare_helper_exec = staticmethod(prepare_helper_exec)

    backend = Backend(root=tmp_path, kube=Mock(return_value=response({'started': True})))
    controller = ExecutionPodController(backend, {'preview_port': 8080}, admitted=True)
    controller._get = Mock(return_value=pod)
    controller._admitted_uid = Mock(return_value='uid')
    controller._port_answers = Mock(return_value=False)
    thread = Mock()
    monkeypatch.setattr('execution_pod.Thread', thread, raising=False)
    return controller, thread


def observe(controller, *, sleep=lambda _: None, monkeypatch):
    pod, active = next(iter(controller.background_watches.items()))
    ticks = iter(range(12))

    def tick(delay):
        if next(ticks, None) is None:
            pytest.fail('watcher did not retire after its operations ended')
        sleep(delay)

    monkeypatch.setattr('time.sleep', tick)
    controller._watch_pod(pod, active)


def signed_result(controller, operation, *, output_incomplete=True, **fields):
    result = dict(operation_id=operation, run_id='run', status='completed',
                  output_incomplete=output_incomplete, **fields)
    key = controller._read_command(operation)['result_proof_key']
    result['auth'] = sandbox_v2.sign_result(result, key.encode())
    return {'running': False, 'result': result, 'output': {}}


def test_starts_share_one_pod_poll_and_keep_the_authenticated_exit_tail(controller, monkeypatch):
    controller, thread = controller
    operations = []
    for number in range(1, 21):
        request = dict(command(number), notify_on_exit=True)
        receipt = controller.submit_command(request)
        assert receipt['status'] == 'running'
        operations.append(request['operation_id'])
    thread.return_value.start.assert_called_once()
    assert controller.backend.kube.call_count == len(operations)
    for launch, operation in zip(controller.backend.kube.call_args_list, operations, strict=True):
        args = launch.args[0]
        key = controller._read_command(operation)['result_proof_key'].encode()
        assert bundle_frame() + key + b'\n' == launch.kwargs['data']
        assert '-c' in args and '/.aidash-helpers/sandbox_v2.py' not in args
        assert not any(key.decode() in argument for argument in args)
        assert launch.kwargs['timeout'] <= 25

    tail = base64.b64encode(b'final stderr after the saved output filled').decode()
    results = {op: signed_result(controller, op, exit_code=3, output_tail=tail) for op in operations}
    backend = controller.backend
    backend.kube.reset_mock()
    backend.kube.return_value = response(results)
    observe(controller, monkeypatch=monkeypatch)
    backend.kube.assert_called_once()
    assert set(json.loads(backend.kube.call_args.kwargs['data'])) == set(operations)
    assert not controller.background_watches
    for operation in operations:
        result = controller.command_result(operation)
        assert (result['status'], result['exit_code'], result['output_tail']) == ('completed', 3, tail)


def test_exit_notice_is_ready_before_all_saved_output_has_been_collected(controller, monkeypatch):
    controller, _ = controller
    request = dict(command(1), notify_on_exit=True)
    operation = request['operation_id']
    controller.submit_command(request)
    tail = base64.b64encode(b'last').decode()
    observed = signed_result(controller, operation, output_incomplete=False, output_tail=tail)
    polled = []

    def kube(args, data, **kwargs):
        offset = json.loads(data)[operation]['offsets']['stdout']
        chunk = b'a' * (256 * 1024) if not offset else b'last'
        observed['output'] = {'stdout': {'data': base64.b64encode(chunk).decode(),
                                         'next_offset': offset + len(chunk), 'eof': bool(offset)},
                              'stderr': {'data': '', 'next_offset': 0, 'eof': True}}
        return response({operation: observed})

    def between_polls(_):
        polled.append(controller.command_result(operation))

    controller.backend.kube.side_effect = kube
    observe(controller, sleep=between_polls, monkeypatch=monkeypatch)
    assert polled[0]['status'] == 'completed'
    assert polled[0]['output_tail'] == tail
    assert polled[0]['truncated'] is True
    assert base64.b64decode(polled[-1]['stdout']) == b'a' * (256 * 1024) + b'last'
    assert polled[-1]['truncated'] is False


def test_poll_failures_are_logged_before_individual_probes(controller, capsys, monkeypatch):
    controller, _ = controller
    for number in (1, 2):
        controller.submit_command(command(number))
    probes = []

    def kube(args, data, **kwargs):
        operations = list(json.loads(data))
        probes.append(operations)
        if len(operations) == 2:
            raise RuntimeError('poll unavailable')
        op = operations[0]
        return response({op: signed_result(controller, op)})

    controller.backend.kube.side_effect = kube
    observe(controller, monkeypatch=monkeypatch)
    assert [len(batch) for batch in probes] == [2, 2, 2, 1, 1]
    assert capsys.readouterr().err.count('poll failed for Pod') == 3


def test_rejects_unverified_result_until_pod_is_replaced(controller, capsys, monkeypatch):
    controller, _ = controller
    operation = command(1)['operation_id']
    controller.submit_command(command(1))
    forged = signed_result(controller, operation)
    forged['result']['auth'] = 'forged'
    controller.backend.kube.return_value = response({operation: forged})
    after_first_poll = []

    def wait(_):
        if not after_first_poll:
            after_first_poll.append(controller.command_result(operation)['status'])
            controller._get.return_value = {'metadata': {'uid': 'replacement'}}

    observe(controller, sleep=wait, monkeypatch=monkeypatch)
    result = controller._read_command(operation)
    assert after_first_poll == ['running']
    assert result['status'] == 'interrupted'
    assert result['stop_reason'] == 'pod_lost'
    assert result['pod_stop_confirmed'] is True
    assert 'result rejected' in capsys.readouterr().err


def test_watcher_start_failure_is_recorded_and_signals_process(controller):
    controller, thread = controller
    thread.return_value.start.side_effect = RuntimeError('cannot start new thread')
    result = controller.submit_command(command(1))
    assert (result['status'], result['error_code']) == ('failed', 'watcher_start_failed')
    assert not controller.background_watches
    assert controller.backend.kube.call_args.args[0][-3:] == ['stop', result['operation_id'], 'uid']


@pytest.mark.parametrize('kind', ['preview', 'stdio MCP'])
def test_background_start_reports_the_pod_process_limit(controller, monkeypatch, kind):
    controller, thread = controller
    monkeypatch.setenv('AIDASH_PROCESS_LIMIT', '64')

    def fork():
        raise BlockingIOError(errno.EAGAIN, 'fork limit')

    monkeypatch.setattr('sandbox_v2.os.fork', fork)
    controller.backend.kube.side_effect = lambda *args, **kwargs: response(sandbox_v2.supervise_background())
    request = command(1)
    with pytest.raises(ExecutionPodError) as caught:
        if kind == 'preview':
            controller.start_preview(dict(request, port=8080))
        else:
            controller.start_mcp(dict(request, install='', command=['mcp'], env={}, plugin_data='/scratch'))
    assert caught.value.code == 'pod_process_limit'
    assert 'v2_processes = 64' in caught.value.message
    thread.assert_not_called()


def test_saved_output_poll_is_bounded_and_resumes_at_previous_offsets(tmp_path, monkeypatch):
    monkeypatch.setenv('AIDASH_POD_UID', 'uid')
    monkeypatch.setattr(sandbox_v2, 'OPERATION_DIRECTORY', tmp_path)
    monkeypatch.setattr(sandbox_v2, 'OUTPUT_DIRECTORY', tmp_path)
    payload = b'x' * (256 * 1024) + b'next-page'
    operations = {}
    for number in range(1, 10):
        operation = command(number)['operation_id']
        for stream in ('stdout', 'stderr'):
            (tmp_path / f'{operation}.{stream}').write_bytes(payload)
        operations[operation] = {'claim': False, 'offsets': dict(stdout=0, stderr=0)}

    first = sandbox_v2.read_background_results(operations, 'uid')
    sizes = [len(base64.b64decode(chunk['data']))
             for observation in first.values() for chunk in observation['output'].values()]
    assert sum(sizes) == 4 * 1024 * 1024
    assert max(sizes) == 256 * 1024
    assert all(not chunk['eof'] for observed in first.values() for chunk in observed['output'].values())
    for op, observed in first.items():
        operations[op]['offsets'].update({stream: chunk['next_offset'] for stream, chunk in observed['output'].items()})
    second = sandbox_v2.read_background_results(operations, 'uid')
    assert base64.b64decode(second[next(iter(operations))]['output']['stdout']['data']) == b'next-page'
    assert base64.b64decode(second[next(reversed(operations))]['output']['stdout']['data']) == payload[:256 * 1024]
