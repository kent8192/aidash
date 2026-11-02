"""Admission evidence from an actual root user child, never from bootstrap."""
import errno
import json
import os
from pathlib import Path
import resource
import signal
import socket
import time


def process_limit_probe(limit):
    children = []
    enforced = False
    try:
        for index in range(limit + 1):
            read_fd, write_fd = os.pipe()
            try:
                pid = os.fork()
            except OSError as error:
                os.close(read_fd)
                os.close(write_fd)
                enforced = error.errno == errno.EAGAIN and len(children) >= max(1, limit - 16)
                break
            if pid == 0:
                os.close(read_fd)
                try:
                    os.setuid(30000 + index)
                    os.write(write_fd, b'1')
                    os.close(write_fd)
                    time.sleep(60)
                except BaseException:
                    try:
                        os.write(write_fd, b'0')
                    except OSError:
                        pass
                os._exit(0)
            children.append(pid)
            os.close(write_fd)
            try:
                if os.read(read_fd, 1) != b'1':
                    break
            finally:
                os.close(read_fd)
    finally:
        for pid in children:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        for pid in children:
            try:
                os.waitpid(pid, 0)
            except ChildProcessError:
                pass
    return enforced


def writable(path):
    try:
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    except OSError:
        return False
    else:
        os.close(fd)
        os.unlink(path)
        return True


def probe():
    fields = dict(line.split(':', 1) for line in Path('/proc/self/status').read_text().splitlines() if ':' in line)
    soft, hard = resource.getrlimit(resource.RLIMIT_NPROC)
    try:
        resource.setrlimit(resource.RLIMIT_NPROC, (soft, hard + 1))
        unraiseable = False
    except (ValueError, OSError):
        unraiseable = True
    try:
        raw = socket.socket(socket.AF_INET, socket.SOCK_RAW, socket.IPPROTO_ICMP)
        raw.close()
        raw_socket = True
    except OSError:
        raw_socket = False
    filename = '.aidash-admission-' + str(os.getpid())
    return {'uid': os.getuid(), 'kernel': os.uname().release,
            'no_new_privs': int(fields['NoNewPrivs']),
            'caps': {key: int(fields[key], 16) for key in ('CapInh', 'CapPrm', 'CapEff', 'CapBnd', 'CapAmb')},
            'process_limit': [soft, hard], 'process_limit_unraiseable': unraiseable,
            'process_limit_enforced': process_limit_probe(hard), 'raw_socket': raw_socket,
            'service_account_absent': not Path('/var/run/secrets/kubernetes.io/serviceaccount/token').exists(),
            'root_writable': writable('/' + filename),
            'helpers_read_only': not writable('/.aidash-helpers/' + filename)}


if __name__ == '__main__':
    print(json.dumps(probe(), separators=(',', ':')))
