"""Collect a frozen Working Area without following links."""
import hashlib
import os
from pathlib import Path
import stat


def refusal(reason, path, limit=None):
    return {'refused': {'reason': reason, 'path': path, 'limit': limit}}


def valid_path(path):
    return (bool(path) and len(path.encode()) <= 1024 and '\\' not in path
            and not any(ord(c) < 32 or 127 <= ord(c) <= 159 for c in path)
            and all(part not in ('', '.', '..') for part in path.split('/')))


def collect(root, maximum, open_file):
    files, total = [], 0
    def walk_error(error):
        raise error
    for directory, dirs, names in os.walk(root, followlinks=False, onerror=walk_error):
        dirs.sort()
        names.sort()
        for name in dirs:
            path = Path(directory) / name
            relative = path.relative_to(root).as_posix()
            if not valid_path(relative):
                return refusal('unsafe_directory', relative)
            mode = os.lstat(path).st_mode
            if stat.S_ISLNK(mode):
                return refusal('symlink', relative)
            if not stat.S_ISDIR(mode):
                return refusal('unsafe_directory', relative)
        for name in names:
            path = Path(directory) / name
            relative = path.relative_to(root).as_posix()
            if not valid_path(relative):
                return refusal('unsafe_file', relative)
            info = os.lstat(path)
            if stat.S_ISLNK(info.st_mode):
                return refusal('symlink', relative)
            if not stat.S_ISREG(info.st_mode):
                return refusal('unsafe_file', relative)
            if info.st_nlink != 1:
                return refusal('hard_link', relative)
            if len(files) >= 4096:
                return refusal('file_count', relative, 4096)
            total += info.st_size
            if total > maximum:
                return refusal('bytes', relative, maximum)
            with open_file(root, relative) as file:
                current = os.fstat(file.fileno())
                if not stat.S_ISREG(current.st_mode) or current.st_nlink != 1 or current.st_size != info.st_size:
                    raise ValueError('file changed during collection')
                digest = hashlib.sha256()
                size = 0
                while data := file.read(1048576):
                    digest.update(data)
                    size += len(data)
                if size != current.st_size:
                    raise ValueError('file changed during collection')
                digest = digest.hexdigest()
            files.append({'path': relative, 'object_id': hashlib.sha256(relative.encode()).hexdigest(),
                          'digest': digest, 'size': info.st_size})
    return {'files': files}
