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
    processes = config.get('processes', 128)
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
    podspec = {'runtimeClassName': config['docker_runtime_class'] if docker else config['runtime_class'],
               'restartPolicy': 'Never',
               'automountServiceAccountToken': False, 'enableServiceLinks': False,
               'hostNetwork': False, 'hostPID': False, 'hostIPC': False,
               'resources': {'limits': limits},
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
                                '/.aidash-helpers/python/bin/python3',
                                '/.aidash-helpers/sandbox_v2.py', 'init'],
                    'env': [{'name': 'PYTHONHOME', 'value': '/.aidash-helpers/python'},
                            {'name': 'AIDASH_RESET_SYSTEM', 'value': str(bool(settings.get('_reset_system'))).lower()}],
                    'securityContext': init_security, 'resources': {'limits': limits},
                    'volumeMounts': mounts[:2]},
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
        self.lock = RLock()
        self.queues = {}
        self.futures = {}
        self.live_output = {}
        self.foreground_jobs = {}
        self.viewers = {}
        self.admit_locks = {}
        self.session_executor = ThreadPoolExecutor(max_workers=128, thread_name_prefix='python-session')

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
                    self.backend.guard_execution_pod(pod, settings, container_name, container_id, epoch)
                    if container_name == 'helpers':
                        probe = self.backend.kube(['exec', name, '-c', 'helpers', '--', 'python3', '-c',
                            'import json,resource;print(json.dumps(resource.getrlimit(resource.RLIMIT_NPROC)))'])
                        wanted = self.config.get('processes', 128)
                        if json.loads(probe.stdout) != [wanted, wanted]:
                            raise ExecutionPodError('admission_failed', 'Guest process ceiling does not match the profile')
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
        self.backend.guard_execution_pod(pod, settings, 'execution', container_id, epoch)
        probe = '''import json,os,resource,socket
fields=dict(line.split(":",1) for line in open("/proc/self/status") if ":" in line)
try:
 s=socket.socket(socket.AF_INET,socket.SOCK_RAW,socket.IPPROTO_ICMP);s.close();raw=True
except OSError: raw=False
print(json.dumps({"uid":os.getuid(),"kernel":os.uname().release,"no_new_privs":int(fields["NoNewPrivs"]),
 "caps":{k:int(fields[k],16) for k in ("CapInh","CapPrm","CapEff","CapBnd","CapAmb")},
 "process_limit":resource.getrlimit(resource.RLIMIT_NPROC),"raw_socket":raw,
 "service_account_absent":not os.path.exists("/var/run/secrets/kubernetes.io/serviceaccount/token")}))'''
        python = ['/.aidash-helpers/loader', '--library-path',
                  '/.aidash-helpers/lib:/.aidash-helpers/python/lib',
                  '/.aidash-helpers/python/bin/python3', '-I', '-c', probe]
        mode = 'docker' if settings['docker_enabled'] else 'normal'
        result = self.backend.kube(['exec', name, '-c', 'execution', '--'] +
            helper_command('chroot', mode, '/scratch-system/rootfs', '.', *python))
        try:
            verify_user_probe(json.loads(result.stdout), docker=settings['docker_enabled'],
                              processes=self.config.get('processes', 128))
        except (ValueError, KeyError, TypeError) as error:
            raise ExecutionPodError('admission_failed', 'User isolation probe failed: ' + str(error)) from error

    def ensure(self, settings):
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
                pod['metadata']['annotations']['aidash.run/epoch'] = str(self.reserve_epoch(settings['scratch_space_id']))
                self.backend.kube(['create', '-f', '-'], json.dumps(pod).encode())
                self.touch_space(settings['scratch_space_id'])
            elif existing_pod.get('metadata', {}).get('annotations', {}).get('aidash.run/spec-digest') != fingerprint:
                raise ExecutionPodError('settings_mismatch', 'Existing Execution Pod has different settings')
        try:
            self._admit_pod(name, settings)
            self.backend.kube(['wait', '--for=condition=Ready', 'pod/' + name, '--timeout=120s'], timeout=130)
            self._verify_running_pod(name, settings)
        except ExecutionPodError:
            raise
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

    def _stop_matching_pod(self, scratch_space_id, pod_uid):
        from control import scratch_resource_name
        name = scratch_resource_name(scratch_space_id)
        if not isinstance(pod_uid, str) or not pod_uid:
            raise ExecutionPodError('pod_identity_missing', 'Cannot verify the original Pod identity')
        with self.lock:
            current = self._get('pod', name)
            if current and current.get('metadata', {}).get('uid') == pod_uid:
                self.backend.kube(['delete', 'pod', name, '--ignore-not-found', '--wait=true'])

    def delete(self, scratch_space_id):
        from control import scratch_resource_name
        name = scratch_resource_name(scratch_space_id)
        with self.lock:
            self.backend.kube(['delete', 'pod', name, '--ignore-not-found', '--wait=true'])
            self.backend.kube(['delete', 'pvc', name, '--ignore-not-found', '--wait=true'])
        return {'deleted': True}

    def recover_commands(self):
        """Retire incomplete executions without ever replaying their commands."""
        import json
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
            self._stop_matching_pod(scratch_id, pod_uid)
        for path, record in records:
            if record['status'] == 'running':
                record.update(status='interrupted', outcome='unknown')
            else:
                record.update(status='cancelled', outcome='not_started')
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
        from control import save
        with self.lock:
            record = self._read_command(operation_id)
            record.update(fields)
            save(self._command_path(operation_id), record)
            return record

    def command_result(self, operation_id):
        import base64
        import json
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
        if not isinstance(run_id, str) or not run_id or len(run_id) > 256:
            raise ValueError('run_id must be a nonempty string of at most 256 characters')
        code_limit = 131072 if kind == 'python_cell' else 65536
        if not isinstance(code, str) or len(code.encode('utf-8')) > code_limit:
            raise ValueError('command exceeds the code limit')
        cwd = request.get('cwd', '.')
        if not isinstance(cwd, str) or cwd.startswith('/') or '..' in cwd.split('/'):
            raise ValueError('cwd must be relative to /scratch')
        background = request.get('background', False)
        if type(background) is not bool:
            raise ValueError('background must be a boolean')
        seconds = execution_timeout(request)
        digest = hashlib.sha256(json.dumps(request, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
        with self.lock:
            if path.exists():
                prior = self._read_command(operation_id)
                if prior.get('request_digest') != digest:
                    raise ExecutionPodError('command_conflict', 'Operation ID was already used for another command')
                return self.command_result(operation_id)
            pod = self._get('pod', name)
            if not pod or pod.get('status', {}).get('phase') != 'Running':
                raise ExecutionPodError('pod_unavailable', 'Execution Pod is not running')
            uid = pod.get('metadata', {}).get('uid')
            if not isinstance(uid, str) or not uid:
                raise ExecutionPodError('pod_unavailable', 'Execution Pod UID is unavailable')
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
            if kind == 'python_session':
                future = self.session_executor.submit(self._execute_command, operation_id)
            else:
                if scratch_id not in self.queues:
                    self.queues[scratch_id] = CommandQueue()
                queue = self.queues[scratch_id]
                future = queue.submit(run_id, operation_id, self._execute_command, background=background)
            self.futures[operation_id] = future
            future.add_done_callback(lambda done: self._cancelled_command(operation_id, done))
            return self.command_result(operation_id)

    def _cancelled_command(self, operation_id, future):
        if future.cancelled():
            self._update_command(operation_id, status='cancelled', outcome='not_started')
        with self.lock:
            record = self._read_command(operation_id)
            if not record['background']:
                scratch_id = record['scratch_space_id']
                self.foreground_jobs[scratch_id] = max(0, self.foreground_jobs.get(scratch_id, 0) - 1)
            self.futures.pop(operation_id, None)

    def _execute_command(self, operation_id):
        import json
        record = self._update_command(operation_id, status='running')
        name, uid = record['pod'], record['pod_uid']
        mode = 'docker' if record['docker_enabled'] else 'normal'
        deadline = 'background' if record['background'] else str(record['timeout_seconds'])
        kind = 'session' if record.get('kind') == 'python_session' else 'command'
        argv = self.backend.command + ['exec', name, '-c', 'execution', '--'] + helper_command(
            'supervise', mode, kind, operation_id, record['run_id'], record['cwd'], deadline, uid, record['code'])

        def publish(output):
            with self.lock:
                self.live_output[operation_id] = output

        try:
            limit = self.config.get('output_bytes', getattr(self.backend, 'config', {}).get('output_bytes', 8388608))
            transport_timeout = None if record['background'] else record['timeout_seconds'] + 180
            transport = drain_process(argv, self.backend.environment, limit, transport_timeout, publish)
            observed = self.backend.kube(['exec', name, '-c', 'execution', '--'] +
                                         helper_command('result', operation_id), check=False)
            outcome = json.loads(observed.stdout) if observed.returncode == 0 else None
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
                                     **({'exit_code': outcome['exit_code']} if 'exit_code' in outcome else {}),
                                     **transport['output'])
        except Exception as error:
            try:
                self._stop_matching_pod(record['scratch_space_id'], uid)
            except Exception:
                pass
            self._update_command(operation_id, status='interrupted', outcome='unknown',
                                 error_code='command_interrupted', message=str(error) or type(error).__name__)
        finally:
            with self.lock:
                self.live_output.pop(operation_id, None)

    def cancel_run(self, run_id):
        if not isinstance(run_id, str) or not run_id:
            raise ValueError('run_id must be a nonempty string')
        with self.lock:
            queues = tuple(self.queues.values())
        for queue in queues:
            queue.cancel_waiting(run_id)
        return {'cancelled_waiting': True}

    def background_processes(self, scratch_space_id):
        import json
        from control import scratch_resource_name

        scratch_resource_name(scratch_space_id)
        directory = self.backend.root / 'execution-v2'
        if not directory.exists():
            return {'processes': []}
        result = []
        for path in directory.glob('*.json'):
            record = json.loads(path.read_bytes())
            if (record.get('scratch_space_id') == scratch_space_id and record.get('background')
                    and record.get('kind') != 'python_session'):
                result.append(self.command_result(record['operation_id']))
        return {'processes': sorted(result, key=lambda item: item['operation_id'])}

    def stop_background(self, operation_id):
        record = self._read_command(operation_id)
        if not record.get('background'):
            raise ExecutionPodError('not_background', 'Command is not a background process')
        with self.lock:
            future = self.futures.get(operation_id)
        if record['status'] == 'queued' and future and future.cancel():
            return self.command_result(operation_id)
        if record['status'] in ('completed', 'timed_out', 'stopped', 'cancelled', 'interrupted'):
            return self.command_result(operation_id)
        pod = self._get('pod', record['pod'])
        if not pod or pod.get('metadata', {}).get('uid') != record['pod_uid']:
            raise ExecutionPodError('pod_interrupted', 'The original Execution Pod is unavailable')
        try:
            self.backend.kube(['exec', record['pod'], '-c', 'execution', '--'] + helper_command('stop', operation_id))
        except Exception as error:
            raise ExecutionPodError('stop_failed', 'Could not stop background process: ' + str(error)) from error
        return {'operation_id': operation_id, 'status': 'stopping'}

    def reset(self, settings):
        if not self.admitted:
            raise ExecutionPodError('admission_unverified', 'Execution Pod admission has not been verified')
        with self.lock:
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
            space = json.loads(path.read_bytes())
            scratch_id = space['scratch_space_id']
            with self.lock:
                # Re-read under the same lock used by command acceptance so an
                # accepted command cannot race an idle deletion.
                space = json.loads(path.read_bytes())
                if current - space['last_command_at'] < 1800 or self.foreground_jobs.get(scratch_id, 0):
                    continue
                viewers = self.viewers.get(scratch_id, {})
                self.viewers[scratch_id] = {key: expiry for key, expiry in viewers.items() if expiry > current}
                if self.viewers[scratch_id]:
                    continue
                pod = self._get('pod', scratch_resource_name(scratch_id))
                if pod:
                    self._stop_matching_pod(scratch_id, pod.get('metadata', {}).get('uid'))

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
        import json
        import shlex
        import uuid
        from control import save, scratch_resource_name

        if not self.admitted:
            raise ExecutionPodError('admission_unverified', 'Execution Pod admission has not been verified')
        path = self._python_session_path(scratch_space_id, run_id)
        with self.lock:
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
        if not isinstance(code, str) or len(code.encode('utf-8')) > 65536:
            raise ValueError('Python code exceeds the limit')
        seconds = execution_timeout(request)
        session = self.start_python(request['scratch_space_id'], request['run_id'])
        output_limit = self.config.get('output_bytes', getattr(self.backend, 'config', {}).get('output_bytes', 8388608))
        cell_limit = max(1, min(8388608, (output_limit - 4096) // 6))
        encoded = base64.b64encode(code.encode('utf-8')).decode('ascii')
        command = shlex.join(python_helper_command('cell', session['socket_path'], str(seconds), str(cell_limit), encoded))
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


def drain_process(argv, environment, output_limit, timeout, on_output=None):
    """Drain a trusted transport without buffering unbounded command output."""
    import os
    import selectors
    import signal
    import subprocess
    import time

    output = BoundedOutput(output_limit)
    process = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, env=environment, start_new_session=True)
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
    return {'exit_code': exit_code, 'transport_timed_out': timed_out, 'output': output.public()}


def verify_user_probe(report, *, docker, processes):
    """Fail closed unless a real user child has the expected isolation profile."""
    if report.get('uid') != 0 or 'gvisor' not in str(report.get('kernel', '')).lower():
        raise ValueError('gVisor root user required')
    if report.get('no_new_privs') != 1 or report.get('process_limit') != [processes, processes]:
        raise ValueError('user process restrictions do not match the profile')
    if report.get('service_account_absent') is not True:
        raise ValueError('service account token is exposed')
    caps = report.get('caps', {})
    names = ('CapInh', 'CapPrm', 'CapEff', 'CapBnd', 'CapAmb')
    if any(type(caps.get(name)) is not int for name in names):
        raise ValueError('capability evidence is incomplete')
    blocked = (1 << 12) | (1 << 13) | (1 << 21)
    if docker:
        if any(caps[name] & blocked != blocked for name in ('CapPrm', 'CapEff', 'CapBnd')):
            raise ValueError('Docker capabilities are unavailable')
        if report.get('raw_socket') is not True:
            raise ValueError('Docker raw sockets are unavailable')
    elif any(caps[name] & blocked for name in names) or report.get('raw_socket') is not False:
        raise ValueError('Docker capabilities remain available to the user process')
