"""Fixed, offline document extraction entrypoint, executed only in gVisor.

The original mount is read-only. We never execute macros, follow external
references, render HTML, recalculate formulas, or claim OCR/format fidelity.
"""
import json
import sys
from pathlib import Path
import zipfile

LIMIT, PAGE_LIMIT, FILE_LIMIT = map(int, sys.argv[1:]) if len(sys.argv) == 4 else (1048576, 200, 10 * 1048576)
assert 4 <= LIMIT <= 1048576 and 1 <= PAGE_LIMIT <= 200 and 1 <= FILE_LIMIT <= 10 * 1048576
# ADR 0053: a page with fewer non-whitespace characters than this carries no text.
MINIMUM_PAGE_TEXT = 16
output = []
used = 0
truncated = False
pages_without_text = []
page_count = None


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
    global page_count
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
        page_count = len(reader.pages)
        for index, page in enumerate(reader.pages):
            text = page.extract_text(extraction_mode='plain') or ''
            if len(''.join(text.split())) < MINIMUM_PAGE_TEXT:
                pages_without_text.append(index + 1)
                if not emit(f'Page {index + 1}: no text', text):
                    break
            elif not emit(f'Page {index + 1}', text):
                break
        if truncated:
            return 'text_limit'
        return 'non_extractable' if len(pages_without_text) == page_count else 'ready'
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
        # Plain text never reaches this parser: Agents read the original.
        return 'unsupported'
    return 'text_limit' if truncated else 'ready' if output else 'non_extractable'


if __name__ == '__main__':
    try:
        state = extract(Path('/references/original'))
    except Exception:
        state = 'malformed'
        output = []
    print(json.dumps({'state': state, 'text': ''.join(output), 'truncated': truncated,
                      'page_count': page_count, 'pages_without_text': pages_without_text}, ensure_ascii=False))
