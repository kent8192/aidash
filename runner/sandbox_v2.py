"""Execution Pod process privilege reduction, called in a child before exec."""
import ctypes
import os

# Linux capability numbers; Docker requires these, normal commands must never
# regain them even through setuid binaries or file capabilities.
_BLOCKED = (12, 13, 21)  # NET_ADMIN, NET_RAW, SYS_ADMIN
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


def chroot_command(root, cwd, argv, docker_enabled):
    """Enter the persistent system root and permanently reduce user authority."""
    if not isinstance(cwd, str) or not argv:
        raise ValueError('cwd and command are required')
    if cwd.startswith('/') or '..' in cwd.split('/'):
        raise ValueError('cwd must be relative to /scratch')
    os.chroot(root)
    target = os.path.realpath(os.path.join('/scratch', cwd))
    if os.path.commonpath(['/scratch', target]) != '/scratch':
        raise ValueError('cwd must resolve beneath /scratch')
    os.chdir(target)
    drop_user_capabilities(docker_enabled)
    os.execvp(argv[0], argv)


def mount_rootfs(docker_enabled):
    """Bind runtime mounts into the persisted root before starting commands."""
    import stat

    root = '/scratch-system/rootfs'
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


def run_supervised(argv, timeout, stop_requested=None):
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
    process = subprocess.Popen(argv, start_new_session=True)
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
                return {'status': 'completed', 'exit_code': exit_code}
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
            return {'status': 'termination_unconfirmed'}
        if not children:
            return {'status': 'stopped' if stopped else 'timed_out'}
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
                return {'status': 'termination_unconfirmed'}
        for pid in children:
            if pid == process.pid:
                process.poll()
            else:
                try:
                    os.waitpid(pid, os.WNOHANG)
                except ChildProcessError:
                    pass
        time.sleep(0.02)
    return {'status': 'termination_unconfirmed'}


def supervise_command(operation_id, run_id, cwd, code, timeout, docker_enabled, *, expected_pod_uid,
                      record_changes=True):
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
    from execution_pod import append_change_log, helper_command, system_tree_diff, system_tree_index

    state = Path('/scratch-system/operations')
    state.mkdir(mode=0o700, exist_ok=True)
    marker = state / (operation_id + '.started')
    fd = os.open(marker, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)
    directory = os.open(state, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)
    root = '/scratch-system/rootfs'
    stopping = threading.Event()
    signal.signal(signal.SIGUSR1, lambda *_: stopping.set())
    running = state / (operation_id + '.running')
    pending_running = state / (operation_id + '.running.new')
    with pending_running.open('x', encoding='utf-8') as file:
        json.dump({'pid': os.getpid(), 'start_time': process_start_time(os.getpid()), 'pod_uid': pod_uid}, file)
        file.flush()
        os.fsync(file.fileno())
    os.replace(pending_running, running)
    before = system_tree_index(root) if record_changes else None
    argv = helper_command('chroot', 'docker' if docker_enabled else 'normal', root, cwd, '/bin/sh', '-c', code)
    result = run_supervised(argv, timeout, stop_requested=stopping)
    if record_changes:
        changes = system_tree_diff(before, system_tree_index(root))
        append_change_log('/scratch-system/changes.jsonl', run_id, changes)
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


if __name__ == '__main__':
    import sys
    if sys.argv[1:] == ['init']:
        from execution_pod import initialize_system_root
        initialize_system_root('/', '/scratch-system', reset=os.environ.get('AIDASH_RESET_SYSTEM') == 'true')
        raise SystemExit(0)
    if sys.argv[1:] == ['bootstrap']:
        import resource
        import signal
        process_limit = int(os.environ['AIDASH_PROCESS_LIMIT'])
        if process_limit < 32:
            raise ValueError('invalid guest process ceiling')
        resource.setrlimit(resource.RLIMIT_NPROC, (process_limit, process_limit))
        mount_rootfs(os.environ.get('AIDASH_DOCKER_ENABLED') == 'true')
        while True:
            signal.pause()
    if (len(sys.argv) == 10 and sys.argv[1] == 'supervise' and sys.argv[2] in ('docker', 'normal')
            and sys.argv[3] in ('command', 'session')):
        seconds = None if sys.argv[7] == 'background' else int(sys.argv[7])
        outcome = supervise_command(sys.argv[4], sys.argv[5], sys.argv[6], sys.argv[9], seconds,
                                    sys.argv[2] == 'docker', expected_pod_uid=sys.argv[8],
                                    record_changes=sys.argv[3] == 'command')
        raise SystemExit(0 if outcome['status'] in ('completed', 'timed_out', 'stopped') else 1)
    if len(sys.argv) == 3 and sys.argv[1] == 'stop':
        stop_operation(sys.argv[2])
        raise SystemExit(0)
    if len(sys.argv) == 4 and sys.argv[1] == 'changes':
        import json
        print(json.dumps(log_chunk('/scratch-system/changes.jsonl', int(sys.argv[2]), int(sys.argv[3]))))
        raise SystemExit(0)
    if len(sys.argv) == 3 and sys.argv[1] == 'result':
        import json
        print(json.dumps(read_result(sys.argv[2])))
        raise SystemExit(0)
    if len(sys.argv) >= 6 and sys.argv[1] == 'chroot' and sys.argv[2] in ('docker', 'normal'):
        chroot_command(sys.argv[3], sys.argv[4], sys.argv[5:], sys.argv[2] == 'docker')
    if len(sys.argv) < 3 or sys.argv[1] not in ('docker', 'normal'):
        raise SystemExit('usage: sandbox_v2.py normal|docker executable [args...]')
    drop_user_capabilities(sys.argv[1] == 'docker')
    os.execvp(sys.argv[2], sys.argv[2:])
