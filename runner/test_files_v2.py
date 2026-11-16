import base64

import pytest

from files_v2 import FileService, InvalidPath


def test_read_search_and_manifest_never_follow_symlinks(tmp_path):
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    (scratch / 'a.txt').write_text('alpha\nbeta\n')
    (tmp_path / 'outside').mkdir()
    (tmp_path / 'outside' / 'secret.txt').write_text('alpha secret')
    (scratch / 'link').symlink_to(tmp_path / 'outside', target_is_directory=True)
    service = FileService(scratch, tmp_path / 'state')
    result = service.read({'path': 'a.txt', 'offset': 1, 'limit': 3})
    assert base64.b64decode(result['data']) == b'lph'
    assert (result['size'], result['next_offset'], result['eof']) == (11, 4, False)
    with pytest.raises(InvalidPath):
        service.read({'path': 'link/secret.txt', 'limit': 10})
    with pytest.raises(InvalidPath):
        service.read({'path': '../outside/secret.txt', 'limit': 10})
    result = service.search({'pattern': 'alpha', 'regex': False, 'max_results': 10})
    assert result == {'matches': [{'path': 'a.txt', 'line': 1, 'text': 'alpha'}], 'truncated': False}
    assert service.search({'path': 'a.txt', 'pattern': 'beta', 'regex': False, 'max_results': 10}) == {
        'matches': [{'path': 'a.txt', 'line': 2, 'text': 'beta'}], 'truncated': False}
    manifest = service.manifest({'root': '.', 'ignore_files': [], 'exclude': [], 'include': [], 'max_files': 10})
    assert [entry['path'] for entry in manifest['files']] == ['a.txt']
    assert manifest['skipped_symlinks'] == ['link']


def test_write_chunks_publish_only_on_final_and_refuse_wrong_offset(tmp_path):
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    (scratch / 'x').write_bytes(b'old')
    service = FileService(scratch, tmp_path / 'state')
    req = {'run_id': 'r', 'path': 'x', 'data': base64.b64encode(b'new').decode(), 'offset': 0, 'final': False}
    assert service.write(req) == {'size': 3}
    assert (scratch / 'x').read_bytes() == b'old'
    with pytest.raises(ValueError):
        service.write(dict(req, offset=2, final=True))
    assert service.write(dict(req, data=base64.b64encode(b'!').decode(), offset=3, final=True)) == {'size': 4}
    assert (scratch / 'x').read_bytes() == b'new!'


def test_manifest_ignore_precedence_and_nested_ignore_files(tmp_path):
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    (scratch / '.gitignore').write_text('*.log\n!keep.log\nbuild/\n')
    (scratch / 'omit.log').write_text('omit')
    (scratch / 'keep.log').write_text('keep')
    (scratch / 'force.log').write_text('force')
    (scratch / 'build').mkdir()
    (scratch / 'build' / 'out').write_text('build')
    (scratch / 'nested').mkdir()
    (scratch / 'nested' / '.gitignore').write_text('*.txt\n')
    (scratch / 'nested' / 'omit.txt').write_text('omit')
    service = FileService(scratch, tmp_path / 'state')
    result = service.manifest({'root': '.', 'ignore_files': ['.gitignore'],
                               'exclude': ['keep.log'], 'include': ['force.log'], 'max_files': 100})
    assert {item['path'] for item in result['files']} == {'.gitignore', 'nested/.gitignore', 'force.log'}


def test_patch_validates_all_hunks_before_changing_any_file(tmp_path):
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    (scratch / 'one').write_text('old\n')
    (scratch / 'two').write_text('other\n')
    service = FileService(scratch, tmp_path / 'state')
    patch = ('*** Begin Patch\n*** Update File: one\n@@\n-old\n+new\n'
             '*** Update File: two\n@@\n-missing\n+wrong\n*** End Patch\n')
    with pytest.raises(ValueError):
        service.patch({'run_id': 'r', 'patch': patch})
    assert (scratch / 'one').read_text() == 'old\n'
    assert (scratch / 'two').read_text() == 'other\n'
    patch = ('*** Begin Patch\n*** Update File: one\n@@\n-old\n+new\n'
             '*** Delete File: two\n*** Add File: three\n+created\n*** End Patch\n')
    assert service.patch({'run_id': 'r', 'patch': patch}) == {'changed': ['one', 'two', 'three']}
    assert (scratch / 'one').read_text() == 'new\n'
    assert not (scratch / 'two').exists()
    assert (scratch / 'three').read_text() == 'created\n'


def test_search_reads_large_text_files_in_bounded_chunks_and_skips_binary(tmp_path):
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    (scratch / 'large').write_bytes(b'x\n' * 600000 + b'target\n')
    (scratch / 'binary').write_bytes(b'target\n\x00binary')
    service = FileService(scratch, tmp_path / 'state')
    result = service.search({'pattern': 'target', 'regex': False, 'max_results': 10})
    assert result == {'matches': [{'path': 'large', 'line': 600001, 'text': 'target'}], 'truncated': False}


def test_gitignore_parent_negation_and_character_classes(tmp_path):
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    (scratch / '.gitignore').write_text('blocked/\n!blocked/keep.txt\n[a-c].txt\n\\#literal\n')
    (scratch / 'blocked').mkdir()
    (scratch / 'blocked' / 'keep.txt').write_text('hidden')
    (scratch / 'a.txt').write_text('hidden')
    (scratch / 'd.txt').write_text('visible')
    (scratch / '#literal').write_text('hidden')
    service = FileService(scratch, tmp_path / 'state')
    result = service.manifest({'root': '.', 'ignore_files': ['.gitignore'], 'include': [],
                               'exclude': [], 'max_files': 20})
    assert {item['path'] for item in result['files']} == {'.gitignore', 'd.txt'}


def test_manifest_subdirectory_honors_ancestor_ignore_file(tmp_path):
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    (scratch / '.gitignore').write_text('*.log\n')
    (scratch / 'nested').mkdir()
    (scratch / 'nested' / 'hidden.log').write_text('hidden')
    (scratch / 'nested' / 'shown.txt').write_text('shown')
    service = FileService(scratch, tmp_path / 'state')
    result = service.manifest({'root': 'nested', 'ignore_files': ['.gitignore'],
                               'include': [], 'exclude': [], 'max_files': 20})
    assert [item['path'] for item in result['files']] == ['nested/shown.txt']


def test_patch_changes_only_the_lines_it_names(tmp_path):
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    (scratch / 'crlf').write_bytes(b'one\r\ntwo\r\nthree\r\n')
    (scratch / 'feed').write_bytes(b'a\x0cb\nold\nlast')
    service = FileService(scratch, tmp_path / 'state')
    patch = ('*** Begin Patch\n*** Update File: crlf\n@@\n-two\n+TWO\n+added\n'
             '*** Update File: feed\n@@\n-old\n+new\n*** End Patch\n')
    assert service.patch({'run_id': 'r', 'patch': patch}) == {'changed': ['crlf', 'feed']}
    assert (scratch / 'crlf').read_bytes() == b'one\r\nTWO\r\nadded\r\nthree\r\n'
    assert (scratch / 'feed').read_bytes() == b'a\x0cb\nnew\nlast'
    patch = '*** Begin Patch\n*** Update File: feed\n@@\n-last\n+final\n*** End of File\n*** End Patch\n'
    service.patch({'run_id': 'r', 'patch': patch})
    assert (scratch / 'feed').read_bytes() == b'a\x0cb\nnew\nfinal'
    patch = '*** Begin Patch\n*** Update File: feed\n@@ final\n+appended\n*** End Patch\n'
    service.patch({'run_id': 'r', 'patch': patch})
    assert (scratch / 'feed').read_bytes() == b'a\x0cb\nnew\nfinal\nappended'


def test_a_fifo_is_refused_at_once_and_writes_keep_a_readable_mode(tmp_path):
    import os
    import stat
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    os.mkfifo(scratch / 'pipe')
    (scratch / 'kept').write_bytes(b'old')
    os.chmod(scratch / 'kept', 0o755)
    service = FileService(scratch, tmp_path / 'state')
    with pytest.raises(InvalidPath):
        service.read({'path': 'pipe', 'limit': 10})
    assert service.search({'pattern': 'x', 'regex': False, 'max_results': 10})['matches'] == []
    chunk = {'run_id': 'r', 'data': base64.b64encode(b'new').decode(), 'offset': 0, 'final': True}
    service.write(dict(chunk, path='fresh'))
    service.write(dict(chunk, path='kept'))
    assert stat.S_IMODE(os.stat(scratch / 'fresh').st_mode) == 0o644
    assert stat.S_IMODE(os.stat(scratch / 'kept').st_mode) == 0o755


def test_a_pathological_search_pattern_stops_at_the_time_budget(tmp_path):
    import time
    from files_v2 import run_bounded
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    (scratch / 'a').write_text('a' * 64 + 'b\n')
    service = FileService(scratch, tmp_path / 'state')
    started = time.monotonic()
    with pytest.raises(ValueError, match='time budget'):
        run_bounded(service, 'search', {'pattern': '(a+)+$', 'regex': True, 'max_results': 1}, seconds=1)
    assert time.monotonic() - started < 5


def test_new_files_get_their_missing_parent_directories(tmp_path):
    scratch = tmp_path / 'scratch'
    scratch.mkdir()
    (tmp_path / 'outside').mkdir()
    (scratch / 'link').symlink_to(tmp_path / 'outside', target_is_directory=True)
    service = FileService(scratch, tmp_path / 'state')
    patch = '*** Begin Patch\n*** Add File: src/pkg/main.py\n+print(1)\n*** End Patch\n'
    assert service.patch({'run_id': 'r', 'patch': patch}) == {'changed': ['src/pkg/main.py']}
    assert (scratch / 'src' / 'pkg' / 'main.py').read_text() == 'print(1)\n'
    chunk = {'run_id': 'r', 'data': base64.b64encode(b'x').decode(), 'offset': 0, 'final': True}
    service.write(dict(chunk, path='copies/deep/x'))
    assert (scratch / 'copies' / 'deep' / 'x').read_bytes() == b'x'
    with pytest.raises(InvalidPath):
        service.write(dict(chunk, path='link/new/x'))
    assert list((tmp_path / 'outside').iterdir()) == []
