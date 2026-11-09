"""Frozen check-runner reference for Rust regression qualification."""

import hashlib
import json
import os
from pathlib import Path
import selectors
import signal
import subprocess
import sys
import time
import uuid

from foreman_checkouts import relative_path
from foreman_supervisor import Invalid


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


def normalise_checks(checks):
    if not isinstance(checks, list) or not 1 <= len(checks) <= 8:
        raise Invalid('Approve one to eight named checks in machine-local project policy')
    result, names = [], set()
    for value in checks:
        if not isinstance(value, dict) or set(value) - {'name', 'kind', 'argv', 'cwd', 'timeout', 'output_limit'}:
            raise Invalid('Unsupported check policy fields')
        check = {'cwd': '.', 'timeout': 300, 'output_limit': 1048576, **value}
        argv = check.get('argv')
        if (not isinstance(check.get('name'), str) or len(check['name']) > 64 or not check['name'].replace('-', '').replace('_', '').isalnum()
                or check['name'] in names or check.get('kind') not in ('test', 'build', 'inspect')
                or not isinstance(argv, list) or not 1 <= len(argv) <= 64
                or any(not isinstance(a, str) or not a or len(a) > 4096 or '\0' in a for a in argv)
                or '/' in argv[0] or argv[0] in ('sh', 'bash', 'zsh', 'fish', 'env', 'sudo', 'rm', 'ssh', 'scp', 'curl', 'wget')
                or (argv[0] == 'git' and (len(argv) < 2 or argv[1] not in ('status', 'diff', 'show', 'log', 'rev-parse', 'ls-files')))
                or type(check['timeout']) not in (int, float) or not 0 < check['timeout'] <= 1800
                or type(check['output_limit']) is not int or not 1024 <= check['output_limit'] <= 1048576):
            raise Invalid('Checks need bounded exact argv, a test/build/inspect kind, and no shell launcher')
        if check['cwd'] != '.':
            relative_path(check['cwd'])
        names.add(check['name'])
        result.append(check)
    return result


def check_cwd(checkout, relative):
    checkout = Path(checkout).resolve()
    cwd = (checkout / relative).resolve()
    if not cwd.is_dir() or not cwd.is_relative_to(checkout):
        raise Invalid('Approved check cwd must stay within its complete candidate checkout')
    return cwd


def environment():
    keys = {'HOME', 'PATH', 'LANG', 'LC_ALL', 'TMPDIR', 'XDG_CACHE_HOME', 'CARGO_TARGET_DIR',
            'GRADLE_USER_HOME', 'ZIG_GLOBAL_CACHE_DIR', 'ZIG_LOCAL_CACHE_DIR', 'PYTHONDONTWRITEBYTECODE'}
    env = {k: v for k, v in os.environ.items() if k in keys}
    env['PATH'] = os.pathsep.join(p for p in os.get_exec_path() if Path(p).is_absolute())
    env['GIT_CONFIG_NOSYSTEM'] = '1'
    env['GIT_CONFIG_GLOBAL'] = '/dev/null'
    env['GIT_TERMINAL_PROMPT'] = '0'
    return env


def execute(check, checkout, log):
    start = time.monotonic()
    timed_out = limited = finished_read = False
    retained = bytearray()
    process = subprocess.Popen(check['argv'], cwd=check_cwd(checkout, check['cwd']), env=environment(),
                               stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                               shell=False, start_new_session=True)
    try:
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ)
            while selector.get_map():
                if time.monotonic() - start > check['timeout']:
                    timed_out = True
                    break
                for key, _ in selector.select(0.05):
                    chunk = os.read(key.fileobj.fileno(), 65536)
                    if not chunk:
                        selector.unregister(key.fileobj)
                    else:
                        available = check['output_limit'] - len(retained)
                        retained.extend(chunk[:available])
                        if len(chunk) > available:
                            limited = True
                            break
                if limited:
                    break
        finished_read = True
    finally:
        if not (timed_out or limited) and finished_read:
            try:
                process.wait(timeout=max(0.001, check['timeout'] - (time.monotonic() - start)))
            except subprocess.TimeoutExpired:
                timed_out = True
        # Even a successful parent may leave background children with redirected pipes.
        # Clean up this check's owned process group, never unrelated agent processes.
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait()
        process.stdout.close()
    Path(log).write_bytes(retained)
    Path(log).chmod(0o600)
    return {'name': check['name'], 'argv': check['argv'], 'returncode': process.returncode,
            'timed_out': timed_out, 'output_limited': limited, 'log': str(log),
            'output_sha256': hashlib.sha256(retained).hexdigest()}


def run(spec):
    os.umask(0o077)
    receipt = {'token': spec['token'], 'snapshot': spec['snapshot'], 'spec_sha256': digest(spec), 'results': [], 'passed': False}
    try:
        policy = Path('/mnt/fastbuild/INFO.md')
        if policy.parent.exists():
            policy.read_text()
            if not os.path.ismount('/mnt/fastbuild'):
                raise Invalid('Required /mnt/fastbuild mount is unavailable')
        checks = normalise_checks(spec['checks'])
        for check in checks:
            result = execute(check, spec['checkout'], Path(spec['directory']) / (check['name'] + '.log'))
            receipt['results'].append(result)
            if result['returncode'] or result['timed_out'] or result['output_limited']:
                break
        receipt['passed'] = len(receipt['results']) == len(checks) and all(r['returncode'] == 0 for r in receipt['results'])
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt['error'] = str(error)[:2000]
    path = Path(spec['directory']) / 'receipt.json'
    temporary = path.with_suffix('.tmp')
    temporary.write_text(json.dumps(receipt) + '\n')
    temporary.chmod(0o600)
    temporary.replace(path)
    return receipt


class CheckRunner:
    def __init__(self):
        self.children = {}

    def reap(self):
        for pid, child in list(self.children.items()):
            if child.poll() is not None:
                self.children.pop(pid)

    def start(self, directory, checkout, checks, snapshot):
        self.reap()
        checks = normalise_checks(checks)
        for check in checks:
            check_cwd(checkout, check['cwd'])
        directory = Path(directory).absolute()
        if directory.exists() or directory.is_symlink() or any(p.is_symlink() for p in directory.parents):
            raise Invalid('Check launch directory already exists or follows a symlink; do not repeat launch')
        directory.mkdir(mode=0o700, parents=True, exist_ok=False)
        spec = {'token': str(uuid.uuid4()), 'checkout': str(Path(checkout).resolve()), 'checks': checks,
                'snapshot': snapshot, 'directory': str(directory)}
        path = directory / 'spec.json'
        path.write_text(json.dumps(spec, sort_keys=True))
        path.chmod(0o600)
        with (directory / 'helper.log').open('wb') as log:
            process = subprocess.Popen([sys.executable, '-B', str(Path(__file__).resolve()), str(path)],
                                       stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                                       cwd=directory, env=environment(), start_new_session=True)
        self.children[process.pid] = process
        return {'spec': str(path), 'receipt': str(directory / 'receipt.json'), 'token': spec['token'],
                'spec_sha256': digest(spec), 'snapshot': snapshot, 'pid': process.pid,
                'deadline': time.time() + sum(c['timeout'] for c in checks) + 30}

    def inspect(self, handle):
        self.reap()
        path = Path(handle['receipt'])
        if not path.exists():
            return None
        if path.is_symlink() or path.stat().st_size > 1048576:
            raise Invalid('Invalid check receipt file')
        spec = json.loads(Path(handle['spec']).read_text())
        receipt = json.loads(path.read_text())
        if (digest(spec) != handle['spec_sha256'] or receipt.get('token') != handle['token']
                or receipt.get('spec_sha256') != handle['spec_sha256'] or receipt.get('snapshot') != handle['snapshot']
                or not isinstance(receipt.get('results'), list) or len(receipt['results']) > len(spec['checks'])):
            raise Invalid('Check receipt does not match the approved launch and snapshot')
        for expected, result in zip(spec['checks'], receipt['results']):
            log = Path(spec['directory']) / (expected['name'] + '.log')
            if (result.get('name') != expected['name'] or result.get('argv') != expected['argv']
                    or result.get('log') != str(log) or log.is_symlink()
                    or hashlib.sha256(log.read_bytes()).hexdigest() != result.get('output_sha256')
                    or type(result.get('returncode')) is not int):
                raise Invalid('Check output or command evidence changed')
        receipt['passed'] = (not receipt.get('error') and len(receipt['results']) == len(spec['checks'])
                             and all(r['returncode'] == 0 and not r.get('timed_out') and not r.get('output_limited')
                                     for r in receipt['results']))
        return receipt


if __name__ == '__main__':
    run(json.loads(Path(sys.argv[1]).read_text()))
