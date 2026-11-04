"""Scheduling primitives for commands sharing an Execution Pod."""
from concurrent.futures import ThreadPoolExecutor
from threading import Lock, RLock


def helper_command(*args):
    """Use the injected read-only loader, interpreter, and helper script."""
    return ['/.aidash-helpers/loader', '--library-path',
            '/.aidash-helpers/lib:/.aidash-helpers/python/lib',
            '/.aidash-helpers/python/bin/python3', '-I', '/.aidash-helpers/sandbox_v2.py', *args]


def python_helper_command(*args):
    return ['/.aidash-helpers/loader', '--library-path',
            '/.aidash-helpers/lib:/.aidash-helpers/python/lib',
            '/.aidash-helpers/python/bin/python3', '-I', '/.aidash-helpers/python_session.py', *args]


class CommandQueue:
    """Bound active commands and allow a Run's queued foreground work to stop."""

    def __init__(self):
        self._executor = ThreadPoolExecutor(max_workers=8, thread_name_prefix='execution-pod')
        self._background = ThreadPoolExecutor(max_workers=16, thread_name_prefix='execution-background')
        self._sessions = ThreadPoolExecutor(max_workers=8, thread_name_prefix='python-session')
        self._lock = Lock()
        self._waiting = {}

    def submit(self, run_id, command, execute, *, background=False):
        if not isinstance(run_id, str) or not run_id:
            raise ValueError('run_id must be a nonempty string')
        with self._lock:
            future = (self._background if background else self._executor).submit(execute, command)
            if not background:
                self._waiting.setdefault(run_id, set()).add(future)
        if not background:
            future.add_done_callback(lambda done: self._forget(run_id, done))
        return future

    def _forget(self, run_id, future):
        with self._lock:
            pending = self._waiting.get(run_id)
            if pending is not None:
                pending.discard(future)
                if not pending:
                    del self._waiting[run_id]

    def cancel_waiting(self, run_id):
        with self._lock:
            pending = tuple(self._waiting.get(run_id, ()))
        for future in pending:
            future.cancel()

    def submit_session(self, operation_id, execute):
        return self._sessions.submit(execute, operation_id)

    def close(self, *, wait=True):
        self._executor.shutdown(wait=wait, cancel_futures=True)
        self._background.shutdown(wait=wait, cancel_futures=True)
        self._sessions.shutdown(wait=wait, cancel_futures=True)


def system_tree_index(root):
    """Index the image tree without entering runtime mounts or following links."""
    import os
    from pathlib import Path
    import stat

    root = Path(root)
    index = {}
    excluded = {'scratch', 'proc', 'dev', 'tmp', 'sys', '.aidash-helpers'}
    for directory, dirs, files, fd in os.fwalk(root, follow_symlinks=False):
        relative = Path(directory).relative_to(root)
        if relative == Path('.'):
            dirs[:] = [name for name in dirs if name not in excluded]
            files = [name for name in files if name not in excluded]
        if relative == Path('var/lib'):
            dirs[:] = [name for name in dirs if name != 'docker']
            files = [name for name in files if name != 'docker']
        for name in sorted(dirs + files):
            try:
                entry = os.stat(name, dir_fd=fd, follow_symlinks=False)
                target = os.readlink(name, dir_fd=fd) if stat.S_ISLNK(entry.st_mode) else None
            except FileNotFoundError:
                # A concurrent command may remove a path while it is indexed.
                continue
            path = (relative / name).as_posix()
            index[path] = (entry.st_mode, entry.st_size, entry.st_mtime_ns, target)
    return index


def system_tree_diff(before, after):
    """Return deterministic path changes between two indexes."""
    old, new = set(before), set(after)
    return {'added': sorted(new - old),
            'modified': sorted(path for path in old & new if before[path] != after[path]),
            'removed': sorted(old - new)}


def initialize_system_root(image_root, store, *, reset=False):
    """Copy image contents once, or replace them after the caller stops the Pod.

    The caller serializes startup and reset and must stop all user processes
    before reset. The scratch data directory is never touched.
    """
    import os
    from pathlib import Path
    import shutil
    import uuid

    image_root, store = Path(image_root), Path(store)
    store.mkdir(mode=0o700, parents=True, exist_ok=True)
    root, backup = store / 'rootfs', store / 'rootfs.previous'
    for stale in store.glob('rootfs.*.new'):
        if stale.is_symlink() or not stale.is_dir():
            stale.unlink()
        else:
            shutil.rmtree(stale)
    if root.is_symlink() or backup.is_symlink():
        raise ValueError('system root must not be a symlink')
    if not root.exists() and backup.exists():
        os.replace(backup, root)
    if root.exists() and not reset:
        return False
    if reset:
        if root.exists():
            shutil.rmtree(root)
        if backup.exists():
            shutil.rmtree(backup)
    staging = store / ('rootfs.' + uuid.uuid4().hex + '.new')
    excluded = {'scratch', 'scratch-system', 'scratch-image', 'proc', 'sys', 'dev', 'tmp', '.aidash-helpers'}
    copied_links = {}

    def copy_image_file(source, target):
        info = os.stat(source, follow_symlinks=False)
        identity = (info.st_dev, info.st_ino)
        if info.st_nlink > 1 and identity in copied_links:
            os.link(copied_links[identity], target)
            return target
        result = shutil.copy2(source, target, follow_symlinks=False)
        if info.st_nlink > 1:
            copied_links[identity] = target
        return result

    try:
        shutil.copytree(image_root, staging, symlinks=True,
                        copy_function=copy_image_file,
                        ignore=lambda directory, names: excluded.intersection(names)
                        if Path(directory) == image_root else set())
        # copytree preserves modes and timestamps, but does not preserve owner.
        # Apply ownership without following image symlinks, then restore mode
        # bits that chown can clear (notably setuid/setgid).
        for directory, dirs, files in os.walk(staging, followlinks=False):
            for target in [Path(directory)] + [Path(directory) / name for name in dirs + files]:
                source = image_root / target.relative_to(staging)
                info = source.lstat()
                os.chown(target, info.st_uid, info.st_gid, follow_symlinks=False)
                shutil.copystat(source, target, follow_symlinks=False)
        for name in ('scratch', 'proc', 'sys', 'dev', 'tmp'):
            (staging / name).mkdir(exist_ok=True)
        if backup.exists():
            shutil.rmtree(backup)
        if root.exists():
            os.replace(root, backup)
        try:
            os.replace(staging, root)
        except OSError:
            if backup.exists() and not root.exists():
                os.replace(backup, root)
            raise
        if backup.exists():
            shutil.rmtree(backup)
    finally:
        if staging.exists():
            shutil.rmtree(staging)
    return True


def append_change_log(path, run_id, changes):
    """Append one Run's observed system-tree changes to the persistent log."""
    import fcntl
    import json
    import os
    import time

    if not isinstance(run_id, str) or not run_id:
        raise ValueError('run_id must be a nonempty string')
    if set(changes) != {'added', 'modified', 'removed'} or any(
            not isinstance(paths, list) or any(not isinstance(path, str) for path in paths)
            for paths in changes.values()):
        raise ValueError('invalid change list')
    record = {'run_id': run_id, 'changes': changes, 'recorded_at': time.time()}
    fd = os.open(path, os.O_WRONLY | os.O_APPEND | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'a', encoding='utf-8') as file:
        fcntl.flock(file, fcntl.LOCK_EX)
        file.write(json.dumps(record, separators=(',', ':')) + '\n')
        file.flush()
        os.fsync(file.fileno())


def change_interval(root, state_dir, operation_id=None, run_id=None, *, finished=False):
    """Attribute changes between active-set transitions under one shared lock."""
    import fcntl
    import json
    import os
    import time
    from pathlib import Path

    directory = Path(state_dir)
    directory.mkdir(parents=True, exist_ok=True)
    with open(directory / 'changes.lock', 'a+b') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        state_path = directory / 'change-tracker.json'
        try:
            state = json.loads(state_path.read_text())
        except FileNotFoundError:
            state = {'active': {}, 'index': None}
        current = {key: list(value) for key, value in system_tree_index(root).items()}
        if state['index'] is not None and state['active']:
            diff = system_tree_diff(state['index'], current)
            if any(diff.values()):
                active = list(state['active'].values())
                record = {'changes': diff, 'recorded_at': time.time()}
                if len(active) == 1:
                    record['run_id'] = active[0]
                else:
                    record.update(attribution='ambiguous', run_ids=sorted(set(active)))
                fd = os.open(directory / 'changes.jsonl', os.O_WRONLY | os.O_APPEND | os.O_CREAT | os.O_NOFOLLOW, 0o600)
                with os.fdopen(fd, 'a', encoding='utf-8') as out:
                    out.write(json.dumps(record, separators=(',', ':')) + '\n')
                    out.flush()
                    os.fsync(out.fileno())
        if operation_id:
            if finished:
                state['active'].pop(operation_id, None)
            else:
                state['active'][operation_id] = run_id
        state['index'] = current
        temporary = directory / 'change-tracker.tmp'
        temporary.write_text(json.dumps(state, separators=(',', ':')))
        os.replace(temporary, state_path)


def read_change_log(path):
    """Read complete records from a persistent change log."""
    import fcntl
    import json
    import os

    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    except FileNotFoundError:
        return []
    with os.fdopen(fd, encoding='utf-8') as file:
        fcntl.flock(file, fcntl.LOCK_SH)
        return [json.loads(line) for line in file]


def execution_manifests(settings, config):
    """Build the durable claim and sandbox Pod for one Scratch Space."""
    import hashlib
    import re
    from control import scratch_resource_name

    name = scratch_resource_name(settings['scratch_space_id'])
    for image in (settings['image'], config['helper_image']):
        if not isinstance(image, str) or not re.search(r'@sha256:[0-9a-f]{64}$', image):
            raise ValueError('execution and helper images must be digest pinned')
    for key in ('cpu', 'memory_bytes', 'capacity_bytes'):
        if type(settings[key]) is not int or settings[key] <= 0:
            raise ValueError(key + ' must be a positive integer')
    for key, ceiling in (('cpu', config.get('max_cpu', 256)),
                         ('memory_bytes', config.get('max_memory_bytes', 1 << 40)),
                         ('capacity_bytes', min(config.get('max_capacity_bytes', 1 << 40), 1 << 40))):
        if type(ceiling) is not int or ceiling <= 0 or settings[key] > ceiling:
            raise ValueError(key + ' exceeds the Deployment ceiling')
    processes = config.get('v2_processes', 512)
    if type(processes) is not int or processes < 32:
        raise ValueError('processes must be an integer of at least 32')
    docker = settings['docker_enabled']
    if type(docker) is not bool:
        raise ValueError('docker_enabled must be a boolean')
    if docker and not config.get('allow_docker', False):
        raise ValueError('Docker execution is not permitted by the Deployment')
    if docker and (not config.get('docker_runtime_class') or
                   config['docker_runtime_class'] == config['runtime_class']):
        raise ValueError('Docker requires a separate raw-socket RuntimeClass')
    metadata = {'name': name, 'namespace': config['namespace']}
    claim = {'apiVersion': 'v1', 'kind': 'PersistentVolumeClaim', 'metadata': dict(metadata),
             'spec': {'accessModes': ['ReadWriteOnce'],
                      'resources': {'requests': {'storage': str(settings['capacity_bytes'])}}}}
    if config.get('storage_class'):
        claim['spec']['storageClassName'] = config['storage_class']
    capabilities = ['SYS_ADMIN', 'SYS_CHROOT', 'SETPCAP', 'CHOWN', 'DAC_OVERRIDE',
                    'FOWNER', 'FSETID', 'SETGID', 'SETUID', 'MKNOD', 'SETFCAP', 'KILL']
    if docker:
        capabilities += ['NET_ADMIN', 'NET_RAW']
    security = {'runAsUser': 0, 'allowPrivilegeEscalation': False,
                'capabilities': {'drop': ['ALL'], 'add': capabilities},
                'seccompProfile': {'type': 'RuntimeDefault'}}
    image_security = {'runAsUser': 0, 'allowPrivilegeEscalation': False,
                     'capabilities': {'drop': ['ALL'],
                                      'add': ['CHOWN', 'DAC_OVERRIDE', 'FOWNER', 'FSETID']},
                     'seccompProfile': {'type': 'RuntimeDefault'}}
    init_security = {'runAsUser': 0, 'allowPrivilegeEscalation': False,
                     'capabilities': {'drop': ['ALL']},
                     'seccompProfile': {'type': 'RuntimeDefault'}}
    volumes = [{'name': 'scratch', 'persistentVolumeClaim': {'claimName': name}},
               {'name': 'helpers', 'emptyDir': {}},
               {'name': 'operations', 'emptyDir': {'medium': 'Memory', 'sizeLimit': str(64 << 20)}},
               {'name': 'tmp', 'emptyDir': {'medium': 'Memory',
                                            'sizeLimit': str(min(settings['memory_bytes'], 1 << 30))}}]
    mounts = [{'name': 'scratch', 'mountPath': '/scratch-system'},
              {'name': 'helpers', 'mountPath': '/.aidash-helpers', 'readOnly': True},
              {'name': 'operations', 'mountPath': '/scratch-system/operations'},
              {'name': 'tmp', 'mountPath': '/tmp'}]
    if docker:
        volumes.append({'name': 'docker', 'emptyDir': {'medium': 'Memory'}})
        mounts.append({'name': 'docker', 'mountPath': '/docker-storage'})
    limits = {'cpu': str(settings['cpu']), 'memory': str(settings['memory_bytes']),
              'ephemeral-storage': str(1 << 30)}
    podspec = {'runtimeClassName': config['docker_runtime_class'] if docker else config['runtime_class'],
               'restartPolicy': 'Never',
               'automountServiceAccountToken': False, 'enableServiceLinks': False,
               'hostNetwork': False, 'hostPID': False, 'hostIPC': False,
               'resources': {'limits': {key: limits[key] for key in ('cpu', 'memory')}},
               'initContainers': [
                   {'name': 'helpers', 'image': config['helper_image'],
                    'command': ['python3', '-I', '/opt/aidash/prepare_helpers.py'],
                    'env': [{'name': 'AIDASH_PROCESS_LIMIT', 'value': str(processes)}],
                    'securityContext': init_security, 'resources': {'limits': limits},
                    'volumeMounts': [{'name': 'helpers', 'mountPath': '/helpers'},
                                     {'name': 'scratch', 'mountPath': '/scratch-system'}]},
                   {'name': 'image-root', 'image': settings['image'],
                    'command': ['/.aidash-helpers/loader', '--library-path',
                                '/.aidash-helpers/lib:/.aidash-helpers/python/lib',
                                '/.aidash-helpers/python/bin/python3', '-I',
                                '/.aidash-helpers/sandbox_v2.py', 'init'],
                    'env': [{'name': key, 'value': ''} for key in ('LD_PRELOAD', 'LD_AUDIT', 'LD_LIBRARY_PATH', 'PYTHONPATH', 'PYTHONHOME')] +
                           [{'name': 'AIDASH_RESET_SYSTEM', 'value': str(bool(settings.get('_reset_system'))).lower()}],
                    'securityContext': image_security, 'resources': {'limits': limits},
                    'volumeMounts': [{'name': 'scratch', 'mountPath': '/scratch-image', 'subPath': 'image-root'},
                                     {'name': 'helpers', 'mountPath': '/.aidash-helpers', 'readOnly': True}]},
               ],
               'containers': [{'name': 'execution', 'image': config['helper_image'],
                               'command': helper_command('bootstrap'),
                               'env': [{'name': 'AIDASH_DOCKER_ENABLED', 'value': str(docker).lower()},
                                       {'name': 'AIDASH_PROCESS_LIMIT', 'value': str(processes)},
                                       {'name': 'AIDASH_POD_UID',
                                        'valueFrom': {'fieldRef': {'fieldPath': 'metadata.uid'}}}],
                               'securityContext': security, 'resources': {'limits': limits},
                               'volumeMounts': mounts}], 'volumes': volumes}
    if docker:
        tenant = settings.get('tenant_id')
        if not isinstance(tenant, str) or not tenant:
            raise ValueError('tenant_id required for Docker execution')
        tenant_label = 'tenant-' + hashlib.sha256(tenant.encode()).hexdigest()[:48]
        podspec['nodeSelector'] = {'aidash.run/tenant': tenant_label}
        podspec['tolerations'] = [{'key': 'aidash.run/tenant', 'operator': 'Equal',
                                   'value': tenant_label, 'effect': 'NoSchedule'}]
    pod = {'apiVersion': 'v1', 'kind': 'Pod', 'metadata': dict(metadata,
           labels={'aidash-sandbox': 'true', 'aidash-v2': 'true'}), 'spec': podspec}
    if config.get('proxy_endpoint'):
        v2_network_policy(config)  # validate before passing the proxy to user processes
        proxy = config['proxy_endpoint']
        podspec['containers'][0]['env'] += [
            {'name': key, 'value': proxy} for key in ('HTTP_PROXY', 'HTTPS_PROXY', 'http_proxy', 'https_proxy')]
    return claim, pod


def quantity_bytes(value):
    """Parse a Kubernetes storage quantity without losing precision."""
    import re
    from decimal import Decimal, InvalidOperation
    match = re.fullmatch(r'([+-]?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+))([eE][+-]?[0-9]+|[numkMGTPE]|[KMGTPE]i)?', str(value))
    if not match:
        raise ValueError('invalid Kubernetes storage quantity')
    number, suffix = match.groups()
    try:
        result = Decimal(number)
        if suffix and suffix.startswith(('e', 'E')) and len(suffix) > 1 and suffix[1] in '+-0123456789':
            result *= Decimal(10) ** int(suffix[1:])
        elif suffix in ('n', 'u', 'm'):
            result *= {'n': Decimal('1e-9'), 'u': Decimal('1e-6'), 'm': Decimal('1e-3')}[suffix]
        elif suffix and suffix.endswith('i'):
            result *= 1024 ** ('KMGTPE'.index(suffix[0]) + 1)
        elif suffix:
            result *= 1000 ** ('kMGTPE'.index(suffix) + 1)
        if not result.is_finite() or result < 0:
            raise ValueError('invalid Kubernetes storage quantity')
        return result
    except (InvalidOperation, OverflowError) as error:
        raise ValueError('invalid Kubernetes storage quantity') from error


class ExecutionPodError(RuntimeError):
    """Typed failure returned by the Execution Pod protocol."""

    def __init__(self, code, message):
        super().__init__(message)
        self.code = code
        self.message = message


class ExecutionPodController:
    """Serialize scratch Pod and persistent claim creation on the control node."""

    def __init__(self, backend, config, *, admitted=False):
        self.backend, self.config, self.admitted = backend, config, admitted
        self.lock = RLock()
        self.queues = {}
        self.futures = {}
        self.live_output = {}
        self.foreground_jobs = {}
        self.viewers = {}
        self.admit_locks = {}
        self.lifecycle_locks = {}

    def _space_lock(self, scratch_space_id):
        with self.lock:
            return self.lifecycle_locks.setdefault(scratch_space_id, RLock())

    def _admitted_uid(self, scratch_space_id):
        import json
        from control import scratch_resource_name
        path = self.backend.root / 'execution-v2-spaces' / (scratch_resource_name(scratch_space_id) + '.json')
        with self.lock:
            return json.loads(path.read_bytes()).get('admitted_pod_uid') if path.exists() else None

    def _set_admitted_uid(self, scratch_space_id, uid):
        import json
        from control import save, scratch_resource_name
        path = self.backend.root / 'execution-v2-spaces' / (scratch_resource_name(scratch_space_id) + '.json')
        with self.lock:
            state = json.loads(path.read_bytes())
            state['admitted_pod_uid'] = uid
            save(path, state)

    def _clear_admitted_uid(self, scratch_space_id, expected_uid=None):
        import json
        from control import save, scratch_resource_name
        path = self.backend.root / 'execution-v2-spaces' / (scratch_resource_name(scratch_space_id) + '.json')
        with self.lock:
            if path.exists():
                state = json.loads(path.read_bytes())
                if expected_uid is None or state.get('admitted_pod_uid') == expected_uid:
                    state.pop('admitted_pod_uid', None)
                    save(path, state)

    def _get(self, kind, name):
        import json
        result = self.backend.kube(['get', kind, name, '-o', 'json', '--ignore-not-found'])
        return json.loads(result.stdout) if result.stdout.strip() else None

    def _admit_pod(self, name, settings):
        import json
        import time

        with self.lock:
            gate = self.admit_locks.setdefault(name, Lock())
        with gate:
            deadline = time.monotonic() + 120
            while time.monotonic() < deadline:
                pod = self._get('pod', name)
                if not pod:
                    raise ExecutionPodError('startup_failed', 'Execution Pod disappeared during startup')
                status = pod.get('status', {})
                if status.get('phase') == 'Failed':
                    raise ExecutionPodError('startup_failed', 'Execution Pod failed during initialization: ' +
                                            str(status.get('message') or status.get('reason') or 'unknown cause'))
                metadata = pod.get('metadata', {})
                epoch = int(metadata.get('annotations', {}).get('aidash.run/epoch', '0'))
                node = pod.get('spec', {}).get('nodeName')
                if not metadata.get('uid') or not node or epoch < 1:
                    time.sleep(.2)
                    continue
                claim = self._get('pvc', name)
                if not claim or not claim.get('spec', {}).get('volumeName'):
                    time.sleep(.2)
                    continue
                volume = self._volume_for_purge(claim)
                self._record_volume(settings['scratch_space_id'], volume)
                candidates = [('helpers', status.get('initContainerStatuses', [])),
                              ('execution', status.get('containerStatuses', []))]
                for container_name, statuses in reversed(candidates):
                    container = next((item for item in statuses if item.get('name') == container_name), None)
                    if not container or 'running' not in container.get('state', {}):
                        continue
                    container_id = container.get('containerID', '').removeprefix('containerd://')
                    if len(container_id) != 64:
                        continue
                    if container_name == 'helpers':
                        waiting = self.backend.kube(['exec', name, '-c', 'helpers', '--', 'python3', '-c',
                            "from pathlib import Path;raise SystemExit(not Path('/helpers/.guard-waiting').is_file())"],
                            check=False)
                        if waiting.returncode:
                            continue
                    evidence = self.backend.guard_execution_pod(pod, settings, container_name, container_id, epoch)
                    self._record_guard(pod, container_name, container_id, epoch, host_evidence=evidence)
                    if container_name == 'helpers':
                        self.backend.kube(['exec', name, '-c', 'helpers', '--', 'python3', '-c',
                                           "from pathlib import Path;Path('/helpers/.guard-ready').touch()"])
                    return
                time.sleep(.2)
            raise ExecutionPodError('startup_timeout', 'Execution Pod did not reach the admission gate')

    def _verify_running_pod(self, name, settings):
        import json
        pod = self._get('pod', name)
        if not pod:
            raise ExecutionPodError('startup_failed', 'Execution Pod disappeared after startup')
        statuses = pod.get('status', {}).get('containerStatuses', [])
        execution = next((item for item in statuses if item.get('name') == 'execution'), None)
        if not execution or 'running' not in execution.get('state', {}):
            raise ExecutionPodError('startup_failed', 'Execution container is not running')
        container_id = execution.get('containerID', '').removeprefix('containerd://')
        epoch = int(pod.get('metadata', {}).get('annotations', {}).get('aidash.run/epoch', '0'))
        if len(container_id) != 64 or epoch < 1:
            raise ExecutionPodError('admission_failed', 'Execution Pod identity evidence is incomplete')
        evidence = self.backend.guard_execution_pod(pod, settings, 'execution', container_id, epoch)
        self._record_guard(pod, 'execution', container_id, epoch, host_evidence=evidence)
        mode = 'docker' if settings['docker_enabled'] else 'normal'
        result = self.backend.kube(['exec', name, '-c', 'execution', '--'] +
            helper_command('probe', mode, '/scratch-system/image-root/rootfs', 'user'))
        try:
            report = json.loads(result.stdout)
            verify_user_probe(report, docker=settings['docker_enabled'],
                              processes=self.config.get('v2_processes', 512))
        except (ValueError, KeyError, TypeError) as error:
            raise ExecutionPodError('admission_failed', 'User isolation probe failed: ' + str(error)) from error
        self._record_guard(pod, 'execution', container_id, epoch, host_evidence=evidence, user_evidence=report)
        self._verify_network_pod(pod, settings, container_id, epoch)
        return pod['metadata']['uid']

    def _verify_network_pod(self, pod, settings, container_id, epoch):
        import ipaddress
        import json
        import time
        import uuid
        from urllib.parse import urlsplit

        # The policy selector must identify the gateway rather than a caller's
        # chosen IP. A positive control independently proves each denied target
        # actually has a listener, so a refused port is not isolation evidence.
        expected_policy = v2_network_policy(self.config)
        policies = self.backend.kube(['get', 'networkpolicies', '-o', 'json'])
        verify_network_policies(json.loads(policies.stdout)['items'], pod['metadata']['labels'], expected_policy)
        gateway = urlsplit(self.config['proxy_endpoint'])
        gateway_ip = str(ipaddress.IPv4Address(gateway.hostname))
        node_ip = str(ipaddress.IPv4Address(pod['status']['hostIP']))
        nonce = uuid.uuid4().hex
        policy, canary = network_canary_manifests(self.config, pod['spec']['nodeName'], nonce)
        canary_name = canary['metadata']['name']
        pod_name = pod['metadata']['name']
        listener = None
        try:
            self.backend.kube(['create', '-f', '-'], json.dumps(policy).encode())
            self.backend.kube(['create', '-f', '-'], json.dumps(canary).encode())
            self.backend.kube(['wait', '--for=condition=Ready', 'pod/' + canary_name, '--timeout=60s'], timeout=70)
            listener = self.backend.guard_execution_pod(pod, settings, 'execution', container_id, epoch,
                                                        action='v2_network_start', nonce=nonce)
            target_pod = self._get('pod', canary_name)
            peer_ip = str(ipaddress.IPv4Address(target_pod['status']['podIP']))
            peer_control = self.backend.guard_execution_pod(pod, settings, 'execution', container_id, epoch,
                action='v2_network_reach', address=peer_ip, port=18080)
            if peer_control.get('reachable') is not True:
                raise ValueError('network positive control unavailable: peer from another Pod')
            targets = {'gateway': {'address': gateway_ip, 'port': gateway.port},
                       'peer': {'address': peer_ip, 'port': 18080},
                       'node': {'address': node_ip, 'port': listener['port']},
                       'metadata': {'address': '169.254.169.254', 'port': 80},
                       'internet': {'address': '1.1.1.1', 'port': 443},
                       'udp_port': 18081, 'nonce': nonce}
            # The canary has permissive egress and proves all ports respond.
            # Its own /ready endpoint also proves the UDP listener is bound.
            control = '''import json,socket,sys,urllib.request
t=json.loads(sys.argv[1]); r={}
for n in ("peer","node","metadata","internet","gateway"):
 try:
  s=socket.create_connection((t[n]["address"],t[n]["port"]),timeout=2);s.close();r[n]=True
 except OSError:r[n]=False
try:r["peer"]=r["peer"] and urllib.request.urlopen("http://127.0.0.1:18080/ready",timeout=2).read()==b"ready"
except Exception:r["peer"]=False
print(json.dumps(r))'''
            positive = self.backend.kube(['exec', canary_name, '-c', 'canary', '--', 'python3', '-I', '-c',
                                          control, json.dumps(targets)])
            verify_positive_controls(json.loads(positive.stdout))
            if settings['docker_enabled']:
                self.backend.guard_execution_pod(pod, settings, 'execution', container_id, epoch,
                    action='v2_network_udp_control', address=peer_ip, port=18081)
                self.backend.kube(['exec', canary_name, '-c', 'canary', '--', 'python3', '-I', '-c',
                    'import socket,sys;s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM);'
                    's.sendto(b"control"+bytes.fromhex(sys.argv[3]),(sys.argv[1],int(sys.argv[2])))',
                    node_ip, str(listener['port']), nonce])
                udp_deadline = time.monotonic() + 5
                while time.monotonic() < udp_deadline:
                    node_control = self.backend.guard_execution_pod(pod, settings, 'execution', container_id, epoch,
                                                                     action='v2_network_seen')
                    peer_control = self.backend.kube(['exec', canary_name, '-c', 'canary', '--', 'python3', '-I',
                        '-c', 'import urllib.request;print(urllib.request.urlopen("http://127.0.0.1:18080/control-seen").read().decode())'])
                    if node_control.get('control_seen') is True and peer_control.stdout.strip() == b'true':
                        break
                    time.sleep(.1)
                else:
                    raise ValueError('network UDP positive controls unavailable')
            mode = 'docker' if settings['docker_enabled'] else 'normal'
            result = self.backend.kube(['exec', pod_name, '-c', 'execution', '--'] +
                helper_command('probe', mode, '/scratch-system/image-root/rootfs', 'network', json.dumps(targets)), timeout=45)
            report = json.loads(result.stdout)
            spoof_seen = False
            if settings['docker_enabled']:
                time.sleep(1)
                node_seen = self.backend.guard_execution_pod(pod, settings, 'execution', container_id, epoch,
                                                              action='v2_network_seen')
                canary_seen = self.backend.kube(['exec', canary_name, '-c', 'canary', '--', 'python3', '-I',
                    '-c', 'import urllib.request;print(urllib.request.urlopen("http://127.0.0.1:18080/seen").read().decode())'])
                if node_seen.get('seen') not in (True, False) or canary_seen.stdout.strip() not in (b'true', b'false'):
                    raise ValueError('spoof receiver evidence is incomplete')
                spoof_seen = node_seen['seen'] or canary_seen.stdout.strip() == b'true'
            verify_network_probe(report, docker=settings['docker_enabled'], spoof_seen=spoof_seen)
        except Exception as error:
            raise ExecutionPodError('admission_failed', 'Network isolation probe failed: ' + str(error)) from error
        finally:
            if listener is not None:
                self.backend.guard_execution_pod(pod, settings, 'execution', container_id, epoch,
                                                 action='v2_network_stop')
            self.backend.kube(['delete', 'pod', canary_name, '--ignore-not-found', '--wait=true'])
            self.backend.kube(['delete', 'networkpolicy', canary_name, '--ignore-not-found', '--wait=true'])

    def ensure(self, settings):
        with self._space_lock(settings['scratch_space_id']):
            return self._ensure(settings)

    def _ensure(self, settings):
        import hashlib
        import json

        if not self.admitted:
            raise ExecutionPodError('admission_unverified', 'Execution Pod admission has not been verified')
        claim, pod = execution_manifests(settings, self.config)
        name = pod['metadata']['name']
        # Attach an immutable digest of the desired manifest. Existing Pods
        # with different settings must never be silently reused.
        fingerprint_spec = json.loads(json.dumps(pod['spec']))
        for item in fingerprint_spec['initContainers'][1]['env']:
            if item['name'] == 'AIDASH_RESET_SYSTEM':
                item['value'] = 'false'
        fingerprint = hashlib.sha256(json.dumps(fingerprint_spec, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
        pod['metadata']['annotations'] = {'aidash.run/spec-digest': fingerprint}
        self._require_space_termination_confirmed(settings['scratch_space_id'])
        existing_claim = self._get('pvc', name)
        if existing_claim is None:
            self.backend.kube(['create', '-f', '-'], json.dumps(claim).encode())
        else:
            current = existing_claim['spec']['resources']['requests']['storage']
            if quantity_bytes(current) != settings['capacity_bytes']:
                raise ExecutionPodError('capacity_mismatch', 'Scratch Space capacity differs from the existing claim')
        existing_pod = self._get('pod', name)
        if existing_pod is None:
            self._terminate_recorded_pod(settings['scratch_space_id'])
            pod['metadata']['annotations']['aidash.run/epoch'] = str(self.reserve_epoch(settings['scratch_space_id']))
            self.backend.kube(['create', '-f', '-'], json.dumps(pod).encode())
            self.touch_space(settings['scratch_space_id'])
        elif existing_pod.get('metadata', {}).get('annotations', {}).get('aidash.run/spec-digest') != fingerprint:
            raise ExecutionPodError('settings_mismatch', 'Existing Execution Pod has different settings')
        uid = existing_pod.get('metadata', {}).get('uid') if existing_pod else None
        if uid and uid == self._admitted_uid(settings['scratch_space_id']):
            return {'protocol': 'aidash-runner/2', 'pod': name, 'claim': name}
        try:
            self._admit_pod(name, settings)
            self.backend.kube(['wait', '--for=condition=Ready', 'pod/' + name, '--timeout=1800s'], timeout=1810)
            admitted_uid = self._verify_running_pod(name, settings)
            verified = self._get('pod', name)
            uid = verified.get('metadata', {}).get('uid') if verified else None
            if not uid or uid != admitted_uid:
                raise ExecutionPodError('admission_failed', 'Verified Pod identity changed')
            self._set_admitted_uid(settings['scratch_space_id'], uid)
        except ExecutionPodError:
            try:
                self.stop(settings['scratch_space_id'])
            except Exception as cleanup:
                self._mark_space_unconfirmed(settings['scratch_space_id'])
                raise ExecutionPodError('termination_unconfirmed', 'Admission cleanup failed: ' + str(cleanup)) from cleanup
            raise
        except Exception as error:
            detail = str(error).strip() or type(error).__name__
            try:
                self.stop(settings['scratch_space_id'])
            except Exception as cleanup:
                self._mark_space_unconfirmed(settings['scratch_space_id'])
                raise ExecutionPodError('termination_unconfirmed', 'Admission cleanup failed: ' + str(cleanup)) from cleanup
            raise ExecutionPodError('startup_failed', 'Execution Pod did not become ready: ' + detail[-2000:]) from error
        return {'protocol': 'aidash-runner/2', 'pod': name, 'claim': name}

    def stop(self, scratch_space_id):
        with self._space_lock(scratch_space_id):
            return self._stop(scratch_space_id)

    def _stop(self, scratch_space_id):
        from control import scratch_resource_name
        name = scratch_resource_name(scratch_space_id)
        self._clear_admitted_uid(scratch_space_id)
        self._interrupt_space_commands(scratch_space_id)
        self._terminate_recorded_pod(scratch_space_id)
        self.backend.kube(['delete', 'pod', name, '--ignore-not-found', '--wait=true'])
        self._clear_space_unconfirmed(scratch_space_id)
        with self.lock:
            queue = self.queues.pop(scratch_space_id, None)
            self.viewers.pop(scratch_space_id, None)
        if queue:
            queue.close(wait=False)
        return {'stopped': True}

    def _interrupt_space_commands(self, scratch_space_id):
        import json
        import time
        from control import save
        directory = self.backend.root / 'execution-v2'
        if not directory.exists():
            return
        with self.lock:
            for path in directory.glob('*.json'):
                record = json.loads(path.read_bytes())
                if record.get('scratch_space_id') == scratch_space_id and record.get('status') in ('queued', 'running'):
                    prior = record['status']
                    record.update(status='interrupted' if prior == 'running' else 'cancelled',
                                  outcome='unknown' if prior == 'running' else 'not_started', terminal_at=time.time())
                    save(path, record)
                    future = self.futures.get(record.get('operation_id'))
                    if prior == 'queued' and future:
                        future.cancel()

    def _stop_matching_pod(self, scratch_space_id, pod_uid):
        from control import scratch_resource_name
        name = scratch_resource_name(scratch_space_id)
        if not isinstance(pod_uid, str) or not pod_uid:
            raise ExecutionPodError('pod_identity_missing', 'Cannot verify the original Pod identity')
        with self._space_lock(scratch_space_id):
            self._clear_admitted_uid(scratch_space_id, expected_uid=pod_uid)
            current = self._get('pod', name)
            self._terminate_recorded_pod(scratch_space_id, expected_uid=pod_uid)
            if current and current.get('metadata', {}).get('uid') == pod_uid:
                self.backend.kube(['delete', 'pod', name, '--ignore-not-found', '--wait=true'])

    def _record_guard(self, pod, container_name, container_id, epoch, *, host_evidence=None, user_evidence=None):
        import json
        from control import save
        name = pod['metadata']['name']
        path = self.backend.root / 'execution-v2-spaces' / (name + '.json')
        with self.lock:
            state = json.loads(path.read_bytes())
            state['guard'] = {'pod_name': name, 'pod_uid': pod['metadata']['uid'],
                              'cluster_node': pod['spec']['nodeName'], 'container_name': container_name,
                              'container_id': container_id, 'epoch': epoch, 'terminated': False,
                              'image': self.config['helper_image']}
            if host_evidence is not None:
                state['guard']['host_evidence'] = host_evidence
            if user_evidence is not None:
                state['guard']['user_evidence'] = user_evidence
            save(path, state)

    def _terminate_recorded_pod(self, scratch_space_id, *, expected_uid=None):
        import json
        from control import save, scratch_resource_name
        name = scratch_resource_name(scratch_space_id)
        path = self.backend.root / 'execution-v2-spaces' / (name + '.json')
        state = json.loads(path.read_bytes()) if path.exists() else {}
        guard = state.get('guard')
        if expected_uid and (not guard or guard.get('pod_uid') != expected_uid):
            if expected_uid in state.get('terminated_pods', []):
                return
            raise ExecutionPodError('termination_unconfirmed', 'No node guard binding exists for the original Pod')
        if guard and not guard.get('terminated'):
            if expected_uid and guard['pod_uid'] != expected_uid:
                raise ExecutionPodError('termination_unconfirmed', 'Recorded Pod identity differs from the current Pod')
            try:
                result = self.backend.terminate_execution_pod(guard)
            except Exception as error:
                state['termination_unconfirmed'] = True
                save(path, state)
                raise ExecutionPodError('termination_unconfirmed',
                                        'Node guard could not confirm Pod termination: ' + str(error)) from error
            if result.get('termination_confirmed') is not True:
                state['termination_unconfirmed'] = True
                save(path, state)
                raise ExecutionPodError('termination_unconfirmed', 'Node guard did not confirm Pod termination')
            guard['terminated'] = True
            state.setdefault('terminated_pods', []).append(guard['pod_uid'])
            state['termination_unconfirmed'] = False
            save(path, state)
        current = self._get('pod', name)
        if expected_uid and current and current.get('metadata', {}).get('uid') != expected_uid:
            return
        if current and (not guard or current.get('metadata', {}).get('uid') != guard['pod_uid']):
            statuses = current.get('status', {})
            image_started = any(item.get('state', {}).get('running') is not None or
                                item.get('state', {}).get('terminated') is not None
                                for item in statuses.get('initContainerStatuses', []) if item.get('name') == 'image-root')
            execution_started = any('running' in item.get('state', {}) or 'terminated' in item.get('state', {})
                                    for item in statuses.get('containerStatuses', []))
            if statuses.get('phase') != 'Pending' or image_started or execution_started:
                raise ExecutionPodError('termination_unconfirmed', 'No node guard binding exists for the running Pod')
        if state.get('termination_unconfirmed') and not guard and not current:
            raise ExecutionPodError('termination_unconfirmed', 'The previous Pod termination cannot be proven')

    def _clear_space_unconfirmed(self, scratch_space_id):
        import json
        from control import save, scratch_resource_name
        path = self.backend.root / 'execution-v2-spaces' / (scratch_resource_name(scratch_space_id) + '.json')
        with self.lock:
            if path.exists():
                state = json.loads(path.read_bytes())
                state['termination_unconfirmed'] = False
                save(path, state)

    def _require_space_termination_confirmed(self, scratch_space_id):
        import json
        from control import scratch_resource_name
        path = self.backend.root / 'execution-v2-spaces' / (scratch_resource_name(scratch_space_id) + '.json')
        if path.exists():
            state = json.loads(path.read_bytes())
            if state.get('purging'):
                raise ExecutionPodError('purge_in_progress', 'Scratch Space purge is in progress')
            if state.get('termination_unconfirmed'):
                raise ExecutionPodError('termination_unconfirmed', 'A previous Pod termination has not been confirmed')

    def _mark_space_unconfirmed(self, scratch_space_id):
        import json
        import time
        from control import save, scratch_resource_name
        path = self.backend.root / 'execution-v2-spaces' / (scratch_resource_name(scratch_space_id) + '.json')
        with self.lock:
            path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            state = json.loads(path.read_bytes()) if path.exists() else {
                'scratch_space_id': scratch_space_id, 'last_command_at': time.time()}
            state['termination_unconfirmed'] = True
            save(path, state)

    def delete(self, scratch_space_id):
        import json
        import time
        from concurrent.futures import wait
        from control import save, scratch_resource_name
        name = scratch_resource_name(scratch_space_id)
        with self._space_lock(scratch_space_id):
            claim = self._get('pvc', name)
            if claim and claim.get('spec', {}).get('volumeName'):
                self._volume_for_purge(claim)
            with self.lock:
                path = self.backend.root / 'execution-v2-spaces' / (name + '.json')
                path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                state = json.loads(path.read_bytes()) if path.exists() else {
                    'scratch_space_id': scratch_space_id, 'last_command_at': time.time()}
                state['purging'] = True
                save(path, state)
                directory = self.backend.root / 'execution-v2'
                operations = []
                for record_path in directory.glob('*.json') if directory.exists() else ():
                    record = json.loads(record_path.read_bytes())
                    if record.get('scratch_space_id') == scratch_space_id:
                        future = self.futures.get(record.get('operation_id'))
                        if future:
                            operations.append(future)
            self._stop(scratch_space_id)
        if operations:
            _, pending = wait(operations, timeout=30)
            if pending:
                raise ExecutionPodError('purge_unconfirmed', 'Command workers have not finished')
        with self._space_lock(scratch_space_id):
            result = self._delete(scratch_space_id)
            self._purge_local_state(scratch_space_id)
            return result

    def _purge_local_state(self, scratch_space_id):
        import json
        from control import scratch_resource_name
        with self.lock:
            for directory in ('execution-v2', 'execution-v2-python'):
                root = self.backend.root / directory
                for path in root.glob('*.json') if root.exists() else ():
                    if json.loads(path.read_bytes()).get('scratch_space_id') == scratch_space_id:
                        path.unlink()
            path = self.backend.root / 'execution-v2-spaces' / (scratch_resource_name(scratch_space_id) + '.json')
            path.unlink(missing_ok=True)
            self.foreground_jobs.pop(scratch_space_id, None)
            self.viewers.pop(scratch_space_id, None)

    def _delete(self, scratch_space_id):
        import json
        import time
        from control import scratch_resource_name
        name = scratch_resource_name(scratch_space_id)
        path = self.backend.root / 'execution-v2-spaces' / (name + '.json')
        state = json.loads(path.read_bytes()) if path.exists() else {}
        claim = self._get('pvc', name)
        volume = self._volume_for_purge(claim) if claim and claim.get('spec', {}).get('volumeName') else None
        if volume:
            self._record_volume(scratch_space_id, volume)
        elif state.get('volume'):
            volume = state['volume']
            current = self._get('pv', volume['name'])
            if claim and current:
                raise ExecutionPodError('purge_unconfirmed', 'Claim lost its recorded volume binding')
        self._terminate_recorded_pod(scratch_space_id)
        self.backend.kube(['delete', 'pod', name, '--ignore-not-found', '--wait=true'])
        self._clear_space_unconfirmed(scratch_space_id)
        self.backend.kube(['delete', 'pvc', name, '--ignore-not-found', '--wait=true'])
        if volume:
            deadline = time.monotonic() + 120
            while time.monotonic() < deadline:
                current = self._get('pv', volume['name'])
                if not current or current.get('metadata', {}).get('uid') != volume['uid']:
                    self._mark_purged(scratch_space_id)
                    return {'deleted': True}
                    time.sleep(.5)
                raise ExecutionPodError('purge_unconfirmed', 'Persistent volume deletion was not confirmed')
            if state.get('guard') and not state.get('purged'):
                raise ExecutionPodError('purge_unconfirmed', 'Persistent volume identity is unavailable')
            self._mark_purged(scratch_space_id)
        return {'deleted': True}

    def _volume_for_purge(self, claim):
        volume_name = claim['spec']['volumeName']
        volume = self._get('pv', volume_name)
        if not volume:
            raise ExecutionPodError('purge_unconfirmed', 'Bound persistent volume is unavailable')
        metadata, spec = volume.get('metadata', {}), volume.get('spec', {})
        if spec.get('claimRef', {}).get('uid') != claim.get('metadata', {}).get('uid'):
            raise ExecutionPodError('purge_unconfirmed', 'Persistent volume claim identity does not match')
        if spec.get('persistentVolumeReclaimPolicy') != 'Delete':
            raise ExecutionPodError('purge_unsupported', 'Persistent volume reclaim policy must be Delete')
        finalizers = set(metadata.get('finalizers', []))
        if not finalizers.intersection({'external-provisioner.volume.kubernetes.io/finalizer',
                                        'kubernetes.io/pv-controller'}):
            raise ExecutionPodError('purge_unsupported', 'Persistent volume deletion finalizer is unavailable')
        if not metadata.get('uid'):
            raise ExecutionPodError('purge_unconfirmed', 'Persistent volume UID is unavailable')
        return {'name': volume_name, 'uid': metadata['uid']}

    def _record_volume(self, scratch_space_id, volume):
        import json
        from control import save, scratch_resource_name
        path = self.backend.root / 'execution-v2-spaces' / (scratch_resource_name(scratch_space_id) + '.json')
        with self.lock:
            state = json.loads(path.read_bytes())
            previous = state.get('volume')
            if previous and previous != volume:
                raise ExecutionPodError('purge_unconfirmed', 'Scratch Space persistent volume identity changed')
            state.update(volume=volume, purged=False)
            save(path, state)

    def _mark_purged(self, scratch_space_id):
        import json
        from control import save, scratch_resource_name
        path = self.backend.root / 'execution-v2-spaces' / (scratch_resource_name(scratch_space_id) + '.json')
        if path.exists():
            state = json.loads(path.read_bytes())
            state.update(purged=True, volume=None)
            save(path, state)

    def recover_commands(self):
        """Retire incomplete executions without ever replaying their commands."""
        import json
        import time
        from control import save

        directory = self.backend.root / 'execution-v2'
        if not directory.exists():
            return
        records = []
        for path in directory.glob('*.json'):
            record = json.loads(path.read_bytes())
            if record['status'] in ('queued', 'running'):
                records.append((path, record))
        # A disconnected kubectl exec cannot prove its remote child stopped.
        # Retire each affected Pod before admitting any new work.
        for scratch_id, pod_uid in {(record['scratch_space_id'], record.get('pod_uid'))
                                    for _, record in records if record['status'] == 'running'}:
            try:
                self._stop_matching_pod(scratch_id, pod_uid)
            except Exception:
                self._mark_space_unconfirmed(scratch_id)
        for path, record in records:
            if record['status'] == 'running':
                record.update(status='interrupted', outcome='unknown')
            else:
                record.update(status='cancelled', outcome='not_started')
            record['terminal_at'] = time.time()
            save(path, record)

    def _command_path(self, operation_id):
        import uuid
        if not isinstance(operation_id, str) or str(uuid.UUID(operation_id)) != operation_id:
            raise ValueError('canonical operation UUID required')
        directory = self.backend.root / 'execution-v2'
        directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        return directory / (operation_id + '.json')

    def _read_command(self, operation_id):
        import json
        try:
            return json.loads(self._command_path(operation_id).read_bytes())
        except FileNotFoundError:
            raise ExecutionPodError('command_not_found', 'Command does not exist') from None

    def _update_command(self, operation_id, **fields):
        import time
        from control import save
        with self.lock:
            record = self._read_command(operation_id)
            if record.get('status') == 'interrupted' and fields.get('status') != 'interrupted':
                return record
            if fields.get('status') in ('completed', 'timed_out', 'stopped', 'cancelled', 'interrupted'):
                fields.setdefault('terminal_at', time.time())
            record.update(fields)
            save(self._command_path(operation_id), record)
            return record

    def command_result(self, operation_id):
        import base64
        import json
        import time
        with self.lock:
            record = self._read_command(operation_id)
            result = {key: record[key] for key in ('operation_id', 'scratch_space_id', 'run_id',
                      'status', 'background', 'outcome', 'exit_code', 'stdout', 'stderr', 'truncated',
                      'error_code', 'message') if key in record}
            if record['status'] == 'running' and operation_id in self.live_output:
                result.update(self.live_output[operation_id])
            if record.get('kind') == 'python_cell' and record['status'] == 'completed' and record.get('exit_code') == 0:
                try:
                    result['python_result'] = json.loads(base64.b64decode(record['stdout'], validate=True))
                except (ValueError, KeyError):
                    result['python_result'] = {'status': 'failed', 'code': 'python_result_invalid',
                                               'message': 'Python result was unavailable or truncated'}
            return result

    def submit_command(self, request, *, kind='shell', session_operation_id=None, client_digest=None):
        with self._space_lock(request['scratch_space_id']):
            return self._submit_command(request, kind=kind, session_operation_id=session_operation_id, client_digest=client_digest)

    def _submit_command(self, request, *, kind='shell', session_operation_id=None, client_digest=None):
        import hashlib
        import json
        import time
        from control import save, scratch_resource_name

        if not self.admitted:
            raise ExecutionPodError('admission_unverified', 'Execution Pod admission has not been verified')
        operation_id = request['operation_id']
        path = self._command_path(operation_id)
        scratch_id, run_id, code = request['scratch_space_id'], request['run_id'], request['code']
        name = scratch_resource_name(scratch_id)
        if not isinstance(run_id, str) or not run_id or len(run_id) > 256 or '\x00' in run_id:
            raise ValueError('run_id must be a nonempty string without NUL')
        env = request.get('env', {})
        if not isinstance(env, dict) or any(not isinstance(k, str) or not isinstance(v, str) or '\x00' in k or '\x00' in v for k, v in env.items()):
            raise ValueError('environment must contain strings without NUL')
        if env:
            raise ValueError('custom environment is not supported')
        code_limit = 131072 if kind == 'python_cell' else 65536
        if not isinstance(code, str) or '\x00' in code or len(code.encode('utf-8')) > code_limit:
            raise ValueError('command exceeds the code limit')
        cwd = request.get('cwd', '.')
        if not isinstance(cwd, str) or '\x00' in cwd or cwd.startswith('/') or '..' in cwd.split('/'):
            raise ValueError('cwd must be relative to /scratch')
        background = request.get('background', False)
        if type(background) is not bool:
            raise ValueError('background must be a boolean')
        seconds = execution_timeout(request)
        digest = hashlib.sha256(json.dumps(request, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
        with self.lock:
            self._require_space_termination_confirmed(scratch_id)
            if path.exists():
                prior = self._read_command(operation_id)
                if prior.get('request_digest') != digest:
                    raise ExecutionPodError('command_conflict', 'Operation ID was already used for another command')
                return self.command_result(operation_id)
        pod = self._get('pod', name)
        with self.lock:
            if not pod or pod.get('status', {}).get('phase') != 'Running':
                raise ExecutionPodError('pod_unavailable', 'Execution Pod is not running')
            uid = pod.get('metadata', {}).get('uid')
            if not isinstance(uid, str) or not uid:
                raise ExecutionPodError('pod_unavailable', 'Execution Pod UID is unavailable')
            if self._admitted_uid(scratch_id) != uid:
                raise ExecutionPodError('admission_unverified', 'Execution Pod has not passed admission')
            containers = pod.get('spec', {}).get('containers', [])
            execution = next((item for item in containers if item.get('name') == 'execution'), None)
            if execution is None:
                raise ExecutionPodError('pod_unavailable', 'Execution Pod container is unavailable')
            caps = set(execution.get('securityContext', {}).get('capabilities', {}).get('add', []))
            if ('NET_ADMIN' in caps) != ('NET_RAW' in caps):
                raise ExecutionPodError('admission_unverified', 'Execution Pod capability configuration is invalid')
            docker = 'NET_ADMIN' in caps
            record = {'operation_id': operation_id, 'scratch_space_id': scratch_id, 'run_id': run_id,
                      'code': code, 'cwd': cwd, 'timeout_seconds': seconds, 'background': background,
                      'pod': name, 'pod_uid': uid, 'docker_enabled': docker, 'status': 'queued',
                      'request_digest': digest, 'accepted_at': time.time(), 'kind': kind}
            if session_operation_id:
                record['session_operation_id'] = session_operation_id
            if client_digest:
                record['client_digest'] = client_digest
            save(path, record)
            self.touch_space(scratch_id)
            if not background:
                self.foreground_jobs[scratch_id] = self.foreground_jobs.get(scratch_id, 0) + 1
            if scratch_id not in self.queues:
                self.queues[scratch_id] = CommandQueue()
            queue = self.queues[scratch_id]
            if kind == 'python_session':
                future = queue.submit_session(operation_id, self._execute_command)
            else:
                future = queue.submit(run_id, operation_id, self._execute_command, background=background)
            self.futures[operation_id] = future
            future.add_done_callback(lambda done: self._cancelled_command(operation_id, done))
            return self.command_result(operation_id)

    def _cancelled_command(self, operation_id, future):
        with self.lock:
            try:
                if future.cancelled():
                    self._update_command(operation_id, status='cancelled', outcome='not_started')
                record = self._read_command(operation_id)
            except ExecutionPodError as error:
                if error.code != 'command_not_found':
                    raise
                self.futures.pop(operation_id, None)
                return
            if not record['background']:
                scratch_id = record['scratch_space_id']
                self.foreground_jobs[scratch_id] = max(0, self.foreground_jobs.get(scratch_id, 0) - 1)
                self.touch_space(scratch_id)
            self.futures.pop(operation_id, None)

    def _await_normal_result(self, record, deadline):
        import json
        import time
        name, operation_id = record['pod'], record['operation_id']
        missing_since = time.monotonic()
        while deadline is None or time.monotonic() < deadline:
            if self._read_command(operation_id).get('status') == 'interrupted':
                return None
            try:
                observed = self.backend.kube(['exec', name, '-c', 'execution', '--'] +
                                             helper_command('result', operation_id), check=False)
                if observed.returncode == 0:
                    return json.loads(observed.stdout)
                pod = self._get('pod', name)
                if (not pod or pod.get('metadata', {}).get('uid') != record['pod_uid']
                        or pod.get('status', {}).get('phase') != 'Running'):
                    return None
                running = self.backend.kube(['exec', name, '-c', 'execution', '--'] +
                                            helper_command('running', operation_id), check=False)
                if running.returncode == 0 and json.loads(running.stdout).get('running') is True:
                    missing_since = time.monotonic()
                elif time.monotonic() - missing_since > 30:
                    return None
            except Exception:
                if time.monotonic() - missing_since > 30:
                    return None
            time.sleep(.5)
        return None

    def _execute_command(self, operation_id):
        import time
        with self.lock:
            record = self._read_command(operation_id)
            if record['status'] != 'queued':
                return
            record = self._update_command(operation_id, status='running')
        name, uid = record['pod'], record['pod_uid']
        mode = 'docker' if record['docker_enabled'] else 'normal'
        deadline = 'background' if record['background'] else str(record['timeout_seconds'])
        kind = 'session' if record.get('kind') == 'python_session' else 'command'
        argv = self.backend.command + ['exec'] + (['-i'] if record['docker_enabled'] else []) + [name, '-c', 'execution', '--'] + helper_command(
            'supervise', mode, kind, operation_id, record['run_id'], record['cwd'], deadline, uid, record['code'])

        def publish(output):
            with self.lock:
                self.live_output[operation_id] = output

        try:
            limit = self.config.get('output_bytes', getattr(self.backend, 'config', {}).get('output_bytes', 8388608))
            transport_timeout = None if record['background'] else record['timeout_seconds'] + 180
            result_deadline = None if transport_timeout is None else time.monotonic() + transport_timeout
            if record['docker_enabled']:
                import secrets
                nonce = secrets.token_hex(32)
                transport = drain_process(argv, self.backend.environment, limit, transport_timeout, publish, frame_token=nonce)
            else:
                transport = drain_process(argv, self.backend.environment, limit, transport_timeout, publish)
            if self._read_command(operation_id)['status'] == 'interrupted':
                return
            if record['docker_enabled']:
                outcome = transport.get('framed_result')
            else:
                outcome = self._await_normal_result(record, result_deadline)
                if self._read_command(operation_id)['status'] == 'interrupted':
                    return
                if transport['exit_code'] != 0:
                    transport['output']['truncated'] = True
            if (transport['transport_timed_out'] or not isinstance(outcome, dict)
                    or outcome.get('operation_id') != operation_id or outcome.get('run_id') != record['run_id']
                    or outcome.get('status') not in ('completed', 'timed_out', 'stopped')):
                self._stop_matching_pod(record['scratch_space_id'], uid)
                self._update_command(operation_id, status='interrupted', outcome='unknown',
                                     **transport['output'])
            else:
                if (record.get('kind') == 'python_cell' and
                    (outcome['status'] == 'timed_out' or
                     (outcome['status'] == 'completed' and outcome.get('exit_code') != 0))):
                    try:
                        self.stop_background(record['session_operation_id'])
                    except Exception:
                        self._stop_matching_pod(record['scratch_space_id'], uid)
                self._update_command(operation_id, status=outcome['status'],
                                     **{key: outcome[key] for key in ('exit_code', 'error_code', 'message') if key in outcome},
                                     **transport['output'])
                try:
                    self.backend.kube(['exec', name, '-c', 'execution', '--'] +
                                      helper_command('forget', operation_id), check=False)
                except Exception:
                    pass  # The Pod's bounded temporary journal is reclaimed when it stops.
        except Exception as error:
            if self._read_command(operation_id)['status'] == 'interrupted':
                return
            try:
                self._stop_matching_pod(record['scratch_space_id'], uid)
            except Exception as stop_error:
                self._mark_space_unconfirmed(record['scratch_space_id'])
                error = RuntimeError(str(error) + '; termination unconfirmed: ' + str(stop_error))
            self._update_command(operation_id, status='interrupted', outcome='unknown',
                                 error_code='command_interrupted', message=str(error) or type(error).__name__)
        finally:
            with self.lock:
                self.live_output.pop(operation_id, None)

    def cancel_run(self, scratch_space_id, run_id):
        import json
        from control import scratch_resource_name
        scratch_resource_name(scratch_space_id)
        if not isinstance(run_id, str) or not run_id:
            raise ValueError('run_id must be a nonempty string')
        with self._space_lock(scratch_space_id):
            with self.lock:
                queue = self.queues.get(scratch_space_id)
            if queue:
                queue.cancel_waiting(run_id)
            directory = self.backend.root / 'execution-v2'
            for path in directory.glob('*.json') if directory.exists() else ():
                record = json.loads(path.read_bytes())
                if (record.get('scratch_space_id') == scratch_space_id and record.get('run_id') == run_id
                        and not record.get('background') and record.get('status') in ('queued', 'running')):
                    self._stop_command(record['operation_id'])
        return {'cancelled_waiting': True}

    def background_processes(self, scratch_space_id):
        import json
        import time
        from control import scratch_resource_name

        scratch_resource_name(scratch_space_id)
        directory = self.backend.root / 'execution-v2'
        if not directory.exists():
            return {'processes': []}
        result = []
        terminal = {'completed', 'timed_out', 'stopped', 'cancelled', 'interrupted'}
        for path in directory.glob('*.json'):
            record = json.loads(path.read_bytes())
            if (record.get('scratch_space_id') == scratch_space_id and record.get('background')
                    and record.get('kind') != 'python_session'):
                finished = record.get('terminal_at') or record.get('accepted_at')
                if record.get('status') in terminal and finished and time.time() - finished > 7 * 86400:
                    continue
                result.append({key: record[key] for key in ('operation_id', 'scratch_space_id', 'run_id',
                              'status', 'background', 'outcome', 'exit_code', 'truncated',
                              'error_code', 'message') if key in record})
        return {'processes': sorted(result, key=lambda item: item['operation_id'])}

    def stop_background(self, operation_id):
        import time
        record = self._read_command(operation_id)
        if not record.get('background'):
            raise ExecutionPodError('not_background', 'Command is not a background process')
        return self._stop_command(operation_id)

    def _stop_command(self, operation_id):
        import time
        record = self._read_command(operation_id)
        with self.lock:
            future = self.futures.get(operation_id)
        if record['status'] == 'queued' and future and future.cancel():
            return self.command_result(operation_id)
        if record['status'] in ('completed', 'timed_out', 'stopped', 'cancelled', 'interrupted'):
            return self.command_result(operation_id)
        pod = self._get('pod', record['pod'])
        if not pod or pod.get('metadata', {}).get('uid') != record['pod_uid']:
            raise ExecutionPodError('pod_interrupted', 'The original Execution Pod is unavailable')
        # A future can leave the queue before its in-Pod supervisor publishes
        # the stop marker. Wait for that short startup window instead of losing
        # a stop request at the queued-to-running transition.
        deadline = time.monotonic() + 10
        while True:
            try:
                self.backend.kube(['exec', record['pod'], '-c', 'execution', '--'] + helper_command('stop', operation_id))
                return {'operation_id': operation_id, 'status': 'stopping'}
            except Exception as error:
                current = self.command_result(operation_id)
                if current['status'] in ('completed', 'timed_out', 'stopped', 'cancelled', 'interrupted'):
                    return current
                if time.monotonic() >= deadline:
                    raise ExecutionPodError('stop_failed', 'Could not stop background process: ' + str(error)) from error
                time.sleep(.1)

    def reset(self, settings):
        with self._space_lock(settings['scratch_space_id']):
            return self._reset(settings)

    def _reset(self, settings):
        if not self.admitted:
            raise ExecutionPodError('admission_unverified', 'Execution Pod admission has not been verified')
        self.stop(settings['scratch_space_id'])
        result = self.ensure(dict(settings, _reset_system=True))
        return dict(result, reset=True)

    def change_log(self, scratch_space_id, offset=0, limit=65536):
        import json
        from control import scratch_resource_name

        if type(offset) is not int or offset < 0 or type(limit) is not int or not 1 <= limit <= 262144:
            raise ValueError('invalid log offset or limit')
        name = scratch_resource_name(scratch_space_id)
        try:
            result = self.backend.kube(['exec', name, '-c', 'execution', '--'] + helper_command('changes', str(offset), str(limit)))
            return json.loads(result.stdout)
        except Exception as error:
            raise ExecutionPodError('change_log_unavailable', 'Could not read system changes: ' + str(error)) from error

    def touch_space(self, scratch_space_id, *, now=None):
        import json
        import time
        from control import save, scratch_resource_name
        name = scratch_resource_name(scratch_space_id)
        directory = self.backend.root / 'execution-v2-spaces'
        with self.lock:
            directory.mkdir(mode=0o700, parents=True, exist_ok=True)
            path = directory / (name + '.json')
            timestamp = time.time() if now is None else now
            state = {'scratch_space_id': scratch_space_id}
            if path.exists():
                state = json.loads(path.read_bytes())
                timestamp = max(timestamp, state['last_command_at'])
            state['last_command_at'] = timestamp
            save(path, state)

    def reserve_epoch(self, scratch_space_id):
        import json
        import time
        from control import save, scratch_resource_name
        name = scratch_resource_name(scratch_space_id)
        directory = self.backend.root / 'execution-v2-spaces'
        with self.lock:
            directory.mkdir(mode=0o700, parents=True, exist_ok=True)
            path = directory / (name + '.json')
            state = json.loads(path.read_bytes()) if path.exists() else {
                'scratch_space_id': scratch_space_id, 'last_command_at': time.time()}
            state['epoch'] = state.get('epoch', 0) + 1
            state.pop('guard', None)
            state.pop('admitted_pod_uid', None)
            save(path, state)
            return state['epoch']

    def viewer_heartbeat(self, scratch_space_id, viewer_id, *, now=None):
        import time
        from control import scratch_resource_name
        scratch_resource_name(scratch_space_id)
        if not isinstance(viewer_id, str) or not viewer_id:
            raise ValueError('viewer_id must be a nonempty string')
        with self.lock:
            expiry = (time.time() if now is None else now) + 60
            self.viewers.setdefault(scratch_space_id, {})[viewer_id] = expiry
            self.touch_space(scratch_space_id, now=expiry)
        return {'active': True}

    def stop_idle(self, *, now=None):
        import json
        import time
        from control import scratch_resource_name
        current = time.time() if now is None else now
        directory = self.backend.root / 'execution-v2-spaces'
        if not directory.exists():
            return
        for path in directory.glob('*.json'):
            try:
                scratch_id = json.loads(path.read_bytes())['scratch_space_id']
                with self._space_lock(scratch_id):
                    with self.lock:
                        space = json.loads(path.read_bytes())
                        unconfirmed = space.get('termination_unconfirmed', False)
                        if not unconfirmed:
                            if current - space['last_command_at'] < 1800 or self.foreground_jobs.get(scratch_id, 0):
                                continue
                            viewers = self.viewers.get(scratch_id, {})
                            self.viewers[scratch_id] = {key: expiry for key, expiry in viewers.items() if expiry > current}
                            if self.viewers[scratch_id]:
                                continue
                    if unconfirmed or self._get('pod', scratch_resource_name(scratch_id)):
                        self.stop(scratch_id)
            except Exception as error:
                import sys
                print('Execution Pod idle cleanup failed: ' + str(error), file=sys.stderr, flush=True)

    def start_idle_monitor(self):
        import sys
        import threading
        import time

        def monitor():
            while True:
                try:
                    self.stop_idle()
                except Exception as error:
                    print('Execution Pod idle monitor failed: ' + str(error), file=sys.stderr, flush=True)
                time.sleep(30)

        threading.Thread(target=monitor, name='execution-pod-idle', daemon=True).start()

    def _python_session_path(self, scratch_space_id, run_id):
        import hashlib
        import json
        from control import scratch_resource_name
        scratch_resource_name(scratch_space_id)
        if not isinstance(run_id, str) or not run_id or len(run_id) > 256:
            raise ValueError('run_id must be a nonempty string of at most 256 characters')
        key = hashlib.sha256(json.dumps([scratch_space_id, run_id], ensure_ascii=False).encode()).hexdigest()
        directory = self.backend.root / 'execution-v2-python'
        directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        return directory / (key + '.json')

    def start_python(self, scratch_space_id, run_id):
        with self._space_lock(scratch_space_id):
            return self._start_python(scratch_space_id, run_id)

    def _start_python(self, scratch_space_id, run_id):
        import json
        import shlex
        import uuid
        from control import save, scratch_resource_name

        if not self.admitted:
            raise ExecutionPodError('admission_unverified', 'Execution Pod admission has not been verified')
        path = self._python_session_path(scratch_space_id, run_id)
        if path.exists():
            prior = json.loads(path.read_bytes())
            record = self._read_command(prior['operation_id'])
            pod = self._get('pod', scratch_resource_name(scratch_space_id))
            if (record['status'] in ('queued', 'running') and pod
                    and pod.get('metadata', {}).get('uid') == record.get('pod_uid')):
                return prior
        operation_id = str(uuid.uuid4())
        socket_path = '/run/aidash/' + operation_id + '.sock'
        code = 'exec ' + shlex.join(python_helper_command('serve', socket_path))
        self.submit_command({'operation_id': operation_id, 'scratch_space_id': scratch_space_id,
                             'run_id': run_id, 'code': code, 'background': True}, kind='python_session')
        session = {'operation_id': operation_id, 'scratch_space_id': scratch_space_id,
                   'run_id': run_id, 'socket_path': socket_path}
        save(path, session)
        return session

    def python_cell(self, request):
        import base64
        import hashlib
        import json
        import shlex

        env = request.get('env', {})
        if not isinstance(env, dict) or any(not isinstance(k, str) or not isinstance(v, str) or '\x00' in k or '\x00' in v for k, v in env.items()):
            raise ValueError('environment must contain strings without NUL')
        if env:
            raise ValueError('custom environment is not supported')

        operation_id = request['operation_id']
        path = self._command_path(operation_id)
        original_digest = hashlib.sha256(json.dumps(request, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
        with self.lock:
            if path.exists():
                prior = self._read_command(operation_id)
                if prior.get('client_digest') != original_digest:
                    raise ExecutionPodError('command_conflict', 'Operation ID was already used for another command')
                return self.command_result(operation_id)
        code = request['code']
        if not isinstance(code, str) or '\x00' in code or len(code.encode('utf-8')) > 65536:
            raise ValueError('Python code exceeds the limit')
        if any(not isinstance(request.get(key, '.'), str) or '\x00' in request.get(key, '.') for key in ('cwd', 'run_id')):
            raise ValueError('Python request contains NUL')
        seconds = execution_timeout(request)
        session = self.start_python(request['scratch_space_id'], request['run_id'])
        output_limit = self.config.get('output_bytes', getattr(self.backend, 'config', {}).get('output_bytes', 8388608))
        cell_limit = max(1, min(8388608, (output_limit - 4096) // 6))
        encoded = base64.b64encode(code.encode('utf-8')).decode('ascii')
        cwd_encoded = base64.b64encode(request.get('cwd', '.').encode('utf-8')).decode('ascii')
        command = shlex.join(python_helper_command('cell', session['socket_path'], str(seconds), str(cell_limit), encoded, cwd_encoded))
        return self.submit_command({'operation_id': operation_id, 'scratch_space_id': request['scratch_space_id'],
                                    'run_id': request['run_id'], 'code': command,
                                    'cwd': request.get('cwd', '.'), 'timeout_seconds': seconds},
                                   kind='python_cell', session_operation_id=session['operation_id'],
                                   client_digest=original_digest)

    def stop_python(self, scratch_space_id, run_id):
        import json
        path = self._python_session_path(scratch_space_id, run_id)
        if not path.exists():
            return {'stopped': True}
        session = json.loads(path.read_bytes())
        return self.stop_background(session['operation_id'])


def execution_timeout(request):
    """Validate the public command deadline in seconds."""
    timeout = request.get('timeout_seconds', 600)
    if type(timeout) is not int or not 1 <= timeout <= 3600:
        raise ValueError('timeout_seconds must be an integer between 1 and 3600')
    return timeout


class BoundedOutput:
    """Drain both streams while retaining at most the configured total bytes."""

    def __init__(self, limit):
        if type(limit) is not int or limit < 0:
            raise ValueError('output limit must be a nonnegative integer')
        self.limit = limit
        self.streams = {'stdout': bytearray(), 'stderr': bytearray()}
        self.truncated = False
        self.used = 0

    def add(self, stream, data):
        room = self.limit - self.used
        chunk = data[:room]
        self.streams[stream].extend(chunk)
        self.used += len(chunk)
        self.truncated |= len(chunk) < len(data)

    def public(self):
        import base64
        return {**{name: base64.b64encode(value).decode('ascii') for name, value in self.streams.items()},
                'truncated': self.truncated}


class _ResultFrameReader:
    def __init__(self, token, output):
        self.prefix = b'\x1eaidash-result:' + token.encode('ascii') + b':'
        self.output = output
        self.pending = b''
        self.result = None

    def add(self, chunk, *, final=False):
        import json
        self.pending += chunk
        while self.pending:
            position = self.pending.find(self.prefix)
            if position < 0:
                count = len(self.pending) if final else max(0, len(self.pending) - len(self.prefix) + 1)
                self.output.add('stderr', self.pending[:count])
                self.pending = self.pending[count:]
                return
            if position:
                self.output.add('stderr', self.pending[:position])
                self.pending = self.pending[position:]
            end = self.pending.find(b'\n', len(self.prefix))
            if end < 0 and len(self.pending) < 8192 and not final:
                return
            if end >= 0 and end < 8192:
                try:
                    parsed = json.loads(self.pending[len(self.prefix):end])
                except (ValueError, UnicodeDecodeError):
                    parsed = None
                if isinstance(parsed, dict) and self.result is None:
                    self.result = parsed
                    self.pending = self.pending[end + 1:]
                    continue
            self.output.add('stderr', self.pending[:1])
            self.pending = self.pending[1:]


def drain_process(argv, environment, output_limit, timeout, on_output=None, *, frame_token=None):
    """Drain a trusted transport without buffering unbounded command output."""
    import os
    import selectors
    import signal
    import subprocess
    import time

    output = BoundedOutput(output_limit)
    frame = _ResultFrameReader(frame_token, output) if frame_token else None
    process = subprocess.Popen(argv, stdin=subprocess.PIPE if frame else subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, env=environment, start_new_session=True)
    if frame:
        try:
            process.stdin.write((frame_token + '\n').encode('ascii'))
            process.stdin.close()
        except BrokenPipeError:
            pass
    expires = None if timeout is None else time.monotonic() + timeout
    timed_out = False
    last_publish = 0.0
    with selectors.DefaultSelector() as selector:
        selector.register(process.stdout, selectors.EVENT_READ, 'stdout')
        selector.register(process.stderr, selectors.EVENT_READ, 'stderr')
        try:
            while selector.get_map():
                if expires is not None and time.monotonic() >= expires and not timed_out:
                    timed_out = True
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    break
                for key, _ in selector.select(.1):
                    chunk = os.read(key.fileobj.fileno(), 65536)
                    if chunk:
                        if frame and key.data == 'stderr':
                            frame.add(chunk)
                        else:
                            output.add(key.data, chunk)
                        if on_output and time.monotonic() - last_publish >= .5:
                            on_output(output.public())
                            last_publish = time.monotonic()
                    else:
                        selector.unregister(key.fileobj)
                        key.fileobj.close()
            exit_code = process.wait()
        finally:
            if process.poll() is None:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait()
            for key in list(selector.get_map().values()):
                key.fileobj.close()
    if frame:
        frame.add(b'', final=True)
    return {'exit_code': exit_code, 'transport_timed_out': timed_out, 'output': output.public(),
            'framed_result': frame.result if frame else None}


def verify_user_probe(report, *, docker, processes):
    """Fail closed unless a real user child has the expected isolation profile."""
    if report.get('uid') != 0 or 'gvisor' not in str(report.get('kernel', '')).lower():
        raise ValueError('gVisor root user required')
    if report.get('no_new_privs') != 1:
        raise ValueError('user process restrictions do not match the profile')
    if not docker and (report.get('process_limit') != [processes, processes]
                       or report.get('process_limit_enforced') is not True
                       or report.get('process_limit_unraiseable') is not True):
        raise ValueError('user process ceiling is not enforced')
    if any(report.get(key) is not True for key in ('system_absent', 'user_namespace_blocked', 'proc_root_blocked')):
        raise ValueError('chroot isolation evidence is incomplete')
    if report.get('service_account_absent') is not True:
        raise ValueError('service account token is exposed')
    if (report.get('root_writable') is not True and report.get('root_full') is not True) or report.get('helpers_read_only') is not True:
        raise ValueError('system root or helper mount protection is invalid')
    caps = report.get('caps', {})
    names = ('CapInh', 'CapPrm', 'CapEff', 'CapBnd', 'CapAmb')
    if any(type(caps.get(name)) is not int for name in names):
        raise ValueError('capability evidence is incomplete')
    if any(caps[name] & ((1 << 19) | (1 << 24)) for name in names):
        raise ValueError('unsafe process-control capabilities remain available')
    blocked = (1 << 12) | (1 << 13) | (1 << 21)
    if docker:
        if any(caps[name] & blocked != blocked for name in ('CapPrm', 'CapEff', 'CapBnd')):
            raise ValueError('Docker capabilities are unavailable')
        if report.get('raw_socket') is not True:
            raise ValueError('Docker raw sockets are unavailable')
    elif any(caps[name] & (blocked | (1 << 18)) for name in names) or report.get('raw_socket') is not False:
        raise ValueError('Docker capabilities remain available to the user process')


def verify_network_probe(report, *, docker, spoof_seen):
    """Check positive gateway and negative network/spoof controls."""
    if report.get('gateway') is not True:
        raise ValueError('egress gateway is unreachable')
    if any(report.get(name) is not False for name in ('peer', 'node', 'metadata', 'internet')):
        raise ValueError('network isolation probe failed')
    if docker and (report.get('raw_sent') is not True or spoof_seen is not False):
        raise ValueError('spoofed packet isolation is unverified')


def v2_network_policy(config):
    """Restrict v2 Pods to the configured, internal Egress gateway."""
    import ipaddress
    from urllib.parse import urlsplit
    endpoint = urlsplit(config['proxy_endpoint'])
    try:
        address = ipaddress.ip_address(endpoint.hostname)
        port = endpoint.port
    except (TypeError, ValueError):
        raise ValueError('Egress gateway must be an IP origin with a valid port') from None
    if (endpoint.scheme not in ('http', 'https') or not port or endpoint.username or endpoint.password
            or endpoint.path not in ('', '/') or endpoint.query or endpoint.fragment
            or not address.is_private or address.is_link_local or address.is_loopback
            or address.is_multicast or address.is_unspecified):
        raise ValueError('Egress gateway must be a private non-link-local IP origin')
    namespace, selector = config.get('proxy_namespace'), config.get('proxy_selector')
    if not isinstance(namespace, str) or not namespace or not isinstance(selector, dict) or not selector:
        raise ValueError('Egress gateway namespace and Pod selector are required')
    if any(not isinstance(key, str) or not isinstance(value, str) for key, value in selector.items()):
        raise ValueError('invalid Egress gateway Pod selector')
    return {'apiVersion': 'networking.k8s.io/v1', 'kind': 'NetworkPolicy',
            'metadata': {'name': 'aidash-v2-egress', 'namespace': config['namespace']},
            'spec': {'podSelector': {'matchLabels': {'aidash-sandbox': 'true', 'aidash-v2': 'true'}},
                     'policyTypes': ['Ingress', 'Egress'], 'ingress': [],
                     'egress': [{'to': [{'namespaceSelector': {'matchLabels': {
                         'kubernetes.io/metadata.name': namespace}}, 'podSelector': {'matchLabels': selector}}],
                         'ports': [{'protocol': 'TCP', 'port': port}]}]}}


def network_canary_manifests(config, node, nonce):
    """Create a disposable listener with permissive ingress as a negative control."""
    import re
    if not isinstance(nonce, str) or not re.fullmatch('[0-9a-f]{32}', nonce):
        raise ValueError('invalid network canary nonce')
    if not isinstance(node, str) or not node:
        raise ValueError('network canary requires a node')
    name = 'admission-' + nonce
    labels = {'aidash-canary': nonce}
    metadata = {'name': name, 'namespace': config['namespace'], 'labels': labels}
    security = {'runAsUser': 10000, 'runAsNonRoot': True, 'allowPrivilegeEscalation': False,
                'readOnlyRootFilesystem': True, 'capabilities': {'drop': ['ALL']},
                'seccompProfile': {'type': 'RuntimeDefault'}}
    pod = {'apiVersion': 'v1', 'kind': 'Pod', 'metadata': metadata,
           'spec': {'nodeName': node, 'restartPolicy': 'Never', 'automountServiceAccountToken': False,
                    'enableServiceLinks': False, 'hostNetwork': False, 'hostPID': False, 'hostIPC': False,
                    'terminationGracePeriodSeconds': 1,
                    'containers': [{'name': 'canary', 'image': config['helper_image'],
                                    'command': ['python3', '-I', '/opt/aidash/network_canary.py',
                                                '18080', '18081', nonce],
                                    'securityContext': security,
                                    'readinessProbe': {'httpGet': {'path': '/ready', 'port': 18080},
                                                       'periodSeconds': 1, 'timeoutSeconds': 1},
                                    'resources': {'limits': {'cpu': '250m', 'memory': '128Mi'}}}]}}
    policy = {'apiVersion': 'networking.k8s.io/v1', 'kind': 'NetworkPolicy',
              'metadata': {'name': name, 'namespace': config['namespace']},
              'spec': {'podSelector': {'matchLabels': labels}, 'policyTypes': ['Ingress', 'Egress'],
                       'ingress': [{'ports': [{'protocol': 'TCP', 'port': 18080},
                                              {'protocol': 'UDP', 'port': 18081}]}], 'egress': [{}]}}
    return policy, pod


def verify_positive_controls(report):
    """Refuse negative evidence unless each target was independently reachable."""
    missing = [name for name in ('peer', 'node', 'metadata', 'internet', 'gateway')
               if report.get(name) is not True]
    if missing:
        raise ValueError('network positive control unavailable: ' + ', '.join(missing))


def _selector_matches(selector, labels):
    for key, value in selector.get('matchLabels', {}).items():
        if labels.get(key) != value:
            return False
    for expression in selector.get('matchExpressions', []):
        key, operator = expression['key'], expression['operator']
        values = expression.get('values', [])
        if operator == 'In' and (key not in labels or labels[key] not in values):
            return False
        if operator == 'NotIn' and key in labels and labels[key] in values:
            return False
        if operator == 'Exists' and key not in labels:
            return False
        if operator == 'DoesNotExist' and key in labels:
            return False
        if operator not in ('In', 'NotIn', 'Exists', 'DoesNotExist'):
            raise ValueError('unknown network policy selector operator')
    return True


def verify_network_policies(policies, labels, expected):
    """Reject additive policies that widen v2 ingress or Egress gateway access."""
    allowed = expected['spec']['egress'][0]
    allowed_peer = allowed['to'][0]
    allowed_port = allowed['ports'][0]['port']
    found = False
    for policy in policies:
        spec = policy['spec']
        if not _selector_matches(spec.get('podSelector', {}), labels):
            continue
        types = spec.get('policyTypes')
        ingress = types is None or 'Ingress' in types
        egress = ('Egress' in types) if types is not None else 'egress' in spec
        if ingress and spec.get('ingress'):
            raise ValueError('network policy admits unexpected ingress')
        if not egress:
            continue
        for rule in spec.get('egress', []):
            peers, ports = rule.get('to'), rule.get('ports')
            if not peers or not ports or any(peer != allowed_peer for peer in peers):
                raise ValueError('network policy admits an unexpected destination')
            for port in ports:
                if port.get('protocol', 'TCP') != 'TCP' or port.get('port') != allowed_port or 'endPort' in port:
                    raise ValueError('network policy admits an unexpected port')
            found = True
    if not found:
        raise ValueError('network policy does not admit the Egress gateway')
