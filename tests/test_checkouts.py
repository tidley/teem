import json
from pathlib import Path
import subprocess
import tempfile
import unittest

from foreman_supervisor import Invalid


def git(path, *args):
    return subprocess.check_output(['git', '-C', str(path), *args], stderr=subprocess.DEVNULL).decode().strip()


class CheckoutTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='teem-managed-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / 'source'
        self.source.mkdir()
        git(self.source, 'init', '-q')
        git(self.source, 'config', 'user.name', 'Test')
        git(self.source, 'config', 'user.email', 'test@example.invalid')
        (self.source / 'src').mkdir()
        (self.source / 'src/a.txt').write_text('base\n')
        (self.source / 'README.md').write_text('Project\n')
        git(self.source, 'add', '.')
        git(self.source, 'commit', '-qm', 'baseline')
        self.base = git(self.source, 'rev-parse', 'HEAD')

    def clone(self, name='worker', snapshot=None):
        from foreman_checkouts import prepare_checkout
        return prepare_checkout(self.source, self.root / name, self.base, snapshot)

    def snapshot(self, checkout, name='snapshot'):
        from foreman_checkouts import capture
        return capture(checkout, self.base, ['src'], self.root / name)

    def test_independent_complete_clones_preserve_source_and_existing_destinations(self):
        a, b = self.clone('a'), self.clone('b')
        (a / 'src/a.txt').write_text('changed\n')
        self.assertEqual((b / 'src/a.txt').read_text(), 'base\n')
        self.assertEqual(git(self.source, 'status', '--porcelain'), '')
        self.assertEqual(git(self.source, 'rev-parse', 'HEAD'), self.base)
        with self.assertRaises(Invalid):
            self.clone('a')
        self.assertEqual((a / 'src/a.txt').read_text(), 'changed\n')

    def test_binary_new_deleted_and_mode_changes_are_reviewed_without_a_commit(self):
        from foreman_checkouts import validate_candidate
        a = self.clone()
        (a / 'src/a.txt').unlink()
        (a / 'src/new.bin').write_bytes(bytes(range(256)))
        (a / 'src/run.py').write_text('print(1)\n')
        (a / 'src/run.py').chmod(0o755)
        snapshot = self.snapshot(a)
        review = self.clone('review', snapshot)
        validate_candidate(review, self.base, snapshot['tree'])
        self.assertEqual((review / 'src/new.bin').read_bytes(), bytes(range(256)))
        self.assertFalse((review / 'src/a.txt').exists())
        self.assertEqual(git(review, 'rev-parse', 'HEAD'), self.base)
        self.assertEqual(git(self.source, 'rev-list', '--count', 'HEAD'), '1')

    def test_out_of_scope_changes_block_snapshot(self):
        a = self.clone()
        (a / 'README.md').write_text('unexpected')
        with self.assertRaises(Invalid):
            self.snapshot(a)

    def test_rename_into_scope_cannot_delete_an_out_of_scope_file(self):
        a = self.clone()
        git(a, 'mv', 'README.md', 'src/README.md')
        with self.assertRaises(Invalid):
            self.snapshot(a)

    def test_tampered_patch_or_review_tree_is_rejected(self):
        from foreman_checkouts import validate_candidate
        a = self.clone()
        (a / 'src/a.txt').write_text('candidate\n')
        snapshot = self.snapshot(a)
        review = self.clone('review', snapshot)
        (review / 'src/a.txt').write_text('tampered\n')
        with self.assertRaises(Invalid):
            validate_candidate(review, self.base, snapshot['tree'])
        Path(snapshot['patch']).chmod(0o600)
        Path(snapshot['patch']).write_text('not the patch')
        with self.assertRaises(Invalid):
            self.clone('another-review', snapshot)

    def test_review_completeness_rejects_a_later_sparse_checkout(self):
        from foreman_checkouts import candidate_tree, validate_candidate
        a = self.clone()
        tree = candidate_tree(a, self.base)
        git(a, 'sparse-checkout', 'set', 'src')
        with self.assertRaises(Invalid):
            validate_candidate(a, self.base, tree)

    def test_dirty_source_and_submodules_are_not_silently_ignored(self):
        from foreman_checkouts import source_revision
        (self.source / 'src/a.txt').write_text('unfinished')
        with self.assertRaises(Invalid):
            source_revision(self.source)

    def test_untracked_local_instructions_survive_without_copying_foreman_config(self):
        (self.source / '.git/info/exclude').write_text('/AGENTS.md\n/opencode.json\n')
        (self.source / 'AGENTS.md').write_text('Keep this local instruction\n')
        (self.source / 'opencode.json').write_text('{"privateForemanBinding":true}')
        a = self.clone()
        self.assertEqual((a / 'AGENTS.md').read_text(), 'Keep this local instruction\n')
        self.assertFalse((a / 'opencode.json').exists())
        self.assertEqual(git(a, 'status', '--porcelain'), '')

    def test_agent_todo_config_is_private_and_does_not_overwrite_project_files(self):
        from foreman_checkouts import enable_agent_todos
        a = self.clone()
        self.assertTrue(enable_agent_todos(a, ['src']))
        value = json.loads((a / 'opencode.json').read_text())
        self.assertTrue(value['experimental']['todo'])
        self.assertNotIn('mcp', value)
        self.assertEqual(git(a, 'status', '--porcelain'), '')
        b = self.clone('other')
        (b / 'opencode.json').write_text('{"userSetting":true}')
        self.assertFalse(enable_agent_todos(b, ['src']))
        self.assertEqual((b / 'opencode.json').read_text(), '{"userSetting":true}')
        c = self.clone('config-task')
        self.assertFalse(enable_agent_todos(c, ['opencode.json']))
        d = self.clone('ignore-override')
        (d / '.gitignore').write_text('!opencode.json\n')
        self.assertFalse(enable_agent_todos(d, ['src']))
        self.assertFalse((d / 'opencode.json').exists())


if __name__ == '__main__':
    unittest.main()
