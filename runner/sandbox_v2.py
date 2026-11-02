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
    mounts = [('/proc', 'proc'), ('/dev', 'dev'), ('/tmp', 'tmp'),
              ('/scratch-system/scratch', 'scratch')]
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


if __name__ == '__main__':
    import sys
    if sys.argv[1:] == ['init']:
        from execution_pod import initialize_system_root
        initialize_system_root('/', '/scratch-system', reset=os.environ.get('AIDASH_RESET_SYSTEM') == 'true')
        raise SystemExit(0)
    if sys.argv[1:] == ['bootstrap']:
        import signal
        mount_rootfs(os.environ.get('AIDASH_DOCKER_ENABLED') == 'true')
        while True:
            signal.pause()
    if len(sys.argv) >= 6 and sys.argv[1] == 'chroot' and sys.argv[2] in ('docker', 'normal'):
        chroot_command(sys.argv[3], sys.argv[4], sys.argv[5:], sys.argv[2] == 'docker')
    if len(sys.argv) < 3 or sys.argv[1] not in ('docker', 'normal'):
        raise SystemExit('usage: sandbox_v2.py normal|docker executable [args...]')
    drop_user_capabilities(sys.argv[1] == 'docker')
    os.execvp(sys.argv[2], sys.argv[2:])
