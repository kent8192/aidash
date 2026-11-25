"""Frame a controller-owned helper program ahead of its existing stdin."""
from functools import lru_cache
import json
from pathlib import Path
import zlib

ROOT = Path(__file__).resolve().parent
MODULES = ('sandbox_v2', 'execution_pod', 'pod_base', 'helper_protocol', 'files_v2', 'user_probe', 'network_probe')


@lru_cache(maxsize=1)
def bundle_frame():
    data = zlib.compress(json.dumps({
        module: (ROOT / (module + '.py')).read_text(encoding='utf-8')
        for module in MODULES
    }, separators=(',', ':')).encode())
    return b'AIDASH-HELPER/1 %d\n' % len(data) + data


def prepare_helper_exec(arguments):
    if not arguments or arguments[0] != 'exec' or '--' not in arguments:
        return arguments, b''
    separator = arguments.index('--')
    command = arguments[separator + 1:]
    if len(command) < 6 or command[5] != '/.aidash-helpers/sandbox_v2.py':
        return arguments, b''
    prefix = arguments[:separator]
    if '-i' not in prefix:
        prefix.insert(1, '-i')
    # Passing this small launcher inline also works with the previous base,
    # whose read-only mount predates the streaming protocol.
    fallback = (ROOT / 'helper_launcher.py').read_text(encoding='utf-8')
    launcher = (
        "from pathlib import Path; p = Path('/.aidash-helpers/helper_launcher.py'); "
        f"exec(compile(p.read_bytes() if p.is_file() else {fallback!r}, str(p), 'exec'))"
    )
    return [*prefix, '--', *command[:5], '-c', launcher, *command[6:]], bundle_frame()
