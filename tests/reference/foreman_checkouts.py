"""Frozen checkout/snapshot reference for Rust regression qualification."""

import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import tempfile

from foreman_supervisor import Invalid


def git_bytes(checkout, *args, input=None, index=None):
    env = {k: v for k, v in os.environ.items() if not k.startswith('GIT_')}
    env.update(GIT_CONFIG_NOSYSTEM='1', GIT_CONFIG_GLOBAL='/dev/null', GIT_TERMINAL_PROMPT='0')
    if index is not None:
        env['GIT_INDEX_FILE'] = str(index)
    try:
        return subprocess.run(['git', '-c', 'core.hooksPath=/dev/null', '-c', 'protocol.file.allow=always',
                               '--no-optional-locks', '-C', str(checkout), *args], input=input,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env,
                              timeout=120, check=True).stdout
    except (OSError, subprocess.SubprocessError):
        raise Invalid('Managed Git operation failed; preserve the checkout for inspection') from None


def git_text(checkout, *args, **kwargs):
    return git_bytes(checkout, *args, **kwargs).decode('utf-8').strip()


def relative_path(value):
    if (not isinstance(value, str) or not value or PurePosixPath(value).is_absolute()
            or any(p in ('', '.', '..', '.git') for p in value.split('/'))
            or any(c in value for c in '*?[]\\\n\r\0')):
        raise Invalid('Use explicit relative paths without traversal, Git metadata, or patterns')
    return value


def source_revision(source):
    source = Path(source).resolve()
    if git_text(source, 'rev-parse', '--show-toplevel') != str(source):
        raise Invalid('Configured project must be a Git checkout root')
    if git_bytes(source, 'status', '--porcelain', '--untracked-files=all'):
        raise Invalid('Source project has unfinished changes; no changes were discarded')
    base = git_text(source, 'rev-parse', 'HEAD')
    if any(entry.startswith(b'160000 ') for entry in git_bytes(source, 'ls-tree', '-rz', base).split(b'\0')):
        raise Invalid('Managed workflows do not yet qualify submodule checkouts')
    return base


def candidate_tree(checkout, base):
    checkout = Path(checkout).resolve()
    if (git_text(checkout, 'rev-parse', '--show-toplevel') != str(checkout)
            or git_text(checkout, 'rev-parse', 'HEAD') != base
            or git_text(checkout, 'rev-parse', '--is-shallow-repository') != 'false'
            or git_text(checkout, 'config', '--bool', '--default', 'false', '--get', 'core.sparseCheckout') != 'false'):
        raise Invalid('Candidate root, base revision, or repository completeness changed')
    gitdir = Path(git_text(checkout, 'rev-parse', '--absolute-git-dir'))
    with tempfile.TemporaryDirectory(prefix='teem-index-', dir=gitdir) as directory:
        index = Path(directory) / 'index'
        git_bytes(checkout, 'read-tree', base, index=index)
        git_bytes(checkout, 'add', '-A', '--', '.', index=index)
        return git_text(checkout, 'write-tree', index=index)


def validate_candidate(checkout, base, tree):
    if candidate_tree(checkout, base) != tree:
        raise Invalid('Candidate contents do not match the exact recorded snapshot')


def verify_snapshot(snapshot):
    patch = Path(snapshot['patch'])
    if patch.is_symlink():
        raise Invalid('Snapshot patch must not be a symlink')
    data = patch.read_bytes()
    if hashlib.sha256(data).hexdigest() != snapshot['sha256']:
        raise Invalid('Snapshot patch digest changed')
    return data


def prepare_checkout(source, destination, base, snapshot=None):
    source = Path(source).resolve()
    destination = Path(destination).absolute()
    if destination.exists() or destination.is_symlink() or any(p.is_symlink() for p in destination.parents):
        raise Invalid('Managed destination already exists or follows a symlink; never overwrite it')
    patch = verify_snapshot(snapshot) if snapshot else None
    destination.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
    git_bytes(source, 'clone', '--no-local', '--no-checkout', '--', str(source), str(destination))
    destination.chmod(0o700)
    git_bytes(destination, 'checkout', '--detach', base)
    tracked_agents = bool(git_bytes(destination, 'ls-files', '--', 'AGENTS.md'))
    if not tracked_agents:
        instructions = source / 'AGENTS.md'
        template = Path.home() / '.config/opencode/templates/AGENTS.md'
        if not instructions.is_file() or instructions.is_symlink():
            instructions = template
        if instructions.is_file() and not instructions.is_symlink():
            shutil.copyfile(instructions, destination / 'AGENTS.md')
            with (destination / '.git/info/exclude').open('a') as stream:
                stream.write('\n/AGENTS.md\n')
    if patch:
        git_bytes(destination, 'apply', '--binary', '--index', '-', input=patch)
    if snapshot:
        validate_candidate(destination, base, snapshot['tree'])
    return destination


def capture(checkout, base, scope, destination):
    for path in scope:
        relative_path(path)
    tree = candidate_tree(checkout, base)
    changed = set(git_bytes(checkout, 'diff', '--no-renames', '--name-only', '-z', base, tree, '--').split(b'\0'))
    for raw in changed - {b''}:
        path = relative_path(raw.decode('utf-8'))
        if not any(path == s or path.startswith(s + '/') for s in scope):
            raise Invalid('Worker changed files outside its assigned scope')
    patch = git_bytes(checkout, 'diff', '--binary', '--no-ext-diff', '--no-textconv', base, tree, '--')
    destination = Path(destination)
    destination.mkdir(mode=0o700, parents=True, exist_ok=False)
    patch_path = destination / 'candidate.patch'
    patch_path.write_bytes(patch)
    patch_path.chmod(0o400)
    snapshot = {'base': base, 'tree': tree, 'sha256': hashlib.sha256(patch).hexdigest(), 'patch': str(patch_path)}
    manifest = destination / 'manifest.json'
    manifest.write_text(json.dumps(snapshot, indent=2) + '\n')
    manifest.chmod(0o400)
    return snapshot


def enable_agent_todos(checkout, scope):
    """Enable only a benign reporting tool, without overwriting tracked/user config."""
    checkout = Path(checkout)
    if any(path in ('opencode.json', 'opencode.jsonc', '.opencode') or path.startswith('.opencode/') for path in scope):
        return False
    config = checkout / 'opencode.json'
    if config.is_symlink():
        return False
    if config.exists():
        try:
            return json.loads(config.read_text()).get('experimental', {}).get('todo') is True
        except (ValueError, AttributeError):
            return False
    if (checkout / 'opencode.jsonc').exists() or (checkout / '.opencode').exists():
        return False
    config.write_text(json.dumps({'experimental': {'todo': True}, 'warming': False}) + '\n')
    config.chmod(0o400)
    with (checkout / '.git/info/exclude').open('a') as stream:
        stream.write('\n/opencode.json\n')
    if git_bytes(checkout, 'status', '--porcelain', '--untracked-files=all', '--', 'opencode.json'):
        # Repository ignore negations can override info/exclude. Remove only our new file.
        config.unlink()
        return False
    return True
