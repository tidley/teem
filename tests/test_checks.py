import json
import os
from pathlib import Path
import signal
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

from foreman_supervisor import Invalid


class CheckTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='teem-checks-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.checkout = self.root / 'checkout'
        self.checkout.mkdir()

    def check(self, code, **changes):
        script = self.checkout / 'check.py'
        script.write_text(code)
        return dict(name='tests', kind='test', argv=['python3', '-B', 'check.py'], cwd='.', timeout=2, output_limit=4096, **changes)

    def run_checks(self, checks):
        from foreman_checks import run, CheckRunner
        with patch('foreman_checks.subprocess.Popen') as launch:
            launch.return_value.pid = 123
            handle = CheckRunner().start(self.root / 'run', self.checkout, checks, 'snapshot-digest')
        spec = json.loads(Path(handle['spec']).read_text())
        run(spec)
        return CheckRunner().inspect(handle)

    def test_success_receipt_binds_command_snapshot_and_output(self):
        result = self.run_checks([self.check('print("six tests passed")\n')])
        self.assertTrue(result['passed'])
        self.assertEqual(result['snapshot'], 'snapshot-digest')
        self.assertEqual(result['results'][0]['returncode'], 0)
        self.assertIn('six tests passed', Path(result['results'][0]['log']).read_text())

    def test_early_closed_output_is_not_killed_before_success(self):
        result = self.run_checks([self.check('import os, time; os.close(1); os.close(2); time.sleep(0.1)\n')])
        self.assertTrue(result['passed'])

    def test_successful_check_does_not_leave_background_group_members_running(self):
        pid_path = self.checkout / 'child.pid'
        code = ('import subprocess,sys\nfrom pathlib import Path\n'
                'p=subprocess.Popen([sys.executable,"-c","import time; time.sleep(30)"], '
                'stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)\n'
                'Path("child.pid").write_text(str(p.pid))\n')
        try:
            result = self.run_checks([self.check(code)])
            self.assertTrue(result['passed'])
            pid = int(pid_path.read_text())
            deadline = time.monotonic() + 0.5
            while time.monotonic() < deadline:
                stat = Path(f'/proc/{pid}/stat')
                try:
                    stopped = stat.read_text().split()[2] == 'Z'
                except (FileNotFoundError, ProcessLookupError):
                    stopped = True
                if stopped:
                    break
                time.sleep(0.01)
            else:
                self.fail('Successful check left a background process running')
        finally:
            if pid_path.exists():
                try:
                    os.kill(int(pid_path.read_text()), signal.SIGKILL)
                except ProcessLookupError:
                    pass

    def test_shell_metacharacters_are_literal_arguments_not_shell_programs(self):
        check = self.check('import sys; print(sys.argv[1])\n')
        check['argv'].append('; touch should-not-exist')
        result = self.run_checks([check])
        self.assertTrue(result['passed'])
        self.assertFalse((self.checkout / 'should-not-exist').exists())

    def test_failure_timeout_and_output_limit_do_not_pass(self):
        for name, code in [('failure', 'raise SystemExit(3)'), ('timeout', 'import time; time.sleep(10)'),
                           ('output', 'print("x" * 100000)')]:
            with self.subTest(name=name), tempfile.TemporaryDirectory(dir=self.root) as run_dir:
                from foreman_checks import run, CheckRunner
                check = self.check(code)
                check['timeout'] = 0.1
                with patch('foreman_checks.subprocess.Popen') as launch:
                    launch.return_value.pid = 123
                    handle = CheckRunner().start(Path(run_dir) / 'run', self.checkout, [check], name)
                run(json.loads(Path(handle['spec']).read_text()))
                self.assertFalse(CheckRunner().inspect(handle)['passed'])

    def test_launch_is_asynchronous_and_an_existing_spec_is_never_relaunched(self):
        from foreman_checks import CheckRunner
        runner = CheckRunner()
        with patch('foreman_checks.subprocess.Popen') as launch:
            launch.return_value.pid = 123
            handle = runner.start(self.root / 'run', self.checkout, [self.check('print(1)')], 'digest')
            self.assertIsNone(runner.inspect(handle))
            with self.assertRaises(Invalid):
                runner.start(self.root / 'run', self.checkout, [self.check('print(1)')], 'digest')
            self.assertEqual(launch.call_count, 1)

    def test_policy_rejects_shell_launchers_traversal_and_unbounded_commands(self):
        from foreman_checks import normalise_checks
        for changes in ({'argv': 'python3 tests.py'}, {'argv': ['sh', '-c', 'anything']},
                        {'cwd': '../outside'}, {'argv': ['/usr/bin/python3', 'tests.py']},
                        {'timeout': 0}, {'output_limit': 999999999}, {'argv': ['git', 'push']}):
            check = self.check('print(1)')
            check.update(changes)
            with self.subTest(changes=changes), self.assertRaises(Invalid):
                normalise_checks([check])

    def test_cwd_symlink_and_tampered_receipt_are_rejected(self):
        from foreman_checks import CheckRunner
        outside = self.root / 'outside'
        outside.mkdir()
        (self.checkout / 'elsewhere').symlink_to(outside, target_is_directory=True)
        check = self.check('print(1)')
        check['cwd'] = 'elsewhere'
        with self.assertRaises(Invalid):
            CheckRunner().start(self.root / 'run', self.checkout, [check], 'digest')
        with patch('foreman_checks.subprocess.Popen') as launch:
            launch.return_value.pid = 123
            handle = CheckRunner().start(self.root / 'good', self.checkout, [self.check('print(1)')], 'digest')
        Path(handle['receipt']).write_text('{"token":"wrong","passed":true}')
        with self.assertRaises(Invalid):
            CheckRunner().inspect(handle)


if __name__ == '__main__':
    unittest.main()
