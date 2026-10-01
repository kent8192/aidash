#!/usr/bin/env python3
"""Format fixtures through the real admitted, offline gVisor Web reader.

Usage: python3 scripts/test-web-extractor.py PRIVATE_RUNTIME_DIRECTORY
The directory is created by setup-capability-runtime.py. Tokens stay in memory;
only fixture outcomes and measured durations are printed.
"""
import base64
import gzip
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import time
import urllib.error
import urllib.request
import uuid


def pdf(pages):
    objects = [b'<< /Type /Catalog /Pages 2 0 R >>', b'']
    kids = []
    for text in pages:
        page, stream = len(objects) + 1, len(objects) + 2
        kids.append(f'{page} 0 R')
        objects.append(f'<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> /Contents {stream} 0 R >>'.encode())
        content = f'BT /F1 12 Tf 50 700 Td ({text}) Tj ET'.encode()
        objects.append(f'<< /Length {len(content)} >>\nstream\n'.encode() + content + b'\nendstream')
    objects[1] = f'<< /Type /Pages /Count {len(pages)} /Kids [{" ".join(kids)}] >>'.encode()
    result, offsets = b'%PDF-1.4\n', [0]
    for index, value in enumerate(objects, 1):
        offsets.append(len(result))
        result += f'{index} 0 obj\n'.encode() + value + b'\nendobj\n'
    xref = len(result)
    result += f'xref\n0 {len(offsets)}\n0000000000 65535 f \n'.encode()
    result += b''.join(f'{offset:010} 00000 n \n'.encode() for offset in offsets[1:])
    return result + f'trailer\n<< /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n'.encode()


def main():
    root = Path(sys.argv[1]).resolve()
    profile = json.loads((root / 'profile.json').read_text())['runner']
    token = (root / 'token').read_text().strip()
    client = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def request(path, value=None):
        data = json.dumps(value).encode() if value is not None else None
        req = urllib.request.Request(profile['endpoint'] + path, data=data,
            headers={'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'})
        with client.open(req, timeout=30) as response:
            return json.load(response)

    health = request('/v1/health')
    assert health['verified'] and health['python_verified']
    assert health['web_extraction_protocol'] == 'aidash-web-extraction/1'
    results = []

    def extract(label, raw, media, encoding='identity'):
        operation, file = str(uuid.uuid4()), str(uuid.uuid4())
        digest = hashlib.sha256(raw).hexdigest()
        code = json.dumps([media, encoding])
        operation_digest = hashlib.sha256((digest + code).encode()).hexdigest()
        value = dict(operation_id=operation, area_id=operation, epoch=1, digest=operation_digest,
            kind='web_extract', code=code, seconds=5,
            files=[dict(file_id=file, path='original', scope='references', size=len(raw), digest=digest)])
        started = time.monotonic()
        assert request('/v1/operations', value)['status'] == 'awaiting_files'
        for offset in range(0, len(raw), 1024 * 1024):
            request(f'/v1/operations/{operation}/inputs/{file}?offset={offset}',
                dict(data=base64.b64encode(raw[offset:offset + 1024 * 1024]).decode()))
        request(f'/v1/operations/{operation}/start', {})
        while time.monotonic() - started < 30:
            observed = request(f'/v1/operations/{operation}')
            if observed['status'] in ('completed', 'failed', 'cancelled', 'uncertain'):
                break
            time.sleep(.1)
        assert observed['status'] == 'completed', (label, observed['status'], observed.get('error'))
        assert observed['termination_confirmed'] and observed['exit_code'] == 0 and not observed['truncated'], label
        output = json.loads(base64.b64decode(observed['stdout']))
        assert output['isolation']['child_processes_denied'], label
        assert sum(len(line['text'].encode()) for line in output['lines']) <= 1024 * 1024
        assert all(len(line['text'].encode()) <= 4096 for line in output['lines'])
        assert not observed['files'] and not observed.get('displays', [])
        request(f'/v1/operations/{operation}/ack', dict(digest=operation_digest))
        retained = request(f'/v1/operations/{operation}')
        assert retained['acknowledged'] and not retained['stdout']
        assert not (root / 'journal' / (operation + '.inputs')).exists()
        results.append(dict(case=label, state=output['state'], duration_seconds=round(time.monotonic()-started, 3)))
        return output

    html = extract('html-untrusted-script', b'<title>Fixture</title><p>Visible evidence</p><script>fetch("http://169.254.169.254/"); throw Error("executed")</script>', 'text/html')
    assert html['state'] == 'complete' and html['title'] == 'Fixture'
    assert 'Visible evidence' in str(html['lines']) and '169.254' not in str(html['lines'])
    text = extract('plain-unicode-gzip', gzip.compress('東京 Straße\nSecond line'.encode()), 'text/plain', 'gzip')
    assert text['state'] == 'complete' and text['lines'][0]['text'] == '東京 Straße'
    document = extract('text-pdf-pages', pdf(['First page evidence', 'Second page evidence']), 'application/pdf')
    assert document['state'] == 'complete' and [line['page'] for line in document['lines']] == [1, 2]
    assert extract('scanned-pdf', pdf(['']), 'application/pdf')['state'] == 'non_extractable'
    assert extract('pdf-page-limit', pdf(['x'] * 201), 'application/pdf')['state'] == 'page_limit'
    assert extract('malformed-pdf', b'%PDF-1.4 broken', 'application/pdf')['state'] == 'malformed_document'
    assert extract('decompression-bomb', gzip.compress(b'x' * (10*1024*1024+1)), 'text/plain', 'gzip')['state'] == 'decompressed_limit'
    assert extract('text-limit', b'x' * (1024*1024+1), 'text/plain')['state'] == 'text_limit'
    # Generate a synthetic encrypted fixture in the pinned image. This does
    # not parse foreign content on the host or expose credentials to the image.
    image_id = profile['image'].split('@')[1] if (root / 'owner.json').exists() else profile['image']
    encrypted = subprocess.check_output(['docker', 'run', '--rm', '--network', 'none', '--entrypoint', 'python', image_id, '-I', '-c',
        'import io,base64;from pypdf import PdfWriter;w=PdfWriter();w.add_blank_page(width=72,height=72);w.encrypt("fixture");b=io.BytesIO();w.write(b);print(base64.b64encode(b.getvalue()).decode())'])
    assert extract('encrypted-pdf', base64.b64decode(encrypted), 'application/pdf')['state'] == 'encrypted_pdf'
    print(json.dumps(dict(protocol=health['web_extraction_protocol'], image=health['image'], verified=health['verified'], cases=results), indent=2))


if __name__ == '__main__':
    main()
