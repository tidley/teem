import copy
import json
from pathlib import Path
import tempfile
import unittest
import uuid

from foreman_supervisor import Store, Invalid


class ProgressTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="teem-progress-")
        self.addCleanup(self.temp.cleanup)
        self.store = Store(Path(self.temp.name) / "state", [])
        self.addCleanup(self.store.close)
        self.store.bind("ses_foreman", "term_foreman")
        self.store.bind("ses_other", "term_other")
        self.task_id = str(uuid.uuid4())
        self.items = [{"id": "worker", "content": "Implement labels", "task_id": self.task_id},
                      {"id": "verify", "content": "Verify tests and review evidence", "status": "pending"}]
        self.sends = []
        self.receipts = []
        self.fail_send = False
        self.accepted = False
        case = self

        class Runtime:
            def notify(self, event):
                case.sends.append(copy.deepcopy(event))
                if case.fail_send:
                    raise OSError("lost response")

            def event_accepted(self, event):
                case.receipts.append(copy.deepcopy(event))
                return case.accepted

        self.runtime = Runtime()

    def add_task(self, state="queued", foreman="ses_foreman", detail=""):
        with self.store.db:
            self.store.db.execute("""INSERT INTO tasks
                (id,foreman,role,checkout,state,request,session_id,prompt_id,sparse,detail,created)
                VALUES (?,?, 'worker','/example/worker',?,'{}',?,?,0,?,0)""",
                (self.task_id, foreman, state, "ses_worker_" + self.task_id, "msg_worker_" + self.task_id, detail))

    def publish(self, now=100):
        from foreman_progress import ProgressPublisher
        ProgressPublisher(self.runtime).deliver(self.store, now=now)

    def test_plan_is_durable_and_unsubmitted_task_is_pending(self):
        self.store.set_plan("ses_foreman", self.items)
        with Store(self.store.directory, []) as reopened:
            snapshot = reopened.progress("ses_foreman")
        self.assertEqual(snapshot["todos"][0]["status"], "pending")
        self.assertIn("not submitted", snapshot["todos"][0]["content"])
        self.assertEqual(snapshot["items"][0]["task_id"], self.task_id)

    def test_execution_updates_do_not_complete_verification(self):
        self.store.set_plan("ses_foreman", self.items)
        self.add_task("running", detail="Requires human permission or answer")
        snapshot = self.store.progress("ses_foreman")
        self.assertEqual(snapshot["todos"][0]["status"], "in_progress")
        self.assertIn("human", snapshot["todos"][0]["content"])
        self.store.update(self.task_id, "succeeded", result="Tests pass says worker")
        snapshot = self.store.progress("ses_foreman")
        self.assertEqual(snapshot["todos"][0]["status"], "completed")
        self.assertIn("execution ended", snapshot["todos"][0]["content"])
        self.assertEqual(snapshot["todos"][1]["status"], "pending")

    def test_failed_uncertain_and_interrupted_are_not_success(self):
        self.store.set_plan("ses_foreman", self.items)
        self.add_task("uncertain")
        snapshot = self.store.progress("ses_foreman")
        self.assertEqual(snapshot["todos"][0]["status"], "in_progress")
        self.assertIn("uncertain", snapshot["todos"][0]["content"])
        for state in ("failed", "interrupted"):
            with self.store.db:
                self.store.db.execute("UPDATE tasks SET state=?", (state,))
            self.assertEqual(self.store.progress("ses_foreman")["todos"][0]["status"], "cancelled")

    def test_plan_rejects_overrides_duplicates_and_foreign_task_links(self):
        self.add_task(foreman="ses_other")
        for items in ([dict(self.items[0], status="completed")], self.items + [self.items[0]], self.items):
            with self.subTest(items=items), self.assertRaises(Invalid):
                self.store.set_plan("ses_foreman", items)

    def test_future_link_does_not_expose_another_foreman_task(self):
        self.store.set_plan("ses_foreman", self.items)
        self.add_task("succeeded", foreman="ses_other", detail="private other status")
        snapshot = self.store.progress("ses_foreman")
        self.assertNotIn("private", json.dumps(snapshot))
        self.assertIn("unavailable", snapshot["todos"][0]["content"])

    def test_no_plan_means_no_notifications_or_overwrite_of_existing_todos(self):
        self.add_task("succeeded")
        self.publish()
        self.assertEqual(self.sends, [])
        self.assertIsNone(self.store.progress("ses_foreman"))

    def test_same_snapshot_is_silent_and_updates_are_coalesced(self):
        self.store.set_plan("ses_foreman", self.items)
        self.publish()
        self.publish(now=101)
        self.add_task("running")
        self.publish(now=102)
        self.store.set_plan("ses_foreman", [self.items[0], dict(self.items[1], status="in_progress")])
        self.publish(now=103)
        self.assertEqual(self.sends, [])
        self.publish(now=111)
        self.assertEqual(self.sends, [])
        projection = json.loads((self.store.directory / 'progress.json').read_text())
        self.assertEqual(projection['sessions']['ses_foreman'], self.store.progress('ses_foreman'))
        self.assertEqual(projection['updated_at'], 111)
        self.assertEqual((self.store.directory / 'progress.json').stat().st_mode & 0o777, 0o600)

    def test_lost_progress_send_is_reconciled_without_replay_across_restart(self):
        self.store.set_plan("ses_foreman", self.items)
        self.fail_send = True
        self.publish()
        self.store.close()
        self.store = Store(Path(self.temp.name) / "state", [])
        self.addCleanup(self.store.close)
        self.accepted = True
        self.publish(now=131)
        self.assertEqual(self.sends, [])
        self.assertEqual(self.receipts, [])
        self.assertEqual(json.loads((self.store.directory / 'progress.json').read_text())['sessions']['ses_foreman'],
                         self.store.progress('ses_foreman'))

    def test_completion_event_covers_terminal_projection_without_second_notification(self):
        self.store.set_plan("ses_foreman", self.items)
        self.add_task("running")
        self.publish()
        self.store.update(self.task_id, "succeeded", result="done")
        self.publish(now=111)
        self.assertEqual(self.sends, [])

    def test_each_foreman_has_independent_progress_and_clear_is_explicit(self):
        self.store.set_plan("ses_foreman", self.items)
        self.store.set_plan("ses_other", [{"id": "other", "content": "Other work", "status": "pending"}])
        self.publish()
        projection = json.loads((self.store.directory / 'progress.json').read_text())
        self.assertEqual(set(projection['sessions']), {'ses_foreman', 'ses_other'})
        self.store.set_plan("ses_foreman", [])
        self.publish(now=111)
        self.assertEqual(self.store.progress("ses_foreman")["todos"], [])
        self.assertEqual(self.sends, [])

    def test_old_queued_progress_history_is_preserved_without_new_foreman_turns(self):
        self.store.set_plan('ses_foreman', self.items)
        with self.store.db:
            for state in ('pending', 'queued', 'uncertain'):
                self.store.db.execute('INSERT INTO progress_events(foreman,fingerprint,snapshot,request_id,state) VALUES (?,?,?,?,?)',
                    ('ses_foreman', state, '{}', str(uuid.uuid4()), state))
        self.publish()
        self.assertEqual(self.sends, [])
        self.assertEqual(self.receipts, [])
        states = [r['state'] for r in self.store.db.execute('SELECT state FROM progress_events ORDER BY seq')]
        self.assertEqual(states, ['superseded', 'queued', 'uncertain'])


if __name__ == "__main__":
    unittest.main()
