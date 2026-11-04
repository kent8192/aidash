"""A Run's unprivileged, stateful Python interpreter inside its Execution Pod."""
import contextlib
import io
import json
import os
import socket
import struct
import sys
import time
import traceback
import threading

_MAX_MESSAGE = 64 * 1048576


def _read_exact(connection, size):
    chunks = bytearray()
    while len(chunks) < size:
        chunk = connection.recv(min(65536, size - len(chunks)))
        if not chunk:
            raise EOFError('Python session closed the connection')
        chunks.extend(chunk)
    return bytes(chunks)


def _receive(connection, limit=_MAX_MESSAGE):
    length = struct.unpack('!I', _read_exact(connection, 4))[0]
    if length > limit:
        raise ValueError('Python session message exceeds the limit')
    return json.loads(_read_exact(connection, length))


def _send(connection, value):
    data = json.dumps(value, ensure_ascii=False, separators=(',', ':')).encode('utf-8')
    if len(data) > _MAX_MESSAGE:
        raise ValueError('Python session response exceeds the limit')
    connection.sendall(struct.pack('!I', len(data)) + data)


class _Output(io.TextIOBase):
    def __init__(self, limit):
        self.limit, self.data, self.truncated = limit, bytearray(), False
        self.lock = threading.Lock()

    def add(self, raw):
        with self.lock:
            chunk = raw[:max(0, self.limit - len(self.data))]
            self.data.extend(chunk)
            self.truncated |= len(chunk) != len(raw)

    def write(self, text):
        self.add(str(text).encode('utf-8', errors='replace'))
        return len(text)

    def value(self):
        with self.lock:
            return self.data.decode('utf-8', errors='replace')


class _DescriptorCapture:
    """Drain native output with one bounded collector for the life of a session."""
    def __init__(self):
        self.read_fd, self.write_fd = os.pipe()
        self.saved = (os.dup(1), os.dup(2))
        os.dup2(self.write_fd, 1)
        os.dup2(self.write_fd, 2)
        self.lock = threading.Lock()
        self.output = None
        self.marker = None
        self.event = None
        self.thread = threading.Thread(target=self._drain, daemon=True)
        self.thread.start()

    def _drain(self):
        pending = b''
        while True:
            chunk = os.read(self.read_fd, 65536)
            if not chunk:
                break
            pending += chunk
            with self.lock:
                marker = self.marker
                position = pending.find(marker) if marker else -1
                if position >= 0:
                    if self.output:
                        self.output.add(pending[:position])
                    pending = pending[position + len(marker):]
                    self.marker = None
                    self.event.set()
                if self.marker:
                    keep = min(len(pending), len(self.marker) - 1)
                    data, pending = pending[:len(pending) - keep], pending[len(pending) - keep:]
                else:
                    data, pending = pending, b''
                if self.output:
                    self.output.add(data)

    def _barrier(self):
        marker = b'\x00aidash-capture-' + os.urandom(32) + b'\x00'
        event = threading.Event()
        with self.lock:
            self.marker, self.event = marker, event
        os.write(self.write_fd, marker)
        if not event.wait(5):
            raise RuntimeError('Python output collector did not drain')

    def begin(self, output):
        self._barrier()
        with self.lock:
            self.output = output

    def end(self):
        self._barrier()
        with self.lock:
            self.output = None

    def close(self):
        os.dup2(self.saved[0], 1)
        os.dup2(self.saved[1], 2)
        for fd in (*self.saved, self.write_fd):
            os.close(fd)



def send_cell(path, code, timeout, output_bytes, *, cwd='.'):
    if not isinstance(code, str) or len(code.encode('utf-8')) > 65536:
        raise ValueError('Python code exceeds the limit')
    if type(timeout) is not int or not 1 <= timeout <= 3600:
        raise ValueError('invalid Python timeout')
    if type(output_bytes) is not int or not 1 <= output_bytes <= 8388608:
        raise ValueError('invalid Python output limit')
    deadline = time.monotonic() + timeout
    while True:
        connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        try:
            connection.settimeout(max(.001, deadline - time.monotonic()))
            connection.connect(path)
            break
        except (FileNotFoundError, ConnectionRefusedError):
            connection.close()
            if time.monotonic() >= deadline:
                raise TimeoutError('Python interpreter did not become ready') from None
            time.sleep(.05)
    with connection:
        connection.settimeout(max(.001, deadline - time.monotonic()))
        _send(connection, {'code': code, 'output_bytes': output_bytes, 'cwd': cwd})
        return _receive(connection)


def serve(path):
    if os.path.lexists(path):
        raise FileExistsError('Python session socket already exists')
    namespace = {'__name__': '__main__'}
    capture = _DescriptorCapture()
    scratch_root = os.path.realpath(os.getcwd())
    last_used = time.monotonic()
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as server:
        server.bind(path)
        os.chmod(path, 0o600)
        server.listen(8)
        server.settimeout(1)
        try:
            while time.monotonic() - last_used < 1800:
                try:
                    connection, _ = server.accept()
                except socket.timeout:
                    continue
                with connection:
                    connection.settimeout(60)
                    try:
                        request = _receive(connection, 131072)
                        code, limit = request['code'], request['output_bytes']
                        cwd = request['cwd']
                        if not isinstance(cwd, str) or cwd.startswith('/') or '..' in cwd.split('/') or '\x00' in cwd:
                            raise ValueError('cwd must be relative to /scratch')
                        target = os.path.realpath(os.path.join(scratch_root, cwd))
                        if os.path.commonpath([scratch_root, target]) != scratch_root:
                            raise ValueError('cwd must resolve beneath /scratch')
                        os.chdir(target)
                        if not isinstance(code, str) or len(code.encode('utf-8')) > 65536:
                            raise ValueError('Python code exceeds the limit')
                        if type(limit) is not int or not 1 <= limit <= 8388608:
                            raise ValueError('invalid Python output limit')
                        output = _Output(limit)
                        status = 'completed'
                        capture.begin(output)
                        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
                            try:
                                try:
                                    compiled = compile(code, '<cell>', 'eval')
                                except SyntaxError:
                                    exec(compile(code, '<cell>', 'exec'), namespace)
                                else:
                                    result = eval(compiled, namespace)
                                    if result is not None:
                                        print(repr(result))
                            except BaseException:
                                status = 'failed'
                                traceback.print_exc()
                        capture.end()
                        _send(connection, {'status': status, 'stdout': output.value(),
                                           'truncated': output.truncated})
                    except Exception as error:
                        try:
                            _send(connection, {'status': 'failed', 'code': 'python_session_error',
                                               'message': str(error) or type(error).__name__})
                        except OSError:
                            pass
                    last_used = time.monotonic()
        finally:
            capture.close()
            try:
                os.unlink(path)
            except FileNotFoundError:
                pass


if __name__ == '__main__':
    if len(sys.argv) == 3 and sys.argv[1] == 'serve':
        serve(sys.argv[2])
    elif len(sys.argv) == 7 and sys.argv[1] == 'cell':
        import base64
        code = base64.b64decode(sys.argv[5], validate=True).decode('utf-8')
        cwd = base64.b64decode(sys.argv[6], validate=True).decode('utf-8')
        print(json.dumps(send_cell(sys.argv[2], code, int(sys.argv[3]), int(sys.argv[4]), cwd=cwd)))
    else:
        raise SystemExit('invalid Python session request')
