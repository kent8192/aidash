"""Inject the controller's interpreter and helper programs into an emptyDir."""
import os
from pathlib import Path
import platform
import resource
import shutil
import sys
import time


def wait_for_guard(receipt, *, timeout=180):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if receipt.is_file() and not receipt.is_symlink():
            return
        time.sleep(.05)
    raise TimeoutError('node guard did not admit the Execution Pod')


def main():
    process_limit = int(os.environ['AIDASH_PROCESS_LIMIT'])
    if process_limit < 32:
        raise ValueError('invalid guest process ceiling')
    resource.setrlimit(resource.RLIMIT_NPROC, (process_limit, process_limit))
    destination = Path('/helpers')
    arch = platform.machine()
    if arch not in ('aarch64', 'x86_64'):
        raise RuntimeError('unsupported helper architecture')
    library = Path('/lib') / (arch + '-linux-gnu')
    loader = Path('/lib/ld-linux-aarch64.so.1' if arch == 'aarch64'
                  else '/lib64/ld-linux-x86-64.so.2')
    (destination / 'python/bin').mkdir(parents=True, exist_ok=True)
    (destination / 'python/lib').mkdir(parents=True, exist_ok=True)
    shutil.copy2(loader.resolve(), destination / 'loader')
    shutil.copytree(library, destination / 'lib', symlinks=True, dirs_exist_ok=True)
    shutil.copy2(Path(sys.executable).resolve(), destination / 'python/bin/python3')
    source_lib = Path(sys.prefix) / 'lib'
    for source in source_lib.glob('libpython*.so*'):
        if source.is_file():
            shutil.copy2(source.resolve(), destination / 'python/lib' / source.name)
    version = f'python{sys.version_info.major}.{sys.version_info.minor}'
    shutil.copytree(source_lib / version, destination / 'python/lib' / version,
                    symlinks=True, dirs_exist_ok=True)
    for name in ('sandbox_v2.py', 'execution_pod.py', 'control.py', 'python_session.py', 'network_probe.py', 'user_probe.py'):
        shutil.copy2(Path('/opt/aidash') / name, destination / name)
    Path('/scratch-system/scratch').mkdir(parents=True, exist_ok=True)
    os.chmod(destination / 'loader', 0o755)
    (destination / '.guard-waiting').touch()
    wait_for_guard(destination / '.guard-ready')


if __name__ == '__main__':
    main()
