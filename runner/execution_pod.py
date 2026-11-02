"""Scheduling primitives for commands sharing an Execution Pod."""
from concurrent.futures import ThreadPoolExecutor
from threading import Lock


class CommandQueue:
    """Bound active commands and allow a Run's queued foreground work to stop."""

    def __init__(self):
        self._executor = ThreadPoolExecutor(max_workers=8, thread_name_prefix='execution-pod')
        self._lock = Lock()
        self._waiting = {}

    def submit(self, run_id, command, execute, *, background=False):
        if not isinstance(run_id, str) or not run_id:
            raise ValueError('run_id must be a nonempty string')
        with self._lock:
            future = self._executor.submit(execute, command)
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

    def close(self):
        self._executor.shutdown(wait=True, cancel_futures=True)


def system_tree_index(root):
    """Index the image tree without entering runtime mounts or following links."""
    import os
    from pathlib import Path
    import stat

    root = Path(root)
    index = {}
    excluded = {'scratch', 'proc', 'dev', 'tmp', 'sys'}
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
    if root.is_symlink() or backup.is_symlink():
        raise ValueError('system root must not be a symlink')
    if not root.exists() and backup.exists():
        os.replace(backup, root)
    if root.exists() and not reset:
        return False
    staging = store / ('rootfs.' + uuid.uuid4().hex + '.new')
    excluded = {'scratch', 'scratch-system', 'proc', 'sys', 'dev', 'tmp', '.aidash-helpers'}
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
    docker = settings['docker_enabled']
    if type(docker) is not bool:
        raise ValueError('docker_enabled must be a boolean')
    if docker and not config.get('allow_docker', False):
        raise ValueError('Docker execution is not permitted by the Deployment')
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
    init_security = {'runAsUser': 0, 'allowPrivilegeEscalation': False,
                     'capabilities': {'drop': ['ALL'],
                                      'add': ['CHOWN', 'DAC_OVERRIDE', 'FOWNER', 'FSETID']},
                     'seccompProfile': {'type': 'RuntimeDefault'}}
    volumes = [{'name': 'scratch', 'persistentVolumeClaim': {'claimName': name}},
               {'name': 'helpers', 'emptyDir': {}}]
    mounts = [{'name': 'scratch', 'mountPath': '/scratch-system'},
              {'name': 'helpers', 'mountPath': '/.aidash-helpers', 'readOnly': True}]
    if docker:
        volumes.append({'name': 'docker', 'emptyDir': {'medium': 'Memory'}})
        mounts.append({'name': 'docker', 'mountPath': '/docker-storage'})
    limits = {'cpu': str(settings['cpu']), 'memory': str(settings['memory_bytes'])}
    podspec = {'runtimeClassName': config['runtime_class'], 'restartPolicy': 'Never',
               'automountServiceAccountToken': False, 'enableServiceLinks': False,
               'hostNetwork': False, 'hostPID': False, 'hostIPC': False,
               'initContainers': [
                   {'name': 'helpers', 'image': config['helper_image'],
                    'command': ['python3', '-I', '/opt/aidash/prepare_helpers.py'],
                    'securityContext': init_security, 'resources': {'limits': limits},
                    'volumeMounts': [{'name': 'helpers', 'mountPath': '/helpers'},
                                     {'name': 'scratch', 'mountPath': '/scratch-system'}]},
                   {'name': 'image-root', 'image': settings['image'],
                    'command': ['/.aidash-helpers/loader', '--library-path',
                                '/.aidash-helpers/lib:/.aidash-helpers/python/lib',
                                '/.aidash-helpers/python/bin/python3',
                                '/.aidash-helpers/sandbox_v2.py', 'init'],
                    'env': [{'name': 'PYTHONHOME', 'value': '/.aidash-helpers/python'},
                            {'name': 'AIDASH_RESET_SYSTEM', 'value': str(bool(settings.get('_reset_system'))).lower()}],
                    'securityContext': init_security, 'resources': {'limits': limits},
                    'volumeMounts': mounts[:2]},
               ],
               'containers': [{'name': 'execution', 'image': config['helper_image'],
                               'command': ['python3', '-I', '/opt/aidash/sandbox_v2.py', 'bootstrap'],
                               'env': [{'name': 'AIDASH_DOCKER_ENABLED', 'value': str(docker).lower()},
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
           labels={'aidash-sandbox': 'true'}), 'spec': podspec}
    return claim, pod


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
        self.lock = Lock()

    def _get(self, kind, name):
        import json
        result = self.backend.kube(['get', kind, name, '-o', 'json', '--ignore-not-found'])
        return json.loads(result.stdout) if result.stdout.strip() else None

    def ensure(self, settings):
        import hashlib
        import json

        if not self.admitted:
            raise ExecutionPodError('admission_unverified', 'Execution Pod admission has not been verified')
        claim, pod = execution_manifests(settings, self.config)
        name = pod['metadata']['name']
        # Attach an immutable digest of the desired manifest. Existing Pods
        # with different settings must never be silently reused.
        fingerprint = hashlib.sha256(json.dumps(pod['spec'], sort_keys=True, separators=(',', ':')).encode()).hexdigest()
        pod['metadata']['annotations'] = {'aidash.run/spec-digest': fingerprint}
        with self.lock:
            existing_claim = self._get('pvc', name)
            if existing_claim is None:
                self.backend.kube(['create', '-f', '-'], json.dumps(claim).encode())
            else:
                current = existing_claim['spec']['resources']['requests']['storage']
                if current != claim['spec']['resources']['requests']['storage']:
                    raise ExecutionPodError('capacity_mismatch', 'Scratch Space capacity differs from the existing claim')
            existing_pod = self._get('pod', name)
            if existing_pod is None:
                self.backend.kube(['create', '-f', '-'], json.dumps(pod).encode())
            elif existing_pod.get('metadata', {}).get('annotations', {}).get('aidash.run/spec-digest') != fingerprint:
                raise ExecutionPodError('settings_mismatch', 'Existing Execution Pod has different settings')
        try:
            self.backend.kube(['wait', '--for=condition=Ready', 'pod/' + name, '--timeout=120s'], timeout=130)
        except Exception as error:
            detail = str(error).strip() or type(error).__name__
            raise ExecutionPodError('startup_failed', 'Execution Pod did not become ready: ' + detail[-2000:]) from error
        return {'protocol': 'aidash-runner/2', 'pod': name, 'claim': name}

    def stop(self, scratch_space_id):
        from control import scratch_resource_name
        name = scratch_resource_name(scratch_space_id)
        with self.lock:
            self.backend.kube(['delete', 'pod', name, '--ignore-not-found', '--wait=true'])
        return {'stopped': True}

    def delete(self, scratch_space_id):
        from control import scratch_resource_name
        name = scratch_resource_name(scratch_space_id)
        with self.lock:
            self.backend.kube(['delete', 'pod', name, '--ignore-not-found', '--wait=true'])
            self.backend.kube(['delete', 'pvc', name, '--ignore-not-found', '--wait=true'])
        return {'deleted': True}


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
