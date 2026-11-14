import json
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import pytest

from mcp_session import call


def test_stdio_mcp_session_exchanges_newline_json_and_stops(tmp_path):
    server_script = tmp_path / 'server.py'
    server_script.write_text('import json,sys\nfor line in sys.stdin:\n'
                             ' req=json.loads(line); print(json.dumps({"jsonrpc":"2.0","id":req["id"],'
                             '"result":req["params"]}),flush=True)\n')
    socket_dir = tempfile.TemporaryDirectory(dir='/tmp', prefix='mcp-')
    socket = Path(socket_dir.name) / 'session.sock'
    config = {'command': [sys.executable, str(server_script)], 'install': '', 'env': {}, 'plugin_data': '.'}
    script = Path(__file__).with_name('mcp_session.py')
    process = subprocess.Popen([sys.executable, str(script), 'serve', str(socket), json.dumps(config)],
                               cwd=tmp_path, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    try:
        deadline = time.monotonic() + 5
        while not socket.exists() and time.monotonic() < deadline:
            time.sleep(.01)
        response = call(str(socket), {'jsonrpc': '2.0', 'id': 7, 'method': 'tools/call',
                                      'params': {'name': 'hello'}}, 3)
        assert response == {'jsonrpc': '2.0', 'id': 7, 'result': {'name': 'hello'}}
        assert process.poll() is None
    finally:
        process.terminate()
        process.wait(timeout=5)
        process.stderr.close()
        socket_dir.cleanup()


def test_uncertain_mcp_call_closes_session_instead_of_reusing_stream(tmp_path):
    server_script = tmp_path / 'slow.py'
    server_script.write_text('import sys,time\nfor line in sys.stdin:\n time.sleep(5)\n')
    socket_dir = tempfile.TemporaryDirectory(dir='/tmp', prefix='mcp-')
    socket = Path(socket_dir.name) / 'session.sock'
    config = {'command': [sys.executable, str(server_script)], 'install': '', 'env': {}, 'plugin_data': '.'}
    process = subprocess.Popen([sys.executable, str(Path(__file__).with_name('mcp_session.py')),
                                'serve', str(socket), json.dumps(config)], cwd=tmp_path,
                               stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    try:
        deadline = time.monotonic() + 5
        while not socket.exists() and time.monotonic() < deadline:
            time.sleep(.01)
        with pytest.raises(RuntimeError, match='timed out'):
            call(str(socket), {'jsonrpc': '2.0', 'id': 1, 'method': 'slow'}, 1)
        process.wait(timeout=5)
        assert not socket.exists()
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=5)
        process.stderr.close()
        socket_dir.cleanup()
