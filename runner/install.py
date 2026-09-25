"""Install an explicit wheel set offline into the Agent's writable overlay.

The previous interpreter is stopped before this operation starts. Installation
cannot implicitly fetch dependencies or execute source build hooks. Failures
leave the previous overlay intact until the final local directory replacement.
"""
import base64
import hashlib
import importlib.metadata
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import stat
import subprocess
import sys
import tempfile
import zipfile
from pip._vendor.packaging.utils import canonicalize_name, parse_wheel_filename
from pip._vendor.packaging.requirements import Requirement

# The isolated interpreter excludes the script directory; this is the fixed,
# read-only runner installation, never an Agent-controlled import directory.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from overlay import publish

request = json.loads(base64.b64decode(sys.argv[1], validate=True))
if not 1 <= len(request['wheels']) <= 16:
    raise ValueError('wheel count limit')
requirements, expected, sources = [], {}, []
expanded = 0
for wheel in request['wheels']:
    path = Path(wheel['path'])
    if not path.is_relative_to('/references/.packages') or path.name != wheel['filename'] or path.is_symlink():
        raise ValueError('invalid wheel mount')
    with path.open('rb') as file:
        if hashlib.file_digest(file, 'sha256').hexdigest() != wheel['sha256']:
            raise ValueError('wheel digest mismatch')
    name, version, _, _ = parse_wheel_filename(path.name)
    if name in expected:
        raise ValueError('conflicting wheel versions')
    expected[name] = str(version)
    with zipfile.ZipFile(path) as archive:
        if len(archive.infolist()) > 4096:
            raise ValueError('wheel file count limit')
        for entry in archive.infolist():
            relative = PurePosixPath(entry.filename)
            mode = entry.external_attr >> 16
            if relative.is_absolute() or '..' in relative.parts or '\\' in entry.filename or stat.S_ISLNK(mode):
                raise ValueError('unsafe wheel path')
            expanded += entry.file_size
            if expanded > 128 << 20:
                raise ValueError('wheel expansion limit')
    requirements.append(f'{path} --hash=sha256:{wheel["sha256"]}')
    sources.append(dict(wheel, name=name, version=str(version)))

with tempfile.TemporaryDirectory(prefix='aidash-install-') as temporary:
    temporary = Path(temporary)
    requirements_file = temporary / 'requirements.txt'
    requirements_file.write_text('\n'.join(requirements) + '\n')
    target = temporary / 'site-packages'
    environment = {k:v for k,v in os.environ.items() if k in ('PATH','HOME','LANG','TMPDIR')}
    subprocess.run([sys.executable, '-I', '-m', 'pip', 'install', '--isolated', '--no-input',
                    '--disable-pip-version-check', '--no-cache-dir', '--no-index', '--no-deps',
                    '--only-binary=:all:', '--require-hashes', '--no-compile', '--ignore-installed',
                    '--target', str(target), '-r', str(requirements_file)], env=environment, check=True)
    distributions = list(importlib.metadata.distributions(path=[str(target)]))
    installed = {canonicalize_name(d.metadata['Name']): d.version for d in distributions}
    if installed != expected:
        raise ValueError('resolved wheel identity mismatch')
    available = {canonicalize_name(d.metadata['Name']):d.version for d in importlib.metadata.distributions()}
    available.update(installed)
    for distribution in distributions:
        for text in distribution.requires or []:
            requirement = Requirement(text)
            if requirement.marker and not requirement.marker.evaluate({'extra':''}):
                continue
            version = available.get(canonicalize_name(requirement.name))
            if version is None or version not in requirement.specifier:
                raise ValueError('dependency must be supplied explicitly: ' + requirement.name)
    overlay = Path('/work/.aidash-python')
    if overlay.is_symlink():
        raise ValueError('unsafe overlay')
    # Stage the package tree and its manifest together on the same filesystem.
    # Atomic exchange keeps the old or complete new overlay usable even if the
    # process dies during publication or deletion of the obsolete tree.
    pending = Path('/work/.aidash-python-pending')
    if pending.is_symlink():
        raise ValueError('unsafe staging overlay')
    if pending.exists():
        shutil.rmtree(pending)
    pending.mkdir()
    shutil.copytree(target, pending / 'site-packages', symlinks=False)
    report = {'protocol':'aidash-python-packages/1', 'image':request['image'],
              'packages':sources, 'outcome':'installed', 'network':'disabled', 'automatic_reinstall':False}
    manifest = pending / 'manifest.json'
    with manifest.open('w') as file:
        json.dump(report, file, sort_keys=True)
        file.flush()
        os.fsync(file.fileno())
    publish(pending, overlay)
    if pending.exists():
        shutil.rmtree(pending)
    print(json.dumps(report, sort_keys=True))
