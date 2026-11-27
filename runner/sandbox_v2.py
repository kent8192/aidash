"""Execution Pod process privilege reduction, called in a child before exec."""
import ctypes
import os

# Script mode under -I has no script directory on sys.path. Streamed
# modules resolve through the launcher's in-memory importer first.
import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parent))
from helper_protocol import HELPER_PROTOCOL
from pod_base import (
    _PR_SET_DUMPABLE, OPERATION_DIRECTORY, OUTPUT_DIRECTORY,
    bootstrap, bootstrap_ready, chroot_command, drop_user_capabilities,
    enter_user_root, save_image_environment,
)

def run_admission_probe(root, docker_enabled, kind, targets):
    # Load trusted code and dependencies before entering the writable image.
    import json
    import sys
    from pathlib import Path
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import user_probe
    import network_probe
    enter_user_root(root, '.', docker_enabled)
    # Re-admission can meet live user processes with the same credentials, so
    # they must not be able to ptrace this process and rewrite its evidence.
    if ctypes.CDLL(None, use_errno=True).prctl(_PR_SET_DUMPABLE, 0, 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), 'prctl')
    if kind == 'user':
        return user_probe.probe()
    if kind == 'network':
        return network_probe.probe(json.loads(targets), docker_enabled)
    raise ValueError('invalid probe kind')


FORWARD_GRACE_SECONDS = 5


def validate_operation_id(operation_id):
    import uuid
    if not isinstance(operation_id, str):
        raise ValueError('invalid operation ID')
    base = operation_id[:-5] if operation_id.endswith('.left') else operation_id
    if str(uuid.UUID(base)) != base:
        raise ValueError('canonical operation UUID required')


def sign_result(result, key):
    import hashlib
    import hmac
    import json
    payload = {name: value for name, value in result.items() if name != 'auth'}
    data = json.dumps(payload, sort_keys=True, separators=(',', ':')).encode()
    return hmac.new(key, data, hashlib.sha256).hexdigest()


def run_supervised(argv, timeout, stop_requested=None, *, output_directory=None, operation_id=None, output_limit=8388608,
                   on_left=None, capture_tail=False, on_started=None):
    """Run one command in a dedicated supervisor and reap escaped descendants.

    This must be called in a single-purpose process: becoming a subreaper also
    adopts orphan descendants. A failure to prove termination is reported to
    the controller, which must stop the Pod rather than replay the command.
    """
    import base64
    import errno
    import signal
    import subprocess
    import time

    if stop_requested is not None and stop_requested.is_set():
        return {'status': 'stopped'}
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
        code = ctypes.get_errno()
        raise OSError(code, os.strerror(code))
    import select
    import threading
    from pathlib import Path
    streams = {}
    readers = []
    output_state = {'used': 0, 'incomplete': False, 'truncated': False, 'forward_incomplete': False}
    tail = bytearray()
    output_lock = threading.Lock()
    forward_lock = threading.Lock()
    reader_stop = threading.Event()
    forward_stop = threading.Event()
    if output_directory is not None:
        try:
            Path(output_directory).mkdir(mode=0o700, parents=True, exist_ok=True)
            for name in ('stdout', 'stderr'):
                fd = os.open(Path(output_directory) / (operation_id + '.' + name),
                             os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
                streams[name] = os.fdopen(fd, 'wb', buffering=0)
        except OSError:
            output_state['incomplete'] = True
    try:
        process = subprocess.Popen(argv, start_new_session=True,
                                   stdout=subprocess.PIPE if output_directory is not None else None,
                                   stderr=subprocess.PIPE if output_directory is not None else None)
    except OSError as error:
        for stream in streams.values():
            stream.close()
        if on_started is not None and error.errno == errno.EAGAIN:
            return process_limit_refusal()
        raise
    def drain(name, source, target):
        forwarding = on_started is None
        save = streams.get(name)
        try:
            while not reader_stop.is_set():
                readable, _, _ = select.select([source], [], [], .1)
                if not readable:
                    continue
                data = os.read(source.fileno(), 65536)
                if not data:
                    break
                with output_lock:
                    if capture_tail:
                        tail.extend(data[-4096:])
                        del tail[:-4096]
                    chunk = data[:max(0, output_limit - output_state['used'])]
                    output_state['used'] += len(chunk)
                    if len(chunk) < len(data):
                        output_state['truncated'] = True
                    if chunk and save:
                        try:
                            view = memoryview(chunk)
                            while view:
                                count = save.write(view)
                                if not count:
                                    raise OSError('output save made no progress')
                                view = view[count:]
                        except OSError:
                            output_state['incomplete'] = True
                            try:
                                save.close()
                            except OSError:
                                pass
                            save = None
                    elif chunk and not save:
                        output_state['incomplete'] = True
                if forwarding and forward_stop.is_set():
                    forwarding = False
                    output_state['forward_incomplete'] = True
                if forwarding:
                    try:
                        view = memoryview(data)
                        while view and not reader_stop.is_set() and not forward_stop.is_set():
                            with forward_lock:
                                if forward_stop.is_set():
                                    break
                                _, writable, _ = select.select([], [target], [], .1)
                                if writable:
                                    count = os.write(target, view[:4096])
                                    if not count:
                                        raise BrokenPipeError()
                                    view = view[count:]
                        if view:
                            forwarding = False
                            output_state['forward_incomplete'] = True
                    except (OSError, ValueError):
                        forwarding = False
                        output_state['forward_incomplete'] = True
        except OSError:
            output_state['incomplete'] = True
        finally:
            if save:
                try:
                    save.close()
                except OSError:
                    output_state['incomplete'] = True
            source.close()
    reader_start_failed = False
    if output_directory is not None:
        for name, source, target in (('stdout', process.stdout, 1), ('stderr', process.stderr, 2)):
            reader = threading.Thread(target=drain, args=(name, source, target), daemon=True)
            try:
                reader.start()
            except RuntimeError:
                if on_started is None:
                    raise
                reader_start_failed = True
                output_state['incomplete'] = True
                source.close()
                if name in streams:
                    streams[name].close()
                continue
            readers.append(reader)
    if on_started is not None and not reader_start_failed:
        on_started()
    def finish(result):
        if readers:
            deadline = time.monotonic() + FORWARD_GRACE_SECONDS
            for reader in readers:
                reader.join(max(0, deadline - time.monotonic()))
            unfinished = any(reader.is_alive() for reader in readers)
            if unfinished:
                forward_stop.set()
                # Reaping every descendant guarantees EOF, so only a survivor can
                # hold a pipe open. The bound keeps a stuck save from losing the result.
                drain_deadline = time.monotonic() + (0 if result['status'] == 'termination_unconfirmed' else 30)
                for reader in readers:
                    reader.join(max(0, drain_deadline - time.monotonic()))
                unfinished = any(reader.is_alive() for reader in readers)
                if unfinished:
                    reader_stop.set()
                    for reader in readers:
                        reader.join(.5)
            result['output_incomplete'] = output_state['incomplete'] or unfinished
            result['output_truncated'] = output_state['truncated']
            result['forward_incomplete'] = output_state['forward_incomplete'] or unfinished
        if capture_tail:
            with output_lock:
                result['output_tail'] = base64.b64encode(tail).decode('ascii')
        return result
    def stop_forwarding():
        forward_stop.set()
        with forward_lock:
            null = os.open('/dev/null', os.O_WRONLY)
            try:
                os.dup2(null, 1)
                os.dup2(null, 2)
            finally:
                os.close(null)

    expires = None if timeout is None else time.monotonic() + timeout
    exited_at = None
    while expires is None or time.monotonic() < expires:
        if reader_start_failed or (stop_requested is not None and stop_requested.is_set()):
            break
        exit_code = process.poll()
        if exit_code is not None:
            if on_left is not None and exited_at is None:
                exited_at = time.monotonic()
                expires = None
            # Orphaned descendants are adopted by this supervisor, even after
            # the shell has exited. Keep them tracked until they have stopped.
            try:
                while os.waitpid(-1, os.WNOHANG)[0] > 0:
                    pass
            except ChildProcessError:
                return finish({'status': 'completed', 'exit_code': exit_code})
            if on_left is not None and time.monotonic() - exited_at >= 2:
                with output_lock:
                    state = dict(output_state)
                on_left(exit_code, stop_forwarding, state)
                on_left = None
        time.sleep(0.02)

    stopped = reader_start_failed or (stop_requested is not None and stop_requested.is_set())
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        # poll reaps the original process before the general waitpid loop.
        process.poll()
        try:
            with open(f'/proc/self/task/{os.getpid()}/children') as file:
                children = [int(value) for value in file.read().split()]
        except FileNotFoundError:
            return finish({'status': 'termination_unconfirmed'})
        if not children:
            return finish({'status': 'stopped' if stopped else 'timed_out', 'exit_code': process.returncode})
        for pid in children:
            try:
                fd = os.pidfd_open(pid)
                try:
                    with open(f'/proc/{pid}/status') as file:
                        parent = next(line for line in file if line.startswith('PPid:'))
                    if int(parent.split()[1]) == os.getpid():
                        signal.pidfd_send_signal(fd, signal.SIGKILL)
                finally:
                    os.close(fd)
            except (ProcessLookupError, FileNotFoundError):
                continue
            except OSError:
                return finish({'status': 'termination_unconfirmed'})
        for pid in children:
            if pid == process.pid:
                process.poll()
            else:
                try:
                    os.waitpid(pid, os.WNOHANG)
                except ChildProcessError:
                    pass
        time.sleep(0.02)
    return finish({'status': 'termination_unconfirmed'})


def disk_tolerant_track(action, reserve, on_full=None):
    import errno
    try:
        action()
        return True
    except OSError as error:
        if error.errno not in (errno.ENOSPC, errno.EDQUOT):
            raise
    if on_full is not None:
        on_full.set()
    try:
        os.unlink(reserve)
    except FileNotFoundError:
        pass
    try:
        action()
        return True
    except OSError as error:
        if error.errno not in (errno.ENOSPC, errno.EDQUOT):
            raise
        return False


def supervise_command(operation_id, run_id, cwd, code, timeout, docker_enabled, *, expected_pod_uid,
                      record_changes=True, output_limit=8388608, notify_left=None, left_auth=None, capture_tail=False,
                      notify_started=None):
    """Execute once, persist its outcome, and record its observed system changes."""
    import json
    from pathlib import Path
    import signal
    import sys
    import threading
    import time
    import uuid

    if str(uuid.UUID(operation_id)) != operation_id:
        raise ValueError('canonical operation UUID required')
    if not isinstance(run_id, str) or not run_id:
        raise ValueError('run_id must be a nonempty string')
    if not isinstance(code, str) or len(code.encode('utf-8')) > 131072:
        raise ValueError('command exceeds the code limit')
    if timeout is not None and (type(timeout) is not int or not 1 <= timeout <= 86400):
        raise ValueError('invalid command timeout')
    pod_uid = os.environ.get('AIDASH_POD_UID')
    if not pod_uid or pod_uid != expected_pod_uid:
        raise ValueError('Execution Pod identity changed')
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from execution_pod import change_interval, chroot_helper_command

    state = Path(OPERATION_DIRECTORY)
    state.mkdir(mode=0o700, exist_ok=True)
    if not claim_operation(operation_id, state):
        raise FileExistsError('operation already started')
    root = '/scratch-system/image-root/rootfs'
    stopping = threading.Event()
    signal.signal(signal.SIGUSR1, lambda *_: stopping.set())
    running = state / (operation_id + '.running')
    pending_running = state / (operation_id + '.running.new')
    marker = {'pid': os.getpid(), 'start_time': process_start_time(os.getpid()), 'pod_uid': pod_uid}
    def publish_running(payload):
        with pending_running.open('x', encoding='utf-8') as file:
            json.dump(payload, file)
            file.flush()
            os.fsync(file.fileno())
        os.replace(pending_running, running)
    publish_running(marker)
    left_id = operation_id + '.left'
    adopted = False
    def persist_result(identifier, result):
        temporary = state / (identifier + '.new')
        with temporary.open('x', encoding='utf-8') as file:
            json.dump(result, file, separators=(',', ':'))
            file.flush()
            os.fsync(file.fileno())
        os.replace(temporary, state / (identifier + '.json'))
        directory = os.open(state, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)

    def adopt(exit_code, stop_forwarding, output_state):
        nonlocal adopted
        if not claim_operation(left_id, state):
            raise FileExistsError('adopted operation already exists')
        for stream in ('stdout', 'stderr'):
            source = Path(OUTPUT_DIRECTORY) / (operation_id + '.' + stream)
            try:
                os.link(source, Path(OUTPUT_DIRECTORY) / (left_id + '.' + stream), follow_symlinks=False)
            except FileNotFoundError:
                pass
        left_running = state / (left_id + '.running')
        with left_running.open('x', encoding='utf-8') as file:
            json.dump({'pid': os.getpid(), 'start_time': process_start_time(os.getpid()), 'pod_uid': pod_uid}, file)
            file.flush()
            os.fsync(file.fileno())
        result = {'operation_id': operation_id, 'run_id': run_id, 'status': 'completed',
                  'exit_code': exit_code, 'left_running': left_id,
                  'output_incomplete': output_state['incomplete'], 'output_truncated': output_state['truncated']}
        persist_result(operation_id, result)
        adopted = True
        stop_forwarding()
        notify_left(result)
    interval_stop = threading.Event()
    interval_errors = []
    disk_full = threading.Event()
    tracking_started = False
    if record_changes:
        tracking_started = disk_tolerant_track(
            lambda: change_interval(root, '/scratch-system', operation_id, run_id),
            '/scratch-system/.space-reserve', disk_full)
        if not tracking_started:
            disk_full.set()
        def track():
            while not interval_stop.wait(2):
                try:
                    if not disk_tolerant_track(lambda: change_interval(root, '/scratch-system'),
                                               '/scratch-system/.space-reserve', disk_full):
                        disk_full.set()
                except Exception as error:
                    interval_errors.append(error)
                    stopping.set()
                    return
        if tracking_started:
            tracker = threading.Thread(target=track, daemon=True)
            tracker.start()
    argv = chroot_helper_command('docker' if docker_enabled else 'normal', root, cwd, '/bin/sh', '-c', code)
    try:
        if capture_tail and left_auth:
            marker.update(operation_id=operation_id, running_at=time.time(), protocol_version=HELPER_PROTOCOL)
            marker['auth'] = sign_result(marker, left_auth)
            publish_running(marker)
        result = run_supervised(argv, timeout, stop_requested=stopping,
                                output_directory=OUTPUT_DIRECTORY,
                                operation_id=operation_id, output_limit=output_limit,
                                on_left=adopt if notify_left else None, capture_tail=capture_tail,
                                on_started=notify_started)
    finally:
        if tracking_started:
            interval_stop.set()
            tracker.join()
            if not disk_tolerant_track(
                    lambda: change_interval(root, '/scratch-system', operation_id, run_id, finished=True),
                    '/scratch-system/.space-reserve', disk_full):
                disk_full.set()
    if interval_errors:
        raise RuntimeError('change tracking failed: ' + str(interval_errors[0]))
    if disk_full.is_set():
        result.update(error_code='disk_full', message='System change tracking was incomplete because the Scratch Space is full')
    result.update(operation_id=left_id if adopted else operation_id, run_id=run_id,
                  protocol_version=HELPER_PROTOCOL)
    if left_auth:
        result['auth'] = sign_result(result, left_auth)
    persist_result(result['operation_id'], result)
    running.unlink(missing_ok=True)
    if adopted:
        (state / (left_id + '.running')).unlink(missing_ok=True)
    return result


def supervise_foreground(*args, **kwargs):
    return supervise_detached(*args, adopt_left=True, **kwargs)


def process_limit_refusal():
    limit = int(os.environ['AIDASH_PROCESS_LIMIT'])
    return {'status': 'completed', 'exit_code': 1, 'error_code': 'pod_process_limit',
            'message': f'The Execution Pod process limit (v2_processes = {limit}) was reached. '
                       'Stop a Background process and retry.', 'process_limit': limit,
            'output_incomplete': True}


def supervise_background(*args, **kwargs):
    """Return a startup receipt while the supervisor retains its deadline and output files."""
    import errno
    import json
    read_fd, write_fd = os.pipe()
    try:
        child = os.fork()
    except OSError as error:
        os.close(read_fd)
        os.close(write_fd)
        if error.errno == errno.EAGAIN:
            return process_limit_refusal()
        raise
    if child:
        os.close(write_fd)
        with os.fdopen(read_fd, 'rb') as stream:
            # Bound the supervisor receipt like the foreground startup pipe.
            data = stream.readline(65537)
        if not data or len(data) > 65536:
            raise RuntimeError('Background supervisor startup receipt unavailable')
        receipt = json.loads(data)
        if not receipt.get('started'):
            os.waitpid(child, 0)
        return receipt
    os.close(read_fd)
    sent = False

    def notify(receipt):
        nonlocal sent
        sent = True
        try:
            os.write(write_fd, json.dumps(receipt, separators=(',', ':')).encode() + b'\n')
        except BrokenPipeError:
            pass  # The Runner can recover through the supervisor's journal.
        finally:
            os.close(write_fd)

    try:
        os.setsid()
        with open('/dev/null', 'rb+') as null:
            for target in (0, 1, 2):
                os.dup2(null.fileno(), target)
        result = supervise_command(*args, notify_started=lambda: notify({'started': True}), **kwargs)
        if not sent:
            notify(result if result.get('error_code') == 'pod_process_limit' else {'started': False})
    except BaseException:
        if not sent:
            notify({'started': False})
    finally:
        os._exit(0)


def supervise_detached(*args, adopt_left=False, **kwargs):
    """Keep the supervisor alive independently of the exec transport session."""
    import json
    read_fd, write_fd = os.pipe()
    child = os.fork()
    if child:
        os.close(write_fd)
        with os.fdopen(read_fd, 'rb') as stream:
            data = stream.readline(65537)
        if not data or len(data) > 65536:
            os.waitpid(child, 0)
            raise RuntimeError('foreground supervisor failed')
        result = json.loads(data)
        if not result.get('left_running'):
            os.waitpid(child, 0)
        if 'supervisor_error' in result:
            raise RuntimeError('foreground supervisor failed')
        return result
    os.close(read_fd)
    sent = False
    def notify(result):
        nonlocal sent
        sent = True
        try:
            os.write(write_fd, json.dumps(result, separators=(',', ':')).encode() + b'\n')
        except BrokenPipeError:
            pass
    try:
        os.setsid()
        result = supervise_command(*args, notify_left=notify if adopt_left else None, **kwargs)
        if not sent:
            notify(result)
    except BaseException:
        if not sent:
            notify({'supervisor_error': True})
    finally:
        os.close(write_fd)
        os._exit(0)


def process_start_time(pid):
    with open(f'/proc/{pid}/stat', encoding='ascii') as file:
        # comm is parenthesized and may itself contain spaces or parentheses.
        fields = file.read().rsplit(')', 1)[1].split()
    return int(fields[19])


def stop_operation(operation_id):
    import json
    import signal
    from pathlib import Path

    validate_operation_id(operation_id)
    path = Path(OPERATION_DIRECTORY) / (operation_id + '.running')
    record = json.loads(path.read_bytes())
    if not os.environ.get('AIDASH_POD_UID') or record['pod_uid'] != os.environ['AIDASH_POD_UID']:
        raise ValueError('operation belongs to another Pod')
    pid = record['pid']
    if type(pid) is not int or pid <= 1:
        raise ValueError('invalid supervisor identity')
    fd = os.pidfd_open(pid)
    try:
        if process_start_time(pid) != record['start_time']:
            raise ValueError('supervisor identity changed')
        signal.pidfd_send_signal(fd, signal.SIGUSR1)
    finally:
        os.close(fd)


def log_chunk(path, offset, limit):
    import base64
    import fcntl
    if (offset is not None and (type(offset) is not int or offset < 0)) or type(limit) is not int or not 1 <= limit <= 262144:
        raise ValueError('invalid log offset or limit')
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    except FileNotFoundError:
        return {'data': '', 'next_offset': offset or 0, 'eof': True, 'size': 0}
    with os.fdopen(fd, 'rb') as file:
        fcntl.flock(file, fcntl.LOCK_SH)
        size = os.fstat(file.fileno()).st_size
        if offset is None:
            offset = max(0, size - limit)
        file.seek(offset)
        data = file.read(limit)
        eof = file.tell() >= size
    return {'data': base64.b64encode(data).decode('ascii'), 'next_offset': offset + len(data), 'eof': eof, 'size': size}


def claim_operation(operation_id, directory='/scratch-system/operations'):
    from pathlib import Path
    validate_operation_id(operation_id)
    directory = Path(directory)
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    try:
        fd = os.open(directory / (operation_id + '.started'),
                     os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    except FileExistsError:
        return False
    try:
        os.fsync(fd)
    finally:
        os.close(fd)
    fd = os.open(directory, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)
    return True


def operation_output(operation_id, stream, offset, limit, directory='/scratch-system/operation-output'):
    from pathlib import Path
    validate_operation_id(operation_id)
    if stream not in ('stdout', 'stderr'):
        raise ValueError('invalid operation output')
    path = Path(directory) / (operation_id + '.' + stream)
    if not path.exists():
        raise FileNotFoundError('operation output unavailable')
    return log_chunk(path, offset, limit)


def forget_operation(operation_id, directory='/scratch-system/operations', output_directory='/scratch-system/operation-output'):
    from pathlib import Path
    validate_operation_id(operation_id)
    for suffix in ('.started', '.running', '.running.new', '.new', '.json'):
        (Path(directory) / (operation_id + suffix)).unlink(missing_ok=True)
    for suffix in ('.stdout', '.stderr'):
        (Path(output_directory) / (operation_id + suffix)).unlink(missing_ok=True)


def operation_running(operation_id):
    return operation_state(operation_id)['running']


def operation_state(operation_id):
    import json
    from pathlib import Path
    validate_operation_id(operation_id)
    path = Path(OPERATION_DIRECTORY) / (operation_id + '.running')
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
        with os.fdopen(fd, 'rb') as file:
            record = json.loads(file.read(8192))
        if (record.get('pod_uid') == os.environ.get('AIDASH_POD_UID')
                and process_start_time(record['pid']) == record['start_time']):
            return {'running': True, 'supervisor': record}
    except (FileNotFoundError, ProcessLookupError):
        pass
    return {'running': False}


def read_result(operation_id):
    import json
    from pathlib import Path
    validate_operation_id(operation_id)
    path = Path(OPERATION_DIRECTORY) / (operation_id + '.json')
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd, 'rb') as file:
        data = file.read(65537)
    if len(data) > 65536:
        raise ValueError('command result exceeds the limit')
    return json.loads(data)


# Bound saved output per exec to 4 MiB, in 256 KiB chunks, so large logs can
# advance across polls without exhausting the watcher transport or starving other operations.
BACKGROUND_POLL_BYTES = 4 << 20
BACKGROUND_STREAM_BYTES = 256 << 10


def read_background_results(operations, expected_pod_uid, budget=BACKGROUND_POLL_BYTES):
    if os.environ.get('AIDASH_POD_UID') != expected_pod_uid:
        raise ValueError('Execution Pod identity changed')
    if not 0 <= budget <= BACKGROUND_POLL_BYTES:
        raise ValueError('invalid Background output budget')
    remaining = budget
    results = {}
    for operation, request in operations.items():
        try:
            if request['claim'] and claim_operation(operation, OPERATION_DIRECTORY):
                results[operation] = {'claimed': True}
                continue
            try:
                result = read_result(operation)
            except FileNotFoundError:
                result = None
            observed = dict(operation_state(operation), result=result, output={})
            for stream in ('stdout', 'stderr'):
                if not remaining:
                    break
                limit = min(remaining, BACKGROUND_STREAM_BYTES)
                try:
                    chunk = operation_output(operation, stream, request['offsets'][stream], limit,
                                             directory=OUTPUT_DIRECTORY)
                except FileNotFoundError:
                    continue
                remaining -= chunk['next_offset'] - request['offsets'][stream]
                observed['output'][stream] = chunk
            results[operation] = observed
        except (OSError, ValueError) as error:
            results[operation] = {'error': str(error)}
    return results


if __name__ == '__main__':
    import sys
    if sys.argv[1:] == ['protocol']:
        import json
        print(json.dumps({'version': HELPER_PROTOCOL}))
        raise SystemExit(0)
    if sys.argv[1:] == ['init']:
        from pathlib import Path
        sys.path.insert(0, str(Path(__file__).resolve().parent))
        from pod_base import initialize_system_root
        copied = initialize_system_root('/', '/scratch-image', reset=os.environ.get('AIDASH_RESET_SYSTEM') == 'true',
                                        image_identity=os.environ['AIDASH_IMAGE_ID'])
        if copied or not (Path('/scratch-image') / 'environment.json').exists():
            save_image_environment('/scratch-image', os.environ)
        raise SystemExit(0)
    if len(sys.argv) == 3 and sys.argv[1] == 'image-status':
        import json
        from pathlib import Path
        sys.path.insert(0, str(Path(__file__).resolve().parent))
        from execution_pod import system_image_changed
        print(json.dumps({'image_changed': system_image_changed('/scratch-system/image-root', sys.argv[2])}))
        raise SystemExit(0)
    if sys.argv[1:] == ['ingress-listen']:
        import json
        import select
        import socket
        import time
        # A re-admitted Pod can contain user processes with the same UID.
        if sys.platform.startswith('linux') and ctypes.CDLL(None, use_errno=True).prctl(_PR_SET_DUMPABLE, 0, 0, 0, 0) != 0:
            raise OSError(ctypes.get_errno(), 'prctl')
        with socket.socket() as server:
            server.bind(('0.0.0.0', 0))
            server.listen(4)
            print(json.dumps({'port': server.getsockname()[1]}), flush=True)
            deadline = time.monotonic() + 90
            while time.monotonic() < deadline:
                ready, _, _ = select.select([server, sys.stdin], [], [], max(0, deadline - time.monotonic()))
                if sys.stdin in ready:
                    break
                if server in ready:
                    connection, _ = server.accept()
                    connection.close()
        raise SystemExit(0)
    if sys.argv[1:] == ['bootstrap']:
        bootstrap()
    if sys.argv[1:] == ['ready']:
        raise SystemExit(0 if bootstrap_ready() else 1)
    if len(sys.argv) in (4, 5) and sys.argv[1] == 'port-check':
        import ipaddress
        import json
        import socket
        if os.environ.get('AIDASH_POD_UID') != sys.argv[2]:
            raise ValueError('Execution Pod identity changed')
        port = int(sys.argv[3])
        if not 1 <= port <= 65535:
            raise ValueError('invalid port')
        # The Pod's own address unless a caller names one (loopback for an occupancy check).
        host = str(ipaddress.ip_address(sys.argv[4])) if len(sys.argv) == 5 else '127.0.0.1'
        try:
            with socket.create_connection((host, port), timeout=1):
                ready = True
        except OSError:
            ready = False
        print(json.dumps({'ready': ready}))
        raise SystemExit(0)
    if len(sys.argv) == 4 and sys.argv[1] == 'files':
        import json
        from pathlib import Path
        if os.environ.get('AIDASH_POD_UID') != sys.argv[3]:
            raise ValueError('Execution Pod identity changed')
        sys.path.insert(0, str(Path(__file__).resolve().parent))
        from files_v2 import FileService, InvalidPath, PatchReceiptUnavailable, run_bounded
        try:
            request = json.loads(sys.stdin.buffer.read(2 * 1048576 + 1))
            service = FileService('/scratch-system/scratch', '/scratch-system/file-writes')
            operation = sys.argv[2]
            if operation not in ('read', 'search', 'write', 'manifest', 'patch'):
                raise ValueError('invalid file operation')
            result = {'result': run_bounded(service, operation, request)}
        except PatchReceiptUnavailable as error:
            result = {'error': {'code': 'patch_receipt_unavailable', 'message': str(error)}}
        except InvalidPath as error:
            result = {'error': {'code': 'invalid_path', 'message': str(error)}}
        except (ValueError, KeyError, TypeError, OSError) as error:
            result = {'error': {'code': 'file_error', 'message': str(error)}}
        print(json.dumps(result, separators=(',', ':')))
        raise SystemExit(0)
    if len(sys.argv) == 4 and sys.argv[1] == 'python-check' and sys.argv[2] in ('docker', 'normal'):
        import json
        import subprocess
        from pathlib import Path
        if os.environ.get('AIDASH_POD_UID') != sys.argv[3]:
            raise ValueError('Execution Pod identity changed')
        sys.path.insert(0, str(Path(__file__).resolve().parent))
        from execution_pod import chroot_helper_command
        probe = subprocess.run(chroot_helper_command(sys.argv[2], '/scratch-system/image-root/rootfs', '.',
            'python3', '-c', 'import sys; sys.exit(0 if sys.version_info >= (3, 8) else 1)'),
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15)
        print(json.dumps({'available': probe.returncode == 0}))
        raise SystemExit(0)
    if (len(sys.argv) in (11, 12, 13) and sys.argv[1] == 'supervise' and sys.argv[2] in ('docker', 'normal')
            and sys.argv[3] in ('command', 'session', 'background')):
        background = sys.argv[3] == 'background'
        seconds = None if sys.argv[7] == 'background' else int(sys.argv[7])
        token = None
        options = sys.argv[11:]
        if options not in ([], ['proof-stdin'], ['proxy-stdin'], ['proof-stdin', 'proxy-stdin']):
            raise ValueError('invalid supervisor input')
        docker = sys.argv[2] == 'docker'
        proof = 'proof-stdin' in options
        for key in ('HTTP_PROXY', 'HTTPS_PROXY', 'http_proxy', 'https_proxy', 'NODE_USE_ENV_PROXY'):
            os.environ.pop(key, None)
        if docker or proof:
            import re
            if ctypes.CDLL(None, use_errno=True).prctl(_PR_SET_DUMPABLE, 0, 0, 0, 0) != 0:
                raise OSError(ctypes.get_errno(), 'prctl')
            def read_key():
                key = sys.stdin.buffer.readline(66).strip()
                if not re.fullmatch(b'[0-9a-f]{64}', key):
                    raise ValueError('invalid result nonce')
                return key
            token = read_key() if docker and not background else None
            left_auth = read_key() if proof else token
        else:
            left_auth = None
        if 'proxy-stdin' in options:
            proxy = sys.stdin.buffer.readline(2049).decode('ascii').strip()
            if not proxy.startswith('http://aidash:') or len(proxy) > 2048:
                raise ValueError('invalid process proxy')
            for key in ('HTTP_PROXY', 'HTTPS_PROXY', 'http_proxy', 'https_proxy'):
                os.environ[key] = proxy
        if docker or proof or 'proxy-stdin' in options:
            null = os.open('/dev/null', os.O_RDONLY)
            os.dup2(null, 0)
            os.close(null)
        supervise = supervise_foreground if seconds is not None and sys.argv[3] == 'command' else supervise_command
        if background:
            if (seconds is not None or docker) and not proof:
                raise ValueError('Background process requires result proof')
            supervise = supervise_background
        outcome = supervise(sys.argv[4], sys.argv[5], sys.argv[6], sys.argv[9], seconds,
                                    sys.argv[2] == 'docker', expected_pod_uid=sys.argv[8],
                                    record_changes=sys.argv[3] != 'session', output_limit=int(sys.argv[10]),
                                    left_auth=left_auth, capture_tail=background)
        if background:
            import json
            print(json.dumps(outcome, separators=(',', ':')))
            raise SystemExit(0)
        if token is not None:
            import json
            frame = b'\x1eaidash-result:' + token + b':' + json.dumps(outcome, separators=(',', ':')).encode() + b'\n'
            os.write(2, frame)
        raise SystemExit(0 if outcome['status'] in ('completed', 'timed_out', 'stopped') else 1)
    if len(sys.argv) == 4 and sys.argv[1] == 'results':
        import json
        operations = json.load(sys.stdin)
        print(json.dumps(read_background_results(operations, sys.argv[2], int(sys.argv[3])), separators=(',', ':')))
        raise SystemExit(0)
    if len(sys.argv) == 3 and sys.argv[1] == 'claim':
        import json
        print(json.dumps({'created': claim_operation(sys.argv[2])}))
        raise SystemExit(0)
    if len(sys.argv) == 6 and sys.argv[1] == 'output':
        import json
        offset = None if sys.argv[4] == 'tail' else int(sys.argv[4])
        print(json.dumps(operation_output(sys.argv[2], sys.argv[3], offset, int(sys.argv[5]))))
        raise SystemExit(0)
    if len(sys.argv) in (3, 4) and sys.argv[1] == 'stop':
        if len(sys.argv) == 4 and os.environ.get('AIDASH_POD_UID') != sys.argv[3]:
            raise ValueError('Execution Pod identity changed')
        stop_operation(sys.argv[2])
        raise SystemExit(0)
    if len(sys.argv) == 4 and sys.argv[1] == 'changes':
        import json
        print(json.dumps(log_chunk('/scratch-system/changes.jsonl', int(sys.argv[2]), int(sys.argv[3]))))
        raise SystemExit(0)
    if len(sys.argv) == 3 and sys.argv[1] == 'forget':
        forget_operation(sys.argv[2])
        raise SystemExit(0)
    if len(sys.argv) == 3 and sys.argv[1] == 'running':
        import json
        print(json.dumps(operation_state(sys.argv[2])))
        raise SystemExit(0)
    if len(sys.argv) == 3 and sys.argv[1] == 'result':
        import json
        print(json.dumps(read_result(sys.argv[2])))
        raise SystemExit(0)
    if len(sys.argv) in (5, 6) and sys.argv[1] == 'probe' and sys.argv[2] in ('docker', 'normal'):
        import json
        result = run_admission_probe(sys.argv[3], sys.argv[2] == 'docker', sys.argv[4],
                                     sys.argv[5] if len(sys.argv) == 6 else None)
        print(json.dumps(result, separators=(',', ':')))
        raise SystemExit(0)
    if len(sys.argv) >= 6 and sys.argv[1] == 'chroot' and sys.argv[2] in ('docker', 'normal'):
        chroot_command(sys.argv[3], sys.argv[4], sys.argv[5:], sys.argv[2] == 'docker')
    if len(sys.argv) < 3 or sys.argv[1] not in ('docker', 'normal'):
        raise SystemExit('usage: sandbox_v2.py normal|docker executable [args...]')
    drop_user_capabilities(sys.argv[1] == 'docker')
    os.execvp(sys.argv[2], sys.argv[2:])
