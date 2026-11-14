import io
import json
from types import SimpleNamespace

import pytest

from control import Handler
from execution_pod import ExecutionPodController, ExecutionPodError


OPERATION = '00000000-0000-4000-8000-000000000001'


def test_refusal_survives_restart_and_expires_after_seven_days(tmp_path):
    controller = ExecutionPodController(SimpleNamespace(root=tmp_path), {}, admitted=False)
    assert controller.refuse_command(OPERATION, 's', 'r') == {'refused': True}
    restarted = ExecutionPodController(SimpleNamespace(root=tmp_path), {}, admitted=False)
    assert restarted.refuse_command(OPERATION, 's', 'r') == {'refused': True}
    with pytest.raises(ExecutionPodError) as error:
        restarted.submit_command({'operation_id': OPERATION, 'scratch_space_id': 's', 'run_id': 'r', 'code': 'true'})
    assert error.value.code == 'command_refused'
    record = restarted._read_command(OPERATION)
    restarted.prune_space('s', now=record['terminal_at'] + 7 * 86400 - 1)
    assert restarted._command_path(OPERATION).exists()
    restarted.prune_space('s', now=record['terminal_at'] + 7 * 86400 + 1)
    assert not restarted._command_path(OPERATION).exists()


def test_refusal_reports_an_existing_accepted_operation_without_mutation(tmp_path):
    controller = ExecutionPodController(SimpleNamespace(root=tmp_path), {}, admitted=True)
    record = {'operation_id': OPERATION, 'scratch_space_id': 's', 'run_id': 'r', 'status': 'running'}
    controller._command_path(OPERATION).write_text(json.dumps(record))
    assert controller.refuse_command(OPERATION, 's', 'r') == {'refused': False, 'status': 'running'}
    assert controller._read_command(OPERATION) == record


def test_refusal_http_route_persists_an_unsent_command(tmp_path):
    controller = ExecutionPodController(SimpleNamespace(root=tmp_path), {}, admitted=False)
    handler = object.__new__(Handler)
    body = json.dumps({'operation_id': OPERATION, 'scratch_space_id': 's', 'run_id': 'r'}).encode()
    handler.server = SimpleNamespace(runner=SimpleNamespace(token='x' * 32, execution_pods=controller))
    handler.command = 'POST'
    handler.path = '/v2/commands/refuse'
    handler.headers = {'Authorization': 'Bearer ' + 'x' * 32, 'Content-Length': str(len(body))}
    handler.rfile = io.BytesIO(body)
    assert handler.handle_request() == {'refused': True}
    assert controller._read_command(OPERATION)['status'] == 'refused'


def test_refusal_wins_a_concurrent_submit_from_another_space(tmp_path):
    import threading
    entered, proceed = threading.Event(), threading.Event()
    controller = ExecutionPodController(SimpleNamespace(root=tmp_path), {}, admitted=True)
    def get_pod(*_):
        entered.set()
        assert proceed.wait(5)
        return {'metadata': {'uid': 'uid'}, 'status': {'phase': 'Running'}, 'spec': {'containers': []}}
    controller._get = get_pod
    result = []
    def submit():
        try:
            controller.submit_command({'operation_id': OPERATION, 'scratch_space_id': 'one',
                                       'run_id': 'r1', 'code': 'true'})
        except ExecutionPodError as error:
            result.append(error.code)
    thread = threading.Thread(target=submit)
    thread.start()
    try:
        assert entered.wait(5)
        assert controller.refuse_command(OPERATION, 'two', 'r2') == {'refused': True}
    finally:
        proceed.set()
        thread.join(timeout=5)
    assert not thread.is_alive()
    assert result == ['command_refused']


def test_idle_cleanup_expires_refusal_even_if_crash_preceded_space_metadata(tmp_path):
    controller = ExecutionPodController(SimpleNamespace(root=tmp_path), {}, admitted=False)
    record = {'operation_id': OPERATION, 'scratch_space_id': 's', 'run_id': 'r',
              'status': 'refused', 'terminal_at': 1}
    controller._command_path(OPERATION).write_text(json.dumps(record))
    controller.stop_idle(now=7 * 86400 + 2)
    assert not controller._command_path(OPERATION).exists()
