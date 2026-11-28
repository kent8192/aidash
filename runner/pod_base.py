"""Stable Execution Pod startup and privilege reduction."""
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
        raise ValueError('cwd must be relative to /work')
    validate_system_root(root)
    import resource
    process_limit = int(os.environ['AIDASH_PROCESS_LIMIT'])
    if process_limit < 32:
        raise ValueError('invalid guest process ceiling')
    resource.setrlimit(resource.RLIMIT_NPROC, (process_limit, process_limit))
    os.chroot(root)
    enter_working_directory('/work', cwd)
    drop_user_capabilities(docker_enabled)


def enter_working_directory(root, cwd):
    """Create Aidash's working folders without following replaced directory links."""
    import uuid

    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW
    parts = cwd.split('/')
    task_directory = False
    if len(parts) == 2 and parts[0] in ('threads', 'duties'):
        try:
            task_directory = str(uuid.UUID(parts[1])) == parts[1]
        except ValueError:
            pass

    directory = os.open(root, flags)
    try:
        try:
            os.mkdir('shared', dir_fd=directory)
        except FileExistsError:
            pass
        if task_directory:
            for part in parts:
                try:
                    os.mkdir(part, dir_fd=directory)
                except FileExistsError:
                    pass
                child = os.open(part, flags, dir_fd=directory)
                os.close(directory)
                directory = child
            os.fchdir(directory)
        else:
            target = os.path.realpath(os.path.join(root, cwd))
            if os.path.commonpath([root, target]) != root:
                raise ValueError('cwd must resolve beneath /work')
            os.chdir(target)
    finally:
        os.close(directory)


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
                   if not key.startswith(('AIDASH_', 'KUBERNETES_')) and key not in
                   ('HTTP_PROXY', 'HTTPS_PROXY', 'http_proxy', 'https_proxy', 'NODE_USE_ENV_PROXY')}
    for key in ('HTTP_PROXY', 'HTTPS_PROXY', 'http_proxy', 'https_proxy', 'NO_PROXY', 'no_proxy'):
        if key in os.environ:
            environment[key] = os.environ[key]
    if 'HTTPS_PROXY' in os.environ:
        environment['NODE_USE_ENV_PROXY'] = '1'
    return environment


def chroot_command(root, cwd, argv, docker_enabled):
    """Enter the persistent system root and permanently reduce user authority."""
    if not argv:
        raise ValueError('command is required')
    validate_system_root(root)
    environment = image_environment(root)
    enter_user_root(root, cwd, docker_enabled)
    os.execvpe(argv[0], argv, environment)



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
              ('/scratch-system/scratch', 'scratch'), ('/scratch-system/scratch', 'work'),
              ('/.aidash-helpers', '.aidash-helpers')]
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
    if os.getpid() == 1:
        signal.signal(signal.SIGCHLD, signal.SIG_IGN)
    while True:
        signal.pause()



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


if __name__ == '__main__':
    import sys
    from pathlib import Path
    if sys.argv[1:] == ['bootstrap']:
        bootstrap()
    elif sys.argv[1:] == ['ready']:
        raise SystemExit(0 if bootstrap_ready() else 1)
    elif sys.argv[1:] == ['init']:
        copied = initialize_system_root('/', '/scratch-image',
                                        reset=os.environ.get('AIDASH_RESET_SYSTEM') == 'true',
                                        image_identity=os.environ['AIDASH_IMAGE_ID'])
        if copied or not (Path('/scratch-image') / 'environment.json').exists():
            save_image_environment('/scratch-image', os.environ)
    elif len(sys.argv) >= 6 and sys.argv[1] == 'chroot' and sys.argv[2] in ('docker', 'normal'):
        chroot_command(sys.argv[3], sys.argv[4], sys.argv[5:], sys.argv[2] == 'docker')
    else:
        raise SystemExit('invalid Pod base operation')
