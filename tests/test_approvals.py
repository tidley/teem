import copy
import json
import sqlite3
from pathlib import Path
import tempfile
import unittest

from foreman_runtime import Missing
from foreman_supervisor import Store


class ApprovalTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="teem-approval-")
        self.addCleanup(self.temp.cleanup)
        self.store = Store(Path(self.temp.name) / "state", ["/example/worker"])
        self.addCleanup(self.store.close)
        self.store.bind("ses_foreman", "term_foreman")
        with self.store.db:
            self.store.db.execute("""INSERT INTO tasks
                (id,foreman,role,checkout,state,request,session_id,prompt_id,sparse,created)
                VALUES ('task','ses_foreman','worker','/example/worker','running','{}',
                        'ses_worker','msg_worker',0,0)""")
        self.pending = [dict(id="per_check", sessionID="ses_worker", action="shell",
                             resources=["git status --short"], source={"type": "tool", "messageID": "msg_tool", "id": "call_check"})]
        self.forms = {}
        self.calls = []
        self.fail_create = self.fail_reply = self.lose_reply_after_acceptance = False
        case = self

        class Runtime:
            def pending_approvals(self, task):
                case.assertEqual(task["session_id"], "ses_worker")
                return [dict(permission=p, input={"command": "git status --short", "workdir": "/example/worker"})
                        for p in copy.deepcopy(case.pending)]

            def request(self, method, path, payload=None):
                case.calls.append((method, path, payload))
                if method == "POST" and path.endswith("/form"):
                    form = dict(copy.deepcopy(payload), sessionID="ses_foreman", state={"status": "pending"})
                    case.forms[form["id"]] = form
                    if case.fail_create:
                        raise OSError("lost response")
                    return {k: v for k, v in form.items() if k != "state"}
                if method == "GET":
                    form_id = path.rsplit("/", 1)[-1]
                    if form_id not in case.forms:
                        raise Missing("Form missing")
                    return copy.deepcopy(case.forms[form_id])
                if method == "DELETE":
                    case.forms[path.rsplit("/", 1)[-1]]["state"] = {"status": "cancelled"}
                    return None
                if method == "POST" and path.endswith("/reply"):
                    if case.fail_reply:
                        raise OSError("lost reply")
                    permission_id = path.split("/")[-2]
                    case.pending = ([] if payload["decision"] == "reject" else
                                    [p for p in case.pending if p["id"] != permission_id])
                    if case.lose_reply_after_acceptance:
                        raise OSError("accepted reply but lost response")
                    return None
                raise AssertionError((method, path))

        self.runtime = Runtime()

    def tick(self):
        from foreman_approvals import ApprovalRelay
        ApprovalRelay(self.runtime).deliver(self.store)

    def answer(self, decision="once"):
        form, = self.forms.values()
        form["state"] = {"status": "answered", "answer": {"decision": decision}}

    def replies(self):
        return [c for c in self.calls if c[0] == "POST" and c[1].endswith("/reply")]

    def test_request_appears_once_in_exact_foreman_without_model_prompt(self):
        self.tick()
        self.tick()
        form, = self.forms.values()
        self.assertEqual(form["sessionID"], "ses_foreman")
        self.assertIn("git status --short", form["fields"][0]["description"])
        self.assertIn("/example/worker", form["fields"][0]["description"])
        self.assertEqual([o["value"] for o in form["fields"][0]["options"]], ["once", "reject"])
        self.assertNotIn("default", form["fields"][0])
        self.assertEqual(sum(c[0] == "POST" for c in self.calls), 1)
        self.assertEqual(self.replies(), [])

    def test_human_once_answer_replies_only_to_original_permission(self):
        self.tick()
        self.answer()
        self.tick()
        self.tick()
        self.assertEqual(self.replies(), [("POST", "/api/session/ses_worker/permission/per_check/reply", {"decision": "once"})])

    def test_reject_is_forwarded_but_always_is_never_allowed(self):
        self.tick()
        self.answer("always")
        self.tick()
        self.assertEqual(self.replies(), [])

    def test_human_rejection_is_forwarded(self):
        self.tick()
        self.answer("reject")
        self.tick()
        self.assertEqual(self.replies()[0][2], {"decision": "reject"})

    def test_stale_answer_does_not_approve_a_replacement_request(self):
        self.tick()
        self.answer()
        self.pending[0]["id"] = "per_replacement"
        self.tick()
        self.assertEqual(self.replies(), [])

    def test_changed_request_with_same_id_is_not_approved(self):
        self.tick()
        self.answer()
        self.pending[0]["resources"] = ["git push"]
        self.tick()
        self.assertEqual(self.replies(), [])

    def test_disappeared_permission_cancels_pending_card(self):
        self.tick()
        self.pending = []
        self.tick()
        form, = self.forms.values()
        self.assertEqual(form["state"]["status"], "cancelled")

    def test_lost_create_response_reconciles_exact_form_without_duplicate(self):
        self.fail_create = True
        self.tick()
        self.tick()
        self.assertEqual(sum(c[0] == "POST" for c in self.calls), 1)
        self.answer()
        self.tick()
        self.assertEqual(len(self.replies()), 1)

    def test_lost_reply_response_is_not_repeated_after_restart(self):
        self.tick()
        self.answer()
        self.fail_reply = True
        self.tick()
        self.tick()
        self.assertEqual(len(self.replies()), 1)
        row, = self.store.approvals("ses_foreman")
        self.assertEqual(row["state"], "uncertain")

    def test_altered_or_wrong_owner_card_does_not_authorise(self):
        self.tick()
        self.answer()
        form, = self.forms.values()
        form["sessionID"] = "ses_other"
        self.tick()
        self.assertEqual(self.replies(), [])

    def test_settled_task_invalidates_card(self):
        self.tick()
        self.answer()
        self.store.update("task", "succeeded", result="done")
        self.tick()
        self.assertEqual(self.replies(), [])

    def test_evicted_form_is_restored_for_same_pending_request_without_approval(self):
        self.tick()
        original_id, = self.forms
        self.forms.clear()
        self.tick()
        self.tick()
        self.assertEqual(list(self.forms), [original_id])
        self.assertEqual(sum(c[0] == "POST" for c in self.calls), 2)
        self.assertEqual(self.replies(), [])
        self.answer()
        self.tick()
        self.assertEqual(len(self.replies()), 1)

    def test_missing_form_after_reply_attempt_is_never_restored(self):
        self.tick()
        self.answer()
        self.fail_reply = True
        self.tick()
        self.forms.clear()
        self.tick()
        self.tick()
        self.assertEqual(self.forms, {})
        self.assertEqual(len(self.replies()), 1)
        self.assertEqual(sum(c[0] == "POST" and c[1].endswith('/form') for c in self.calls), 1)

    def test_missing_form_with_changed_permission_is_not_restored(self):
        self.tick()
        self.forms.clear()
        self.pending[0]["resources"] = ["git push"]
        self.tick()
        self.assertEqual(self.forms, {})
        self.assertEqual(self.replies(), [])

    def test_missing_form_recovery_failure_backs_off_without_approving(self):
        self.tick()
        self.forms.clear()
        self.fail_create = True
        self.tick()
        self.forms.clear()
        self.tick()
        self.assertEqual(sum(c[0] == "POST" for c in self.calls), 2)
        self.assertEqual(self.replies(), [])

    def test_once_preserves_other_parallel_requests(self):
        self.pending.append(dict(self.pending[0], id="per_second"))
        self.tick()
        form = next(f for f in self.forms.values() if f["metadata"]["teem_permission"] == "per_check")
        form["state"] = {"status": "answered", "answer": {"decision": "once"}}
        self.tick()
        self.assertEqual([p["id"] for p in self.pending], ["per_second"])
        self.assertEqual(len(self.replies()), 1)

    def test_reject_warns_about_native_batch_scope_and_cancels_other_cards(self):
        self.pending.append(dict(self.pending[0], id="per_second"))
        self.tick()
        form = next(f for f in self.forms.values() if f["metadata"]["teem_permission"] == "per_check")
        self.assertIn("all pending permissions in this worker session", form["fields"][0]["description"])
        form["state"] = {"status": "answered", "answer": {"decision": "reject"}}
        self.tick()
        self.tick()
        self.assertEqual(self.pending, [])
        self.assertEqual(len(self.replies()), 1)
        other = next(f for f in self.forms.values() if f["metadata"]["teem_permission"] == "per_second")
        self.assertEqual(other["state"]["status"], "cancelled")
        row = next(r for r in self.store.approvals("ses_foreman") if r["permission_id"] == "per_check")
        self.assertIn("all pending permissions", row["detail"])

    def test_acceptance_then_lost_reply_response_is_not_repeated_with_reopened_sqlite(self):
        self.tick()
        self.answer()
        self.lose_reply_after_acceptance = True
        self.tick()
        directory = self.store.directory
        self.store.close()
        self.store = Store(directory, ["/example/worker"])
        self.addCleanup(self.store.close)
        self.tick()
        self.assertEqual(len(self.replies()), 1)
        self.assertEqual(self.pending, [])

    def test_pending_card_survives_reopened_sqlite_without_duplicate(self):
        self.tick()
        directory = self.store.directory
        self.store.close()
        self.store = Store(directory, ["/example/worker"])
        self.addCleanup(self.store.close)
        self.tick()
        self.assertEqual(sum(c[0] == "POST" for c in self.calls), 1)
        self.answer()
        self.tick()
        self.assertEqual(len(self.replies()), 1)

    def test_old_uncertain_rows_migrate_without_assuming_no_reply(self):
        directory = Path(self.temp.name) / "old-state"
        directory.mkdir()
        with sqlite3.connect(directory / "tasks.sqlite3") as db:
            db.execute("""CREATE TABLE approvals (
                task_id TEXT NOT NULL, permission_id TEXT NOT NULL, foreman TEXT NOT NULL,
                form_id TEXT NOT NULL UNIQUE, snapshot TEXT NOT NULL, payload TEXT NOT NULL,
                state TEXT NOT NULL, detail TEXT NOT NULL DEFAULT '', PRIMARY KEY(task_id,permission_id))""")
            for state in ("pending", "uncertain", "replying", "done"):
                db.execute("INSERT INTO approvals VALUES (?,?,?,?,?,?,?,?)",
                           ("task", "per_" + state, "ses_foreman", "frm_" + state, "{}", "{}", state, "preserved"))
        with Store(directory, []) as migrated:
            rows = migrated.db.execute("SELECT state,reply_attempted,detail FROM approvals").fetchall()
            self.assertEqual({r["state"]: r["reply_attempted"] for r in rows},
                             {"pending": 0, "uncertain": 1, "replying": 1, "done": 1})
            self.assertTrue(all(r["detail"] == "preserved" for r in rows))

    def test_reply_attempt_marker_survives_sqlite_reopen_and_form_loss(self):
        self.tick()
        self.answer()
        self.fail_reply = True
        self.tick()
        directory = self.store.directory
        self.store.close()
        self.store = Store(directory, ["/example/worker"])
        self.addCleanup(self.store.close)
        self.forms.clear()
        self.tick()
        self.assertEqual(self.forms, {})
        self.assertEqual(len(self.replies()), 1)


if __name__ == "__main__":
    unittest.main()
