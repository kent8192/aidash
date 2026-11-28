"""Read a trusted program frame from stdin and run its entry point in memory."""
import ctypes
import importlib.abc
import importlib.util
import json
import sys
import zlib


class Bundle(importlib.abc.MetaPathFinder, importlib.abc.Loader):
    def __init__(self, sources):
        self.sources = sources

    def find_spec(self, fullname, path=None, target=None):
        if fullname in self.sources:
            return importlib.util.spec_from_loader(fullname, self)

    def create_module(self, spec):
        return None

    def exec_module(self, module):
        module.__file__ = '/.aidash-helpers/<streamed>/' + module.__name__ + '.py'
        exec(compile(self.sources[module.__name__], module.__file__, 'exec'), module.__dict__)


def main():
    # Disable tracing before reading any supervisor input. User code can share
    # the Pod, but it must not modify this process or read the result nonce.
    if ctypes.CDLL(None, use_errno=True).prctl(4, 0, 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), 'PR_SET_DUMPABLE')
    stream = sys.stdin.buffer
    header = stream.readline(32)
    if not header.startswith(b'AIDASH-HELPER/1 ') or not header.endswith(b'\n'):
        raise ValueError('invalid helper frame')
    size = int(header.split()[1])
    if not 0 < size <= 2 * 1024 * 1024:
        raise ValueError('invalid helper frame size')
    data = stream.read(size)
    if len(data) != size:
        raise ValueError('incomplete helper frame')
    sources = json.loads(zlib.decompress(data))
    sys.meta_path.insert(0, Bundle(sources))
    filename = '/.aidash-helpers/<streamed>/sandbox_v2.py'
    sys.argv[0] = filename
    exec(compile(sources['sandbox_v2'], filename, 'exec'), {'__name__': '__main__', '__file__': filename})


if __name__ == '__main__':
    main()
