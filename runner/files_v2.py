"""Bounded file operations rooted at a Scratch Space, without following links."""
import base64
import errno
import fnmatch
import hashlib
import json
import os
import re
import stat
import tempfile
import uuid
from pathlib import Path


class InvalidPath(ValueError):
    pass


class PatchReceiptUnavailable(ValueError):
    pass


def save_patch_receipt(path, record):
    fd, name = tempfile.mkstemp(prefix='.patch-', dir=path.parent)
    try:
        with os.fdopen(fd, 'w') as file:
            json.dump(record, file)
            file.flush()
            os.fsync(file.fileno())
        os.replace(name, path)
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        Path(name).unlink(missing_ok=True)


def parts(path, allow_root=False):
    if not isinstance(path, str) or '\x00' in path or path.startswith('/'):
        raise InvalidPath('path must be relative to the Scratch Space')
    components = path.split('/')
    if path == '.' and allow_root:
        return []
    if not path or any(component in ('', '.', '..') for component in components):
        raise InvalidPath('invalid path component')
    return components


def open_dir(root, components, create=False):
    """Open a directory below root without following links; `create` makes missing ones (0755)."""
    fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        for component in components:
            if create:
                try:
                    os.mkdir(component, 0o755, dir_fd=fd)
                except FileExistsError:
                    pass
            try:
                child = os.open(component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            except OSError as error:
                if error.errno in (errno.ELOOP, errno.ENOTDIR):
                    raise InvalidPath('path contains a symlink or non-directory') from None
                raise
            os.close(fd)
            fd = child
        return fd
    except BaseException:
        os.close(fd)
        raise


def open_file(fd, name, flags=os.O_RDONLY, mode=0o600):
    import fcntl
    # Non-blocking, so a FIFO is refused at once instead of waiting for a writer.
    try:
        result = os.open(name, flags | os.O_NOFOLLOW | os.O_NONBLOCK, mode, dir_fd=fd)
    except OSError as error:
        if error.errno == errno.ELOOP:
            raise InvalidPath('symlinks are not allowed') from None
        if error.errno == errno.ENXIO:
            raise InvalidPath('path is not a regular file') from None
        raise
    if not stat.S_ISREG(os.fstat(result).st_mode):
        os.close(result)
        raise InvalidPath('path is not a regular file')
    fcntl.fcntl(result, fcntl.F_SETFL, fcntl.fcntl(result, fcntl.F_GETFL) & ~os.O_NONBLOCK)
    return result


def split_lines(text):
    r"""Split on '\n' only, keeping each line's ending: '\r\n', '\n' or '' for a last line without one."""
    pieces = text.split('\n')
    lines = [(piece[:-1], '\r\n') if piece.endswith('\r') else (piece, '\n') for piece in pieces[:-1]]
    if pieces[-1]:
        lines.append((pieces[-1], ''))
    return lines


def pattern_matches(path, pattern, *, is_dir=False):
    """Match git-style wildcards without allowing a single star across directories."""
    directory_only = pattern.endswith('/')
    pattern = pattern.rstrip('/')
    anchored = pattern.startswith('/')
    pattern = pattern.lstrip('/')
    if not pattern:
        return False
    tokens = []
    i = 0
    while i < len(pattern):
        if pattern[i] == '\\' and i + 1 < len(pattern):
            tokens.append(re.escape(pattern[i + 1]))
            i += 2
        elif pattern[i:i + 2] == '**':
            i += 2
            if i < len(pattern) and pattern[i] == '/':
                tokens.append('(?:.*/)?')
                i += 1
            else:
                tokens.append('.*')
        elif pattern[i] == '*':
            tokens.append('[^/]*')
            i += 1
        elif pattern[i] == '?':
            tokens.append('[^/]')
            i += 1
        elif pattern[i] == '[' and ']' in pattern[i + 1:]:
            end = pattern.index(']', i + 1) + 1
            translated = fnmatch.translate(pattern[i:end])
            tokens.append(translated[4:-3])
            i = end
        else:
            tokens.append(re.escape(pattern[i]))
            i += 1
    prefix = '' if anchored or '/' in pattern else '(?:.*/)?'
    suffix = ('(?:/.*)?' if is_dir else '/.*') if directory_only else ''
    return re.fullmatch(prefix + ''.join(tokens) + suffix, path) is not None


class FileService:
    def __init__(self, root, state):
        self.root = os.fspath(root)
        self.state = Path(state)

    def clear_thread(self, request):
        """Remove one Thread directory, confined to the Scratch Space root."""
        import shutil

        thread = request['thread_id']
        if not isinstance(thread, str) or str(uuid.UUID(thread)) != thread:
            raise InvalidPath('a canonical Thread ID is required')
        if not shutil.rmtree.avoids_symlink_attacks:
            raise RuntimeError('safe directory removal is unavailable')
        try:
            directory = open_dir(self.root, ['threads'])
        except FileNotFoundError:
            return {'cleared': True}
        try:
            try:
                entry = os.stat(thread, dir_fd=directory, follow_symlinks=False)
                if stat.S_ISDIR(entry.st_mode):
                    shutil.rmtree(thread, dir_fd=directory)
                else:
                    os.unlink(thread, dir_fd=directory)
            except FileNotFoundError:
                pass
        finally:
            os.close(directory)
        return {'cleared': True}

    def read(self, request):
        path = parts(request['path'])
        offset = request.get('offset')
        offset = 0 if offset is None else offset
        limit = request['limit']
        if type(offset) is not int or offset < 0 or type(limit) is not int or not 1 <= limit <= 1048576:
            raise ValueError('invalid file offset or limit')
        parent = open_dir(self.root, path[:-1])
        try:
            with os.fdopen(open_file(parent, path[-1]), 'rb') as file:
                size = os.fstat(file.fileno()).st_size
                file.seek(offset)
                data = file.read(limit)
        finally:
            os.close(parent)
        return {'data': base64.b64encode(data).decode(), 'size': size,
                'next_offset': offset + len(data), 'eof': offset + len(data) >= size}

    def write(self, request):
        import fcntl
        path = parts(request['path'])
        run = request['run_id']
        if not isinstance(run, str) or not run or len(run) > 256:
            raise ValueError('invalid run_id')
        data = base64.b64decode(request['data'], validate=True)
        offset, final = request['offset'], request['final']
        if len(data) > 1048576 or type(offset) is not int or offset < 0 or type(final) is not bool:
            raise ValueError('invalid write chunk')
        mode = request.get('mode')
        if mode is not None and (type(mode) is not int or not 0 <= mode <= 0o777):
            raise ValueError('invalid mode')
        self.state.mkdir(mode=0o700, parents=True, exist_ok=True)
        state = os.open(self.state, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            parent = open_dir(self.root, path[:-1], create=True)
        except BaseException:
            os.close(state)
            raise
        key = hashlib.sha256((run + '\0' + request['path']).encode()).hexdigest()
        lock = os.open(key + '.lock', os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600, dir_fd=state)
        try:
            fcntl.flock(lock, fcntl.LOCK_EX)
            flags = os.O_RDWR | os.O_CREAT
            if offset == 0:
                flags |= os.O_TRUNC
            fd = open_file(state, key + '.tmp', flags)
            with os.fdopen(fd, 'r+b') as file:
                size = os.fstat(file.fileno()).st_size
                if size != offset:
                    raise ValueError('write offset does not match staged size')
                file.seek(offset)
                file.write(data)
                file.flush()
                if mode is not None:
                    os.fchmod(file.fileno(), mode)
                os.fsync(file.fileno())
            size = offset + len(data)
            if final:
                try:
                    existing = os.stat(path[-1], dir_fd=parent, follow_symlinks=False)
                    if not stat.S_ISREG(existing.st_mode):
                        raise InvalidPath('target is not a regular file')
                except FileNotFoundError:
                    existing = None
                if mode is None:
                    # Without a mode, keep the target's or give a new file the usual 0644.
                    staged = open_file(state, key + '.tmp')
                    try:
                        os.fchmod(staged, stat.S_IMODE(existing.st_mode) if existing else 0o644)
                    finally:
                        os.close(staged)
                os.replace(key + '.tmp', path[-1], src_dir_fd=state, dst_dir_fd=parent)
                os.fsync(parent)
            return {'size': size}
        finally:
            os.close(lock)
            os.close(parent)
            os.close(state)

    def _walk(self, root):
        prefix = parts(root, allow_root=True)
        if prefix:
            parent = open_dir(self.root, prefix[:-1])
            try:
                info = os.stat(prefix[-1], dir_fd=parent, follow_symlinks=False)
                if stat.S_ISLNK(info.st_mode):
                    raise InvalidPath('symlinks are not allowed')
                if stat.S_ISREG(info.st_mode):
                    yield '/'.join(prefix), parent, prefix[-1], info
                    return
            finally:
                os.close(parent)
        fd = open_dir(self.root, prefix)
        try:
            for directory, dirs, files, current in os.fwalk('.', dir_fd=fd, follow_symlinks=False):
                dirs.sort()
                rel = [] if directory == '.' else directory[2:].split('/')
                for name in sorted(dirs + files):
                    try:
                        info = os.stat(name, dir_fd=current, follow_symlinks=False)
                    except FileNotFoundError:
                        continue
                    path = '/'.join(prefix + rel + [name])
                    yield path, current, name, info
        finally:
            os.close(fd)

    def usage(self, _request):
        # The Scratch Space lives on its own persistent volume.
        stats = os.statvfs(self.root)
        return {'bytes': (stats.f_blocks - stats.f_bfree) * stats.f_frsize}

    def search(self, request):
        import codecs
        pattern = request['pattern']
        regex = request['regex']
        limit = request['max_results']
        if not isinstance(pattern, str) or len(pattern) > 4096 or type(regex) is not bool or type(limit) is not int or not 1 <= limit <= 500:
            raise ValueError('invalid search request')
        expression = re.compile(pattern) if regex else None
        glob = request.get('glob')
        matches = []
        truncated = False
        for path, fd, name, info in self._walk(request.get('path') or '.'):
            if not stat.S_ISREG(info.st_mode) or (glob and not fnmatch.fnmatch(path, glob)):
                continue
            try:
                with os.fdopen(open_file(fd, name), 'rb') as file:
                    decoder = codecs.getincrementaldecoder('utf-8')()
                    binary = False
                    while True:
                        chunk = file.read(131072)
                        if not chunk:
                            decoder.decode(b'', final=True)
                            break
                        if b'\0' in chunk:
                            binary = True
                            break
                        decoder.decode(chunk)
                    if binary:
                        continue
                    file.seek(0)
                    index = 0
                    while True:
                        raw = file.readline(1048577)
                        if not raw:
                            break
                        index += 1
                        if len(raw) == 1048577 and not raw.endswith(b'\n'):
                            truncated = True
                            while raw and not raw.endswith(b'\n'):
                                raw = file.readline(1048577)
                            continue
                        line = raw.decode('utf-8').rstrip('\r\n')
                        if (expression.search(line) if expression else pattern in line):
                            if len(matches) == limit:
                                return {'matches': matches, 'truncated': True}
                            matches.append({'path': path, 'line': index, 'text': line[:2000]})
            except (UnicodeDecodeError, FileNotFoundError, InvalidPath):
                continue
        return {'matches': matches, 'truncated': truncated}

    def manifest(self, request):
        root = request.get('root') or '.'
        limit = request['max_files']
        if type(limit) is not int or limit < 1:
            raise ValueError('invalid max_files')
        limit = min(limit, 10000)
        include, exclude = request['include'], request['exclude']
        ignore_files = request['ignore_files']
        if (not isinstance(include, list) or not isinstance(exclude, list) or not isinstance(ignore_files, list)
                or any(not isinstance(item, str) for item in include + exclude + ignore_files)):
            raise ValueError('invalid manifest patterns')
        for name in ignore_files:
            if len(parts(name)) != 1:
                raise ValueError('ignore file names must be single path components')
        components = parts(root, allow_root=True)
        if request.get('missing_ok') is True and components:
            # A sync save may select a path that was deleted: it lists nothing.
            try:
                parent = open_dir(self.root, components[:-1])
                try:
                    os.stat(components[-1], dir_fd=parent, follow_symlinks=False)
                finally:
                    os.close(parent)
            except FileNotFoundError:
                return {'files': [], 'skipped_symlinks': [], 'truncated': False}
        rule_cache = {}
        def rules(directory):
            key = '/'.join(directory)
            if key in rule_cache:
                return rule_cache[key]
            found = []
            parent = open_dir(self.root, directory)
            try:
                for name in ignore_files:
                    try:
                        with os.fdopen(open_file(parent, name), 'rb') as file:
                            raw = file.read(1048577)
                        if len(raw) > 1048576:
                            raise ValueError('ignore file exceeds limit')
                        found.extend(raw.decode('utf-8').splitlines())
                    except (FileNotFoundError, InvalidPath, UnicodeDecodeError):
                        continue
            finally:
                os.close(parent)
            rule_cache[key] = found
            return found
        def ignored(path):
            components = path.split('/')
            for end in range(1, len(components) + 1):
                directory_target = end < len(components)
                result = False
                for depth in range(end):
                    directory = components[:depth]
                    relative = '/'.join(components[depth:end])
                    for rule in rules(directory):
                        while rule.endswith(' ') and not rule.endswith('\\ '):
                            rule = rule[:-1]
                        if not rule or rule.startswith('#'):
                            continue
                        negate = rule.startswith('!')
                        rule = rule[1:] if negate else rule
                        if pattern_matches(relative, rule, is_dir=directory_target):
                            result = not negate
                if result:
                    return True
            return False
        from concurrent.futures import ThreadPoolExecutor

        def hash_file(path, file):
            with file:
                digest = hashlib.sha256()
                size = 0
                while data := file.read(131072):
                    size += len(data)
                    digest.update(data)
            return {'path': path, 'size': size, 'sha256': digest.hexdigest()}

        def order(path):
            components = path.split('/')
            return components[:-1], components[-1]

        after = request.get('after')
        if after:
            parts(after)
        entries, skipped, batch = [], [], []
        truncated = False
        with ThreadPoolExecutor(max_workers=100 if after is not None else 1) as executor:
            for path, fd, name, info in self._walk(root):
                if after and order(path) <= order(after):
                    continue
                if stat.S_ISLNK(info.st_mode):
                    if len(skipped) < limit:
                        skipped.append(path)
                    continue
                if not stat.S_ISREG(info.st_mode):
                    continue
                included = any(pattern_matches(path, item) for item in include)
                if not included and ('.git' in path.split('/') or any(pattern_matches(path, item) for item in exclude)
                                     or ignored(path)):
                    continue
                if len(entries) + len(batch) == limit:
                    truncated = True
                    break
                try:
                    file = os.fdopen(open_file(fd, name), 'rb')
                except (FileNotFoundError, InvalidPath):
                    continue
                batch.append(executor.submit(hash_file, path, file))
                if len(batch) == 100:
                    entries.extend(future.result() for future in batch)
                    batch.clear()
            entries.extend(future.result() for future in batch)
        return {'files': entries, 'skipped_symlinks': sorted(skipped), 'truncated': truncated}

    def patch(self, request):
        import fcntl

        text, run = request['patch'], request['run_id']
        if not isinstance(text, str) or len(text.encode()) > 1048576:
            raise ValueError('invalid patch size')
        if not isinstance(run, str) or not run or len(run) > 256 or type(request['apply']) is not bool:
            raise ValueError('invalid patch request')
        claim = uuid.UUID(request['claim_id'])
        digest = hashlib.sha256(json.dumps([run, text]).encode()).hexdigest()
        self.state.mkdir(mode=0o700, parents=True, exist_ok=True)
        receipt = self.state / f'patch-{claim}.json'
        lock = os.open(self.state / 'patch.lock', os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
        try:
            fcntl.flock(lock, fcntl.LOCK_EX)
            if receipt.exists():
                previous = json.loads(receipt.read_bytes())
                if previous['digest'] != digest:
                    raise ValueError('patch claim conflict')
                if previous['result'] is None:
                    raise PatchReceiptUnavailable('patch receipt unavailable; effect is uncertain')
                return previous['result']
            if not request['apply']:
                raise PatchReceiptUnavailable('patch receipt unavailable; effect is uncertain')
            # A helper interrupted after claiming a patch may have started editing
            # files. Keep that evidence and refuse to run the same claim again.
            save_patch_receipt(receipt, {'digest': digest, 'result': None})
            result = self._apply_patch(text)
            save_patch_receipt(receipt, {'digest': digest, 'result': result})
            return result
        finally:
            os.close(lock)

    def _apply_patch(self, text):
        # Split on newlines only: splitlines() would also split on form feeds
        # and other separators that can appear inside a line.
        lines = [line[:-1] if line.endswith('\r') else line for line in text.split('\n')]
        if lines and lines[-1] == '':
            lines.pop()
        if len(lines) < 3 or lines[0] != '*** Begin Patch' or lines[-1] != '*** End Patch':
            raise ValueError('invalid patch grammar')
        edits, seen, i = [], set(), 1
        headers = ('*** Add File: ', '*** Delete File: ', '*** Update File: ')
        while i < len(lines) - 1:
            header = lines[i]
            mode = next((n for n, prefix in enumerate(headers) if header.startswith(prefix)), None)
            if mode is None:
                raise ValueError('invalid patch header')
            path = header[len(headers[mode]):]
            parts(path)
            if path in seen:
                raise ValueError('duplicate patch path')
            seen.add(path)
            i += 1
            if mode == 0:
                added = []
                while i < len(lines) - 1 and not lines[i].startswith('*** '):
                    if not lines[i].startswith('+'):
                        raise ValueError('invalid add line')
                    added.append(lines[i][1:])
                    i += 1
                change = ''.join(line + '\n' for line in added).encode()
            elif mode == 1:
                change = None
            else:
                change = []
                while i < len(lines) - 1 and not any(lines[i].startswith(h) for h in headers):
                    context = None
                    if lines[i] == '@@':
                        i += 1
                    elif lines[i].startswith('@@ '):
                        context = lines[i][3:]
                        i += 1
                    elif change:
                        raise ValueError('invalid patch hunk')
                    old, new = [], []
                    while i < len(lines) - 1 and not lines[i].startswith(('@@', '*** ')):
                        line = lines[i]
                        if not line or line[0] not in ' +-':
                            raise ValueError('invalid patch hunk line')
                        if line[0] in ' -':
                            old.append(line[1:])
                        if line[0] in ' +':
                            new.append(line[1:])
                        i += 1
                    if not old and not new:
                        raise ValueError('empty patch hunk')
                    eof = i < len(lines) and lines[i] == '*** End of File'
                    if eof:
                        i += 1
                    change.append((context, old, new, eof))
                if not change:
                    raise ValueError('missing patch hunk')
            edits.append((path, mode, change))
        if not edits:
            raise ValueError('empty patch')
        opened, prepared, applied = [], [], []
        try:
            for path, mode, change in edits:
                components = parts(path)
                parent = open_dir(self.root, components[:-1], create=mode == 0)
                opened.append(parent)
                name = components[-1]
                try:
                    fd = open_file(parent, name)
                except FileNotFoundError:
                    fd = None
                if (mode == 0 and fd is not None) or (mode != 0 and fd is None):
                    if fd is not None:
                        os.close(fd)
                    raise ValueError('patch target existence conflict')
                old_mode = 0o644
                if fd is not None:
                    with os.fdopen(fd, 'rb') as file:
                        old_mode = stat.S_IMODE(os.fstat(file.fileno()).st_mode)
                        original = file.read((16 << 20) + 1)
                    if len(original) > 16 << 20:
                        raise ValueError('patch source exceeds limit')
                if mode == 2:
                    # Lines keep their own endings, so a patch changes only the
                    # lines it names (CRLF, a missing final newline, form feeds).
                    source = split_lines(original.decode('utf-8'))
                    default = next((ending for _, ending in source if ending), '\n')
                    cursor = 0
                    for context, old, new, eof in change:
                        if context is not None:
                            try:
                                cursor = next(pos for pos in range(cursor, len(source))
                                              if source[pos][0] == context) + 1
                            except StopIteration:
                                raise ValueError('patch context not found') from None
                        found = next((pos for pos in range(cursor, len(source) + 1)
                                      if [line for line, _ in source[pos:pos + len(old)]] == old
                                      and (not eof or pos + len(old) == len(source))), None)
                        if found is None:
                            raise ValueError('patch hunk not found')
                        replaced = source[found:found + len(old)]
                        endings = [replaced[k][1] if k < len(replaced) and replaced[k][1] else default
                                   for k in range(len(new))]
                        at_end = found + len(old) == len(source)
                        if at_end and new and (replaced[-1][1] == '' if replaced
                                               else bool(source) and source[-1][1] == ''):
                            endings[-1] = ''
                            if not replaced and source:
                                source[-1] = (source[-1][0], default)
                        source[found:found + len(old)] = list(zip(new, endings))
                        cursor = found + len(new)
                    change = ''.join(line + ending for line, ending in source).encode()
                token = '.aidash-patch-' + uuid.uuid4().hex
                temp, backup = token + '.new', token + '.old'
                prepared.append((parent, name, temp, backup, mode))
                if change is not None:
                    fd = os.open(temp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, old_mode, dir_fd=parent)
                    with os.fdopen(fd, 'wb') as file:
                        file.write(change)
                        file.flush()
                        os.fsync(file.fileno())
            for parent, name, temp, backup, mode in prepared:
                if mode != 0:
                    os.rename(name, backup, src_dir_fd=parent, dst_dir_fd=parent)
                try:
                    if mode != 1:
                        os.rename(temp, name, src_dir_fd=parent, dst_dir_fd=parent)
                except BaseException:
                    if mode != 0:
                        os.rename(backup, name, src_dir_fd=parent, dst_dir_fd=parent)
                    raise
                applied.append((parent, name, backup, mode))
            for parent in opened:
                os.fsync(parent)
            for parent, _name, backup, mode in applied:
                if mode != 0:
                    try:
                        os.unlink(backup, dir_fd=parent)
                    except OSError:
                        pass
            return {'changed': [path for path, _, _ in edits]}
        except BaseException:
            for parent, name, backup, mode in reversed(applied):
                if mode != 1:
                    try:
                        os.unlink(name, dir_fd=parent)
                    except FileNotFoundError:
                        pass
                if mode != 0:
                    os.rename(backup, name, src_dir_fd=parent, dst_dir_fd=parent)
            raise
        finally:
            for parent, _, temp, _, _ in prepared:
                try:
                    os.unlink(temp, dir_fd=parent)
                except FileNotFoundError:
                    pass
            for parent in opened:
                os.close(parent)


# Read-only operations end well before the Runner's 45 s transport limit, which
# holds the Scratch Space's lock while it waits.
FILE_OPERATION_SECONDS = 30


def run_bounded(service, operation, request, seconds=FILE_OPERATION_SECONDS):
    """Run one file operation; a read-only walk or pathological pattern stops at the budget."""
    if operation not in ('read', 'search', 'manifest'):
        return getattr(service, operation)(request)
    import signal

    def over_budget(*_):
        raise ValueError(f'file operation exceeded its {seconds} s time budget')

    previous = signal.signal(signal.SIGALRM, over_budget)
    signal.alarm(seconds)
    try:
        return getattr(service, operation)(request)
    finally:
        signal.alarm(0)
        signal.signal(signal.SIGALRM, previous)
