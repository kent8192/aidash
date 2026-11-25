"""SemVer of streamed Execution Pod helper commands and their receipts."""
import re

HELPER_PROTOCOL = '2.1.0'
LEGACY_HELPER_PROTOCOL = '1.0.0'
_SEMVER = re.compile(
    r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)'
    r'(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?'
    r'(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?'
)


def protocol_supported(version):
    match = _SEMVER.fullmatch(version) if isinstance(version, str) else None
    current_major = int(HELPER_PROTOCOL.split('.')[0])
    return bool(match and match[1] in (str(current_major), str(current_major - 1)))
