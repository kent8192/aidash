import contextlib
import io
import json
import os
import tempfile
import unittest
import uuid
from pathlib import Path
from unittest.mock import patch

os.environ.setdefault('AIDASH_OUTPUT_BYTES', '1024')
import collector
import node_guard


class ExportRefusalTest(unittest.TestCase):
    def export(self, root, limit=1024):
        output = io.StringIO()
        with patch.dict(collector.ROOTS, {'working': root}), \
             patch.object(collector.sys, 'argv', ['collector.py', 'export', str(limit)]), \
             contextlib.redirect_stdout(output):
            collector.export()
        return json.loads(output.getvalue())

    def test_symlink(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'link').symlink_to('/tmp')
            self.assertEqual(self.export(root), {'refused': {'reason': 'symlink', 'path': 'link', 'limit': None}})

    def test_hard_link(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'a').write_text('x')
            os.link(root / 'a', root / 'b')
            self.assertEqual(self.export(root), {'refused': {'reason': 'hard_link', 'path': 'a', 'limit': None}})

    def test_file_count(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for i in range(4097):
                (root / f'{i:04}').touch()
            self.assertEqual(self.export(root), {'refused': {'reason': 'file_count', 'path': '4096', 'limit': 4096}})

    def test_unsafe_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'bad\\name').mkdir()
            self.assertEqual(self.export(root), {'refused': {'reason': 'unsafe_directory', 'path': 'bad\\name', 'limit': None}})

    def test_guard_export_refuses_without_following_symlink(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            work = root / 'work'
            work.mkdir()
            (work / 'link').symlink_to('/tmp')
            area, pod, cid = str(uuid.uuid4()), str(uuid.uuid4()), 'a' * 64
            request = {'action': 'export', 'area_id': area, 'pod_uid': pod, 'container_id': cid,
                       'image': 'image', 'epoch': 1, 'working_bytes': 1024}
            with patch.object(node_guard, 'ROOT', root / 'state'), \
                 patch.object(node_guard, 'validate', return_value=(cid, pod, area, {'status': 'stopped'})), \
                 patch.object(node_guard, 'volume', return_value=work):
                result = node_guard.handle(request)
            self.assertEqual(result, {'refused': {'reason': 'symlink', 'path': 'link', 'limit': None}})

    def test_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'a').write_text('abc')
            self.assertEqual(self.export(root, 2), {'refused': {'reason': 'bytes', 'path': 'a', 'limit': 2}})
