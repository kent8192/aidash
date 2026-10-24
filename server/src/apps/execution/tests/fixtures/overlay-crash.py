"""Exercise the production atomic directory swap under real process death."""
import json
import os
from pathlib import Path
import signal
import sys
import tempfile

sys.path.insert(0, '/opt/aidash')
from overlay import publish


def tree(path, version):
    (path / 'site-packages').mkdir(parents=True)
    (path / 'site-packages' / 'package.py').write_text(version)
    (path / 'manifest.json').write_text(json.dumps({'version': version}))


def version(path, expected):
    assert (path / 'site-packages' / 'package.py').read_text() == expected
    assert json.loads((path / 'manifest.json').read_text())['version'] == expected


with tempfile.TemporaryDirectory(dir='/work') as root:
    root = Path(root)
    destination = root / 'overlay'
    pending = root / 'pending'
    tree(destination, 'old')
    tree(pending, 'new')
    for phase in ('before_publish', 'during_cleanup'):
        read, write = os.pipe()
        child = os.fork()
        if child == 0:
            os.close(read)
            if phase == 'during_cleanup':
                publish(pending, destination)
                # Simulate interruption after partially deleting the old tree.
                (pending / 'site-packages' / 'package.py').unlink()
            os.write(write, b'ready')
            signal.pause()
            os._exit(1)
        os.close(write)
        assert os.read(read, 5) == b'ready'
        os.close(read)
        os.kill(child, signal.SIGKILL)
        _, status = os.waitpid(child, 0)
        assert os.WIFSIGNALED(status) and os.WTERMSIG(status) == signal.SIGKILL
        version(destination, 'old' if phase == 'before_publish' else 'new')
print('atomic overlay survived both interruption points')
