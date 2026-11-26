"""Scheduling primitives for commands sharing an Execution Pod."""
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from threading import Lock, RLock

STOP_RETRY_SECONDS = 15 * 60
LATE_RESULT_SECONDS = 24 * 60 * 60
OUTPUT_RETRY_SECONDS = 60
# Late result observation lasts 24h; leave one 15-minute stop retry window as
# margin. RunnerHttp's v2 command calls time out after 30s and its execution_pod
# adapter does not retry requests itself (crates/aidash-integrations/src/runner).
JOURNAL_RETENTION_SECONDS = max(LATE_RESULT_SECONDS, STOP_RETRY_SECONDS) + STOP_RETRY_SECONDS
# RunnerHttp allows up to 15 minutes for a session start. Keep a full day
# after a stop for requests already dispatched, including across restarts.
SESSION_FENCE_RETENTION_SECONDS = 24 * 60 * 60
REFUSAL_RETENTION_SECONDS = 7 * 86400
JOURNAL_MAX_RECORDS_PER_SPACE = 4096
# How long a Python interpreter may take to show a terminal result once its stop was accepted. After the
# stop signal the in-Pod supervisor (sandbox_v2.run_supervised) kills and reaps the interpreter's processes
# within 5 s, joins its output readers for FORWARD_GRACE_SECONDS (5 s), drains a reader that is still held
# open for up to 30 s more, and gives up on each of the two readers after 0.5 s: 5 + 5 + 30 + 1 = 41 s.
# A session records no system changes (record_changes is off for kind `session`), so no tree scan adds to
# that. The other 19 s are for the 5 s kubectl exec of each result read and for scheduling.
PYTHON_STOP_SECONDS = 60
RUN_SESSION_KINDS = frozenset(("python_session", "python_cell", "mcp_session"))


def helper_command(*args):
    """Use the injected read-only loader, interpreter, and helper script."""
    return ['/.aidash-helpers/loader', '--library-path',
            '/.aidash-helpers/lib:/.aidash-helpers/python/lib',
            '/.aidash-helpers/python/bin/python3', '-I', '/.aidash-helpers/sandbox_v2.py', *args]


def python_helper_command(*args):
    return ['/.aidash-helpers/loader', '--library-path',
            '/.aidash-helpers/lib:/.aidash-helpers/python/lib',
            '/.aidash-helpers/python/bin/python3', '-I', '/.aidash-helpers/python_session.py', *args]


def mcp_helper_command(*args):
    return ['/.aidash-helpers/loader', '--library-path',
            '/.aidash-helpers/lib:/.aidash-helpers/python/lib',
            '/.aidash-helpers/python/bin/python3', '-I', '/.aidash-helpers/mcp_session.py', *args]


def verify_adopted_result(result, key):
    import hashlib
    import hmac
    import json
    if not isinstance(result, dict) or not isinstance(key, bytes) or not isinstance(result.get('auth'), str):
        return False
    payload = {name: value for name, value in result.items() if name != 'auth'}
    data = json.dumps(payload, sort_keys=True, separators=(',', ':')).encode()
    return hmac.compare_digest(result['auth'], hmac.new(key, data, hashlib.sha256).hexdigest())


class CommandQueue:
    """Bound active commands and allow a Run's queued foreground work to stop."""

    def __init__(self, command_slots=8):
        self._executor = ThreadPoolExecutor(max_workers=command_slots, thread_name_prefix='execution-pod')
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


def initialize_system_root(image_root, store, *, reset=False, image_identity=None):
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
    # Never leave an old identity attached to a root that may be replaced.
    (store / 'image-identity.json').unlink(missing_ok=True)
    directory = os.open(store, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)
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
        if image_identity is not None:
            import json
            marker_tmp = store / ('image-identity.' + uuid.uuid4().hex + '.new')
            try:
                with marker_tmp.open('x', encoding='utf-8') as file:
                    json.dump({'image': image_identity}, file)
                    file.flush()
                    os.fsync(file.fileno())
                os.replace(marker_tmp, store / 'image-identity.json')
                directory = os.open(store, os.O_RDONLY | os.O_DIRECTORY)
                try:
                    os.fsync(directory)
                finally:
                    os.close(directory)
            finally:
                marker_tmp.unlink(missing_ok=True)
    finally:
        if staging.exists():
            shutil.rmtree(staging)
    return True


def system_image_changed(store, requested_image):
    import json
    import os
    from pathlib import Path
    try:
        fd = os.open(Path(store) / 'image-identity.json', os.O_RDONLY | os.O_NOFOLLOW)
    except FileNotFoundError:
        # Legacy roots have no trustworthy identity; require an explicit reset.
        return True
    with os.fdopen(fd, 'rb') as file:
        marker = json.load(file)
    return marker.get('image') != requested_image


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


def pod_proxy_url(endpoint, credential):
    """The proxy address a Pod uses, carrying its credential to the Egress gateway."""
    import re
    from urllib.parse import urlsplit, urlunsplit
    if credential is None:
        return endpoint
    if not isinstance(credential, str) or not re.fullmatch(r'[A-Za-z0-9._-]{1,512}', credential):
        raise ValueError('invalid egress credential')
    parts = urlsplit(endpoint)
    return urlunsplit(parts._replace(netloc=f'aidash:{credential}@{parts.netloc}'))


def v2_process_limit(settings, config):
    # The sentry shares the Pod's memory; bound the default to 128 per 2 GiB.
    return config.get('v2_processes', max(32, min(128, settings['memory_bytes'] // (16 << 20))))


def execution_manifests(settings, config):
    """Build the durable claim and sandbox Pod for one Scratch Space."""
    import hashlib
    import re
    from urllib.parse import urlsplit
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
    command_slots = settings.get('command_slots', 8)
    if type(command_slots) is not int or not 1 <= command_slots <= config.get('max_command_slots', 32):
        raise ValueError('command_slots exceeds the Deployment ceiling')
    processes = v2_process_limit(settings, config)
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
                           [{'name': 'AIDASH_RESET_SYSTEM', 'value': str(bool(settings.get('_reset_system'))).lower()},
                            {'name': 'AIDASH_IMAGE_ID', 'value': settings['image']}],
                    'securityContext': image_security, 'resources': {'limits': limits},
                    'volumeMounts': [{'name': 'scratch', 'mountPath': '/scratch-image', 'subPath': 'image-root'},
                                     {'name': 'helpers', 'mountPath': '/.aidash-helpers', 'readOnly': True}]},
               ],
               'containers': [{'name': 'execution', 'image': config['helper_image'],
                               'command': helper_command('bootstrap'),
                               'readinessProbe': {'exec': {'command': helper_command('ready')},
                                                  'periodSeconds': 2, 'timeoutSeconds': 5},
                               'env': [{'name': 'AIDASH_DOCKER_ENABLED', 'value': str(docker).lower()},
                                       {'name': 'AIDASH_PROCESS_LIMIT', 'value': str(processes)},
                                       {'name': 'AIDASH_POD_UID',
                                        'valueFrom': {'fieldRef': {'fieldPath': 'metadata.uid'}}}],
                               'securityContext': security, 'resources': {'limits': limits},
                               'volumeMounts': mounts}], 'volumes': volumes}
    podspec['nodeSelector'] = dict(config.get('node_selector', {}))
    podspec['tolerations'] = list(config.get('tolerations', []))
    if docker:
        tenant = settings.get('tenant_id')
        if not isinstance(tenant, str) or not tenant:
            raise ValueError('tenant_id required for Docker execution')
        tenant_label = 'tenant-' + hashlib.sha256(tenant.encode()).hexdigest()[:48]
        if podspec['nodeSelector'].get('aidash.run/tenant', tenant_label) != tenant_label:
            raise ValueError('sandbox selector conflicts with tenant')
        podspec['nodeSelector']['aidash.run/tenant'] = tenant_label
        podspec['tolerations'].append({'key': 'aidash.run/tenant', 'operator': 'Equal',
                                       'value': tenant_label, 'effect': 'NoSchedule'})
    pod = {'apiVersion': 'v1', 'kind': 'Pod', 'metadata': dict(metadata,
           labels={'aidash-sandbox': 'true', 'aidash-v2': 'true'}), 'spec': podspec}
    if config.get('proxy_endpoint'):
        v2_network_policy(config)  # validate before passing the proxy to user processes
        # Loopback (Live previews) and the gateway itself must not be sent through the proxy.
        bypass = 'localhost,127.0.0.1,::1,' + urlsplit(config['proxy_endpoint']).hostname
        podspec['containers'][0]['env'] += [{'name': key, 'value': bypass} for key in ('NO_PROXY', 'no_proxy')]
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
        self.process_proxies = {}
        self.preview_starts = Counter()
        self.mcp_locks = {}
        self.left_result_keys = {}
        self.live_output = {}
        self.foreground_jobs = {}
        self.viewers = {}
        self.admit_locks = {}
        self.lifecycle_locks = {}
        self.command_stop_locks = [RLock() for _ in range(64)]
        self.helper_checks = set()
        self.index_lock = Lock()
        self.journal_index = None
        self.session_index = None
        self.journal_metadata = {}

    def _ensure_journal_index(self):
        """Rebuild the per-Space index once from the durable journals at startup."""
        import json
        if self.journal_index is not None:
            return
        with self.index_lock:
            if self.journal_index is not None:
                return
            indexes = []
            for directory in ('execution-v2', 'execution-v2-python'):
                index = {}
                root = self.backend.root / directory
                suffixes = ('.json', '.stopped') if directory == 'execution-v2-python' else ('.json',)
                for path in root.iterdir() if root.exists() else ():
                    if path.suffix not in suffixes:
                        continue
                    try:
                        record = json.loads(path.read_bytes())
                        space = record.get('scratch_space_id')
                        if not isinstance(space, str) or not space:
                            # Preview stops can arrive before the Runner knows their Space.
                            if not (directory == 'execution-v2' and space is None
                                    and record.get('kind') == 'preview' and record.get('status') == 'refused'):
                                raise ValueError('invalid journal Space identity')
                        index.setdefault(space, set()).add(path.name)
                        if directory == 'execution-v2':
                            self.journal_metadata[path.name] = self._prune_metadata(record)
                    except (OSError, ValueError, KeyError, TypeError) as error:
                        import sys
                        print('Execution Pod journal index recovery failed: ' + str(error), file=sys.stderr, flush=True)
                indexes.append(index)
            # All new records wait for the rebuild before they are written.
            self.session_index = indexes[1]
            self.journal_index = indexes[0]

    @staticmethod
    def _prune_metadata(record):
        return (record.get('status'), record.get('terminal_at'), record.get('stop_pending'))

    def _space_journal_paths(self, scratch_space_id, *, sessions=False):
        self._ensure_journal_index()
        with self.lock:
            index = self.session_index if sessions else self.journal_index
            names = tuple(index.get(scratch_space_id, ()))
        directory = 'execution-v2-python' if sessions else 'execution-v2'
        return [self.backend.root / directory / name for name in names]

    def prune_space(self, scratch_space_id, *, now=None):
        """Remove only settled records after the retry and late-result window."""
        import json
        import time
        current = time.time() if now is None else now
        with self._space_lock(scratch_space_id):
            expired = []
            paths = self._space_journal_paths(scratch_space_id)
            with self.lock:
                candidates = [(path, self.journal_metadata.get(path.name)) for path in paths]
            for path, metadata in candidates:
                if metadata is None:
                    continue
                status, terminal_at, stop_pending = metadata
                retention = REFUSAL_RETENTION_SECONDS if status == 'refused' else JOURNAL_RETENTION_SECONDS
                if (status not in ('completed', 'timed_out', 'stopped', 'cancelled', 'interrupted', 'refused')
                        or not isinstance(terminal_at, (int, float)) or terminal_at > current - retention
                        or stop_pending):
                    continue
                expired.append((terminal_at, path))
            for _, path in sorted(expired):
                with self.lock:
                    if not path.exists():
                        continue
                    record = json.loads(path.read_bytes())
                    terminal_at = record.get('terminal_at')
                    retention = REFUSAL_RETENTION_SECONDS if record.get('status') == 'refused' else JOURNAL_RETENTION_SECONDS
                    if (record.get('status') not in ('completed', 'timed_out', 'stopped', 'cancelled', 'interrupted', 'refused')
                            or not isinstance(terminal_at, (int, float)) or terminal_at > current - retention
                            or record.get('stop_pending') or record.get('operation_id') in self.futures
                            or record.get('operation_id') in self.preview_starts):
                        continue
                    # A Python session pointer cannot outlive the record it names.
                    run = record.get('run_id')
                    if record.get('kind') == 'python_session' and run:
                        session_path = self._python_session_path(scratch_space_id, run)
                        if session_path.exists() and json.loads(session_path.read_bytes()).get('operation_id') == record['operation_id']:
                            session_path.unlink()
                            self.session_index.get(scratch_space_id, set()).discard(session_path.name)
                    if record.get('background') and record.get('pod'):
                        try:
                            pod = self._get('pod', record['pod'])
                            if pod and pod.get('metadata', {}).get('uid') == record.get('pod_uid'):
                                self.backend.kube(['exec', record['pod'], '-c', 'execution', '--'] +
                                                  helper_command('forget', record['operation_id']), check=False, timeout=5)
                        except Exception:
                            continue
                    path.unlink()
                    self.journal_index.get(scratch_space_id, set()).discard(path.name)
                    self.journal_metadata.pop(path.name, None)
            for path in self._space_journal_paths(scratch_space_id, sessions=True):
                if path.suffix == '.stopped' and path.stat().st_mtime <= current - SESSION_FENCE_RETENTION_SECONDS:
                    path.unlink()
                    with self.lock:
                        self.session_index[scratch_space_id].discard(path.name)

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
                              processes=v2_process_limit(settings, self.config))
        except (ValueError, KeyError, TypeError) as error:
            raise ExecutionPodError('admission_failed', 'User isolation probe failed: ' + str(error)) from error
        self._record_guard(pod, 'execution', container_id, epoch, host_evidence=evidence, user_evidence=report)
        self._verify_network_pod(pod, settings, container_id, epoch)
        return pod['metadata']['uid']

    def _verify_ingress_pod(self, pod, canary_name):
        """Prove a live listener is unreachable from another Pod."""
        import ipaddress
        import json
        import select
        import subprocess
        address = str(ipaddress.IPv4Address(pod['status']['podIP']))
        name = pod['metadata']['name']
        transport = subprocess.Popen(self.backend.command + ['exec', '-i', name, '-c', 'execution', '--'] +
                                     helper_command('ingress-listen'), stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                     env=self.backend.environment)
        try:
            readable, _, _ = select.select([transport.stdout], [], [], 15)
            if not readable:
                raise ValueError('ingress listener did not start')
            port = json.loads(transport.stdout.readline())['port']
            if type(port) is not int or not 1 <= port <= 65535:
                raise ValueError('ingress listener returned an invalid port')
            probe = ('import json,socket,sys\n'
                     'try:\n s=socket.create_connection((sys.argv[1],int(sys.argv[2])),timeout=2);s.close();ok=True\n'
                     'except OSError:ok=False\n'
                     'print(json.dumps({"connected":ok}))')
            def connect(target, container, ip):
                result = self.backend.kube(['exec', target, '-c', container, '--',
                                            'python3', '-I', '-c', probe, ip, str(port)])
                return json.loads(result.stdout)['connected']
            if connect(name, 'execution', '127.0.0.1') is not True:
                raise ValueError('ingress listener positive control unavailable')
            if connect(canary_name, 'canary', address) is not False:
                raise ValueError('network isolation admits ingress from a peer Pod')
        finally:
            if transport.stdin:
                transport.stdin.close()
            try:
                transport.wait(timeout=5)
            except subprocess.TimeoutExpired:
                transport.kill()
                transport.wait()
            if transport.stdout:
                transport.stdout.close()

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
            verify_positive_controls(json.loads(positive.stdout),
                                     metadata_server_absent=self.config.get('metadata_server_absent', False))
            self._verify_ingress_pod(pod, canary_name)
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

    def sweep_canaries(self):
        import json
        # Remove permissive policies before their listener Pods.
        for kind in ('networkpolicy', 'pod'):
            result = self.backend.kube(['get', kind, '-l', 'aidash.run/v2-canary=true', '-o', 'json'])
            for item in json.loads(result.stdout)['items']:
                name = item['metadata']['name']
                self.backend.kube(['delete', kind, name, '--ignore-not-found', '--wait=true'])

    def _old_helper_image(self, pod):
        containers = pod.get('spec', {}).get('containers', [])
        execution = next((item for item in containers if item.get('name') == 'execution'), None)
        image = self.config.get('helper_image')
        return bool(image and execution and execution.get('image') and execution['image'] != image)

    def _pod_receipt(self, name, requested_image):
        import json
        try:
            result = self.backend.kube(['exec', name, '-c', 'execution', '--'] +
                                       helper_command('image-status', requested_image))
            changed = json.loads(result.stdout)['image_changed']
            if type(changed) is not bool:
                raise ValueError('invalid image status')
        except Exception as error:
            raise ExecutionPodError('image_status_unavailable', 'Could not verify the system image identity') from error
        return {'protocol': 'aidash-runner/2', 'pod': name, 'claim': name, 'image_changed': changed}

    def ensure(self, settings):
        with self._space_lock(settings['scratch_space_id']):
            try:
                return self._ensure(settings)
            except Exception as error:
                credential = settings.get('egress_credential')
                if isinstance(credential, str) and credential and credential in str(error):
                    message = str(error).replace(credential, '[redacted]')
                    if isinstance(error, ExecutionPodError):
                        raise ExecutionPodError(error.code, message) from None
                    raise RuntimeError(message) from None
                raise

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
        pod['metadata']['annotations'] = {
            'aidash.run/spec-digest': fingerprint,
            'aidash.run/command-slots': str(settings.get('command_slots', 8)),
        }
        self._require_space_termination_confirmed(settings['scratch_space_id'])
        existing_claim = self._get('pvc', name)
        if existing_claim is None:
            self.backend.kube(['create', '-f', '-'], json.dumps(claim).encode())
        else:
            current = existing_claim['spec']['resources']['requests']['storage']
            if quantity_bytes(current) != settings['capacity_bytes']:
                raise ExecutionPodError('capacity_mismatch', 'Scratch Space capacity differs from the existing claim')
        existing_pod = self._get('pod', name)
        if existing_pod:
            status = existing_pod.get('status', {})
            execution = next((item for item in status.get('containerStatuses', [])
                              if item.get('name') == 'execution'), {})
            if status.get('phase') in ('Failed', 'Succeeded') or 'terminated' in execution.get('state', {}):
                # Retire the exact guarded writer and interrupt its command journal before replacement.
                self.stop(settings['scratch_space_id'])
                existing_pod = None
        if existing_pod is None:
            self._terminate_recorded_pod(settings['scratch_space_id'])
            pod['metadata']['annotations']['aidash.run/epoch'] = str(self.reserve_epoch(settings['scratch_space_id']))
            self.backend.kube(['create', '-f', '-'], json.dumps(pod).encode())
            self.touch_space(settings['scratch_space_id'])
        elif self._old_helper_image(existing_pod):
            raise ExecutionPodError('pod_updating', 'Execution Pod is updating; retry shortly')
        elif existing_pod.get('metadata', {}).get('annotations', {}).get('aidash.run/spec-digest') != fingerprint:
            old_image = next((item.get('image') for item in existing_pod.get('spec', {}).get('initContainers', [])
                              if item.get('name') == 'image-root'), None)
            if old_image and old_image != settings['image']:
                prior_spec = json.loads(json.dumps(fingerprint_spec))
                prior_spec['initContainers'][1]['image'] = old_image
                for item in prior_spec['initContainers'][1]['env']:
                    if item['name'] == 'AIDASH_IMAGE_ID':
                        item['value'] = old_image
                prior_digest = hashlib.sha256(json.dumps(prior_spec, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
            else:
                prior_digest = None
            if existing_pod.get('metadata', {}).get('annotations', {}).get('aidash.run/spec-digest') != prior_digest:
                raise ExecutionPodError('settings_mismatch', 'Existing Execution Pod has different settings')
        uid = existing_pod.get('metadata', {}).get('uid') if existing_pod else None
        if uid and uid == self._admitted_uid(settings['scratch_space_id']):
            status = existing_pod.get('status', {})
            execution = next((item for item in status.get('containerStatuses', [])
                              if item.get('name') == 'execution'), {})
            if (status.get('phase') != 'Running' or not execution.get('ready') or
                    'running' not in execution.get('state', {})):
                raise ExecutionPodError('pod_unavailable', 'Execution Pod is not running and ready')
            return self._pod_receipt(name, settings['image'])
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
        return self._pod_receipt(name, settings['image'])

    def stop(self, scratch_space_id):
        with self._space_lock(scratch_space_id):
            return self._stop(scratch_space_id)

    def _stop(self, scratch_space_id):
        import json
        import time
        from control import save
        from control import scratch_resource_name
        name = scratch_resource_name(scratch_space_id)
        self._clear_admitted_uid(scratch_space_id)
        self._interrupt_space_commands(scratch_space_id)
        path = self.backend.root / 'execution-v2-spaces' / (name + '.json')
        try:
            prior = json.loads(path.read_bytes()) if path.exists() else {}
            since = prior.get('stop_requested_at')
            if since is not None and time.time() >= since + STOP_RETRY_SECONDS:
                raise ExecutionPodError('termination_unconfirmed', 'Node guard retry window expired')
            self._terminate_recorded_pod(scratch_space_id)
        except Exception:
            self._mark_space_unconfirmed(scratch_space_id)
            state = json.loads(path.read_bytes())
            since = state.setdefault('stop_requested_at', time.time())
            save(path, state)
            if time.time() < since + STOP_RETRY_SECONDS:
                raise
            guard = state.get('guard') or {}
            node = guard.get('cluster_node')
            node_uid = guard.get('node_uid')
            pod = self._get('pod', name)
            registered = self._get('node', node) if node else None
            if not node_uid or (registered or {}).get('metadata', {}).get('uid') != node_uid:
                raise ExecutionPodError('termination_unconfirmed',
                    'The node is absent; a Deployment Administrator must verify the writer stopped') from None
            uid = (pod or {}).get('metadata', {}).get('uid')
            # The delete may have been applied by an earlier attempt whose wait, lookup or reply was lost.
            # Its Pod is then gone, and only this record can bind that deletion to the guarded Pod.
            issued = state.get('normal_delete_pod_uid')
            already_deleted = bool(issued) and pod is None and issued == guard.get('pod_uid')
            if not already_deleted:
                if (not uid or guard.get('pod_uid') != uid
                        or pod.get('spec', {}).get('nodeName') != node):
                    raise ExecutionPodError('termination_unconfirmed', 'The original Pod cannot be bound to a normal deletion') from None
                # Persist which Pod the delete is issued for before issuing it: a reply lost after
                # the delete applied must not leave the Space fenced for good. The record is only
                # intent, so a delete the API server refused takes it back again, unless an earlier
                # attempt for the same Pod left its outcome open.
                unresolved = issued == uid
                state['normal_delete_pod_uid'] = uid
                save(path, state)
                endpoint = '/api/v1/namespaces/' + self.config['namespace'] + '/pods/' + name
                try:
                    self.backend.kube(['delete', '--raw', endpoint, '-f', '-'],
                                      json.dumps({'preconditions':{'uid':uid}}).encode(), timeout=70)
                except Exception as error:
                    if not unresolved and self._delete_refused(error):
                        state.pop('normal_delete_pod_uid', None)
                        save(path, state)
                    raise
                try:
                    self.backend.kube(['wait', '--for=delete', 'pod/' + name, '--timeout=60s'], timeout=70)
                except Exception:
                    if self._get('pod', name) is not None:
                        raise
            # A completed normal delete proves kubelet stopped the Pod only while its node remains registered.
            registered = self._get('node', node)
            if (registered or {}).get('metadata', {}).get('uid') != node_uid or self._get('pod', name):
                raise ExecutionPodError('termination_unconfirmed', 'Pod deletion or node presence is unconfirmed') from None
            if guard:
                guard['terminated'] = True
                state.setdefault('terminated_pods', []).append(guard['pod_uid'])
            state['termination_unconfirmed'] = False
            state.pop('stop_requested_at', None)
            state.pop('normal_delete_pod_uid', None)
            save(path, state)
        else:
            try:
                self.backend.kube(['delete', 'pod', name, '--ignore-not-found', '--wait=true'])
            except Exception:
                self._mark_space_unconfirmed(scratch_space_id)
                raise
        self._clear_space_unconfirmed(scratch_space_id)
        with self.lock:
            queue = self.queues.pop(scratch_space_id, None)
            self.viewers.pop(scratch_space_id, None)
        if queue:
            queue.close(wait=False)
        return {'stopped': True}

    # Statuses with which the API server rejects a request without acting on it. Any other failure,
    # such as a timeout, a dropped connection or a status like ServerTimeout or ServiceUnavailable,
    # leaves it open whether the request was applied.
    REFUSED_REASONS = frozenset({'Unauthorized', 'Forbidden', 'NotFound', 'Invalid', 'BadRequest', 'Conflict',
                                 'MethodNotAllowed', 'NotAcceptable', 'RequestEntityTooLarge',
                                 'UnsupportedMediaType', 'TooManyRequests'})

    @classmethod
    def _delete_refused(cls, error):
        """Whether a failed kubectl call shows that the API server answered and refused the request."""
        import re
        if not isinstance(error, RuntimeError):
            return False
        # Backend.kube turns a nonzero kubectl exit into RuntimeError(stderr); kubectl prints the API
        # status as `Error from server (<Reason>): <message>`.
        match = re.search(r'(?m)^Error from server \((\w+)\)', str(error))
        return bool(match) and match.group(1) in cls.REFUSED_REASONS

    def _interrupt_space_commands(self, scratch_space_id):
        import json
        import time
        from control import save
        for path in self._space_journal_paths(scratch_space_id):
            try:
                candidate = json.loads(path.read_bytes())
            except FileNotFoundError:
                continue
            if candidate.get('status') not in ('queued', 'running'):
                continue
            with self.lock:
                if not path.exists():
                    continue
                record = json.loads(path.read_bytes())
                if record.get('status') in ('queued', 'running'):
                    prior = record['status']
                    record.update(status='interrupted' if prior == 'running' else 'cancelled',
                                  outcome='unknown' if prior == 'running' else 'not_started', terminal_at=time.time())
                    save(path, record)
                    self.journal_metadata[path.name] = self._prune_metadata(record)
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
        node_name = pod['spec']['nodeName']
        node = self._get('node', node_name)
        node_uid = (node or {}).get('metadata', {}).get('uid')
        if not node_uid:
            raise ExecutionPodError('admission_failed', 'Execution Pod node identity is unavailable')
        with self.lock:
            state = json.loads(path.read_bytes())
            prior = state.get('guard') or {}
            if prior.get('pod_uid') == pod['metadata']['uid'] and prior.get('node_uid') != node_uid:
                raise ExecutionPodError('admission_failed', 'Execution Pod node identity changed')
            state['guard'] = {'pod_name': name, 'pod_uid': pod['metadata']['uid'],
                              'cluster_node': node_name, 'node_uid': node_uid, 'container_name': container_name,
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
                state.pop('stop_requested_at', None)
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
            journal_paths = self._space_journal_paths(scratch_space_id)
            with self.lock:
                path = self.backend.root / 'execution-v2-spaces' / (name + '.json')
                path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                state = json.loads(path.read_bytes()) if path.exists() else {
                    'scratch_space_id': scratch_space_id, 'last_command_at': time.time()}
                state['purging'] = True
                save(path, state)
                operations = [self.futures[path.stem] for path in journal_paths if path.stem in self.futures]
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
        from control import scratch_resource_name
        journal_paths = self._space_journal_paths(scratch_space_id)
        session_paths = self._space_journal_paths(scratch_space_id, sessions=True)
        for path in journal_paths + session_paths:
            path.unlink(missing_ok=True)
        with self.lock:
            for name in self.journal_index.pop(scratch_space_id, set()):
                self.journal_metadata.pop(name, None)
            self.session_index.pop(scratch_space_id, None)
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
        """Restore watches without replaying commands or blocking startup on Kubernetes."""
        import json
        spaces = self.backend.root / 'execution-v2-spaces'
        if spaces.exists():
            for path in spaces.glob('*.json'):
                try:
                    state = json.loads(path.read_bytes())
                    if not state.get('purged'):
                        self.touch_space(state['scratch_space_id'])
                        self.helper_checks.add(state['scratch_space_id'])
                except Exception as error:
                    import sys
                    print('Execution Pod Space recovery failed: ' + str(error), file=sys.stderr, flush=True)
        directory = self.backend.root / 'execution-v2'
        if not directory.exists():
            return
        for path in directory.glob('*.json'):
            try:
                record = json.loads(path.read_bytes())
                operation = record['operation_id']
                if record['status'] == 'queued':
                    self._update_command(operation, status='cancelled', outcome='not_started')
                elif record['status'] == 'running':
                    if record.get('docker_enabled'):
                        self._update_command(operation, status='interrupted', outcome='unknown')
                        continue
                    scratch_id, run_id = record['scratch_space_id'], record['run_id']
                    foreground = not record.get('background')
                    with self.lock:
                        if scratch_id not in self.queues:
                            self.queues[scratch_id] = CommandQueue(record.get('command_slots', 8))
                        queue = self.queues[scratch_id]
                        if foreground:
                            self.foreground_jobs[scratch_id] = self.foreground_jobs.get(scratch_id, 0) + 1
                        try:
                            if record.get('kind') == 'python_session':
                                future = queue.submit_session(operation, self._resume_command)
                            else:
                                future = queue.submit(run_id, operation, self._resume_command,
                                                      background=record.get('background', False))
                        except Exception:
                            if foreground:
                                self.foreground_jobs[scratch_id] -= 1
                            raise
                        self.futures[operation] = future
                        future.add_done_callback(lambda done, op=operation: self._cancelled_command(op, done))
            except Exception as error:
                import sys
                print('Execution Pod command recovery failed: ' + str(error), file=sys.stderr, flush=True)

    def _command_path(self, operation_id):
        import uuid
        base = operation_id[:-5] if isinstance(operation_id, str) and operation_id.endswith('.left') else operation_id
        if not isinstance(base, str) or str(uuid.UUID(base)) != base:
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

    def _update_command(self, operation_id, *, allow_late=False, **fields):
        import time
        from control import save
        self._ensure_journal_index()
        with self.lock:
            record = self._read_command(operation_id)
            if allow_late and record.get('status') != 'interrupted':
                return record
            terminal = ('completed', 'timed_out', 'stopped', 'cancelled', 'interrupted')
            if (record.get('status') in terminal and fields.get('status') != record.get('status')
                    and not (allow_late and record.get('status') == 'interrupted')):
                return record
            if allow_late:
                record.pop('error_code', None)
                record.pop('message', None)
            if fields.get('status') in terminal:
                fields.setdefault('terminal_at', time.time())
            record.update(fields)
            path = self._command_path(operation_id)
            save(path, record)
            self.journal_metadata[path.name] = self._prune_metadata(record)
            return record

    def command_result(self, operation_id, *, retry_stop=True, observe_late=True):
        import base64
        import json
        import time
        record = self._read_command(operation_id)
        if (observe_late and record.get('status') == 'interrupted'
                and time.time() - record.get('terminal_at', 0) < LATE_RESULT_SECONDS):
            try:
                pod = self._get('pod', record['pod'])
                if pod and pod.get('metadata', {}).get('uid') == record['pod_uid']:
                    adopted = False
                    if not record.get('docker_enabled'):
                        observed = self.backend.kube(['exec', record['pod'], '-c', 'execution', '--'] +
                                                     helper_command('result', operation_id), check=False, timeout=5)
                        if observed.returncode == 0:
                            outcome = json.loads(observed.stdout)
                            if (outcome.get('operation_id') == operation_id and outcome.get('run_id') == record['run_id']
                                    and outcome.get('status') in ('completed', 'timed_out', 'stopped')):
                                self._finish_command(record, outcome, allow_late=True)
                                adopted = True
                    if not adopted and retry_stop and record.get('stop_pending'):
                        self._stop_untracked_command(record)
            except Exception:
                pass
        with self.lock:
            record = self._read_command(operation_id)
            result = {key: record[key] for key in ('operation_id', 'scratch_space_id', 'run_id',
                      'status', 'background', 'pod_uid', 'outcome', 'exit_code', 'stdout', 'stderr', 'truncated',
                      'error_code', 'message', 'session_replaced', 'output_fetched_at', 'left_running') if key in record}
            if record.get('kind') == 'python_cell' and 'session_operation_id' in record:
                result['session_id'] = record['session_operation_id']
            if record['status'] == 'running' and operation_id in self.live_output:
                result.update(self.live_output[operation_id])
            if record.get('kind') == 'python_cell' and record['status'] == 'completed' and record.get('exit_code') == 0:
                try:
                    result['python_result'] = json.loads(base64.b64decode(record['stdout'], validate=True))
                except (ValueError, KeyError):
                    result['python_result'] = {'status': 'failed', 'code': 'python_result_invalid',
                                               'message': 'Python result was unavailable or truncated'}
            return result

    def refuse_command(self, operation_id, scratch_space_id, run_id):
        import time
        from control import save, scratch_resource_name
        scratch_resource_name(scratch_space_id)
        if not isinstance(run_id, str) or not run_id or len(run_id) > 256 or '\x00' in run_id:
            raise ValueError('invalid run_id')
        if not isinstance(operation_id, str) or operation_id.endswith('.left'):
            raise ValueError('canonical operation UUID required')
        path = self._command_path(operation_id)
        with self._space_lock(scratch_space_id):
            self.prune_space(scratch_space_id)
            with self.lock:
                if path.exists():
                    prior = self._read_command(operation_id)
                    if prior.get('status') == 'refused':
                        if prior.get('scratch_space_id') != scratch_space_id or prior.get('run_id') != run_id:
                            raise ExecutionPodError('command_conflict', 'Operation ID belongs to another Run')
                        return {'refused': True}
                    return {'refused': False, 'status': prior['status']}
                record = {'operation_id': operation_id, 'scratch_space_id': scratch_space_id,
                          'run_id': run_id, 'status': 'refused', 'terminal_at': time.time()}
                save(path, record)
                self.journal_index.setdefault(scratch_space_id, set()).add(path.name)
                self.journal_metadata[path.name] = self._prune_metadata(record)
                space_path = self.backend.root / 'execution-v2-spaces' / (scratch_resource_name(scratch_space_id) + '.json')
                if not space_path.exists():
                    self.touch_space(scratch_space_id)
                return {'refused': True}

    def submit_command(self, request, *, kind='shell', session_operation_id=None, client_digest=None, session_replaced=False):
        with self._space_lock(request['scratch_space_id']):
            return self._submit_command(request, kind=kind, session_operation_id=session_operation_id, client_digest=client_digest, session_replaced=session_replaced)

    def _submit_command(self, request, *, kind='shell', session_operation_id=None, client_digest=None, session_replaced=False):
        import hashlib
        import json
        import time
        from control import save, scratch_resource_name

        operation_id = request['operation_id']
        if not isinstance(operation_id, str) or operation_id.endswith('.left'):
            raise ValueError('canonical operation UUID required')
        path = self._command_path(operation_id)
        scratch_id, run_id, code = request['scratch_space_id'], request['run_id'], request['code']
        if path.exists():
            self.prune_space(scratch_id)
        with self.lock:
            if path.exists() and self._read_command(operation_id).get('status') == 'refused':
                raise ExecutionPodError('command_refused', 'Command operation was refused')
        if not self.admitted:
            raise ExecutionPodError('admission_unverified', 'Execution Pod admission has not been verified')
        name = scratch_resource_name(scratch_id)
        if not isinstance(run_id, str) or not run_id or len(run_id) > 256 or '\x00' in run_id:
            raise ValueError('run_id must be a nonempty string without NUL')
        if kind in RUN_SESSION_KINDS:
            self._check_sessions_open(scratch_id, run_id)
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
        credential = request.get('proxy_credential')
        proxy = None
        if credential is not None:
            if not self.config.get('proxy_endpoint'):
                raise ValueError('proxy endpoint is unavailable')
            proxy = pod_proxy_url(self.config['proxy_endpoint'], credential)
        digest = hashlib.sha256(json.dumps(request, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
        with self.lock:
            self._require_space_termination_confirmed(scratch_id)
            if path.exists():
                prior = self._read_command(operation_id)
                if prior.get('status') == 'refused':
                    raise ExecutionPodError('command_refused', 'Command operation was refused')
                if prior.get('request_digest') != digest:
                    raise ExecutionPodError('command_conflict', 'Operation ID was already used for another command')
                return self.command_result(operation_id, observe_late=False)
        self.prune_space(scratch_id)
        if len(self._space_journal_paths(scratch_id)) >= JOURNAL_MAX_RECORDS_PER_SPACE:
            raise ExecutionPodError('journal_full', 'Scratch Space command journal is full; retry after retention expires')
        pod = self._get('pod', name)
        with self.lock:
            # Different Spaces use different lifecycle locks, so another Space
            # may have claimed this operation while the Pod lookup was running.
            if path.exists():
                prior = self._read_command(operation_id)
                if prior.get('status') == 'refused':
                    raise ExecutionPodError('command_refused', 'Command operation was refused')
                if prior.get('request_digest') != digest:
                    raise ExecutionPodError('command_conflict', 'Operation ID was already used for another command')
                return self.command_result(operation_id, observe_late=False)
            if pod and self._old_helper_image(pod):
                raise ExecutionPodError('pod_updating', 'Execution Pod is updating; retry shortly')
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
            record['command_slots'] = int(pod.get('metadata', {}).get('annotations', {}).get('aidash.run/command-slots', '8'))
            record['proxy_credential_given'] = credential is not None
            if session_replaced:
                record['session_replaced'] = True
            if session_operation_id:
                record['session_operation_id'] = session_operation_id
            if client_digest:
                record['client_digest'] = client_digest
            save(path, record)
            if proxy is not None:
                self.process_proxies[operation_id] = proxy
            self.journal_index.setdefault(scratch_id, set()).add(path.name)
            self.journal_metadata[path.name] = self._prune_metadata(record)
            self.touch_space(scratch_id)
            if not background:
                self.foreground_jobs[scratch_id] = self.foreground_jobs.get(scratch_id, 0) + 1
            if scratch_id not in self.queues:
                self.queues[scratch_id] = CommandQueue(record.get('command_slots', 8))
            queue = self.queues[scratch_id]
            if kind == 'python_session':
                future = queue.submit_session(operation_id, self._execute_command)
            else:
                future = queue.submit(run_id, operation_id, self._execute_command, background=background)
            self.futures[operation_id] = future
            future.add_done_callback(lambda done: self._cancelled_command(operation_id, done))
            return self.command_result(operation_id, observe_late=False)

    def _cancelled_command(self, operation_id, future):
        with self.lock:
            if future.done():
                self.left_result_keys.pop(operation_id, None)
            try:
                if future.cancelled():
                    self.process_proxies.pop(operation_id, None)
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

    def _await_normal_result(self, record, deadline, *, publish_saved=False):
        import json
        import time
        name, operation_id = record['pod'], record['operation_id']
        missing_since = None
        preview_at = 0
        preview = None
        if publish_saved:
            limit = self.config.get('output_bytes', getattr(self.backend, 'config', {}).get('output_bytes', 8388608))
            preview = {'output': BoundedOutput(limit), 'offsets': {'stdout': 0, 'stderr': 0}}
        interval = (5 if record.get('background') else 2) if publish_saved else .5
        while deadline is None or time.monotonic() < deadline:
            if self._read_command(operation_id).get('status') == 'interrupted':
                return None
            try:
                if publish_saved and time.monotonic() >= preview_at:
                    preview_at = time.monotonic() + interval
                    try:
                        output = self._preview_saved_output(record, preview)
                        output['output_fetched_at'] = time.time()
                        with self.lock:
                            self.live_output[operation_id] = output
                    except Exception:
                        pass
                observed = self.backend.kube(['exec', name, '-c', 'execution', '--'] +
                                             helper_command('result', operation_id), check=False)
                if observed.returncode == 0:
                    outcome = json.loads(observed.stdout)
                    if record.get('kind') == 'left':
                        proof_key = record.get('result_proof_key')
                        if proof_key or record.get('docker_enabled'):
                            with self.lock:
                                key = proof_key.encode('ascii') if proof_key else self.left_result_keys.get(operation_id)
                            if not verify_adopted_result(outcome, key):
                                raise ValueError('unverified adopted result')
                    return outcome
                pod = self._get('pod', name)
                if not pod or pod.get('metadata', {}).get('uid') != record['pod_uid']:
                    if publish_saved:
                        self._update_command(operation_id, status='interrupted', outcome='unknown',
                                             **self._partial_output(record))
                    return None
                if pod.get('status', {}).get('phase') in ('Failed', 'Succeeded'):
                    return None
                running = self.backend.kube(['exec', name, '-c', 'execution', '--'] +
                                            helper_command('running', operation_id), check=False)
                if running.returncode == 0 and json.loads(running.stdout).get('running') is True:
                    missing_since = None
                elif running.returncode == 0:
                    if missing_since is None:
                        missing_since = time.monotonic()
                    elif time.monotonic() - missing_since >= 30:
                        return None
                else:
                    missing_since = None
            except Exception:
                missing_since = None
            time.sleep(interval)
        return None

    def _execute_command(self, operation_id):
        import secrets
        import time
        with self.lock:
            record = self._read_command(operation_id)
            proxy = self.process_proxies.pop(operation_id, None)
            if record['status'] != 'queued':
                return
            session = record.get('kind') in ('python_session', 'mcp_session')
            authenticate = session or (not record['background'] and record.get('kind') in ('shell', 'python_cell'))
            proof_key = secrets.token_hex(32) if authenticate else None
            fields = {'result_proof_key': proof_key} if proof_key else {}
            record = self._update_command(operation_id, status='running', running_at=time.time(), **fields)
        name, uid = record['pod'], record['pod_uid']
        mode = 'docker' if record['docker_enabled'] else 'normal'
        deadline = 'background' if record['background'] else str(record['timeout_seconds'])
        kind = 'session' if record.get('kind') == 'python_session' else 'command'
        limit = self.config.get('output_bytes', getattr(self.backend, 'config', {}).get('output_bytes', 8388608))
        frame_token = secrets.token_hex(32) if record['docker_enabled'] else None
        argv = self.backend.command + ['exec'] + (['-i'] if frame_token or proof_key or proxy else []) + [name, '-c', 'execution', '--'] + helper_command(
            'supervise', mode, kind, operation_id, record['run_id'], record['cwd'], deadline, uid, record['code'], str(limit))
        supervisor_input = []
        if proof_key:
            argv.append('proof-stdin')
            supervisor_input.append(proof_key)
        if proxy:
            argv.append('proxy-stdin')
            supervisor_input.append(proxy)

        def publish(output):
            with self.lock:
                self.live_output[operation_id] = output

        try:
            recover_output = False
            transport_timeout = None if record['background'] else record['timeout_seconds'] + 180
            result_deadline = None if transport_timeout is None else time.monotonic() + transport_timeout
            transport = drain_process(argv, self.backend.environment, limit, transport_timeout, publish,
                                      frame_token=frame_token,
                                      input_data=''.join(line + '\n' for line in supervisor_input).encode() or None)
            if self._read_command(operation_id)['status'] == 'interrupted':
                return
            if record['docker_enabled']:
                outcome = transport.get('framed_result')
                # The framed result is authenticated by the nonce; the saved files are not trusted here,
                # so lost forwarding can only be reported, not recovered.
                if isinstance(outcome, dict) and outcome.get('forward_incomplete'):
                    transport['output']['truncated'] = True
            else:
                outcome = self._await_normal_result(record, result_deadline)
                if self._read_command(operation_id)['status'] == 'interrupted':
                    return
                recover_output = transport['exit_code'] != 0 or (
                    isinstance(outcome, dict) and outcome.get('forward_incomplete'))
            if (transport['transport_timed_out'] or not isinstance(outcome, dict)
                    or outcome.get('operation_id') != operation_id or outcome.get('run_id') != record['run_id']
                    or outcome.get('status') not in ('completed', 'timed_out', 'stopped')):
                transport['output']['truncated'] = True
                self._update_command(operation_id, status='interrupted', outcome='unknown', stop_pending=True,
                                     **transport['output'])
                self._stop_untracked_command(record)
            else:
                left_auth = proof_key or frame_token
                self._finish_command(record, outcome, transport['output'],
                                     recover_output=recover_output,
                                     left_auth=left_auth.encode('ascii') if left_auth else None)
        except Exception as error:
            if self._read_command(operation_id)['status'] == 'interrupted':
                return
            self._update_command(operation_id, status='interrupted', outcome='unknown', stop_pending=True,
                                 error_code='command_interrupted', message=str(error) or type(error).__name__)
            self._stop_untracked_command(record)
        finally:
            with self.lock:
                self.live_output.pop(operation_id, None)

    def _partial_output(self, record):
        import base64
        with self.lock:
            live = dict(self.live_output.get(record['operation_id'], {}))
        output = {'truncated': True}
        for stream in ('stdout', 'stderr'):
            choices = [value for value in (record.get(stream), live.get(stream)) if isinstance(value, str)]
            def length(value):
                try:
                    return len(base64.b64decode(value, validate=True))
                except ValueError:
                    return -1
            output[stream] = max(choices, key=length) if choices else ''
        if 'output_fetched_at' in live:
            output['output_fetched_at'] = live['output_fetched_at']
        return output

    def _output_chunk(self, record, stream, offset, timeout):
        import base64
        import json
        result = self.backend.kube(['exec', record['pod'], '-c', 'execution', '--'] +
            helper_command('output', record['operation_id'], stream, str(offset), '262144'),
            check=False, timeout=timeout)
        if result.returncode != 0:
            raise RuntimeError('saved output unavailable')
        chunk = json.loads(result.stdout)
        data = base64.b64decode(chunk['data'], validate=True)
        if chunk['next_offset'] != offset + len(data) or (not data and not chunk['eof']):
            raise ValueError('invalid saved output chunk')
        return data, chunk['eof']

    def _preview_saved_output(self, record, state):
        output, offsets = state['output'], state['offsets']
        for stream in ('stdout', 'stderr'):
            while True:
                data, eof = self._output_chunk(record, stream, offsets[stream], 5)
                offsets[stream] += len(data)
                output.add(stream, data)
                if offsets[stream] > output.limit:
                    raise ValueError('saved output exceeds limit')
                if eof:
                    break
        return output.public()

    def _saved_output(self, record, outcome, *, tolerate_missing=False):
        import base64
        import time
        limit = self.config.get('output_bytes', getattr(self.backend, 'config', {}).get('output_bytes', 8388608))
        output = BoundedOutput(limit)
        fallback = self._partial_output(record) if tolerate_missing else None
        # The supervisor already reported a file it could not fully save: one read is worth trying,
        # but waiting for it to improve is not.
        unusable = bool(outcome.get('output_incomplete'))
        incomplete = unusable
        # The budget bounds how long a failing read is retried. Every chunk that was read restarts it,
        # because a large output takes many reads and must not run out of time while they succeed.
        deadline = time.monotonic() + OUTPUT_RETRY_SECONDS
        for stream in ('stdout', 'stderr'):
            data_so_far = bytearray()
            failed = False
            while True:
                try:
                    timeout = max(.001, min(15, deadline - time.monotonic()))
                    data, eof = self._output_chunk(record, stream, len(data_so_far), timeout)
                    data_so_far.extend(data)
                    if len(data_so_far) > limit:
                        raise ValueError('saved output exceeds limit')
                    deadline = time.monotonic() + OUTPUT_RETRY_SECONDS
                    if eof:
                        break
                except Exception:
                    remaining = deadline - time.monotonic()
                    if remaining <= 0 or unusable:
                        if not tolerate_missing:
                            raise
                        failed = True
                        break
                    time.sleep(min(.5, remaining))
            if failed:
                incomplete = True
                try:
                    prior = base64.b64decode(fallback[stream], validate=True)
                except ValueError:
                    prior = b''
                if len(prior) > len(data_so_far):
                    data_so_far = bytearray(prior)
            output.add(stream, data_so_far)
        public = output.public()
        public['truncated'] |= incomplete or outcome.get('output_truncated', False)
        if incomplete:
            public['output_incomplete'] = True
        return public

    def _finish_command(self, record, outcome, output=None, *, allow_late=False, left_auth=None,
                        recover_output=False):
        operation_id = record['operation_id']
        proof_key = record.get('result_proof_key')
        if (record.get('kind') in ('python_session', 'mcp_session') and proof_key
                and not verify_adopted_result(outcome, proof_key.encode('ascii'))):
            raise ValueError('unverified session result')
        if output is None:
            try:
                output = self._saved_output(record, outcome, tolerate_missing=True)
            except Exception:
                output = dict(self._partial_output(record), output_incomplete=True)
        elif recover_output:
            output['truncated'] = True
            try:
                output = self._saved_output(record, outcome)
            except Exception:
                pass
        incomplete = output.pop('output_incomplete', False) or outcome.get('output_incomplete')
        if (record.get('kind') == 'python_cell' and
            (outcome['status'] in ('timed_out', 'stopped') or
             (outcome['status'] == 'completed' and outcome.get('exit_code') != 0))):
            try:
                self._stop_python_interpreter(record['session_operation_id'])
            except Exception:
                if not allow_late:
                    self._stop_matching_pod(record['scratch_space_id'], record['pod_uid'])
        left = outcome.get('left_running')
        if left is not None:
            if record.get('background') or left != operation_id + '.left':
                raise ValueError('invalid adopted operation ID')
            self._adopt_left(record, left, left_auth)
        fields = {key: outcome[key] for key in ('exit_code', 'error_code', 'message', 'left_running') if key in outcome}
        if output.get('truncated') and not fields.get('message') and incomplete:
            fields['message'] = 'Output was lost to a Runner restart or a full disk'
        if allow_late:
            fields['outcome'] = 'known'
        self._update_command(operation_id, allow_late=allow_late, status=outcome['status'], **fields, **output)
        if record.get('background'):
            return
        try:
            self.backend.kube(['exec', record['pod'], '-c', 'execution', '--'] +
                              helper_command('forget', operation_id), check=False, timeout=5)
        except Exception:
            pass

    def _adopt_left(self, record, operation_id, left_auth=None):
        import time
        from control import save
        scratch = record['scratch_space_id']
        if record.get('docker_enabled') and not left_auth:
            raise ValueError('adopted Docker process requires authenticated result tracking')
        with self._space_lock(scratch):
            self._ensure_journal_index()
            path = self._command_path(operation_id)
            with self.lock:
                if path.exists():
                    return
                adopted = {key: record[key] for key in ('scratch_space_id', 'run_id', 'pod', 'pod_uid',
                           'docker_enabled', 'cwd', 'result_proof_key', 'command_slots') if key in record}
                adopted.update(operation_id=operation_id, status='running', background=True,
                               kind='left', accepted_at=time.time(), running_at=time.time())
                save(path, adopted)
                self.journal_index.setdefault(scratch, set()).add(path.name)
                self.journal_metadata[path.name] = self._prune_metadata(adopted)
                if left_auth:
                    self.left_result_keys[operation_id] = left_auth
                if scratch not in self.queues:
                    self.queues[scratch] = CommandQueue(adopted.get('command_slots', 8))
                future = self.queues[scratch].submit(record['run_id'], operation_id, self._resume_command,
                                                     background=True)
                self.futures[operation_id] = future
                future.add_done_callback(lambda done: self._cancelled_command(operation_id, done))

    def _resume_command(self, operation_id):
        import json
        import time
        record = self._read_command(operation_id)
        deadline = None
        if not record.get('background') and record.get('kind') != 'python_session':
            remaining = record.get('running_at', record.get('accepted_at', time.time())) + record['timeout_seconds'] + 180 - time.time()
            deadline = time.monotonic() + max(30, remaining)
        try:
            while deadline is None or time.monotonic() < deadline:
                if self._read_command(operation_id)['status'] != 'running':
                    return
                try:
                    pod = self._get('pod', record['pod'])
                    if (not pod or pod.get('metadata', {}).get('uid') != record['pod_uid']
                            or pod.get('status', {}).get('phase') in ('Failed', 'Succeeded')):
                        self._update_command(operation_id, status='interrupted', outcome='unknown')
                        return
                    claim = self.backend.kube(['exec', record['pod'], '-c', 'execution', '--'] +
                                              helper_command('claim', operation_id), check=False, timeout=5)
                    if claim.returncode != 0:
                        raise RuntimeError('claim unavailable')
                    created = json.loads(claim.stdout)['created']
                    if type(created) is not bool:
                        raise ValueError('invalid claim response')
                    if created:
                        if record.get('kind') == 'left' and record.get('docker_enabled'):
                            self._update_command(operation_id, status='interrupted', outcome='unknown', stop_pending=True)
                            self._stop_untracked_command(record)
                            return
                        self._update_command(operation_id, status='cancelled', outcome='not_started')
                        return
                    break
                except Exception:
                    time.sleep(5 if record.get('background') else 2)
            outcome = self._await_normal_result(record, deadline, publish_saved=True)
            if self._read_command(operation_id)['status'] != 'running':
                return
            if (isinstance(outcome, dict) and outcome.get('operation_id') == operation_id
                    and outcome.get('run_id') == record['run_id']
                    and outcome.get('status') in ('completed', 'timed_out', 'stopped')):
                self._finish_command(record, outcome)
            else:
                with self.lock:
                    output = dict(self.live_output.get(operation_id, {'stdout': '', 'stderr': ''}))
                output['truncated'] = True
                self._update_command(operation_id, status='interrupted', outcome='unknown', stop_pending=True, **output)
                self._stop_untracked_command(record)
        except Exception as error:
            if self._read_command(operation_id)['status'] == 'running':
                self._update_command(operation_id, status='interrupted', outcome='unknown', stop_pending=True,
                                     error_code='command_interrupted', message=str(error) or type(error).__name__)
                self._stop_untracked_command(record)
        finally:
            with self.lock:
                self.live_output.pop(operation_id, None)

    def _stop_untracked_command(self, record):
        import uuid
        operation_id = record['operation_id']
        base = operation_id[:-5] if operation_id.endswith('.left') else operation_id
        gate = self.command_stop_locks[uuid.UUID(base).int % len(self.command_stop_locks)]
        with gate:
            current = self._read_command(operation_id)
            if current.get('status') != 'interrupted' or not current.get('stop_pending'):
                return
            self._stop_untracked_command_locked(current)

    def _stop_untracked_command_locked(self, record):
        import time
        try:
            pod = self._get('pod', record['pod'])
        except Exception:
            return
        if not pod or pod.get('metadata', {}).get('uid') != record['pod_uid']:
            return
        try:
            self.backend.kube(['exec', record['pod'], '-c', 'execution', '--'] +
                              helper_command('stop', record['operation_id']))
            with self.lock:
                current = self._read_command(record['operation_id'])
                if current.get('status') == 'interrupted':
                    self._update_command(record['operation_id'], status='interrupted', stop_pending=False,
                                         terminal_at=current['terminal_at'])
        except Exception:
            with self.lock:
                latest = self._read_command(record['operation_id'])
                if latest.get('status') != 'interrupted' or not latest.get('stop_pending'):
                    return
                since = latest.get('stop_failure_at')
                if since is None:
                    since = time.time()
                    self._update_command(record['operation_id'], status='interrupted', stop_failure_at=since,
                                         terminal_at=latest['terminal_at'])
                if time.time() < since + STOP_RETRY_SECONDS:
                    return
            try:
                if self.command_result(record['operation_id'], retry_stop=False)['status'] != 'interrupted':
                    return
                current = self._get('pod', record['pod'])
            except Exception:
                return
            if not current or current.get('metadata', {}).get('uid') != record['pod_uid']:
                return
            try:
                self._stop_matching_pod(record['scratch_space_id'], record['pod_uid'])
            except Exception:
                self._mark_space_unconfirmed(record['scratch_space_id'])

    def _stop_python_interpreter(self, operation_id):
        import time
        # From here until the Pod shows a terminal result the interpreter is on its way out, and
        # `_start_python` must not hand the session to a new cell.
        self._update_command(operation_id, interpreter_stopping=True)
        result = self.stop_background(operation_id)
        # The Pod's own result is the evidence: a session watcher resumed after a Runner restart polls only
        # every few seconds and then reads saved output, and must not decide whether the stop worked. An
        # accepted stop request proves nothing by itself, so without a terminal result the limit ends in
        # stop_failed and the Pod is deleted.
        deadline = time.monotonic() + PYTHON_STOP_SECONDS
        while True:
            status = result.get('status')
            if status in ('completed', 'timed_out', 'stopped', 'cancelled'):
                return
            if status == 'interrupted':
                raise ExecutionPodError('stop_failed', 'Python interpreter termination could not be confirmed')
            outcome = self._interpreter_outcome(operation_id)
            if outcome in ('completed', 'timed_out', 'stopped'):
                return
            if outcome == 'termination_unconfirmed' or time.monotonic() >= deadline:
                raise ExecutionPodError('stop_failed', 'Python interpreter termination could not be confirmed')
            time.sleep(.25)
            result = self.command_result(operation_id)

    def _interpreter_outcome(self, operation_id):
        """Read the status the Pod recorded for a Python session, or None while it is unavailable."""
        import json
        record = self._read_command(operation_id)
        if record.get('docker_enabled'):
            # Docker mode never trusts a result file; its journal comes from the authenticated frame.
            return None
        try:
            observed = self.backend.kube(['exec', record['pod'], '-c', 'execution', '--'] +
                                         helper_command('result', operation_id), check=False, timeout=5)
            if observed.returncode != 0:
                return None
            outcome = json.loads(observed.stdout)
        except Exception:
            return None
        if (isinstance(outcome, dict) and outcome.get('operation_id') == operation_id
                and outcome.get('run_id') == record.get('run_id')):
            return outcome.get('status')
        return None

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
            failures = []
            for path in self._space_journal_paths(scratch_space_id):
                try:
                    record = json.loads(path.read_bytes())
                except FileNotFoundError:
                    continue
                if (record.get('scratch_space_id') == scratch_space_id and record.get('run_id') == run_id
                        and not record.get('background') and (record.get('status') in ('queued', 'running')
                            or (record.get('status') == 'interrupted' and record.get('docker_enabled')))):
                    try:
                        self._stop_command(record['operation_id'])
                    except Exception as error:
                        failures.append(str(error) or type(error).__name__)
            if failures:
                raise ExecutionPodError('stop_failed', '; '.join(failures))
        return {'cancelled_waiting': True}

    def stop_command(self, operation_id):
        """Stop one foreground command; the Run's other commands keep running."""
        record = self._read_command(operation_id)
        if record.get('background'):
            raise ExecutionPodError('invalid_request', 'Use the Background process stop for this operation')
        with self._space_lock(record['scratch_space_id']):
            return self._stop_command(operation_id)

    def stop_run(self, scratch_space_id, run_id, kinds=None, *, seal_sessions=False):
        import json
        import time
        from control import scratch_resource_name
        scratch_resource_name(scratch_space_id)
        if not isinstance(run_id, str) or not run_id or len(run_id) > 256 or '\x00' in run_id:
            raise ValueError('invalid run_id')
        if kinds is None or seal_sessions:
            # A start already on the wire may arrive after the stop. Fence it
            # under the same Space lock as admission, including across restarts.
            from control import save
            with self._space_lock(scratch_space_id):
                self._ensure_journal_index()
                path = self._session_stop_path(scratch_space_id, run_id)
                save(path, {'scratch_space_id': scratch_space_id})
                with self.lock:
                    self.session_index.setdefault(scratch_space_id, set()).add(path.name)
        stopped, resolved, first_seen = set(), set(), {}
        deadline = time.monotonic() + 60
        while True:
            active, failures = [], []
            with self._space_lock(scratch_space_id):
                for path in self._space_journal_paths(scratch_space_id):
                    try:
                        record = json.loads(path.read_bytes())
                    except FileNotFoundError:
                        continue
                    operation = record['operation_id']
                    if (record.get('scratch_space_id') != scratch_space_id or record.get('run_id') != run_id
                            or operation in resolved or record.get('run_stop_confirmed') or
                            record.get('status') not in ('queued', 'running', 'interrupted')
                            or (kinds is not None and record.get('kind') not in kinds)):
                        continue
                    active.append(operation)
                    if record.get('kind') in RUN_SESSION_KINDS and record.get('pod_uid'):
                        pod = self._get('pod', record['pod'])
                        uid = pod.get('metadata', {}).get('uid') if pod else None
                        if uid and uid != record['pod_uid']:
                            self._update_command(operation, status='interrupted', stop_pending=False,
                                                 run_stop_confirmed=True)
                            resolved.add(operation)
                            continue
                    if record.get('status') == 'interrupted' and not record.get('docker_enabled'):
                        try:
                            first_seen.setdefault(operation, time.monotonic())
                            done, signaled = self._stop_interrupted_run_process(
                                record, operation in stopped, time.monotonic() - first_seen[operation])
                            if signaled:
                                stopped.add(operation)
                            if done:
                                self._update_command(operation, status='interrupted', stop_pending=False,
                                                     run_stop_confirmed=True)
                                resolved.add(operation)
                        except Exception as error:
                            failures.append(str(error) or type(error).__name__)
                        continue
                    if operation in stopped and record.get('status') != 'interrupted':
                        continue
                    try:
                        response = self._stop_command(operation)
                        stopped.add(operation)
                        if isinstance(response, dict) and response.get('status') in ('completed', 'stopped', 'cancelled', 'timed_out', 'interrupted'):
                            resolved.add(operation)
                    except Exception as error:
                        failures.append(str(error) or type(error).__name__)
                if failures:
                    raise ExecutionPodError('stop_failed', '; '.join(failures))
            if not active or all(operation in resolved for operation in active):
                # Re-scan after a signal: a foreground process may have adopted
                # descendants while its result was being published.
                if active:
                    continue
                return {'stopped': sorted(stopped)}
            if time.monotonic() >= deadline:
                raise ExecutionPodError('stop_failed', 'Run processes did not confirm termination')
            time.sleep(.1)

    def _stop_interrupted_run_process(self, record, signaled, elapsed):
        import json
        pod = self._get('pod', record['pod'])
        if not pod or pod.get('metadata', {}).get('uid') != record.get('pod_uid'):
            return True, signaled
        operation = record['operation_id']
        result = self.backend.kube(['exec', record['pod'], '-c', 'execution', '--'] +
                                   helper_command('running', operation), check=False, timeout=5)
        if result.returncode != 0:
            raise RuntimeError('Could not confirm interrupted process state')
        running = json.loads(result.stdout).get('running')
        if type(running) is not bool:
            raise ValueError('Invalid interrupted process state')
        if running:
            if not signaled:
                self.backend.kube(['exec', record['pod'], '-c', 'execution', '--'] +
                                  helper_command('stop', operation), timeout=5)
            return False, True
        if signaled:
            return True, True
        claim = self.backend.kube(['exec', record['pod'], '-c', 'execution', '--'] +
                                  helper_command('claim', operation), check=False, timeout=5)
        if claim.returncode != 0:
            raise RuntimeError('Could not fence interrupted process startup')
        created = json.loads(claim.stdout).get('created')
        if type(created) is not bool:
            raise ValueError('Invalid interrupted process claim')
        return created or elapsed >= 10, False

    def background_ended(self, scratch_space_id, run_id, operation_id, pod_uid=None):
        """Prove the original Pod terminated or its supervisor reaped the last descendant."""
        import json
        from control import scratch_resource_name

        name = scratch_resource_name(scratch_space_id)
        left = operation_id + '.left'
        token = None
        # The adopted process can outlive the parent journal's retention window.
        for candidate in (operation_id, left):
            try:
                record = self._read_command(candidate)
            except ExecutionPodError as error:
                if error.code != 'command_not_found':
                    raise
                continue
            if record.get('scratch_space_id') != scratch_space_id or record.get('run_id') != run_id:
                return {'ended': False}
            original_uid = record.get('pod_uid')
            if pod_uid and pod_uid != original_uid:
                return {'ended': False}
            pod_uid = original_uid
            token = record.get('result_proof_key')
            if token:
                break
        if not isinstance(pod_uid, str) or not pod_uid:
            return {'ended': False}
        # Only confirmed termination writes this trusted, Space-scoped record.
        path = self.backend.root / 'execution-v2-spaces' / (name + '.json')
        try:
            state = json.loads(path.read_bytes())
        except FileNotFoundError:
            state = {}
        if pod_uid in state.get('terminated_pods', []):
            return {'ended': True}
        pod = self._get('pod', name)
        current_uid = pod.get('metadata', {}).get('uid') if pod else None
        if not isinstance(current_uid, str) or not current_uid:
            return {'ended': False}
        if current_uid != pod_uid:
            return {'ended': True}

        if not token:
            return {'ended': False}
        evidence = self.backend.kube(['exec', name, '-c', 'execution', '--'] +
                                     helper_command('result', left), check=False, timeout=5)
        if evidence.returncode != 0:
            return {'ended': False}
        outcome = json.loads(evidence.stdout)
        # Only the out-of-chroot supervisor knows the token. It emits a
        # signed terminal result after waitpid/children report no descendants.
        return {'ended': bool(isinstance(outcome, dict)
                              and outcome.get('operation_id') == left and outcome.get('run_id') == run_id
                              and outcome.get('status') in ('completed', 'stopped', 'timed_out')
                              and verify_adopted_result(outcome, token.encode('ascii')))}

    def background_processes(self, scratch_space_id):
        import json
        import time
        from control import scratch_resource_name

        scratch_resource_name(scratch_space_id)
        result = []
        terminal = {'completed', 'timed_out', 'stopped', 'cancelled', 'interrupted'}
        for path in self._space_journal_paths(scratch_space_id):
            try:
                record = json.loads(path.read_bytes())
            except FileNotFoundError:
                continue
            if (record.get('scratch_space_id') == scratch_space_id and record.get('background')
                    and record.get('kind') != 'python_session'):
                finished = record.get('terminal_at') or record.get('accepted_at')
                if record.get('status') in terminal and finished and time.time() - finished > 7 * 86400:
                    continue
                result.append({key: record[key] for key in ('operation_id', 'scratch_space_id', 'run_id',
                              'status', 'background', 'outcome', 'exit_code', 'truncated',
                              'error_code', 'message') if key in record})
        return {'processes': sorted(result, key=lambda item: item['operation_id'])}

    def _mcp_exec(self, record, action, *args, data=None, timeout=10):
        pod = self._get('pod', record['pod'])
        if not pod or pod.get('metadata', {}).get('uid') != record.get('pod_uid'):
            raise ExecutionPodError('pod_interrupted', 'The original Execution Pod is unavailable')
        mode = 'docker' if record.get('docker_enabled') else 'normal'
        return self.backend.kube(['exec', '-i', record['pod'], '-c', 'execution', '--'] +
            helper_command('chroot', mode, '/scratch-system/image-root/rootfs', '.',
                           *mcp_helper_command(action, *args)), data=data, check=False, timeout=timeout)

    def start_mcp(self, request):
        import json
        import shlex
        import time
        operation = request['operation_id']
        self._command_path(operation)
        config = {key: request[key] for key in ('install', 'command', 'env', 'plugin_data')}
        if (not isinstance(config['install'], str) or '\x00' in config['install']
                or not isinstance(config['command'], list) or not config['command']
                or any(not isinstance(value, str) or not value or '\x00' in value for value in config['command'])
                or not isinstance(config['env'], dict)
                or any(not isinstance(k, str) or not isinstance(v, str) or not k or '=' in k or '\x00' in k + v
                       for k, v in config['env'].items())
                or not isinstance(config['plugin_data'], str)
                or (config['plugin_data'].startswith('/') and config['plugin_data'] != '/scratch'
                    and not config['plugin_data'].startswith('/scratch/'))
                or '..' in config['plugin_data'].split('/') or '\x00' in config['plugin_data']):
            raise ValueError('invalid MCP start request')
        socket_path = '/run/aidash/mcp-' + operation + '.sock'
        script = shlex.join(mcp_helper_command('serve', socket_path, json.dumps(config, separators=(',', ':'))))
        command = {'operation_id': operation, 'scratch_space_id': request['scratch_space_id'],
                   'run_id': request['run_id'], 'background': True, 'code': 'exec ' + script,
                   'proxy_credential': request.get('proxy_credential')}
        self.submit_command(command, kind='mcp_session')
        deadline = time.monotonic() + 300
        while time.monotonic() < deadline:
            status = self.command_result(operation)
            if status.get('status') not in ('queued', 'running'):
                break
            record = self._read_command(operation)
            try:
                result = self._mcp_exec(record, 'ping', socket_path, timeout=5)
                if result.returncode == 0 and json.loads(result.stdout).get('ready') is True:
                    return {'operation_id': operation}
            except Exception:
                pass
            time.sleep(.5)
        try:
            self.stop_background(operation)
        except Exception:
            pass
        raise ExecutionPodError('mcp_start_failed', 'MCP process did not become ready')

    def call_mcp(self, request):
        import json
        operation, message, timeout = request['operation_id'], request['message'], request['timeout_seconds']
        if not isinstance(message, dict) or type(timeout) is not int or not 1 <= timeout <= 3600:
            raise ValueError('invalid MCP call')
        data = json.dumps(message, separators=(',', ':'), ensure_ascii=False).encode()
        if len(data) > 4 * 1048576:
            raise ValueError('MCP message exceeds 4 MiB')
        record = self._read_command(operation)
        if record.get('kind') != 'mcp_session' or record.get('status') not in ('running', 'queued'):
            raise ExecutionPodError('mcp_unavailable', 'MCP session is not running')
        with self.lock:
            lock = self.mcp_locks.setdefault(operation, Lock())
        if not lock.acquire(blocking=False):
            raise ExecutionPodError('mcp_busy', 'An MCP call is already in flight')
        try:
            socket_path = '/run/aidash/mcp-' + operation + '.sock'
            try:
                result = self._mcp_exec(record, 'call', socket_path, str(timeout), data=data + b'\n', timeout=timeout + 10)
            except Exception:
                raise ExecutionPodError('mcp_call_failed', 'MCP call outcome is unknown') from None
            if result.returncode:
                raise ExecutionPodError('mcp_call_failed', 'MCP call outcome is unknown')
            response = json.loads(result.stdout)
            if not isinstance(response.get('message'), dict):
                raise ExecutionPodError('mcp_call_failed', 'Invalid MCP response')
            return response
        finally:
            lock.release()

    def stop_preview(self, operation_id):
        """Stop an admitted Live preview or durably refuse its future admission."""
        import time
        from control import save

        if not isinstance(operation_id, str) or operation_id.endswith('.left'):
            raise ValueError('canonical operation UUID required')
        path = self._command_path(operation_id)
        self._ensure_journal_index()
        while True:
            # Use the same lock as the final submit check and journal write, so
            # a start still looking up its Pod cannot slip past a successful stop.
            with self.lock:
                try:
                    record = self._read_command(operation_id)
                except ExecutionPodError as error:
                    if error.code != 'command_not_found':
                        raise
                    record = {'operation_id': operation_id, 'kind': 'preview',
                              'status': 'refused', 'terminal_at': time.time()}
                    save(path, record)
                    self.journal_index.setdefault(None, set()).add(path.name)
                    self.journal_metadata[path.name] = self._prune_metadata(record)
                if record.get('kind') != 'preview':
                    raise ExecutionPodError('command_conflict', 'Operation ID does not belong to a Live preview')
                if record['status'] == 'refused':
                    return {'operation_id': operation_id, 'status': 'cancelled', 'outcome': 'not_started'}
            try:
                return self.stop_background(operation_id)
            except ExecutionPodError as error:
                if error.code != 'command_not_found':
                    raise
                # Cleanup removed the record between reads. Recheck and fence
                # a future submission before acknowledging the stop.

    def stop_mcp(self, operation_id):
        if self._read_command(operation_id).get('kind') != 'mcp_session':
            raise ExecutionPodError('mcp_unavailable', 'MCP session does not exist')
        return self.stop_background(operation_id)

    def stop_mcp_run(self, scratch_space_id, run_id):
        """Stop every stdio MCP session of a Run, as `stop_python` does for its Python session.

        The server stops the sessions it knows by id; this also reaches one whose start it lost
        track of (a dropped or timed-out start). It is the Run stop limited to MCP sessions, so an
        interrupted session counts as stopped only on the same evidence: its Pod was replaced, or
        the Pod shows no process of it. Other Background processes of the Run keep running."""
        return self.stop_run(scratch_space_id, run_id, kinds=('mcp_session',))

    def start_preview(self, request):
        operation = request['operation_id']
        if not isinstance(operation, str):
            raise ValueError('canonical operation UUID required')
        # A delayed handler must see its refusal even if retention expires
        # while it is still in preflight or waiting for command admission.
        with self.lock:
            self.preview_starts[operation] += 1
        try:
            return self._start_preview(request)
        finally:
            with self.lock:
                self.preview_starts[operation] -= 1
                if not self.preview_starts[operation]:
                    del self.preview_starts[operation]

    def _start_preview(self, request):
        import base64
        import ipaddress
        import time
        from control import scratch_resource_name
        operation, scratch, port = request['operation_id'], request['scratch_space_id'], request['port']
        if type(port) is not int or not 1 <= port <= 65535:
            raise ValueError('invalid preview port')
        refresh = getattr(self.backend, 'ensure_v2_network_policy', None)
        if refresh is not None:
            refresh()
        if port != self.config.get('preview_port'):
            raise ValueError('preview port not allowed')
        # Runs share the Pod and its one preview port: a port that already answers
        # belongs to another process, whose server must not be shown as this one.
        pod = self._get('pod', scratch_resource_name(scratch))
        uid = (pod or {}).get('metadata', {}).get('uid')
        if uid and any(self._port_answers(scratch, uid, port, host)
                       for host in ('127.0.0.1', (pod or {}).get('status', {}).get('podIP')) if host):
            raise ExecutionPodError('preview_start_failed',
                                    f'Port {port} is already serving in this Scratch Space; stop that process first')
        command = {key: request[key] for key in ('operation_id', 'scratch_space_id', 'run_id', 'code', 'proxy_credential')
                   if key in request}
        command.update(background=True, cwd=request.get('cwd') or '.')
        self.submit_command(command, kind='preview')
        original_uid = self._read_command(operation)['pod_uid']
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            record = self.command_result(operation)
            pod = self._get('pod', scratch_resource_name(scratch))
            if not pod or pod.get('status', {}).get('phase') != 'Running':
                break
            uid, address = pod.get('metadata', {}).get('uid'), pod.get('status', {}).get('podIP')
            if uid != original_uid:
                break
            if record.get('status') not in ('queued', 'running'):
                break
            if record.get('status') == 'running' and uid and address:
                # Probe the Pod address the preview gateway uses: a loopback-only
                # server is not ready for it.
                if self._port_answers(scratch, uid, port, address):
                    parsed = ipaddress.ip_address(address)
                    host = '[' + str(parsed) + ']' if parsed.version == 6 else str(parsed)
                    return {'operation_id': operation, 'address': f'{host}:{port}'}
            time.sleep(.5)
        tail = []
        for stream in ('stdout', 'stderr'):
            try:
                chunk = self.background_log({'operation_id': operation, 'stream': stream, 'limit': 4096})
                tail.append(base64.b64decode(chunk['data'], validate=True).decode('utf-8', errors='replace'))
            except Exception:
                pass
        stop_failed = False
        try:
            self.stop_background(operation)
        except Exception:
            stop_failed = True
        message = ''.join(tail)[-8192:]
        credential = request.get('proxy_credential')
        if isinstance(credential, str) and credential:
            message = message.replace(credential, '[redacted]')
        if stop_failed:
            message += '\nCould not stop the preview process.'
        raise ExecutionPodError('preview_start_failed', message or 'Preview did not accept connections within 60 seconds')

    def _port_answers(self, scratch, uid, port, host):
        import json
        from control import scratch_resource_name
        try:
            probe = self.backend.kube(['exec', scratch_resource_name(scratch), '-c', 'execution', '--'] +
                                      helper_command('port-check', uid, str(port), host), check=False, timeout=5)
            return probe.returncode == 0 and json.loads(probe.stdout).get('ready') is True
        except Exception:
            return False

    def clear_thread(self, request):
        from control import scratch_resource_name

        settings = request['settings']
        scratch = settings['scratch_space_id']
        name = scratch_resource_name(scratch)
        with self._space_lock(scratch):
            if self._get('pvc', name) is None:
                return {'cleared': True}
            self.ensure(settings)
            return self.file_operation('clear_thread', {
                'scratch_space_id': scratch, 'thread_id': request['thread_id'],
            })

    def file_operation(self, operation, request):
        import json
        from control import scratch_resource_name
        if operation not in ('read', 'search', 'write', 'manifest', 'patch', 'clear_thread'):
            raise ValueError('invalid file operation')
        scratch = request['scratch_space_id']
        name = scratch_resource_name(scratch)
        with self._space_lock(scratch):
            pod = self._get('pod', name)
            if not pod or pod.get('status', {}).get('phase') != 'Running':
                raise ExecutionPodError('pod_unavailable', 'Execution Pod is not running')
            uid = pod.get('metadata', {}).get('uid')
            if not self.admitted or not uid or self._admitted_uid(scratch) != uid:
                raise ExecutionPodError('admission_unverified', 'Execution Pod has not passed admission')
            result = self.backend.kube(['exec', '-i', name, '-c', 'execution', '--'] +
                                       helper_command('files', operation, uid),
                                       data=json.dumps(request, separators=(',', ':')).encode(), check=False, timeout=45)
            if result.returncode:
                # The Pod is running, so the helper may have changed files before the exec
                # failed: unlike `pod_unavailable`, this is no proof that nothing happened,
                # and callers must not repeat a patch or write on it.
                raise ExecutionPodError('file_helper_failed', 'File helper failed; the call may have taken effect')
            response = json.loads(result.stdout)
            if 'error' in response:
                raise ExecutionPodError(response['error']['code'], response['error']['message'])
            return response['result']

    def background_log(self, request):
        import json
        operation_id = request['operation_id']
        record = self._read_command(operation_id)
        if not record.get('background'):
            raise ExecutionPodError('not_background', 'Command is not a background process')
        stream, offset, limit = request['stream'], request.get('offset'), request['limit']
        if (stream not in ('stdout', 'stderr') or type(limit) is not int or not 1 <= limit <= 262144
                or (offset is not None and (type(offset) is not int or offset < 0))):
            raise ValueError('invalid log request')
        result = self.backend.kube(['exec', record['pod'], '-c', 'execution', '--'] +
            helper_command('output', operation_id, stream, 'tail' if offset is None else str(offset), str(limit)),
            check=False, timeout=45)
        if result.returncode != 0:
            raise ExecutionPodError('log_unavailable', 'Background output unavailable')
        return json.loads(result.stdout)

    def stop_background(self, operation_id):
        record = self._read_command(operation_id)
        if not record.get('background'):
            raise ExecutionPodError('not_background', 'Command is not a background process')
        if record['status'] == 'interrupted' and not record.get('docker_enabled'):
            return self.command_result(operation_id)
        return self._stop_command(operation_id)

    def _stop_command(self, operation_id):
        import json
        import time
        record = self._read_command(operation_id)
        with self.lock:
            future = self.futures.get(operation_id)
        if record['status'] == 'queued' and future and future.cancel():
            return self.command_result(operation_id, retry_stop=False)
        if (record['status'] in ('completed', 'timed_out', 'stopped', 'cancelled', 'interrupted')
                and not (record['status'] == 'interrupted' and record.get('docker_enabled'))):
            return self.command_result(operation_id, retry_stop=False)
        interrupted_docker = record['status'] == 'interrupted' and record.get('docker_enabled')
        pod = self._get('pod', record['pod'])
        if not pod or pod.get('metadata', {}).get('uid') != record['pod_uid']:
            if interrupted_docker:
                return self.command_result(operation_id, retry_stop=False)
            raise ExecutionPodError('pod_interrupted', 'The original Execution Pod is unavailable')
        def docker_running():
            observed = self.backend.kube(['exec', record['pod'], '-c', 'execution', '--'] +
                                         helper_command('running', operation_id), check=False, timeout=5)
            if observed.returncode != 0:
                raise RuntimeError('Could not check Docker command state')
            running = json.loads(observed.stdout).get('running')
            if type(running) is not bool:
                raise ValueError('Invalid Docker command state')
            return running
        def docker_finished():
            # An interrupted Docker command whose supervisor has not published its marker yet is not
            # finished. Claiming the operation first fences that supervisor out, so it can never start
            # a command that was meant to stop.
            observed = self.backend.kube(['exec', record['pod'], '-c', 'execution', '--'] +
                                         helper_command('claim', operation_id), check=False, timeout=5)
            if observed.returncode != 0:
                raise RuntimeError('Could not fence Docker command start')
            created = json.loads(observed.stdout).get('created')
            if type(created) is not bool:
                raise ValueError('Invalid Docker command claim')
            if created:
                return True
            # A claim that already exists shows only that the supervisor started. It may have claimed
            # without publishing `.running` yet, so the command counts as finished only when that marker
            # stays absent for the same window the stop below waits for it. A Docker command's result
            # files are never evidence here.
            # A cancel holds the Space's lock, so only the first stop waits out the window. A later one
            # still looks once: a supervisor that published late must still get its stop.
            window = time.monotonic() + (0 if record.get('fence_window_elapsed') else 10)
            while not docker_running():
                if time.monotonic() >= window:
                    return True
                time.sleep(.1)
            return False
        if interrupted_docker and docker_finished():
            self._update_command(operation_id, status='interrupted', terminal_at=record.get('terminal_at', time.time()),
                                 fence_window_elapsed=True)
            return self.command_result(operation_id, retry_stop=False)
        # A future can leave the queue before its in-Pod supervisor publishes
        # the stop marker. Wait for that short startup window instead of losing
        # a stop request at the queued-to-running transition.
        deadline = time.monotonic() + 10
        while True:
            try:
                self.backend.kube(['exec', record['pod'], '-c', 'execution', '--'] + helper_command('stop', operation_id))
                return {'operation_id': operation_id, 'status': 'stopping'}
            except Exception as error:
                current = self.command_result(operation_id, retry_stop=False)
                if current['status'] in ('completed', 'timed_out', 'stopped', 'cancelled'):
                    return current
                if current['status'] == 'interrupted':
                    if not interrupted_docker:
                        return current
                    try:
                        if not docker_running():
                            return current
                    except Exception:
                        pass
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
                timestamp = max(timestamp, state.get('last_command_at', timestamp))
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
        self._ensure_journal_index()
        with self.lock:
            spaces_with_records = self.journal_index.keys() | self.session_index.keys()
        for scratch_id in spaces_with_records:
            try:
                self.prune_space(scratch_id, now=current)
            except Exception as error:
                import sys
                print('Execution Pod journal cleanup failed: ' + str(error), file=sys.stderr, flush=True)
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
                            if self.foreground_jobs.get(scratch_id, 0):
                                continue
                    # The helper image only changes across a restart, so one look at
                    # each Space after it is enough; a failed look is retried.
                    outdated = False
                    if not unconfirmed and scratch_id in self.helper_checks:
                        pod = self._get('pod', scratch_resource_name(scratch_id))
                        outdated = bool(pod and self._old_helper_image(pod))
                        if not outdated:
                            self.helper_checks.discard(scratch_id)
                    if not unconfirmed and not outdated:
                        with self.lock:
                            if current - space['last_command_at'] < 1800:
                                continue
                            viewers = self.viewers.get(scratch_id, {})
                            self.viewers[scratch_id] = {key: expiry for key, expiry in viewers.items() if expiry > current}
                            if self.viewers[scratch_id]:
                                continue
                    if outdated or unconfirmed or self._get('pod', scratch_resource_name(scratch_id)):
                        self.stop(scratch_id)
                        self.helper_checks.discard(scratch_id)
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

    def _session_stop_path(self, scratch_space_id, run_id):
        return self._python_session_path(scratch_space_id, run_id).with_suffix('.stopped')

    def _check_sessions_open(self, scratch_space_id, run_id):
        if self._session_stop_path(scratch_space_id, run_id).exists():
            raise ExecutionPodError('command_refused', "This Run's sessions have ended")

    def stop_sessions(self, scratch_space_id, run_id):
        return self.stop_run(scratch_space_id, run_id, RUN_SESSION_KINDS, seal_sessions=True)

    def start_python(self, scratch_space_id, run_id, proxy_credential=None):
        with self._space_lock(scratch_space_id):
            return self._start_python(scratch_space_id, run_id, proxy_credential)

    def _start_python(self, scratch_space_id, run_id, proxy_credential=None, *, expected_session_id=None):
        import json
        import shlex
        import uuid
        from control import save, scratch_resource_name

        if not self.admitted:
            raise ExecutionPodError('admission_unverified', 'Execution Pod admission has not been verified')
        self._check_sessions_open(scratch_space_id, run_id)
        path = self._python_session_path(scratch_space_id, run_id)
        prior = json.loads(path.read_bytes()) if path.exists() else None
        reset_message = 'The expected Python session ended or was replaced. Rebuild its state before retrying.'
        if expected_session_id is not None and (not prior or prior['operation_id'] != expected_session_id):
            raise ExecutionPodError('session_reset', reset_message)
        if prior:
            record = self._read_command(prior['operation_id'])
            pod = self._get('pod', scratch_resource_name(scratch_space_id))
            same_pod = bool(pod and pod.get('metadata', {}).get('uid') == record.get('pod_uid'))
            if record['status'] == 'interrupted' and same_pod:
                observed = self.command_result(prior['operation_id'], retry_stop=False)
                if observed['status'] == 'interrupted':
                    raise ExecutionPodError('session_interrupted',
                                            'The previous Python session outcome is unknown; it cannot be restarted yet')
                record = self._read_command(prior['operation_id'])
            if record['status'] in ('queued', 'running') and same_pod:
                stopping = record.get('interpreter_stopping')
                # The Pod can record an interpreter exit before the Runner's journal catches up.
                check_outcome = stopping or (expected_session_id is not None and record['status'] == 'running')
                outcome = self._interpreter_outcome(prior['operation_id']) if check_outcome else None
                if outcome not in ('completed', 'timed_out', 'stopped'):
                    if stopping:
                        raise ExecutionPodError('session_interrupted',
                                                'The previous Python session is being stopped; it cannot be restarted yet')
                    return prior
            replaced = not same_pod and record['status'] in ('running', 'queued', 'interrupted')
        else:
            replaced = False
        if expected_session_id is not None:
            raise ExecutionPodError('session_reset', reset_message)
        self._check_python(scratch_space_id)
        operation_id = str(uuid.uuid4())
        socket_path = '/run/aidash/' + operation_id + '.sock'
        code = 'exec ' + shlex.join(['python3', '-I', '/.aidash-helpers/python_session.py', 'serve', socket_path])
        self.submit_command({'operation_id': operation_id, 'scratch_space_id': scratch_space_id,
                             'run_id': run_id, 'code': code, 'background': True,
                             'proxy_credential': proxy_credential}, kind='python_session')
        session = {'operation_id': operation_id, 'scratch_space_id': scratch_space_id,
                   'run_id': run_id, 'socket_path': socket_path}
        self._ensure_journal_index()
        save(path, session)
        with self.lock:
            self.session_index.setdefault(scratch_space_id, set()).add(path.name)
        return dict(session, session_replaced=True) if replaced else session

    def _check_python(self, scratch_space_id):
        import json
        from control import scratch_resource_name
        pod = self._get('pod', scratch_resource_name(scratch_space_id))
        if not pod or pod.get('status', {}).get('phase') != 'Running':
            raise ExecutionPodError('pod_unavailable', 'Execution Pod is not running')
        uid = pod.get('metadata', {}).get('uid')
        containers = pod.get('spec', {}).get('containers', [])
        execution = next((item for item in containers if item.get('name') == 'execution'), {})
        caps = execution.get('securityContext', {}).get('capabilities', {}).get('add', [])
        mode = 'docker' if 'NET_ADMIN' in caps else 'normal'
        probe = self.backend.kube(['exec', scratch_resource_name(scratch_space_id), '-c', 'execution', '--'] +
                                  helper_command('python-check', mode, uid), check=False, timeout=20)
        if probe.returncode != 0:
            raise ExecutionPodError('pod_unavailable', 'Could not check the image Python interpreter')
        if json.loads(probe.stdout).get('available') is not True:
            raise ExecutionPodError('python_unavailable', 'This image has no python3. Install it with apt (for example `apt-get install -y python3`) and try again.')

    def python_cell(self, request):
        with self._space_lock(request['scratch_space_id']):
            return self._python_cell(request)

    def _python_cell(self, request):
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
                return self.command_result(operation_id, observe_late=False)
        code = request['code']
        if not isinstance(code, str) or '\x00' in code or len(code.encode('utf-8')) > 65536:
            raise ValueError('Python code exceeds the limit')
        if any(not isinstance(request.get(key, '.'), str) or '\x00' in request.get(key, '.') for key in ('cwd', 'run_id')):
            raise ValueError('Python request contains NUL')
        seconds = execution_timeout(request)
        session = self._start_python(request['scratch_space_id'], request['run_id'], request.get('proxy_credential'),
                                     expected_session_id=request.get('expected_session_id'))
        output_limit = self.config.get('output_bytes', getattr(self.backend, 'config', {}).get('output_bytes', 8388608))
        cell_limit = max(1, min(8388608, (output_limit - 4096) // 6))
        encoded = base64.b64encode(code.encode('utf-8')).decode('ascii')
        cwd_encoded = base64.b64encode(request.get('cwd', '.').encode('utf-8')).decode('ascii')
        command = shlex.join(python_helper_command('cell', session['socket_path'], str(seconds), str(cell_limit), encoded, cwd_encoded))
        return self.submit_command({'operation_id': operation_id, 'scratch_space_id': request['scratch_space_id'],
                                    'run_id': request['run_id'], 'code': command,
                                    'cwd': request.get('cwd', '.'), 'timeout_seconds': seconds},
                                   kind='python_cell', session_operation_id=session['operation_id'],
                                   client_digest=original_digest, session_replaced=session.get('session_replaced', False))

    def stop_python(self, scratch_space_id, run_id):
        return self.stop_run(scratch_space_id, run_id, {'python_session'})


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


def drain_process(argv, environment, output_limit, timeout, on_output=None, *, frame_token=None, input_data=None):
    """Drain a trusted transport without buffering unbounded command output."""
    import os
    import selectors
    import signal
    import subprocess
    import time

    output = BoundedOutput(output_limit)
    frame = _ResultFrameReader(frame_token, output) if frame_token else None
    process = subprocess.Popen(argv, stdin=subprocess.PIPE if frame or input_data else subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, env=environment, start_new_session=True)
    if frame or input_data:
        try:
            if frame:
                process.stdin.write((frame_token + '\n').encode('ascii'))
            if input_data:
                process.stdin.write(input_data)
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


def preview_ingress_from_legacy(policies):
    """Read the gateway identity and port from the deployed v1 Preview policy."""
    candidates = [policy for policy in policies if policy.get('metadata', {}).get('name', '').endswith('-preview-sandbox')]
    if not candidates:
        return None
    if len(candidates) != 1:
        raise ValueError('multiple Preview gateway policies')
    policy = candidates[0]
    release = policy['metadata']['name'][:-len('-preview-sandbox')]
    spec = policy.get('spec', {})
    selector = spec.get('podSelector', {})
    expressions = selector.get('matchExpressions', [])
    if (not release or spec.get('policyTypes') != ['Ingress'] or spec.get('egress') or
            selector.get('matchLabels') != {'aidash-sandbox': 'true'} or
            len(expressions) != 2 or
            {(item.get('key'), item.get('operator')) for item in expressions} !=
            {('aidash-v2', 'DoesNotExist'), ('aidash-journal', 'DoesNotExist')} or
            any(set(item) != {'key', 'operator'} for item in expressions)):
        raise ValueError('invalid Preview gateway policy selector')
    rules = spec.get('ingress')
    if not isinstance(rules, list) or len(rules) != 1:
        raise ValueError('invalid Preview gateway ingress')
    rule = rules[0]
    peers, ports = rule.get('from'), rule.get('ports')
    if set(rule) != {'from', 'ports'} or not isinstance(peers, list) or len(peers) != 1 or not isinstance(ports, list) or len(ports) != 1:
        raise ValueError('invalid Preview gateway ingress')
    peer, port = peers[0], ports[0]
    namespace = peer.get('namespaceSelector', {}).get('matchLabels', {})
    selector = peer.get('podSelector', {}).get('matchLabels', {})
    if (set(peer) != {'namespaceSelector', 'podSelector'} or
            set(peer['namespaceSelector']) != {'matchLabels'} or set(peer['podSelector']) != {'matchLabels'} or
            set(namespace) != {'kubernetes.io/metadata.name'} or not isinstance(namespace['kubernetes.io/metadata.name'], str) or
            not namespace['kubernetes.io/metadata.name'] or
            set(selector) != {'app.kubernetes.io/instance', 'app.kubernetes.io/component'} or
            selector['app.kubernetes.io/instance'] != release or selector['app.kubernetes.io/component'] not in ('server', 'preview') or
            set(port) - {'protocol', 'port'} or port.get('protocol', 'TCP') != 'TCP' or
            type(port.get('port')) is not int or not 1 <= port['port'] <= 65535):
        raise ValueError('invalid Preview gateway identity or port')
    return {'preview_namespace': namespace['kubernetes.io/metadata.name'],
            'preview_selector': selector, 'preview_port': port['port']}


def v2_preview_ingress(config):
    port, namespace, selector = (config.get(key) for key in ('preview_port', 'preview_namespace', 'preview_selector'))
    if port is None and namespace is None and selector is None:
        return []
    if (type(port) is not int or not 1 <= port <= 65535 or not isinstance(namespace, str) or not namespace or
            not isinstance(selector, dict) or set(selector) != {'app.kubernetes.io/instance', 'app.kubernetes.io/component'} or
            not isinstance(selector['app.kubernetes.io/instance'], str) or not selector['app.kubernetes.io/instance'] or
            selector['app.kubernetes.io/component'] not in ('server', 'preview')):
        raise ValueError('invalid Preview gateway configuration')
    return [{'from': [{'namespaceSelector': {'matchLabels': {'kubernetes.io/metadata.name': namespace}},
                      'podSelector': {'matchLabels': selector}}], 'ports': [{'protocol': 'TCP', 'port': port}]}]


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
                     'policyTypes': ['Ingress', 'Egress'], 'ingress': v2_preview_ingress(config),
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
    metadata = {'name': name, 'namespace': config['namespace'],
                'labels': dict(labels, **{'aidash.run/v2-canary': 'true'})}
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
              'metadata': {'name': name, 'namespace': config['namespace'],
                           'labels': {'aidash.run/v2-canary': 'true'}},
              'spec': {'podSelector': {'matchLabels': labels}, 'policyTypes': ['Ingress', 'Egress'],
                       'ingress': [{'ports': [{'protocol': 'TCP', 'port': 18080},
                                              {'protocol': 'UDP', 'port': 18081}]}], 'egress': [{}]}}
    return policy, pod


def verify_positive_controls(report, *, metadata_server_absent=False):
    """Refuse negative evidence unless each target was independently reachable."""
    if type(metadata_server_absent) is not bool:
        raise ValueError('metadata_server_absent must be a boolean')
    missing = [name for name in ('peer', 'node', 'metadata', 'internet', 'gateway')
               if not (name == 'metadata' and metadata_server_absent) and report.get(name) is not True]
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
    allowed_ingress = expected['spec'].get('ingress', [])
    found = False
    found_ingress = []
    for policy in policies:
        spec = policy['spec']
        if not _selector_matches(spec.get('podSelector', {}), labels):
            continue
        types = spec.get('policyTypes')
        ingress = types is None or 'Ingress' in types
        egress = ('Egress' in types) if types is not None else 'egress' in spec
        if ingress and any(rule not in allowed_ingress for rule in spec.get('ingress', [])):
            raise ValueError('network policy admits unexpected ingress')
        if ingress:
            found_ingress.extend(spec.get('ingress', []))
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
    if any(rule not in found_ingress for rule in allowed_ingress):
        raise ValueError('network policy does not admit Preview gateway ingress')
