"""Trusted, node-local gVisor lifecycle adapter. Never executes submitted code.

Only the dedicated runner may invoke this executable. It requires host runtime
control and is deliberately NOT mounted in or reachable from a sandbox. Inputs
name an observed execution container/pod UUID, never an arbitrary host path.
"""
import base64
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import select
import signal
import stat
import subprocess
import sys
import time
import uuid

ROOT = Path('/var/lib/aidash-node-guard')
RUNTIME = '/run/containerd/runsc/k8s.io'
NAMESPACE = os.environ.get('AIDASH_SANDBOX_NAMESPACE', 'aidash-sandbox')


def command(*args):
    return subprocess.check_output(args, stderr=subprocess.PIPE, timeout=15)


def save(path, data, owner=None):
    temporary = path.with_suffix('.pending')
    with temporary.open('wb') as file:
        os.chmod(file.fileno(), 0o600)
        if owner is not None:
            os.fchown(file.fileno(), owner, owner)
        file.write(json.dumps(data).encode())
        file.flush()
        os.fsync(file.fileno())
    os.replace(temporary, path)
    fd = os.open(path.parent, os.O_DIRECTORY)
    os.fsync(fd)
    os.close(fd)


def validate(request):
    cid = request['container_id']
    pod = str(uuid.UUID(request['pod_uid']))
    area = str(uuid.UUID(request['area_id']))
    if not re.fullmatch('[0-9a-f]{64}', cid) or type(request['epoch']) is not int or request['epoch'] < 1:
        raise ValueError('invalid execution identity')
    info = json.loads(command('ctr', '-n', 'k8s.io', 'containers', 'info', cid))
    labels = info['Labels']
    state = json.loads(command('runsc', '--root=' + RUNTIME, 'state', cid))
    if (info['Runtime']['Name'] != 'io.containerd.runsc.v1'
            or labels.get('io.kubernetes.pod.namespace') != NAMESPACE
            or labels.get('io.kubernetes.pod.uid') != pod
            or labels.get('io.kubernetes.container.name') != 'execution'
            or not labels.get('io.kubernetes.pod.name', '').startswith('operation-')
            or state['annotations'].get('io.kubernetes.cri.image-name') != request['image']):
        raise ValueError('sandbox binding mismatch')
    return cid, pod, area, state


def volume(pod, name):
    if name not in ('work', 'temp', 'control-temp'):
        raise ValueError('invalid volume')
    # The Pod UID was checked against containerd, and every path component is
    # opened without following a symlink. Payload paths never reach this prefix.
    return Path('/var/lib/kubelet/pods') / pod / 'volumes/kubernetes.io~empty-dir' / name


def open_file(root, path):
    parts = path.split('/')
    if len(path) > 1024 or not parts or any(p in ('', '.', '..') for p in parts) or '\\' in path or any(ord(c) < 32 for c in path):
        raise ValueError('invalid file path')
    fd = os.open(root, os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        for part in parts[:-1]:
            child = os.open(part, os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            os.close(fd)
            fd = child
        result = os.open(parts[-1], os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=fd)
        info = os.fstat(result)
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
            os.close(result)
            raise ValueError('unsafe file type')
        return os.fdopen(result, 'rb')
    finally:
        os.close(fd)


def kill_sandbox(state):
    pid = state['pid']
    if not isinstance(pid, int) or pid <= 1:
        raise ValueError('invalid sandbox pid')
    # A pidfd avoids signalling a reused PID. Validate the expected runtime
    # executable after opening the descriptor and before delivering SIGKILL.
    fd = os.pidfd_open(pid)
    try:
        executable = f'/proc/{pid}/exe'
        binaries = ('/usr/local/bin/runsc', '/usr/local/bin/gvisor-bin/gvisor_sentry')
        if not any(os.path.exists(p) and os.path.samefile(executable, p) for p in binaries):
            raise ValueError('sandbox process identity changed')
        signal.pidfd_send_signal(fd, signal.SIGKILL)
        poll = select.poll()
        poll.register(fd, select.POLLIN)
        if not poll.poll(15000):
            raise RuntimeError('sandbox termination unconfirmed')
    finally:
        os.close(fd)


def process_identity(pid):
    # Linux starttime fences PID reuse; the comm field may itself contain ')'.
    fields = (Path('/proc') / str(pid) / 'stat').read_text().rsplit(')', 1)[1].split()
    return fields[19], fields[0]


def gone(record):
    if not record.get('sentry_start') or not record.get('sentry_pid'):
        return False
    try:
        started, status = process_identity(record['sentry_pid'])
        return started != record['sentry_start'] or status == 'Z'
    except FileNotFoundError:
        return True


def binding(request, state, **fields):
    value = {k:v for k,v in request.items() if k != 'code'}
    if state.get('pid', 0) > 1:
        value.update(sentry_pid=state['pid'], sentry_start=process_identity(state['pid'])[0])
    value.update(fields)
    return value


def handle(request):
    # Serialize validation with the watchdog. Its exact-bound pidfd tombstone
    # proves termination even when runsc has already removed its state. Only
    # observation/export can use that proof; it can never resurrect a sandbox.
    area = str(uuid.UUID(request['area_id']))
    ROOT.mkdir(mode=0o700, parents=True, exist_ok=True)
    with (ROOT / (area + '.lock')).open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        path = ROOT / (area + '.json')
        previous = json.loads(path.read_bytes()) if path.exists() else None
        bound = previous and all(
            previous.get(k) == request.get(k) for k in
            ('container_id', 'pod_uid', 'area_id', 'epoch', 'image'))
        terminated = bound and (previous.get('terminated') or gone(previous))
        if terminated:
            if not previous.get('terminated'):
                previous.update(terminated=True, deadline=None, frozen=False)
                save(path, previous)
            if request['action'] in ('terminate', 'status', 'cell_status'):
                return {'termination_confirmed': True, 'status': 'stopped'}
            if request['action'] not in ('export', 'read', 'logs'):
                raise ValueError('sandbox terminated; reset required')
            cid, pod, state = previous['container_id'], str(uuid.UUID(previous['pod_uid'])), {'status': 'stopped'}
        else:
            cid, pod, area, state = validate(request)
        if previous and (request['epoch'] < previous['epoch'] or
                         request['epoch'] == previous['epoch'] and previous['container_id'] != cid):
            raise ValueError('stale execution epoch')
        op = request['action']
        if op == 'limits':
            group = Path('/proc') / str(state['pid']) / 'cgroup'
            hierarchy = next(line[3:] for line in group.read_text().splitlines() if line.startswith('0::'))
            root = Path('/sys/fs/cgroup')
            current = root / hierarchy.lstrip('/')
            # The gVisor Sentry uses the Pod sandbox cgroup, which can retain
            # unlimited swap even when application containers have NoSwap.
            # Set this exact verified Sentry boundary before releasing code.
            (current / 'memory.swap.max').write_text('0')
            if (current / 'memory.swap.max').read_text().strip() != '0':
                raise ValueError('swap must be disabled at the Sentry boundary')
            cpus, memory, processes = [], [], []
            while current.is_relative_to(root):
                cpu = (current / 'cpu.max').read_text().split() if (current / 'cpu.max').exists() else ['max']
                if cpu[0] != 'max':
                    cpus.append(int(cpu[0]) / int(cpu[1]))
                for name, values in [('memory.max', memory), ('pids.max', processes)]:
                    if (current / name).exists():
                        value = (current / name).read_text().strip()
                        if value != 'max':
                            values.append(int(value))
                current = current.parent
            if not cpus or not memory or not processes:
                raise ValueError('cgroup v2 resource ceilings required')
            disk = {}
            for name in ('work', 'temp'):
                fs = os.statvfs(volume(pod, name))
                disk[name] = fs.f_blocks * fs.f_frsize
            result = {'cpu':min(cpus), 'memory_bytes':min(memory), 'processes':min(processes),
                      'working_bytes':disk['work'], 'temporary_bytes':disk['temp']}
            if any(result[k] != request[k] for k in result):
                raise ValueError('observed cgroup/filesystem limits differ from execution profile: ' + str(result))
            save(path, binding(request, state, frozen=False, deadline=None))
            result['swap_bytes'] = 0
            return result
        if op == 'status':
            return {'status': state['status'], 'termination_confirmed':state['status']=='stopped'}
        if op == 'freeze':
            if state['status'] == 'running':
                command('runsc', '--root=' + RUNTIME, 'pause', cid)
            state = json.loads(command('runsc', '--root=' + RUNTIME, 'state', cid))
            if state['status'] not in ('paused', 'stopped'):
                raise RuntimeError('writer freeze unconfirmed')
            save(path, binding(request, state, frozen=True, deadline=time.time()+min(request.get('idle_seconds',1800),1800)))
            return {'writer_frozen': state['status']=='paused', 'termination_confirmed': state['status']=='stopped', 'container_id': cid, 'pod_uid': pod}
        if op == 'terminate':
            if state['status'] in ('running', 'paused'):
                kill_sandbox(state)
            save(path, dict(request, frozen=False, terminated=True, deadline=None))
            return {'termination_confirmed': True}
        if op == 'cell':
            # Refuse admission without an independent deadline enforcer.
            with (ROOT / 'watch.lock').open('a') as watcher:
                try:
                    fcntl.flock(watcher, fcntl.LOCK_EX | fcntl.LOCK_NB)
                except BlockingIOError:
                    pass
                else:
                    raise RuntimeError('node watchdog is unavailable')
            operation = str(uuid.UUID(request['operation_id']))
            if not isinstance(request['code'], str) or len(request['code'].encode()) > 65536:
                raise ValueError('code budget')
            seconds = request['seconds']
            if type(seconds) is not int or not 1 <= seconds <= 600:
                raise ValueError('time budget')
            control = volume(pod, 'control-temp')
            inputs = control / 'cells'
            inputs.mkdir(mode=0o700, exist_ok=True)
            os.chown(inputs, 10000, 10000)
            target = inputs / (operation + '.input')
            cell = {'operation_id': operation, 'code': request['code'], 'digest': request['digest']}
            if target.exists():
                if json.loads(target.read_bytes()) != cell:
                    raise ValueError('cell identity changed')
                # Recovery observes the recorded cell; it must not reset the
                # watchdog deadline or send it for execution a second time.
                return {'accepted': True}
            deadline = time.time() + seconds
            save(path, binding(request, state, frozen=False, deadline=deadline))
            save(target, cell, owner=10000)
            if state['status'] == 'paused':
                command('runsc', '--root=' + RUNTIME, 'resume', cid)
            return {'accepted': True}
        if op == 'cell_status':
            if state['status'] == 'stopped':
                return {'status': 'stopped', 'termination_confirmed': True}
            operation = str(uuid.UUID(request['operation_id']))
            target = volume(pod, 'control-temp') / 'cells' / (operation + '.result')
            if not target.exists():
                return {'status': 'running'}
            if target.stat().st_size > 24 << 20:
                raise ValueError('result budget')
            return json.loads(target.read_bytes())
        if op not in ('export', 'read', 'logs') or state['status'] not in ('paused', 'stopped'):
            raise ValueError('verified frozen writer required')
        if op == 'logs':
            root = volume(pod, 'control-temp')
            try:
                with open_file(root, 'output.bin') as file:
                    data = file.read(8 << 20)
            except FileNotFoundError:
                data = b''
            # Abrupt Sentry loss may drop in-flight output; never claim a
            # complete capture from a collector that could not finish draining.
            return {'stdout':base64.b64encode(data).decode(), 'truncated':True}
        root = volume(pod, 'work')
        if op == 'read':
            offset = request['offset']
            if type(offset) is not int or offset < 0:
                raise ValueError('invalid offset')
            with open_file(root, request['path']) as file:
                file.seek(offset)
                data = file.read(4194304)
            return {'data': base64.b64encode(data).decode()}
        maximum = request['working_bytes']
        if type(maximum) is not int or not 1 <= maximum <= 1 << 30:
            raise ValueError('working quota')
        files, total = [], 0
        for directory, dirs, names in os.walk(root, followlinks=False):
            dirs.sort()
            names.sort()
            for name in dirs:
                if (Path(directory) / name).is_symlink():
                    raise ValueError('symlink directory')
            for name in names:
                relative = str((Path(directory) / name).relative_to(root))
                with open_file(root, relative) as file:
                    size = os.fstat(file.fileno()).st_size
                    total += size
                    if total > maximum or len(files) >= 4096:
                        raise ValueError('working quota')
                    digest = hashlib.file_digest(file, 'sha256').hexdigest()
                files.append({'path': relative, 'object_id': hashlib.sha256(relative.encode()).hexdigest(),
                              'digest': digest, 'size': size})
        return {'files': files}


def watch():
    ROOT.mkdir(mode=0o700, parents=True, exist_ok=True)
    with (ROOT / 'watch.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        while True:
            for path in ROOT.glob('*.json'):
                try:
                    record = json.loads(path.read_bytes())
                    if not record.get('terminated') and (gone(record) or
                            record.get('deadline') is not None and record['deadline'] <= time.time()):
                        handle(dict(record, action='terminate'))
                except Exception as error:
                    print(type(error).__name__ + ': watchdog could not confirm termination', file=sys.stderr, flush=True)
            time.sleep(.2)


if __name__ == '__main__':
    if sys.argv[1:] == ['watch']:
        watch()
    else:
        try:
            data = sys.stdin.buffer.read(131073)
            if len(data) > 131072:
                raise ValueError('control request budget')
            print(json.dumps(handle(json.loads(data))))
        except Exception as error:
            print(json.dumps({'error': str(error)[-500:]}))
            raise SystemExit(1) from None
