"""One stdio MCP process and its serialized local socket bridge."""
import errno
import json
import os
import select
import signal
import socket
import subprocess
import sys
import time

LIMIT = 4 * 1048576
# Reserved for the bridge's own spawn refusal. Plugin exit statuses never leave the bridge.
SPAWN_PROCESS_LIMIT_EXIT = 75


def encode(value, limit=LIMIT):
    data = json.dumps(value, separators=(',', ':'), ensure_ascii=False).encode('utf-8')
    if len(data) > limit:
        raise ValueError('MCP message exceeds 4 MiB')
    return data + b'\n'


def receive_line(stream, limit=LIMIT):
    data = stream.readline(limit + 2)
    if not data or len(data) > limit + 1 or not data.endswith(b'\n'):
        raise ValueError('MCP message is missing or exceeds 4 MiB')
    return json.loads(data)


def call(path, message, timeout):
    if not isinstance(message, dict) or type(timeout) is not int or not 1 <= timeout <= 3600:
        raise ValueError('invalid MCP call')
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(timeout + 1)
        connection.connect(path)
        encode(message)
        connection.sendall(encode({'message': message, 'timeout_seconds': timeout}, LIMIT + 4096))
        with connection.makefile('rb') as stream:
            response = receive_line(stream, LIMIT + 4096)
    if 'error' in response:
        raise RuntimeError(response['error'])
    return response['message']


def write_all(process, data, deadline):
    """Write one message to the server's stdin before `deadline`."""
    outbound = memoryview(data)
    while outbound:
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not select.select([], [process.stdin], [], remaining)[1]:
            raise TimeoutError('MCP call timed out; session closed')
        try:
            written = os.write(process.stdin.fileno(), outbound[:65536])
        except BlockingIOError:
            continue
        if not written:
            raise EOFError('MCP process closed stdin')
        outbound = outbound[written:]


def answer_server_request(request):
    """The answer to a request the server sent: `ping` succeeds, anything else is unsupported."""
    if request.get('method') == 'ping':
        return {'jsonrpc': '2.0', 'id': request['id'], 'result': {}}
    return {'jsonrpc': '2.0', 'id': request['id'],
            'error': {'code': -32601, 'message': 'Method not found'}}


def is_response(message, request_id):
    """Only a message without `method` that carries `result` or `error` answers a request;
    a server request may reuse the pending request's id."""
    return ('method' not in message and ('result' in message or 'error' in message)
            and message.get('id') == request_id)


def ping(path):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(2)
        connection.connect(path)
        connection.sendall(b'{"ping":true}\n')
        with connection.makefile('rb') as stream:
            return receive_line(stream).get('ready') is True


def serve(path, config):
    command, install, env, plugin_data = (config[key] for key in ('command', 'install', 'env', 'plugin_data'))
    if (not isinstance(command, list) or not command or any(not isinstance(x, str) or not x or '\x00' in x for x in command)
            or not isinstance(install, str) or '\x00' in install or not isinstance(env, dict)
            or any(not isinstance(k, str) or not isinstance(v, str) or not k or '=' in k or '\x00' in k + v
                   for k, v in env.items()) or not isinstance(plugin_data, str)):
        raise ValueError('invalid MCP configuration')
    scratch = os.path.realpath(os.getcwd())
    target = os.path.realpath(os.path.join(scratch, plugin_data))
    if os.path.commonpath((scratch, target)) != scratch:
        raise ValueError('plugin_data must be under Scratch Space')
    os.makedirs(target, mode=0o700, exist_ok=True)
    environment = os.environ.copy()
    for key, value in env.items():
        if key not in ('HTTP_PROXY', 'HTTPS_PROXY', 'http_proxy', 'https_proxy', 'NO_PROXY', 'no_proxy', 'NODE_USE_ENV_PROXY', 'PLUGIN_DATA'):
            environment[key] = value
    environment['PLUGIN_DATA'] = target
    # A stop signals the bridge: end through `finally`, so the server process is stopped
    # and the socket removed, instead of leaving the server behind.
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
    try:
        if install:
            subprocess.run(install, shell=True, check=True, env=environment, timeout=300)
        process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=None, env=environment, bufsize=0)
    except OSError as error:
        if error.errno != errno.EAGAIN:
            raise
        return SPAWN_PROCESS_LIMIT_EXIT
    os.set_blocking(process.stdin.fileno(), False)
    pending = bytearray()
    ready_reported = False
    if os.path.lexists(path):
        raise FileExistsError('MCP socket already exists')
    try:
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as server:
            server.bind(path)
            os.chmod(path, 0o600)
            server.listen(8)
            server.settimeout(1)
            while process.poll() is None:
                try:
                    connection, _ = server.accept()
                except socket.timeout:
                    continue
                with connection:
                    connection.settimeout(3601)
                    try:
                        with connection.makefile('rb') as stream:
                            request = receive_line(stream, LIMIT + 4096)
                        if request.get('ping') is True:
                            ready = process.poll() is None
                            connection.sendall(encode({'ready': ready}))
                            ready_reported |= ready
                            continue
                        message, timeout = request['message'], request['timeout_seconds']
                        if not isinstance(message, dict) or type(timeout) is not int or not 1 <= timeout <= 3600:
                            raise ValueError('invalid MCP call')
                        deadline = time.monotonic() + timeout
                        write_all(process, encode(message), deadline)
                        if 'id' not in message:
                            connection.sendall(encode({'message': {}}))
                            continue
                        while True:
                            newline = pending.find(b'\n')
                            if newline >= 0:
                                line = bytes(pending[:newline])
                                del pending[:newline + 1]
                                reply = json.loads(line)
                                if not isinstance(reply, dict):
                                    raise ValueError('invalid MCP response')
                                if is_response(reply, message['id']):
                                    encode(reply)
                                    connection.sendall(encode({'message': reply}, LIMIT + 4096))
                                    break
                                # A server request waits for its answer, so it is answered now;
                                # notifications and stray responses need none.
                                if 'method' in reply and 'id' in reply:
                                    write_all(process, encode(answer_server_request(reply)), deadline)
                                continue
                            remaining = deadline - time.monotonic()
                            if remaining <= 0 or not select.select([process.stdout], [], [], remaining)[0]:
                                raise TimeoutError('MCP call timed out; session closed')
                            chunk = os.read(process.stdout.fileno(), 65536)
                            if not chunk:
                                raise EOFError('MCP process exited')
                            pending.extend(chunk)
                            if len(pending) > LIMIT + 1:
                                raise ValueError('MCP response exceeds 4 MiB')
                    except Exception as error:
                        try:
                            connection.sendall(encode({'error': str(error) or type(error).__name__}))
                        except OSError:
                            pass
                        # The call may have reached the server. Never reuse an uncertain stream.
                        break
            # Let the supervisor inspect stderr after a startup shell exits.
            # Keep this status distinct from the bridge's own spawn refusal.
            if not ready_reported and (process.poll() or 0) > 0:
                return 1
    finally:
        process.terminate()
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
        try:
            os.unlink(path)
        except FileNotFoundError:
            pass


if __name__ == '__main__':
    if len(sys.argv) == 4 and sys.argv[1] == 'serve':
        sys.exit(serve(sys.argv[2], json.loads(sys.argv[3])))
    elif len(sys.argv) == 3 and sys.argv[1] == 'ping':
        print(json.dumps({'ready': ping(sys.argv[2])}))
    elif len(sys.argv) == 4 and sys.argv[1] == 'call':
        request = receive_line(sys.stdin.buffer)
        print(json.dumps({'message': call(sys.argv[2], request, int(sys.argv[3]))}, separators=(',', ':')))
    else:
        raise SystemExit('invalid MCP session request')
