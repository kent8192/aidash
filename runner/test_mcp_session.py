import json
import os
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import pytest

from mcp_session import call

# Each test server records its pid here, so a test can tell the server process ended.
PID_LINE = 'import os,pathlib\npathlib.Path("server.pid").write_text(str(os.getpid()))\n'


def gone(pid, seconds=5):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return True
        time.sleep(.02)
    return False


def server_pid(tmp_path):
    path = tmp_path / 'server.pid'
    deadline = time.monotonic() + 5
    while not path.exists() and time.monotonic() < deadline:
        time.sleep(.01)
    return int(path.read_text())


def test_stdio_mcp_session_exchanges_newline_json_and_stops(tmp_path):
    server_script = tmp_path / 'server.py'
    # The server ignores the end of its input, so only the bridge's stop ends it.
    server_script.write_text(PID_LINE + 'import json,signal,sys\n'
                             'signal.signal(signal.SIGHUP, signal.SIG_IGN)\n'
                             'for line in sys.stdin:\n'
                             ' req=json.loads(line)\n'
                             ' if "id" not in req: continue\n'
                             ' print(json.dumps({"jsonrpc":"2.0","id":req["id"],'
                             '"result":req["params"]}),flush=True)\n'
                             'import time\nwhile True: time.sleep(1)\n')
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
        assert call(str(socket), {'jsonrpc': '2.0', 'method': 'notifications/initialized'}, 3) == {}
        response = call(str(socket), {'jsonrpc': '2.0', 'id': 7, 'method': 'tools/call',
                                      'params': {'name': 'hello'}}, 3)
        assert response == {'jsonrpc': '2.0', 'id': 7, 'result': {'name': 'hello'}}
        assert process.poll() is None
        pid = server_pid(tmp_path)
        # A stop signals the bridge; the session ends with its server and its socket.
        process.send_signal(signal.SIGTERM)
        process.wait(timeout=10)
        assert gone(pid)
        assert not socket.exists()
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=5)
        process.stderr.close()
        socket_dir.cleanup()


def test_server_requests_are_answered_and_never_taken_for_the_reply(tmp_path):
    server_script = tmp_path / 'pinging.py'
    # Before answering, the server pings with the pending call's own id, sends a
    # notification and an unsupported request, and waits for each answer.
    server_script.write_text(PID_LINE + 'import json,sys\n'
                             'def send(value): print(json.dumps(value),flush=True)\n'
                             'def receive(): return json.loads(sys.stdin.readline())\n'
                             'while True:\n'
                             ' req=receive()\n'
                             ' if "id" not in req: continue\n'
                             ' send({"jsonrpc":"2.0","id":req["id"],"method":"ping"})\n'
                             ' pong=receive()\n'
                             ' send({"jsonrpc":"2.0","method":"notifications/message","params":{}})\n'
                             ' send({"jsonrpc":"2.0","id":"s1","method":"sampling/createMessage","params":{}})\n'
                             ' other=receive()\n'
                             ' send({"jsonrpc":"2.0","id":req["id"],"result":{"pong":pong,"other":other}})\n')
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
        response = call(str(socket), {'jsonrpc': '2.0', 'id': 3, 'method': 'tools/call',
                                      'params': {'name': 'slow'}}, 5)
        assert response == {'jsonrpc': '2.0', 'id': 3, 'result': {
            'pong': {'jsonrpc': '2.0', 'id': 3, 'result': {}},
            'other': {'jsonrpc': '2.0', 'id': 's1',
                      'error': {'code': -32601, 'message': 'Method not found'}}}}
        # The session is still usable after answering the server.
        again = call(str(socket), {'jsonrpc': '2.0', 'id': 4, 'method': 'tools/call',
                                   'params': {'name': 'slow'}}, 5)
        assert again['id'] == 4 and again['result']['pong']['id'] == 4
        pid = server_pid(tmp_path)
        process.send_signal(signal.SIGTERM)
        process.wait(timeout=10)
        assert gone(pid)
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=5)
        process.stderr.close()
        socket_dir.cleanup()


def test_uncertain_mcp_call_closes_session_instead_of_reusing_stream(tmp_path):
    server_script = tmp_path / 'slow.py'
    server_script.write_text(PID_LINE + 'import sys,time\nfor line in sys.stdin:\n time.sleep(5)\n')
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
        process.wait(timeout=10)
        assert not socket.exists()
        assert gone(server_pid(tmp_path))
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=5)
        process.stderr.close()
        socket_dir.cleanup()
