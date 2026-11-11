"""Execution Pod process privilege reduction, called in a child before exec."""
import ctypes
import os

# Linux capability numbers; Docker requires these, normal commands must never
# regain them even through setuid binaries or file capabilities.
_BLOCKED = (12, 13, 18, 21)  # NET_ADMIN, NET_RAW, SYS_CHROOT, SYS_ADMIN
_PR_SET_DUMPABLE = 4
_PR_CAPBSET_DROP = 24
_PR_SET_NO_NEW_PRIVS = 38
_PR_CAP_AMBIENT = 47
_PR_CAP_AMBIENT_CLEAR_ALL = 4
_CAP_VERSION_3 = 0x20080522


class _Header(ctypes.Structure):
    _fields_ = [('version', ctypes.c_uint32), ('pid', ctypes.c_int)]


class _Data(ctypes.Structure):
    _fields_ = [('effective', ctypes.c_uint32), ('permitted', ctypes.c_uint32),
                ('inheritable', ctypes.c_uint32)]


def drop_user_capabilities(docker_enabled):
    """Permanently reduce this process only; call after forking bootstrap."""
    if type(docker_enabled) is not bool:
        raise ValueError('docker_enabled must be a boolean')
    libc = ctypes.CDLL(None, use_errno=True)

    def check(result):
        if result != 0:
            code = ctypes.get_errno()
            raise OSError(code, os.strerror(code))

    if not docker_enabled:
        # Removing a capability from the bounding set requires CAP_SETPCAP,
        # which must still be effective while this loop executes.
        for capability in _BLOCKED:
            check(libc.prctl(_PR_CAPBSET_DROP, capability, 0, 0, 0))
        header, data = _Header(_CAP_VERSION_3, 0), (_Data * 2)()
        check(libc.capget(ctypes.byref(header), ctypes.byref(data)))
        for capability in _BLOCKED:
            item, bit = data[capability // 32], 1 << (capability % 32)
            item.effective &= ~bit
            item.permitted &= ~bit
            item.inheritable &= ~bit
        check(libc.capset(ctypes.byref(header), ctypes.byref(data)))
        check(libc.prctl(_PR_CAP_AMBIENT, _PR_CAP_AMBIENT_CLEAR_ALL, 0, 0, 0))
    check(libc.prctl(_PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0))


def validate_system_root(root):
    """Reject attacker-controlled links in the privileged chroot path."""
    import stat
    root = os.path.abspath(root)
    if root == '/' or os.path.realpath(root) == '/':
        raise ValueError('system root cannot be the container root')
    for path in (os.path.dirname(root), root):
        if not stat.S_ISDIR(os.lstat(path).st_mode):
            raise ValueError('system root must be a directory without symlinks')


def enter_user_root(root, cwd, docker_enabled):
    if not isinstance(cwd, str):
        raise ValueError('cwd is required')
    if cwd.startswith('/') or '..' in cwd.split('/'):
        raise ValueError('cwd must be relative to /scratch')
    validate_system_root(root)
    import resource
    process_limit = int(os.environ['AIDASH_PROCESS_LIMIT'])
    if process_limit < 32:
        raise ValueError('invalid guest process ceiling')
    resource.setrlimit(resource.RLIMIT_NPROC, (process_limit, process_limit))
    os.chroot(root)
    target = os.path.realpath(os.path.join('/scratch', cwd))
    if os.path.commonpath(['/scratch', target]) != '/scratch':
        raise ValueError('cwd must resolve beneath /scratch')
    os.chdir(target)
    drop_user_capabilities(docker_enabled)


def save_image_environment(store, environ):
    """Capture image-provided environment outside the guest filesystem."""
    import json
    import uuid
    from pathlib import Path
    store = Path(store)
    values = {key: value for key, value in environ.items()
              if not key.startswith(('AIDASH_', 'KUBERNETES_')) and key != 'HOSTNAME'}
    data = json.dumps(values, separators=(',', ':')).encode()
    if len(data) > 131072:
        raise ValueError('image environment exceeds the limit')
    temporary = store / ('environment.' + uuid.uuid4().hex + '.new')
    try:
        fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, 'wb') as file:
            file.write(data)
            file.flush()
            os.fsync(file.fileno())
        os.replace(temporary, store / 'environment.json')
    finally:
        temporary.unlink(missing_ok=True)


def image_environment(root):
    import json
    fd = os.open(os.path.join(os.path.dirname(root), 'environment.json'), os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd, 'rb') as file:
        data = file.read(131073)
    if len(data) > 131072:
        raise ValueError('image environment exceeds the limit')
    values = json.loads(data)
    if not isinstance(values, dict) or any(not isinstance(k, str) or not isinstance(v, str)
            or not k or '=' in k or '\x00' in k or '\x00' in v for k, v in values.items()):
        raise ValueError('invalid image environment')
    environment = {key: value for key, value in values.items()
                   if not key.startswith(('AIDASH_', 'KUBERNETES_'))}
    for key in ('HTTP_PROXY', 'HTTPS_PROXY', 'http_proxy', 'https_proxy', 'NO_PROXY', 'no_proxy'):
        if key in os.environ:
            environment[key] = os.environ[key]
    return environment


def chroot_command(root, cwd, argv, docker_enabled):
    """Enter the persistent system root and permanently reduce user authority."""
    if not argv:
        raise ValueError('command is required')
    validate_system_root(root)
    environment = image_environment(root)
    enter_user_root(root, cwd, docker_enabled)
    os.execvpe(argv[0], argv, environment)


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


def mount_rootfs(docker_enabled):
    """Bind runtime mounts into the persisted root before starting commands."""
    import stat

    root = '/scratch-system/image-root/rootfs'
    validate_system_root(root)
    libc = ctypes.CDLL(None, use_errno=True)
    runtime = root
    for component in ('run', 'aidash'):
        runtime = os.path.join(runtime, component)
        try:
            os.mkdir(runtime, 0o700)
        except FileExistsError:
            pass
        if not stat.S_ISDIR(os.lstat(runtime).st_mode):
            raise ValueError('Python session directory must not be a symlink')
    mounts = [('/proc', 'proc'), ('/dev', 'dev'), ('/tmp', 'tmp'),
              ('/scratch-system/scratch', 'scratch'), ('/.aidash-helpers', '.aidash-helpers')]
    if docker_enabled:
        mounts.append(('/docker-storage', 'var/lib/docker'))
    for source, relative in mounts:
        target = root
        for component in relative.split('/'):
            target = os.path.join(target, component)
            try:
                os.mkdir(target, 0o755)
            except FileExistsError:
                pass
            if not stat.S_ISDIR(os.lstat(target).st_mode):
                raise ValueError('mount target must be a directory: ' + target)
        # /dev contains the nested devpts mount; a plain bind omits it.
        flags = 4096 | (16384 if source in ('/dev', '/proc') else 0)  # MS_BIND | MS_REC
        if libc.mount(os.fsencode(source), os.fsencode(target), None, flags, None) != 0:
            code = ctypes.get_errno()
            raise OSError(code, os.strerror(code), target)


FORWARD_GRACE_SECONDS = 5


def run_supervised(argv, timeout, stop_requested=None, *, output_directory=None, operation_id=None, output_limit=8388608):
    """Run one command in a dedicated supervisor and reap escaped descendants.

    This must be called in a single-purpose process: becoming a subreaper also
    adopts orphan descendants. A failure to prove termination is reported to
    the controller, which must stop the Pod rather than replay the command.
    """
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
    output_lock = threading.Lock()
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
    process = subprocess.Popen(argv, start_new_session=True,
                               stdout=subprocess.PIPE if output_directory is not None else None,
                               stderr=subprocess.PIPE if output_directory is not None else None)
    def drain(name, source, target):
        forwarding = True
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
    if output_directory is not None:
        for name, source, target in (('stdout', process.stdout, 1), ('stderr', process.stderr, 2)):
            reader = threading.Thread(target=drain, args=(name, source, target), daemon=True)
            reader.start()
            readers.append(reader)
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
        return result
    expires = None if timeout is None else time.monotonic() + timeout
    while expires is None or time.monotonic() < expires:
        if stop_requested is not None and stop_requested.is_set():
            break
        exit_code = process.poll()
        if exit_code is not None:
            # Orphaned descendants are adopted by this supervisor, even after
            # the shell has exited. Keep them tracked until they have stopped.
            try:
                while os.waitpid(-1, os.WNOHANG)[0] > 0:
                    pass
            except ChildProcessError:
                return finish({'status': 'completed', 'exit_code': exit_code})
        time.sleep(0.02)

    stopped = stop_requested is not None and stop_requested.is_set()
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
            return finish({'status': 'stopped' if stopped else 'timed_out'})
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
                      record_changes=True, output_limit=8388608):
    """Execute once, persist its outcome, and record its observed system changes."""
    import json
    from pathlib import Path
    import signal
    import sys
    import threading
    import uuid

    if str(uuid.UUID(operation_id)) != operation_id:
        raise ValueError('canonical operation UUID required')
    if not isinstance(run_id, str) or not run_id:
        raise ValueError('run_id must be a nonempty string')
    if not isinstance(code, str) or len(code.encode('utf-8')) > 131072:
        raise ValueError('command exceeds the code limit')
    if timeout is not None and (type(timeout) is not int or not 1 <= timeout <= 3600):
        raise ValueError('invalid command timeout')
    pod_uid = os.environ.get('AIDASH_POD_UID')
    if not pod_uid or pod_uid != expected_pod_uid:
        raise ValueError('Execution Pod identity changed')
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from execution_pod import change_interval, helper_command

    state = Path(OPERATION_DIRECTORY)
    state.mkdir(mode=0o700, exist_ok=True)
    if not claim_operation(operation_id, state):
        raise FileExistsError('operation already started')
    root = '/scratch-system/image-root/rootfs'
    stopping = threading.Event()
    signal.signal(signal.SIGUSR1, lambda *_: stopping.set())
    running = state / (operation_id + '.running')
    pending_running = state / (operation_id + '.running.new')
    with pending_running.open('x', encoding='utf-8') as file:
        json.dump({'pid': os.getpid(), 'start_time': process_start_time(os.getpid()), 'pod_uid': pod_uid}, file)
        file.flush()
        os.fsync(file.fileno())
    os.replace(pending_running, running)
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
    argv = helper_command('chroot', 'docker' if docker_enabled else 'normal', root, cwd, '/bin/sh', '-c', code)
    try:
        result = run_supervised(argv, timeout, stop_requested=stopping,
                                output_directory=OUTPUT_DIRECTORY,
                                operation_id=operation_id, output_limit=output_limit)
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
    result.update(operation_id=operation_id, run_id=run_id)
    temporary = state / (operation_id + '.new')
    with temporary.open('x', encoding='utf-8') as file:
        json.dump(result, file, separators=(',', ':'))
        file.flush()
        os.fsync(file.fileno())
    os.replace(temporary, state / (operation_id + '.json'))
    running.unlink(missing_ok=True)
    directory = os.open(state, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)
    return result


def process_start_time(pid):
    with open(f'/proc/{pid}/stat', encoding='ascii') as file:
        # comm is parenthesized and may itself contain spaces or parentheses.
        fields = file.read().rsplit(')', 1)[1].split()
    return int(fields[19])


def stop_operation(operation_id):
    import json
    import signal
    import uuid
    from pathlib import Path

    if str(uuid.UUID(operation_id)) != operation_id:
        raise ValueError('canonical operation UUID required')
    path = Path('/scratch-system/operations') / (operation_id + '.running')
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
    if type(offset) is not int or offset < 0 or type(limit) is not int or not 1 <= limit <= 262144:
        raise ValueError('invalid log offset or limit')
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    except FileNotFoundError:
        return {'data': '', 'next_offset': offset, 'eof': True}
    with os.fdopen(fd, 'rb') as file:
        fcntl.flock(file, fcntl.LOCK_SH)
        file.seek(offset)
        data = file.read(limit)
        eof = file.tell() >= os.fstat(file.fileno()).st_size
    return {'data': base64.b64encode(data).decode('ascii'), 'next_offset': offset + len(data), 'eof': eof}


def claim_operation(operation_id, directory='/scratch-system/operations'):
    import uuid
    from pathlib import Path
    if str(uuid.UUID(operation_id)) != operation_id:
        raise ValueError('canonical operation UUID required')
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
    import uuid
    from pathlib import Path
    if str(uuid.UUID(operation_id)) != operation_id or stream not in ('stdout', 'stderr'):
        raise ValueError('invalid operation output')
    path = Path(directory) / (operation_id + '.' + stream)
    if not path.exists():
        raise FileNotFoundError('operation output unavailable')
    return log_chunk(path, offset, limit)


def forget_operation(operation_id, directory='/scratch-system/operations', output_directory='/scratch-system/operation-output'):
    import uuid
    from pathlib import Path
    if str(uuid.UUID(operation_id)) != operation_id:
        raise ValueError('canonical operation UUID required')
    for suffix in ('.started', '.running', '.running.new', '.new', '.json'):
        (Path(directory) / (operation_id + suffix)).unlink(missing_ok=True)
    for suffix in ('.stdout', '.stderr'):
        (Path(output_directory) / (operation_id + suffix)).unlink(missing_ok=True)


def operation_running(operation_id):
    import json
    import uuid
    from pathlib import Path
    if str(uuid.UUID(operation_id)) != operation_id:
        raise ValueError('canonical operation UUID required')
    path = Path('/scratch-system/operations') / (operation_id + '.running')
    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
        with os.fdopen(fd, 'rb') as file:
            record = json.load(file)
        if record.get('pod_uid') != os.environ.get('AIDASH_POD_UID'):
            return False
        return process_start_time(record['pid']) == record['start_time']
    except (FileNotFoundError, ProcessLookupError):
        return False


def read_result(operation_id):
    import json
    import uuid
    from pathlib import Path
    if str(uuid.UUID(operation_id)) != operation_id:
        raise ValueError('canonical operation UUID required')
    path = Path('/scratch-system/operations') / (operation_id + '.json')
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd, 'rb') as file:
        data = file.read(65537)
    if len(data) > 65536:
        raise ValueError('command result exceeds the limit')
    return json.loads(data)


OPERATION_DIRECTORY = '/scratch-system/operations'
OUTPUT_DIRECTORY = '/scratch-system/operation-output'
BOOTSTRAP_READY_PATH = '/scratch-system/operations/.bootstrap-ready'
BOOTSTRAP_TRACKER_PATH = '/scratch-system/change-tracker.json'


def bootstrap_ready():
    uid = os.environ.get('AIDASH_POD_UID', '')
    if not uid or len(uid.encode('utf-8')) > 256:
        return False
    try:
        fd = os.open(BOOTSTRAP_READY_PATH, os.O_RDONLY | os.O_NOFOLLOW)
        with os.fdopen(fd, 'rb') as file:
            return file.read(257) == uid.encode('utf-8')
    except OSError:
        return False


def bootstrap():
    import resource
    import signal
    from pathlib import Path
    uid = os.environ.get('AIDASH_POD_UID', '')
    if not uid or len(uid.encode('utf-8')) > 256:
        raise ValueError('invalid Execution Pod UID')
    Path(BOOTSTRAP_READY_PATH).unlink(missing_ok=True)
    process_limit = int(os.environ['AIDASH_PROCESS_LIMIT'])
    if process_limit < 32:
        raise ValueError('invalid guest process ceiling')
    resource.setrlimit(resource.RLIMIT_NPROC, (process_limit, process_limit))
    import shutil
    output = Path(OUTPUT_DIRECTORY)
    try:
        if output.is_symlink():
            output.unlink()
        elif output.exists():
            shutil.rmtree(output)
        output.mkdir(mode=0o700)
    except OSError:
        pass
    mount_rootfs(os.environ.get('AIDASH_DOCKER_ENABLED') == 'true')
    Path(BOOTSTRAP_TRACKER_PATH).unlink(missing_ok=True)
    fd = os.open(BOOTSTRAP_READY_PATH, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'wb') as file:
        file.write(uid.encode('utf-8'))
    while True:
        signal.pause()


if __name__ == '__main__':
    import sys
    if sys.argv[1:] == ['init']:
        from pathlib import Path
        sys.path.insert(0, str(Path(__file__).resolve().parent))
        from execution_pod import initialize_system_root
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
    if (len(sys.argv) == 11 and sys.argv[1] == 'supervise' and sys.argv[2] in ('docker', 'normal')
            and sys.argv[3] in ('command', 'session')):
        seconds = None if sys.argv[7] == 'background' else int(sys.argv[7])
        token = None
        if sys.argv[2] == 'docker':
            import re
            token = sys.stdin.buffer.readline(66).strip()
            if not re.fullmatch(b'[0-9a-f]{64}', token):
                raise ValueError('invalid result nonce')
            null = os.open('/dev/null', os.O_RDONLY)
            os.dup2(null, 0)
            os.close(null)
        outcome = supervise_command(sys.argv[4], sys.argv[5], sys.argv[6], sys.argv[9], seconds,
                                    sys.argv[2] == 'docker', expected_pod_uid=sys.argv[8],
                                    record_changes=sys.argv[3] == 'command', output_limit=int(sys.argv[10]))
        if token is not None:
            import json
            frame = b'\x1eaidash-result:' + token + b':' + json.dumps(outcome, separators=(',', ':')).encode() + b'\n'
            os.write(2, frame)
        raise SystemExit(0 if outcome['status'] in ('completed', 'timed_out', 'stopped') else 1)
    if len(sys.argv) == 3 and sys.argv[1] == 'claim':
        import json
        print(json.dumps({'created': claim_operation(sys.argv[2])}))
        raise SystemExit(0)
    if len(sys.argv) == 6 and sys.argv[1] == 'output':
        import json
        print(json.dumps(operation_output(sys.argv[2], sys.argv[3], int(sys.argv[4]), int(sys.argv[5]))))
        raise SystemExit(0)
    if len(sys.argv) == 3 and sys.argv[1] == 'stop':
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
        print(json.dumps({'running': operation_running(sys.argv[2])}))
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
