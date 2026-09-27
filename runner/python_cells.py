"""Trusted Jupyter client in the collector's separate PID/filesystem namespace.

Only node-guard may create requests in /tmp/cells. Kernel messages are untrusted
results. A cell becoming idle is NOT proof that background writers stopped;
node-guard freezes the whole gVisor sandbox before any files are published.
"""
import base64
import json
import os
from pathlib import Path
import queue
import time
from jupyter_client import BlockingKernelClient


def save(path, data):
    temporary = path.with_suffix('.pending')
    with temporary.open('w') as file:
        json.dump(data, file, separators=(',', ':'))
        file.flush()
        os.fsync(file.fileno())
    os.replace(temporary, path)
    fd = os.open(path.parent, os.O_DIRECTORY)
    os.fsync(fd)
    os.close(fd)


def serve():
    directory = Path('/tmp/cells')
    directory.mkdir(mode=0o700, exist_ok=True)
    client = None
    limit = int(os.environ['AIDASH_OUTPUT_BYTES'])
    while True:
        requests = sorted(directory.glob('*.input'))
        for path in requests:
            result_path = path.with_suffix('.result')
            if result_path.exists():
                continue
            incoming = json.loads(path.read_bytes())
            if client is None:
                if not Path('/request/kernel.json').exists():
                    break
                client = BlockingKernelClient(connection_file='/request/kernel.json')
                client.load_connection_file()
                client.start_channels()
                try:
                    client.wait_for_ready(timeout=30)
                except Exception:
                    client.stop_channels()
                    client = None
                    break
            output = bytearray()
            displays = []
            truncated = False
            failure = None
            # Durable in this live Pod before sending an effectful execute.
            # A collector restart leaves an explicit uncertain cell, never replay.
            save(result_path, {'status': 'submitting', 'digest': incoming['digest']})
            try:
                message_id = client.execute(incoming['code'], store_history=False, allow_stdin=False, stop_on_error=True)
                save(result_path, {'status': 'running', 'message_id': message_id, 'digest': incoming['digest']})
                last_preview = 0
                while True:
                    try:
                        message = client.get_iopub_msg(timeout=1)
                    except queue.Empty:
                        continue
                    if message.get('parent_header', {}).get('msg_id') != message_id:
                        continue
                    kind, content = message['msg_type'], message['content']
                    if kind == 'stream':
                        chunk = str(content.get('text', '')).encode()
                        remaining = max(0, limit - len(output))
                        output.extend(chunk[:remaining])
                        truncated |= len(chunk) > remaining
                    elif kind == 'error':
                        failure = {'type': str(content.get('ename', 'Error'))[:128], 'message': str(content.get('evalue', ''))[:2048]}
                    elif kind in ('display_data', 'execute_result'):
                        data = content.get('data', {})
                        if isinstance(data.get('text/plain'), str):
                            chunk = data['text/plain'].encode() + b'\n'
                            remaining = max(0, limit - len(output))
                            output.extend(chunk[:remaining])
                            truncated |= len(chunk) > remaining
                        encoded = data.get('image/png')
                        if isinstance(encoded, str):
                            try:
                                image = base64.b64decode(encoded, validate=True)
                                total = sum(len(d['data']) * 3 // 4 for d in displays)
                                if len(displays) < 16 and total + len(image) <= limit and image.startswith(b'\x89PNG\r\n\x1a\n'):
                                    displays.append({'mime': 'image/png', 'data': encoded})
                                else:
                                    truncated = True
                            except ValueError:
                                truncated = True
                    elif kind == 'status' and content.get('execution_state') == 'idle':
                        break
                    if time.monotonic() - last_preview > .5:
                        save(result_path, {'status': 'running', 'digest': incoming['digest'],
                                          'stdout': base64.b64encode(output[:16384]).decode(), 'truncated': truncated})
                        last_preview = time.monotonic()
                save(result_path, {'status': 'failed' if failure else 'completed', 'digest': incoming['digest'],
                                  'stdout': base64.b64encode(output).decode(), 'displays': displays,
                                  'truncated': truncated, 'error': failure, 'exit_code': 1 if failure else 0})
            except Exception as error:
                save(result_path, {'status': 'uncertain', 'digest': incoming['digest'],
                                  'error': {'type': type(error).__name__, 'message': 'Jupyter reply unavailable; code was not replayed.'}})
        time.sleep(.05)
