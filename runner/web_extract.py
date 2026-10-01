"""Fixed offline Web parser. Never interpret page text as code or fetch resources.

Invoked in the admitted gVisor web_extract operation, with one immutable input,
one CPU, 256 MiB, a five-second deadline and no child processes or credentials.
"""
import gzip
import io
import json
import resource
import sys
import zlib
from html.parser import HTMLParser
from pathlib import Path

MAX_BYTES = 10 * 1024 * 1024
MAX_TEXT = 1024 * 1024
MAX_LINES = 32768


def deny_children():
    # RLIMIT_NPROC=0 on the sandbox's init task also prevents runsc's trusted
    # collector execs. A per-task seccomp filter forbids process/thread creation
    # while leaving the controller's independent result collector operational.
    # https://docs.kernel.org/userspace-api/seccomp_filter.html
    import ctypes
    import os
    import platform
    contracts = {'aarch64': (0xc00000b7, [220, 435]),
                 'x86_64': (0xc000003e, [56, 57, 58, 435])}
    architecture = platform.machine()
    if architecture not in contracts:
        raise RuntimeError('unsupported isolation architecture')
    audit_arch, calls = contracts[architecture]

    class Instruction(ctypes.Structure):
        _fields_ = [('code', ctypes.c_ushort), ('jt', ctypes.c_ubyte),
                    ('jf', ctypes.c_ubyte), ('k', ctypes.c_uint32)]

    class Program(ctypes.Structure):
        _fields_ = [('length', ctypes.c_ushort), ('filter', ctypes.POINTER(Instruction))]

    code = [(0x20, 0, 0, 4), (0x15, 1, 0, audit_arch), (0x06, 0, 0, 0x80000000),
            (0x20, 0, 0, 0), (0x35, 0, 1, 0x40000000), (0x06, 0, 0, 0x00050001)]
    for call in calls:
        code += [(0x15, 0, 1, call), (0x06, 0, 0, 0x00050001)]
    code += [(0x06, 0, 0, 0x7fff0000)]
    instructions = (Instruction * len(code))(*(Instruction(*row) for row in code))
    program = Program(len(code), instructions)
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.prctl(38, 1, 0, 0, 0) or libc.prctl(22, 2, ctypes.byref(program), 0, 0):
        raise RuntimeError('child-process isolation unavailable')
    try:
        child = os.fork()
    except PermissionError:
        return
    if child == 0:
        os._exit(1)
    os.waitpid(child, 0)
    raise RuntimeError('child-process prohibition failed')


class VisibleHTML(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.hidden = []
        self.parts = []
        self.title = []
        self.in_title = False
        self.bytes = 0

    def handle_starttag(self, tag, attrs):
        if tag in ('script', 'style', 'noscript', 'template', 'iframe', 'object', 'svg'):
            self.hidden.append(tag)
        if tag == 'title':
            self.in_title = True
        if not self.hidden and tag in ('p', 'br', 'div', 'li', 'tr', 'h1', 'h2', 'h3', 'pre', 'section'):
            self.handle_data('\n')

    def handle_endtag(self, tag):
        if self.hidden and tag == self.hidden[-1]:
            self.hidden.pop()
        if tag == 'title':
            self.in_title = False
        if not self.hidden and tag in ('p', 'div', 'li', 'tr', 'pre'):
            self.handle_data('\n')

    def handle_data(self, text):
        if self.hidden:
            return
        if self.in_title:
            if sum(len(part) for part in self.title) < 512:
                self.title.append(text[:512])
            return
        # Bound allocation before append, including malformed HTML with a
        # single large data node. Resource limits protect parser internals too.
        size = len(text.encode('utf-8'))
        if self.bytes + size > MAX_TEXT:
            remaining = MAX_TEXT - self.bytes
            self.parts.append(text.encode('utf-8')[:remaining].decode('utf-8', errors='ignore'))
            self.bytes = MAX_TEXT + 1
            raise OverflowError('text_limit')
        self.parts.append(text)
        self.bytes += size


def decompress(raw, encoding):
    if len(raw) > MAX_BYTES:
        raise OverflowError('download_limit')
    if encoding == 'identity':
        return raw
    if encoding == 'gzip':
        with gzip.GzipFile(fileobj=io.BytesIO(raw)) as source:
            result = source.read(MAX_BYTES + 1)
    elif encoding == 'deflate':
        decoder = zlib.decompressobj()
        result = decoder.decompress(raw, MAX_BYTES + 1)
        if decoder.unconsumed_tail:
            raise OverflowError('decompressed_limit')
        if not decoder.eof or decoder.unused_data:
            raise ValueError('incomplete_encoding')
    else:
        raise ValueError('unsupported_encoding')
    if len(result) > MAX_BYTES:
        raise OverflowError('decompressed_limit')
    return result


def extract(raw, media_type, encoding):
    raw = decompress(raw, encoding)
    title, pages, state = '', [], 'complete'
    if media_type == 'application/pdf':
        from pypdf import PdfReader
        reader = PdfReader(io.BytesIO(raw), strict=True)
        if reader.is_encrypted:
            return dict(state='encrypted_pdf', lines=[], title='')
        if len(reader.pages) > 200:
            return dict(state='page_limit', lines=[], title='')
        used = 0
        for index, page in enumerate(reader.pages, 1):
            text = page.extract_text(extraction_mode='plain') or ''
            budget = MAX_TEXT - used
            encoded = text.encode('utf-8')
            if len(encoded) > budget:
                text = encoded[:budget].decode('utf-8', errors='ignore')
                state = 'text_limit'
            pages.append((index, text))
            used += len(text.encode('utf-8'))
            if state == 'text_limit':
                break
    elif media_type == 'text/html':
        parser = VisibleHTML()
        try:
            parser.feed(raw.decode('utf-8', errors='replace'))
            parser.close()
        except OverflowError:
            state = 'text_limit'
        pages = [(None, ''.join(parser.parts))]
        title = ''.join(parser.title).strip()[:512]
    elif media_type == 'text/plain':
        text = raw.decode('utf-8', errors='replace')
        if len(text.encode('utf-8')) > MAX_TEXT:
            text = text.encode('utf-8')[:MAX_TEXT].decode('utf-8', errors='ignore')
            state = 'text_limit'
        pages = [(None, text)]
    else:
        return dict(state='unsupported_media_type', lines=[], title='')
    lines = []
    # Long lines are located parts of a single logical line, not silent loss.
    for page, text in pages:
        for line in text.splitlines():
            if not line.strip():
                continue
            encoded = line.encode('utf-8')
            while encoded:
                piece = encoded[:4096].decode('utf-8', errors='ignore')
                encoded = encoded[len(piece.encode('utf-8')):]
                lines.append(dict(text=piece, page=page))
                if len(lines) >= MAX_LINES:
                    return dict(state='text_limit', lines=lines, title=title)
    if not lines:
        state = 'non_extractable'
    return dict(state=state, lines=lines, title=title)


if __name__ == '__main__':
    resource.setrlimit(resource.RLIMIT_CPU, (5, 5))
    resource.setrlimit(resource.RLIMIT_AS, (256 * 1024 * 1024, 256 * 1024 * 1024))
    deny_children()
    try:
        media_type, encoding = json.loads(sys.argv[1])
        value = extract(Path('/references/original').read_bytes(), media_type, encoding)
    except OverflowError as error:
        value = dict(state=str(error), lines=[], title='')
    except Exception:
        value = dict(state='malformed_document', lines=[], title='')
    value['isolation'] = dict(child_processes_denied=True)
    print(json.dumps(value, ensure_ascii=False, separators=(',', ':')))
