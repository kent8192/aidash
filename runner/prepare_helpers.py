"""Inject the controller's interpreter and helper programs into an emptyDir."""
import os
from pathlib import Path
import platform
import shutil
import sys


def main():
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
    for name in ('sandbox_v2.py', 'execution_pod.py', 'control.py', 'python_session.py'):
        shutil.copy2(Path('/opt/aidash') / name, destination / name)
    Path('/scratch-system/scratch').mkdir(parents=True, exist_ok=True)
    os.chmod(destination / 'loader', 0o755)


if __name__ == '__main__':
    main()
