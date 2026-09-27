"""Publish a complete package overlay without a missing/partially deleted tree."""
import ctypes
import os


def publish(pending, destination):
    if destination.exists():
        # Replacing a nonempty directory needs Linux RENAME_EXCHANGE. A
        # two-rename fallback would expose a missing overlay if killed between
        # calls, so unsupported kernels fail with the old directory intact.
        libc = ctypes.CDLL(None, use_errno=True)
        exchange = getattr(libc, 'renameat2', None)
        if exchange is None:
            raise RuntimeError('atomic overlay replacement unavailable')
        exchange.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
        exchange.restype = ctypes.c_int
        if exchange(-100, os.fsencode(pending), -100, os.fsencode(destination), 2):
            error = ctypes.get_errno()
            raise OSError(error, os.strerror(error))
    else:
        os.replace(pending, destination)
    directory = os.open(destination.parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)
