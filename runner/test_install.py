import base64
import hashlib
import json
from pathlib import Path
import runpy
from types import SimpleNamespace
import zipfile

import pytest


def test_wheel_install_checks_publication_space_before_pip(tmp_path, monkeypatch, capsys):
    wheels = []
    for index in range(17):
        wheel = tmp_path / f'package{index}-1.0-py3-none-any.whl'
        with zipfile.ZipFile(wheel, 'w', compression=zipfile.ZIP_DEFLATED) as archive:
            archive.writestr('module.py', b'pass\n')
            if index == 0:
                for entry in range(4097):
                    archive.writestr(f'package/item{entry}', b'')
                with archive.open('package/data', 'w') as member:
                    block = b'0' * (1024 * 1024)
                    for _ in range(129):
                        member.write(block)
        wheels.append(dict(path=str(wheel), filename=wheel.name, sha256=hashlib.sha256(wheel.read_bytes()).hexdigest()))

    installer = Path(__file__).with_name('install.py')
    argv = [str(installer), base64.b64encode(json.dumps({'wheels': wheels}).encode()).decode()]
    monkeypatch.setattr('sys.argv', argv)
    relative = Path.is_relative_to
    monkeypatch.setattr(Path, 'is_relative_to', lambda path, other: relative(
        path, tmp_path if other == '/references/.packages' else other))
    stat = Path.stat
    monkeypatch.setattr(Path, 'stat', lambda path, *args, **kwargs: SimpleNamespace(st_dev=-1)
                        if path == Path('/work') else stat(path, *args, **kwargs))
    needed = 129 * 1024 * 1024 + 17 * len(b'pass\n')
    free = needed - 1
    monkeypatch.setattr('shutil.disk_usage', lambda target: SimpleNamespace(free=free))

    def extract(*args, **kwargs):
        pytest.fail('pip ran before the installer checked free disk space')

    monkeypatch.setattr('subprocess.run', extract)
    with pytest.raises(SystemExit) as stopped:
        runpy.run_path(str(installer), run_name='__main__')
    assert stopped.value.code == 1
    refusal = json.loads(capsys.readouterr().out)
    assert refusal['code'] == 'wheel_disk_full'
    assert (refusal['needed_bytes'], refusal['free_bytes']) == (needed, free)
    assert str(needed) in refusal['message'] and str(free) in refusal['message']
