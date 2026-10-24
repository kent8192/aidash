"""Fixed, offline document extraction entrypoint, executed only in gVisor.

The original mount is read-only. We never execute macros, follow external
references, render HTML, recalculate formulas, or claim OCR/format fidelity.
"""
import json
import sys
from pathlib import Path
import zipfile

LIMIT, PAGE_LIMIT, FILE_LIMIT = map(int, sys.argv[1:]) if len(sys.argv) == 4 else (65536, 200, 10 * 1048576)
assert 4 <= LIMIT <= 65536 and 1 <= PAGE_LIMIT <= 200 and 1 <= FILE_LIMIT <= 10 * 1048576
output = []
used = 0
truncated = False


def emit(location, text):
    global used, truncated
    value = f'[{location}]\n{text}\n'
    encoded = value.encode('utf-8')
    remaining = LIMIT - used
    if len(encoded) > remaining:
        value = encoded[:remaining].decode('utf-8', errors='ignore')
        truncated = True
    used += len(value.encode('utf-8'))
    if value:
        output.append(value)
    return not truncated


def extract(path):
    if path.stat().st_size > FILE_LIMIT:
        return 'file_limit'
    with path.open('rb') as file:
        magic = file.read(8)
    if magic.startswith(b'%PDF-'):
        from pypdf import PdfReader
        reader = PdfReader(path, strict=True)
        if reader.is_encrypted:
            return 'encrypted'
        if len(reader.pages) > PAGE_LIMIT:
            return 'page_limit'
        for index, page in enumerate(reader.pages):
            text = page.extract_text(extraction_mode='plain')
            if text and not emit(f'Page {index + 1}', text):
                break
    elif magic.startswith(b'PK'):
        with zipfile.ZipFile(path) as archive:
            entries = archive.infolist()
            if len(entries) > 10000 or sum(e.file_size for e in entries) > 100 * 1048576:
                return 'expanded_size_limit'
            if any(e.flag_bits & 1 for e in entries):
                return 'encrypted'
            if 'xl/workbook.xml' not in archive.namelist():
                return 'unsupported'
        import openpyxl
        workbook = openpyxl.load_workbook(path.open('rb'), read_only=True, data_only=False, keep_links=False)
        try:
            for sheet in workbook:
                # Ignore declared dimensions, which may be malformed or stale.
                sheet.reset_dimensions()
                for row in sheet.iter_rows():
                    for cell in row:
                        if cell.value is not None and not emit(f'Sheet {sheet.title}!{cell.coordinate}', str(cell.value)):
                            return 'text_limit'
        finally:
            workbook.close()
    else:
        try:
            data = path.read_bytes().decode('utf-8')
        except UnicodeDecodeError:
            return 'unsupported'
        if '\x00' in data:
            return 'unsupported'
        for number, line in enumerate(data.splitlines(), 1):
            if not emit(f'Line {number}', line):
                break
    return 'text_limit' if truncated else 'ready' if output else 'non_extractable'


try:
    state = extract(Path('/references/original'))
except Exception:
    state = 'malformed'
    output = []
print(json.dumps({'state': state, 'text': ''.join(output), 'truncated': truncated}, ensure_ascii=False))
