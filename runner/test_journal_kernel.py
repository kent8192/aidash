"""Run in a disposable container with the fixed /journal and /request mounts."""
import os
import subprocess
import sys
import unittest
from pathlib import Path


@unittest.skipUnless(os.environ.get("AIDASH_JOURNAL_KERNEL_TEST") == "1", "requires an isolated runner filesystem")
class JournalKernelTest(unittest.TestCase):
    def test_variables_persist_and_journal_reads_the_replaced_file(self):
        from jupyter_client import BlockingKernelClient
        from jupyter_client.connect import write_connection_file

        journal = Path('/journal/journal.jsonl')
        journal.write_text('{"seq":1}\n')
        write_connection_file('/request/kernel.json', ip='127.0.0.1')
        process = subprocess.Popen([sys.executable, '-I', '/opt/aidash/kernel.py'],
                                   env={**os.environ, 'AIDASH_JOURNAL': '1'},
                                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        client = BlockingKernelClient(connection_file='/request/kernel.json')
        try:
            client.load_connection_file()
            client.start_channels()
            client.wait_for_ready(timeout=30)

            def execute(code):
                msg_id = client.execute(code, store_history=False, allow_stdin=False)
                output = []
                while True:
                    message = client.get_iopub_msg(timeout=30)
                    if message['parent_header'].get('msg_id') != msg_id:
                        continue
                    if message['msg_type'] == 'error':
                        self.fail(str(message['content']))
                    if message['msg_type'] == 'stream':
                        output.append(message['content']['text'])
                    if message['msg_type'] == 'status' and message['content']['execution_state'] == 'idle':
                        return ''.join(output)

            self.assertEqual(execute("saved = 17\nprint(journal()[0]['seq'])").strip(), '1')
            replacement = journal.with_suffix('.new')
            replacement.write_text('{"seq":2}\n')
            replacement.replace(journal)
            self.assertEqual(execute("print(saved, journal()[0]['seq'])").strip(), '17 2')

            # A withdrawal requires a new heap: a variable holding the old
            # export must not survive into the next cell.
            self.assertEqual(execute("withdrawn = journal()").strip(), '')
            client.stop_channels()
            process.terminate()
            process.wait(timeout=10)
            process.stderr.close()
            journal.write_text('{"seq":3,"result":"revoked"}\n')
            process = subprocess.Popen([sys.executable, '-I', '/opt/aidash/kernel.py'],
                                       env={**os.environ, 'AIDASH_JOURNAL': '1'},
                                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            client = BlockingKernelClient(connection_file='/request/kernel.json')
            client.load_connection_file()
            client.start_channels()
            client.wait_for_ready(timeout=30)
            self.assertEqual(execute("print('withdrawn' in globals(), journal()[0]['result'])").strip(),
                             'False revoked')
        finally:
            client.stop_channels()
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=10)
            if process.stderr:
                process.stderr.close()


if __name__ == '__main__':
    unittest.main()
