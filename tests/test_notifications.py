import json
from pathlib import Path
import sqlite3
import tempfile
import unittest
import uuid

from foreman_supervisor import Store, Notifier


class NotificationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="teem-event-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.store = Store(self.root / "state", [])
        self.addCleanup(self.store.close)
        self.store.bind("ses_foreman", "term_foreman")
        self.event = self.add_event("task-test")
        self.sends = []
        self.reads = []
        self.mode = "accepted"
        case = self

        class Runtime:
            def notify(self, event):
                case.sends.append(dict(event))
                if case.mode == "unknown":
                    raise OSError("Lost native response")

            def event_accepted(self, event):
                case.reads.append(dict(event))
                return case.mode == "accepted"

        self.runtime = Runtime()
        self.notifier = Notifier(self.runtime)

    def add_event(self, task_id, foreman="ses_foreman", state="pending"):
        event_id = str(uuid.uuid4())
        with self.store.db:
            self.store.db.execute("INSERT INTO events (id,task_id,foreman,request_id,state) VALUES (?,?,?,?,?)",
                                  (event_id, task_id, foreman, str(uuid.uuid4()), state))
        return event_id

    def test_native_queue_acceptance_is_not_foreman_consumption(self):
        self.notifier.deliver(self.store)
        self.notifier.deliver(self.store)
        self.assertEqual(len(self.sends), 1)
        event, = self.store.events("ses_foreman")
        self.assertEqual(event["transport"], "native")
        self.assertEqual(event["state"], "queued")
        self.assertFalse(event["acked"])

    def test_unknown_send_is_not_automatically_repeated(self):
        self.mode = "unknown"
        self.notifier.deliver(self.store)
        self.notifier.deliver(self.store)
        self.assertEqual(len(self.sends), 1)
        self.assertEqual(self.store.events("ses_foreman")[0]["state"], "uncertain")

    def test_workflow_notice_unknown_admission_is_read_only_after_restart(self):
        with self.store.db:
            self.store.db.execute('UPDATE events SET workflow_id=? WHERE id=?', ('workflow-test', self.event))
        self.mode = 'unknown'
        self.notifier.deliver(self.store)
        self.mode = 'accepted'
        with self.store.db:
            self.store.db.execute('UPDATE events SET next_receipt=0')
        with Store(self.root / 'state', []) as reopened:
            Notifier(self.runtime).deliver(reopened)
            event, = reopened.events('ses_foreman')
        self.assertEqual(len(self.sends), 1)
        self.assertEqual(len(self.reads), 1)
        self.assertEqual(event['state'], 'queued')
        self.assertEqual(event['workflow_id'], 'workflow-test')

    def test_crash_after_attempt_uses_native_reads_only(self):
        with self.store.db:
            self.store.db.execute("UPDATE events SET state='attempting'")
        self.notifier.deliver(self.store)
        self.assertEqual(self.sends, [])
        self.assertEqual(len(self.reads), 1)
        self.assertEqual(self.store.events("ses_foreman")[0]["state"], "queued")

    def test_receipt_reads_back_off_across_restarts(self):
        self.mode = "unknown"
        with self.store.db:
            self.store.db.execute("UPDATE events SET state='uncertain'")
        self.notifier.deliver(self.store)
        with Store(self.root / "state", []) as reopened:
            Notifier(self.runtime).deliver(reopened)
        self.assertEqual(len(self.reads), 1)
        self.assertEqual(self.sends, [])

    def test_old_uncertain_events_do_not_starve_new_events(self):
        self.mode = "unknown"
        with self.store.db:
            self.store.db.execute("UPDATE events SET state='uncertain'")
        for i in range(32):
            self.add_event(f"old-{i}", state="uncertain")
        new = self.add_event("new-task")
        self.notifier.deliver(self.store)
        self.assertEqual([event["id"] for event in self.sends], [new])

    def test_foreman_without_codr_configuration_receives_events(self):
        self.store.bind("ses_other", "term_other")
        other = self.add_event("other-task", foreman="ses_other")
        self.notifier.deliver(self.store)
        self.assertEqual({event["foreman"] for event in self.sends}, {"ses_foreman", "ses_other"})
        self.assertIn(other, {event["id"] for event in self.sends})

    def test_acknowledged_events_are_not_sent(self):
        self.store.ack("ses_foreman", self.event)
        self.notifier.deliver(self.store)
        self.assertEqual(self.sends, [])

    def test_legacy_events_and_bindings_survive_without_replay(self):
        directory = self.root / "legacy"
        directory.mkdir()
        path = directory / "tasks.sqlite3"
        old_notification = json.dumps({"historical_adapter": "do-not-contact"})
        with sqlite3.connect(path) as old:
            old.executescript("""
                CREATE TABLE foremen (id TEXT PRIMARY KEY, terminal_id TEXT NOT NULL, notification TEXT);
                CREATE TABLE events (id TEXT PRIMARY KEY, task_id TEXT NOT NULL UNIQUE, foreman TEXT NOT NULL,
                    state TEXT NOT NULL DEFAULT 'pending', request_id TEXT NOT NULL UNIQUE,
                    acked INTEGER NOT NULL DEFAULT 0, next_receipt REAL NOT NULL DEFAULT 0);
            """)
            old.execute("INSERT INTO foremen VALUES (?,?,?)", ("ses_foreman", "term_foreman", old_notification))
            for i, state in enumerate(("pending", "attempting", "uncertain", "queued", "rejected")):
                old.execute("INSERT INTO events (id,task_id,foreman,state,request_id) VALUES (?,?,?,?,?)",
                            (str(uuid.uuid4()), f"old-{i}", "ses_foreman", state, str(uuid.uuid4())))
            before = old.execute("SELECT * FROM events ORDER BY rowid").fetchall()
        with Store(directory, []) as migrated:
            self.notifier.deliver(migrated)
            after = migrated.db.execute("SELECT id,task_id,foreman,state,request_id,acked,next_receipt FROM events ORDER BY rowid").fetchall()
            self.assertEqual([tuple(row) for row in after], before)
            self.assertTrue(all(e["transport"] == "legacy-codr" for e in migrated.events("ses_foreman")))
            self.assertEqual(migrated.binding("ses_foreman")["notification"], old_notification)
            migrated.bind("ses_foreman", "term_foreman")
            self.assertEqual(migrated.binding("ses_foreman")["notification"], old_notification)
        self.assertEqual(self.sends, [])
        self.assertEqual(self.reads, [])


if __name__ == "__main__":
    unittest.main()
