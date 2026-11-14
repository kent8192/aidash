import json
import tempfile
from pathlib import Path
from unittest.mock import Mock

from execution_pod import ExecutionPodController


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
