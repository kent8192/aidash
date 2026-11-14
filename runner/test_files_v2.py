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
