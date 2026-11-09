import json
from pathlib import Path
import unittest
import uuid

import test_checkouts
from test_checkouts import git
from foreman_supervisor import Store, Invalid


class WorkflowTests(unittest.TestCase):
    def setUp(self):
        test_checkouts.CheckoutTests.setUp(self)
        self.projects = {'pilot': {'path': str(self.source), 'checks': [
            {'name': 'tests', 'kind': 'test', 'argv': ['python3', '-B', '-m', 'unittest', 'discover', '-s', 'tests', '-v']}],
            'max_revisions': 1}}
        from foreman_workflows import normalise_projects, WorkflowEngine
        self.projects = normalise_projects(self.projects)
        self.store = Store(self.root / 'state', [], projects=self.projects)
        self.addCleanup(self.store.close)
        self.store.bind('ses_foreman', 'term_foreman')
        self.workflow_id = str(uuid.uuid4())
        self.foreman_directory = str(self.source)
        self.check_passed = True
        self.launches = []
        case = self

        class Runtime:
            def foreman_directory(self, foreman):
                case.assertEqual(foreman, 'ses_foreman')
                return case.foreman_directory

        class Checks:
            def start(self, directory, checkout, checks, snapshot):
                handle = {'snapshot': snapshot, 'checkout': str(checkout), 'deadline': 999999999999}
                case.launches.append(handle)
                return handle

            def inspect(self, handle):
                return {'passed': case.check_passed, 'snapshot': handle['snapshot'], 'results': [
                    {'name': 'tests', 'returncode': 0 if case.check_passed else 1}]}

        self.engine = WorkflowEngine(self.store, Runtime(), Checks())

    def submit(self, **changes):
        values = dict(workflow_id=self.workflow_id, project='pilot', scope=['src'], prompt='Improve labels')
        values.update(changes)
        return self.store.submit_workflow('ses_foreman', **values)

    def working(self):
        self.submit()
        self.engine.tick()
        workflow = self.store.workflow(self.workflow_id)
        self.assertEqual(workflow['state'], 'working')
        return self.store.task(workflow['data']['rounds'][-1]['worker_id'])

    def finish_worker(self, task):
        (Path(task['checkout']) / 'src/a.txt').write_text('candidate\n')
        self.store.update(task['id'], 'succeeded', result='Implementation ready; supervisor runs checks')
        self.engine.tick()
        self.assertEqual(self.store.workflow(self.workflow_id)['state'], 'checking')

    def finish_reviewers(self, verdict='pass'):
        workflow = self.store.workflow(self.workflow_id)
        round = workflow['data']['rounds'][-1]
        self.assertEqual(len(round['review_ids']), 2)
        for task_id in round['review_ids']:
            result = {'snapshot': round['snapshot']['sha256'], 'verdict': verdict, 'findings': []}
            if verdict == 'changes_requested':
                result['findings'] = [{'path': 'src/a.txt', 'line': 1, 'message': 'Fix demonstrated issue'}]
            self.store.update(task_id, 'succeeded', result=json.dumps(result))
        self.engine.tick()

    def test_automatic_checkout_and_baseline_without_touching_source(self):
        task = self.working()
        self.assertNotEqual(task['checkout'], str(self.source))
        self.assertEqual(task['request']['revision'], self.base)
        self.assertTrue(Path(task['checkout']).is_relative_to(self.store.directory))
        self.assertEqual(git(self.source, 'status', '--porcelain'), '')
        self.assertIn('supervisor', task['request']['prompt'].lower())

    def team(self):
        self.submit(workers=[{'scope': ['src/a.txt'], 'prompt': 'Implement labels'},
                             {'scope': ['src/b.txt'], 'prompt': 'Add label helpers'}])
        self.engine.tick()
        round = self.store.workflow(self.workflow_id)['data']['rounds'][-1]
        return [self.store.task(i) for i in round['worker_ids']]

    def test_two_workers_combine_before_checks_and_both_reviews(self):
        workers = self.team()
        self.assertEqual(len(workers), 2)
        self.assertNotEqual(workers[0]['checkout'], workers[1]['checkout'])
        for task, path in zip(workers, ('src/a.txt', 'src/b.txt')):
            (Path(task['checkout']) / path).write_text(path + '\n')
            self.store.update(task['id'], 'succeeded', result='Done')
        self.engine.tick()
        round = self.store.workflow(self.workflow_id)['data']['rounds'][-1]
        for path in ('src/a.txt', 'src/b.txt'):
            self.assertEqual((Path(round['check_checkout']) / path).read_text(), path + '\n')
        self.engine.tick()
        self.finish_reviewers()
        self.assertEqual(self.store.workflow(self.workflow_id)['state'], 'ready')
        self.assertEqual(git(self.source, 'status', '--porcelain'), '')

    def test_separate_tasks_each_get_two_worker_slots_and_uncertainty_retains_slot(self):
        first = self.team()
        self.workflow_id = str(uuid.uuid4())
        second = self.team()
        claimed = [self.store.claim() for _ in range(4)]
        self.assertEqual({t['id'] for t in claimed}, {t['id'] for t in first + second})
        self.store.update(first[0]['id'], 'uncertain')
        self.assertIsNone(self.store.claim())

    def test_worker_assignments_require_disjoint_scopes_within_task(self):
        for workers in ([{'scope': ['src'], 'prompt': 'One'}, {'scope': ['src/a.txt'], 'prompt': 'Two'}],
                        [{'scope': ['tests'], 'prompt': 'Outside task'}], [],
                        [{'scope': ['src'], 'prompt': 'One'}] * 3):
            with self.subTest(workers=workers), self.assertRaises(Invalid):
                self.submit(workers=workers)

    def test_team_waits_for_every_worker_and_never_replays_uncertainty(self):
        workers = self.team()
        self.store.update(workers[0]['id'], 'succeeded', result='Done')
        self.engine.tick()
        self.assertEqual(self.launches, [])
        self.store.update(workers[1]['id'], 'uncertain')
        self.engine.tick()
        self.assertEqual(self.store.workflow(self.workflow_id)['state'], 'uncertain')
        self.assertEqual(len(self.store.tasks()), 2)

    def test_managed_agent_completions_do_not_wake_foreman_for_ui_updates(self):
        from foreman_supervisor import Notifier
        task = self.working()
        self.store.update(task['id'], 'succeeded', result='Ready')
        class SilentRuntime:
            def notify(self, event):
                raise AssertionError('Managed progress must not resume Foreman')
        Notifier(SilentRuntime()).deliver(self.store)
        events = self.store.events('ses_foreman')
        self.assertEqual(len(events), 1)
        self.assertEqual(events[0]['state'], 'passive')
        self.assertEqual(events[0]['refresh_todos'], 0)

    def test_worker_cannot_modify_teammate_scope(self):
        workers = self.team()
        (Path(workers[0]['checkout']) / 'src/b.txt').write_text('wrong owner\n')
        for task in workers:
            self.store.update(task['id'], 'succeeded', result='Done')
        self.engine.tick()
        self.assertEqual(self.store.workflow(self.workflow_id)['state'], 'blocked')
        self.assertEqual(self.launches, [])

    def test_single_explicit_worker_cannot_modify_overall_task_scope(self):
        self.submit(workers=[{'scope': ['src/a.txt'], 'prompt': 'Change only a.txt'}])
        self.engine.tick()
        round = self.store.workflow(self.workflow_id)['data']['rounds'][-1]
        task = self.store.task(round['worker_id'])
        (Path(task['checkout']) / 'src/b.txt').write_text('outside assignment\n')
        self.store.update(task['id'], 'succeeded', result='Done')
        self.engine.tick()
        self.assertEqual(self.store.workflow(self.workflow_id)['state'], 'blocked')
        self.assertEqual(self.launches, [])

    def test_current_progress_excludes_completed_tasks_without_deleting_history(self):
        task = self.working()
        self.finish_worker(task)
        self.engine.tick()
        self.finish_reviewers()
        old_id = self.workflow_id
        self.workflow_id = str(uuid.uuid4())
        self.submit(title='Current work')
        progress = self.store.progress('ses_foreman')
        self.assertEqual({r['workflow_id'] for r in progress['items']}, {self.workflow_id})
        self.assertEqual(self.store.workflow(old_id)['state'], 'ready')

    def test_finished_workflow_has_clear_result_outside_active_todos_and_one_notice(self):
        from foreman_supervisor import Notifier
        task = self.working()
        self.finish_worker(task)
        self.engine.tick()
        self.finish_reviewers()
        progress = self.store.progress('ses_foreman')
        self.assertEqual(progress['todos'], [])
        outcome = progress['latest_result']
        self.assertEqual(outcome['workflow_id'], self.workflow_id)
        self.assertEqual(outcome['state'], 'ready')
        self.assertIn('checks and both reviews passed', outcome['content'])
        self.assertIn('not integrated', outcome['content'])
        sends = []
        class Runtime:
            def notify(self, event):
                sends.append(event)
        Notifier(Runtime()).deliver(self.store)
        self.engine.tick()
        Notifier(Runtime()).deliver(self.store)
        self.assertEqual(len(sends), 1)
        self.assertEqual(sends[0]['workflow_id'], self.workflow_id)
        self.assertEqual(sends[0]['refresh_todos'], 0)
        self.assertFalse(sends[0]['acked'])

    def test_terminal_failures_show_attention_not_success_and_queue_one_notice(self):
        task = self.working()
        self.store.update(task['id'], 'failed', detail='Native worker failed')
        self.engine.tick()
        outcome = self.store.progress('ses_foreman')['latest_result']
        self.assertEqual(outcome['state'], 'blocked')
        self.assertIn('Blocked', outcome['content'])
        workflow = self.store.workflow(self.workflow_id)
        timestamp = workflow['data']['finished_at']
        self.engine.save(workflow, 'blocked', workflow['detail'])
        self.assertEqual(self.store.workflow(self.workflow_id)['data']['finished_at'], timestamp)
        notices = [e for e in self.store.events('ses_foreman') if e.get('workflow_id')]
        self.assertEqual(len(notices), 1)

    def test_legacy_terminal_workflow_is_displayed_without_backfilled_notification(self):
        self.submit(title='Old completed work')
        with self.store.db:
            self.store.db.execute("UPDATE workflows SET state='ready'")
        self.engine.tick()
        self.assertEqual(self.store.events('ses_foreman'), [])
        self.assertEqual(self.store.progress('ses_foreman')['latest_result']['state'], 'ready')

    def test_latest_result_is_by_finish_time_not_submission_and_stays_separate_from_new_work(self):
        self.submit(title='Submitted first, finished last')
        first = self.store.workflow(self.workflow_id)
        self.workflow_id = str(uuid.uuid4())
        self.submit(title='Submitted second, finished first')
        second = self.store.workflow(self.workflow_id)
        self.engine.save(second, 'blocked', 'Stopped')
        self.engine.save(first, 'blocked', 'Stopped later')
        self.workflow_id = str(uuid.uuid4())
        self.submit(title='Still working')
        progress = self.store.progress('ses_foreman')
        self.assertEqual(progress['latest_result']['workflow_id'], first['id'])
        self.assertEqual({i['workflow_id'] for i in progress['items']}, {self.workflow_id})

    def test_two_worker_revision_starts_from_exact_combined_candidate(self):
        workers = self.team()
        for task, path in zip(workers, ('src/a.txt', 'src/b.txt')):
            (Path(task['checkout']) / path).write_text('combined content\n')
            self.store.update(task['id'], 'succeeded', result='Done')
        self.engine.tick()
        self.engine.tick()
        self.finish_reviewers('changes_requested')
        workflow = self.store.workflow(self.workflow_id)
        self.assertEqual(workflow['round'], 1)
        round = workflow['data']['rounds'][-1]
        self.assertEqual(len(round['worker_ids']), 1)
        revision = self.store.task(round['worker_id'])
        for path in ('src/a.txt', 'src/b.txt'):
            self.assertEqual((Path(revision['checkout']) / path).read_text(), 'combined content\n')
        self.assertEqual(revision['request']['scope'], ['src'])
        self.assertIn('Fix demonstrated issue', revision['request']['prompt'])

    def test_two_tasks_can_run_both_review_teams_independently(self):
        reviews = []
        for _ in range(2):
            self.workflow_id = str(uuid.uuid4())
            task = self.working()
            self.finish_worker(task)
            self.engine.tick()
            round = self.store.workflow(self.workflow_id)['data']['rounds'][-1]
            reviews.extend(round['review_ids'])
        self.assertEqual({self.store.claim()['id'] for _ in range(4)}, set(reviews))

    def test_combining_ignore_rule_cannot_silently_drop_teammate_file(self):
        self.submit(scope=['.gitignore', 'src'], workers=[
            {'scope': ['.gitignore'], 'prompt': 'Ignore generated files'},
            {'scope': ['src/b.txt'], 'prompt': 'Add a helper'}])
        self.engine.tick()
        round = self.store.workflow(self.workflow_id)['data']['rounds'][-1]
        tasks = [self.store.task(i) for i in round['worker_ids']]
        (Path(tasks[0]['checkout']) / '.gitignore').write_text('src/b.txt\n')
        (Path(tasks[1]['checkout']) / 'src/b.txt').write_text('must not disappear\n')
        for task in tasks:
            self.store.update(task['id'], 'succeeded', result='Done')
        self.engine.tick()
        self.assertEqual(self.store.workflow(self.workflow_id)['state'], 'blocked')
        self.assertEqual(self.launches, [])

    def test_exact_snapshot_checks_and_two_reviews_are_required_for_ready(self):
        task = self.working()
        self.finish_worker(task)
        self.engine.tick()
        workflow = self.store.workflow(self.workflow_id)
        self.assertEqual(workflow['state'], 'reviewing')
        round = workflow['data']['rounds'][-1]
        paths = [self.store.task(i)['checkout'] for i in round['review_ids']]
        self.assertEqual(len(set(paths + [task['checkout'], round['check_checkout']])), 4)
        for task_id in round['review_ids']:
            review = self.store.task(task_id)
            self.assertEqual(review['request']['input_tree'], round['snapshot']['tree'])
            self.assertEqual((Path(review['checkout']) / 'src/a.txt').read_text(), 'candidate\n')
        self.finish_reviewers()
        self.assertEqual(self.store.workflow(self.workflow_id)['state'], 'ready')
        self.assertEqual(git(self.source, 'rev-parse', 'HEAD'), self.base)
        self.assertEqual(git(self.source, 'status', '--porcelain'), '')

    def test_findings_create_new_bounded_revision_not_a_replay(self):
        task = self.working()
        self.finish_worker(task)
        self.engine.tick()
        self.finish_reviewers('changes_requested')
        workflow = self.store.workflow(self.workflow_id)
        self.assertEqual(workflow['state'], 'working')
        self.assertEqual(workflow['round'], 1)
        revised = self.store.task(workflow['data']['rounds'][-1]['worker_id'])
        self.assertNotEqual(revised['session_id'], task['session_id'])
        self.assertNotEqual(revised['checkout'], task['checkout'])
        self.assertIn('Fix demonstrated issue', revised['request']['prompt'])
        self.finish_worker(revised)
        self.engine.tick()
        self.finish_reviewers('changes_requested')
        self.assertEqual(self.store.workflow(self.workflow_id)['state'], 'needs_attention')

    def test_check_failure_uses_revision_budget_without_running_reviewers(self):
        task = self.working()
        self.finish_worker(task)
        self.check_passed = False
        self.engine.tick()
        workflow = self.store.workflow(self.workflow_id)
        self.assertEqual(workflow['state'], 'working')
        self.assertEqual(workflow['round'], 1)
        self.assertFalse(any(t['role'] == 'reviewer' for t in self.store.tasks()))

    def test_superseded_candidate_review_stages_do_not_stay_pending(self):
        task = self.working()
        self.finish_worker(task)
        self.check_passed = False
        self.engine.tick()
        self.check_passed = True
        workflow = self.store.workflow(self.workflow_id)
        revised = self.store.task(workflow['data']['rounds'][-1]['worker_id'])
        self.finish_worker(revised)
        self.engine.tick()
        self.finish_reviewers()
        snapshot = self.store.progress('ses_foreman')
        self.assertFalse(any(t['status'] in ('pending', 'in_progress') for t in snapshot['todos']))

    def test_failed_check_revision_receives_verified_diagnostic_text(self):
        task = self.working()
        self.finish_worker(task)
        log = self.root / 'failure.log'
        log.write_text('AssertionError: expected normalized label, got repeated spaces\n')
        import hashlib
        self.engine.checks.inspect = lambda handle: {'passed': False, 'snapshot': handle['snapshot'], 'results': [
            {'name': 'tests', 'returncode': 1, 'log': str(log),
             'output_sha256': hashlib.sha256(log.read_bytes()).hexdigest()}]}
        self.engine.tick()
        workflow = self.store.workflow(self.workflow_id)
        revision = self.store.task(workflow['data']['rounds'][-1]['worker_id'])
        self.assertIn('AssertionError: expected normalized label', revision['request']['prompt'])

    def test_read_only_reviewers_can_read_exact_patch_without_shell(self):
        task = self.working()
        self.finish_worker(task)
        self.engine.tick()
        round = self.store.workflow(self.workflow_id)['data']['rounds'][-1]
        for task_id in round['review_ids']:
            review = self.store.task(task_id)
            context = Path(review['checkout']) / '.git/teem-review.patch'
            self.assertEqual(context.read_bytes(), Path(round['snapshot']['patch']).read_bytes())
            self.assertIn('.git/teem-review.patch', review['request']['prompt'])

    def test_uncertain_worker_and_invalid_review_never_pass_or_repeat_work(self):
        task = self.working()
        self.store.update(task['id'], 'uncertain', detail='Runtime unknown')
        self.engine.tick()
        self.assertEqual(len(self.store.tasks()), 1)
        self.assertEqual(self.store.workflow(self.workflow_id)['state'], 'uncertain')

    def test_wrong_snapshot_review_is_not_accepted(self):
        task = self.working()
        self.finish_worker(task)
        self.engine.tick()
        workflow = self.store.workflow(self.workflow_id)
        for task_id in workflow['data']['rounds'][-1]['review_ids']:
            self.store.update(task_id, 'succeeded', result=json.dumps({'snapshot': 'wrong', 'verdict': 'pass', 'findings': []}))
        self.engine.tick()
        self.assertEqual(self.store.workflow(self.workflow_id)['state'], 'blocked')

    def test_source_or_check_candidate_drift_cannot_qualify_ready(self):
        task = self.working()
        self.finish_worker(task)
        self.engine.tick()
        workflow = self.store.workflow(self.workflow_id)
        round = workflow['data']['rounds'][-1]
        (Path(round['check_checkout']) / 'src/a.txt').write_text('changed after checks\n')
        self.finish_reviewers()
        self.assertEqual(self.store.workflow(self.workflow_id)['state'], 'blocked')

    def test_workflow_progress_is_automatic_and_does_not_claim_ready_while_working(self):
        self.submit()
        snapshot = self.store.progress('ses_foreman')
        self.assertEqual(snapshot['items'][0]['workflow_id'], self.workflow_id)
        self.assertEqual(snapshot['todos'][0]['status'], 'pending')
        self.engine.tick()
        self.assertEqual(self.store.progress('ses_foreman')['todos'][0]['status'], 'in_progress')

    def test_readable_title_and_worker_check_reviewer_rows_hide_machine_ids(self):
        self.submit(title='Add unique labels')
        self.engine.tick()
        snapshot = self.store.progress('ses_foreman')
        content = '\n'.join(t['content'] for t in snapshot['todos'])
        self.assertIn('Add unique labels', content)
        self.assertNotIn(self.workflow_id[:8], content)
        self.assertIn('Worker', content)
        self.assertIn('Check: tests', content)
        self.assertIn('Correctness review', content)
        self.assertIn('Tests and scope review', content)
        self.assertGreaterEqual(sum(t['status'] == 'pending' for t in snapshot['todos']), 3)

    def test_old_workflow_gets_readable_fallback_without_changing_immutable_request(self):
        self.submit(prompt='Add unique_labels(labels) to src/labels.py. Add tests.')
        original = self.store.workflow(self.workflow_id)['request']
        snapshot = self.store.progress('ses_foreman')
        self.assertIn('Add unique labels', snapshot['todos'][0]['content'])
        self.assertNotIn(self.workflow_id[:8], snapshot['todos'][0]['content'])
        self.assertEqual(self.store.workflow(self.workflow_id)['request'], original)

    def test_finished_review_execution_does_not_claim_passing_verdict(self):
        task = self.working()
        self.finish_worker(task)
        self.engine.tick()
        round = self.store.workflow(self.workflow_id)['data']['rounds'][-1]
        self.store.update(round['review_ids'][0], 'succeeded', result='not parsed yet')
        snapshot = self.store.progress('ses_foreman')
        review = next(i for i in snapshot['items'] if i.get('task_id') == round['review_ids'][0])
        self.assertEqual(review['status'], 'in_progress')
        self.assertIn('verdict pending', review['content'])

    def test_agent_todos_are_detail_not_check_or_review_evidence(self):
        task = self.working()
        with self.store.db:
            self.store.db.execute('INSERT INTO agent_todos(task_id,todos,message_id) VALUES (?,?,?)',
                (task['id'], json.dumps([{'content': 'Write tests', 'status': 'completed'}]), 'msg_report'))
        snapshot = self.store.progress('ses_foreman')
        detail = next(i for i in snapshot['items'] if i['source'] == 'agent-reported')
        self.assertIn('reported', detail['content'])
        self.assertEqual(snapshot['todos'][0]['status'], 'in_progress')
        check = next(i for i in snapshot['items'] if 'Check: tests' in i['content'])
        self.assertEqual(check['status'], 'pending')

    def test_real_check_helper_returns_without_blocking_the_controller(self):
        import time
        from foreman_checks import CheckRunner
        runner = CheckRunner()
        self.engine.checks = runner
        (self.source / 'src/check.py').write_text('import unittest\nclass T(unittest.TestCase):\n def test_ok(self): self.assertTrue(True)\nunittest.main()\n')
        git(self.source, 'add', 'src/check.py')
        git(self.source, 'commit', '-qm', 'local check fixture')
        self.base = git(self.source, 'rev-parse', 'HEAD')
        self.store.projects['pilot']['checks'][0]['argv'] = ['python3', '-B', 'src/check.py']
        task = self.working()
        # This local fixture checks the process transport, not a live model.
        self.finish_worker(task)
        deadline = time.monotonic() + 10
        handle = self.store.workflow(self.workflow_id)['data']['rounds'][-1]['check_handle']
        while runner.inspect(handle) is None and time.monotonic() < deadline:
            time.sleep(0.01)
        self.engine.tick()
        runner.reap()
        self.assertEqual(self.store.workflow(self.workflow_id)['state'], 'reviewing')
        for child in runner.children.values():
            child.wait(timeout=5)

    def test_policy_revocation_stops_progress_and_queued_managed_launches(self):
        task = self.working()
        self.store.projects.clear()
        self.assertFalse(self.store.is_allowed(task['checkout'], 'ses_foreman'))
        self.engine.tick()
        self.assertEqual(self.store.workflow(self.workflow_id)['state'], 'blocked')

    def test_duplicate_request_is_idempotent_but_changes_and_foreign_locations_fail(self):
        self.submit()
        self.submit()
        self.assertEqual(len(self.store.workflows('ses_foreman')), 1)
        with self.assertRaises(Invalid):
            self.submit(prompt='Different assignment')
        self.foreman_directory = str(self.root / 'other')
        self.engine.tick()
        self.assertEqual(self.store.workflow(self.workflow_id)['state'], 'blocked')
        self.assertEqual(self.store.tasks(), [])

    def test_crash_marker_is_not_reexecuted(self):
        self.submit()
        with self.store.db:
            self.store.db.execute("UPDATE workflows SET state='preparing'")
        self.engine.tick()
        self.engine.tick()
        self.assertEqual(self.store.workflow(self.workflow_id)['state'], 'uncertain')
        self.assertEqual(self.store.tasks(), [])


if __name__ == '__main__':
    unittest.main()
