"""Trusted upstream gVisor installer for Ubuntu/containerd Cluster Nodes."""

import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import ssl
import subprocess
import time
import tomllib
import urllib.request

GVISOR_VERSION = '20260921.0'
GVISOR_SHA = '3dd478770dd751d09c257ba14d739b179348a36c5f2d9e954b773f5f90bff646'
# ARM is for disposable local verification only; GKE Ubuntu N2 uses x86_64.
ARCHIVE_HASHES = {'x86_64': GVISOR_SHA,
                 'aarch64': 'edf717346495ec5e995551e84e47beb5d0ecbfd872d9773ffb554aa24a158c4e'}
REQUIRED = {'runsc', 'containerd-shim-runsc-v1', 'gvisor-bin/gvisor_sentry'}
# Executables the node guard identifies a live sandbox by (`samefile` on /proc/<pid>/exe).
SENTRY_BINARIES = ('usr/local/bin/runsc', 'usr/local/bin/gvisor-bin/gvisor_sentry')
# containerd starts this per sandbox Pod; a running shim can still launch a Sentry.
SHIM_BINARY = 'usr/local/bin/containerd-shim-runsc-v1'


def relative(name):
    path = PurePosixPath(name)
    if path.is_absolute() or '..' in path.parts or not path.parts or '\\' in name:
        raise ValueError('unsafe runtime archive path')
    return str(path)


def destination(root, name):
    path = root / relative(name)
    # Do not let an existing host symlink redirect a privileged write.
    for part in (path, *path.parents):
        if part.is_symlink():
            raise ValueError('runtime destination must not contain symlinks')
        if part == root:
            break
    return path


def write(path, data, mode=0o644):
    if path.is_symlink():
        raise ValueError('runtime destination must be regular')
    if path.is_file() and path.read_bytes() == data and path.stat().st_mode & 0o777 == mode:
        return False
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + '.new')
    fd = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, mode)
    with os.fdopen(fd, 'wb') as stream:
        os.fchmod(stream.fileno(), mode)
        stream.write(data)
        stream.flush()
        os.fsync(stream.fileno())
    temporary.replace(path)
    return True


def matches(path, checksum):
    return (not path.is_symlink() and path.is_file()
            and path.stat().st_mode & 0o777 == 0o755
            and hashlib.sha256(path.read_bytes()).hexdigest() == checksum)


def installed(root, version, checksum):
    receipt = root / 'usr/local/share/aidash/gvisor.json'
    try:
        value = json.loads(receipt.read_bytes())
        files = value['files']
        return (value['version'] == version and value['archive_sha'] == checksum
                and REQUIRED <= files.keys()
                and all(matches(destination(root / 'usr/local/bin', name), digest)
                        for name, digest in files.items()))
    except (OSError, ValueError, KeyError, TypeError):
        return False


def archive_files(data, checksum):
    import tarfile
    if hashlib.sha256(data).hexdigest() != checksum:
        raise ValueError('runtime archive checksum mismatch')
    files = {}
    with tarfile.open(fileobj=io.BytesIO(data), mode='r:bz2') as archive:
        for member in archive.getmembers():
            name = relative(member.name)
            if member.isdir():
                continue
            if not member.isfile() or name in files:
                raise ValueError('runtime archive must contain unique regular files')
            files[name] = archive.extractfile(member).read()
    if not REQUIRED <= files.keys():
        raise ValueError('incomplete runtime archive')
    return files


def configure(root, runsc_root='/run/containerd/runsc'):
    if not runsc_root.startswith('/') or '..' in PurePosixPath(runsc_root).parts:
        raise ValueError('absolute runsc root required')
    config_path = root / 'etc/containerd/config.toml'
    text = config_path.read_text()
    config = tomllib.loads(text)
    if config.get('version') != 2:
        raise ValueError('only verified containerd configuration version 2 is supported')
    cri = config.get('plugins', {}).get('io.containerd.grpc.v1.cri', {})
    runtime = cri.get('containerd', {}).get('runtimes', {}).get('runsc')
    expected = {'runtime_type': 'io.containerd.runsc.v1',
                'options': {'TypeUrl': 'io.containerd.runsc.v1.options',
                            'ConfigPath': '/etc/containerd/runsc.toml'}}
    if runtime is not None and runtime != expected:
        raise ValueError('existing runsc registration differs from verified configuration')
    changed = write(root / 'etc/containerd/runsc.toml',
                    (f'root = {json.dumps(runsc_root)}\n[runsc_config]\nplatform = "systrap"\n').encode())
    if runtime is None:
        text += ('\n[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.runsc]\n'
                 'runtime_type = "io.containerd.runsc.v1"\n'
                 '[plugins."io.containerd.grpc.v1.cri".containerd.runtimes.runsc.options]\n'
                 'TypeUrl = "io.containerd.runsc.v1.options"\n'
                 'ConfigPath = "/etc/containerd/runsc.toml"\n')
        changed |= write(config_path, text.encode())
    return changed


def live_sentries(root, proc='/proc', binaries=SENTRY_BINARIES):
    """PIDs running an installed Sentry executable, judged as the node guard does."""
    targets = [root / name for name in binaries if (root / name).exists()]
    pids = []
    for entry in Path(proc).iterdir():
        if not entry.name.isdigit():
            continue
        try:
            if any(os.path.samefile(entry / 'exe', target) for target in targets):
                pids.append(int(entry.name))
        except OSError:
            continue  # the process exited while it was being inspected
    return pids


def live_sandboxes(root, proc='/proc'):
    """Sentries, plus shims that started before the fence and may still launch one."""
    return live_sentries(root, proc, SENTRY_BINARIES + (SHIM_BINARY,))


def fence(root):
    """Stop containerd from starting any new sandbox before the drain's scans.

    Withdrawing the admission label only stops scheduling: kubelet does not
    recheck RuntimeClass scheduling for a Pod already bound to this node. Every
    sandbox starts by executing the shim, and execve needs an execute bit even
    for root, so clearing them refuses such Pods until the replacement restores
    0755. The guard's runsc and Sentry identities are left untouched.
    """
    shim = destination(root, SHIM_BINARY)
    if shim.is_file():
        os.chmod(shim, 0o644)


def drain(root, seconds, sleep=time.sleep, clock=time.monotonic, live=live_sandboxes):
    """Wait for live sandboxes to exit; refuse to proceed while any remain.

    A running Sentry keeps its old executable inode. Replacing the file would
    leave the node guard unable to recognize, and so unable to kill, that process.
    One empty scan is confirmed by another after a pause, so a launch already
    past execve's permission check when the fence landed is also observed.
    """
    end = clock() + seconds
    confirmed = False
    while True:
        if pids := live(root):
            confirmed = False
            if clock() >= end:
                raise RuntimeError(f'live gVisor sandboxes still use the installed runtime: {sorted(pids)[:8]}')
        elif confirmed:
            return
        else:
            confirmed = True
        sleep(5)


def install(root, version, checksum, arch, download, restart, runsc_root='/run/containerd/runsc',
            drain=lambda root: None):
    if not installed(root, version, checksum):
        url = f'https://storage.googleapis.com/gvisor/releases/release/{version}/{arch}/gvisor.tar.bz2'
        files = archive_files(download(url), checksum)
        # Validate every destination before changing any executable.
        paths = {name: destination(root / 'usr/local/bin', name) for name in files}
        # The fence persists if the drain fails: `installed` then requires a
        # retry, and only writing the verified runtime makes the shim executable.
        fence(root)
        drain(root)
        for name, data in files.items():
            write(paths[name], data, 0o755)
        write(root / 'usr/local/share/aidash/gvisor.json', json.dumps({
            'version': version, 'archive_sha': checksum,
            'files': {name: hashlib.sha256(data).hexdigest() for name, data in files.items()}
        }, sort_keys=True).encode())
    if not installed(root, version, checksum):
        raise ValueError('pinned gVisor installation verification failed')
    if configure(root, runsc_root):
        restart()


def label(values):
    """This KSA has cluster-wide get/patch nodes; no workload permissions."""
    account = Path('/var/run/secrets/kubernetes.io/serviceaccount')
    host = os.environ['KUBERNETES_SERVICE_HOST']
    port = os.environ['KUBERNETES_SERVICE_PORT']
    name = os.environ['CLUSTER_NODE_NAME']
    url = f'https://{host}:{port}/api/v1/nodes/{name}'
    headers = {'Authorization': 'Bearer ' + (account / 'token').read_text().strip(),
               'Content-Type': 'application/merge-patch+json'}
    data = json.dumps({'metadata': {'labels': values}}).encode()
    context = ssl.create_default_context(cafile=str(account / 'ca.crt'))
    with urllib.request.urlopen(urllib.request.Request(url, data=data, headers=headers,
                                                      method='PATCH'), context=context, timeout=30):
        pass


def main():
    root = Path(os.environ.get('AIDASH_HOST_ROOT', '/host'))
    version = os.environ.get('AIDASH_GVISOR_VERSION', GVISOR_VERSION)
    arch = os.uname().machine
    checksum = os.environ.get('AIDASH_GVISOR_SHA', ARCHIVE_HASHES[arch])
    runsc_root = os.environ.get('AIDASH_RUNSC_ROOT', '/run/containerd/runsc')
    # The admission label gates new sandboxes (RuntimeClass); the installed label
    # keeps the node-local guard scheduled and is never withdrawn once verified.
    admission = os.environ.get('AIDASH_GVISOR_LABEL', 'aidash.run/gvisor')
    installed = os.environ.get('AIDASH_GVISOR_INSTALLED_LABEL', 'aidash.run/gvisor-installed')
    ready = Path(os.environ.get('AIDASH_GVISOR_READY', '/tmp/gvisor-ready'))
    # Existing cells end by maximum_seconds/idle_seconds; allow one idle interval plus margin.
    drain_seconds = int(os.environ.get('AIDASH_GVISOR_DRAIN_SECONDS', '3600'))

    def download(url):
        with urllib.request.urlopen(url, timeout=120) as response:
            return response.read()

    def restart():
        subprocess.run(['nsenter', '-t', '1', '-m', '-u', '-i', '-n', '-p', '--',
                        'systemctl', 'restart', 'containerd'], check=True, timeout=120)

    try:
        # Withdraw only admission during startup/repair. The guard must keep
        # watching live sandboxes while a runtime replacement drains them.
        label({admission: None})
        install(root, version, checksum, arch, download, restart, runsc_root,
                drain=lambda host: drain(host, drain_seconds))
        label({admission: version, installed: 'true'})
        ready.touch()
    except Exception:
        ready.unlink(missing_ok=True)
        label({admission: None})
        raise
    while True:
        time.sleep(60)


if __name__ == '__main__':
    main()
