import concurrent.futures
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
import uuid

from foreman_supervisor import Store, Supervisor, Invalid


def git(path, *args):
    return subprocess.check_output(["git", "-C", str(path), *args], text=True,
                                   stderr=subprocess.DEVNULL).strip()


class FakeRuntime:
    def __init__(self):
        self.sessions = {}
        self.prompts = []
        self.fail_prompt = False

    def create(self, task):
        self.sessions[task["session_id"]] = {
            "task": task, "state": "missing", "result": "", "accepted": False}

    def prompt(self, task):
        self.prompts.append(task["prompt_id"])
        if self.fail_prompt:
            raise OSError("lost response")
        self.sessions[task["session_id"]].update(state="accepted", accepted=True)

    def inspect(self, task):
        session = self.sessions.get(task["session_id"])
        if session is None:
            return {"state": "uncertain", "detail": "Session not found"}
        return {"state": session["state"], "result": session["result"]}


class SupervisorTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="foreman-", dir="/tmp/opencode")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.checkouts = []
        for i in range(6):
            path = self.root / f"repo{i}"
            path.mkdir()
            git(path, "init", "-q")
            git(path, "config", "user.name", "Test")
            git(path, "config", "user.email", "test@example.invalid")
            (path / "src").mkdir()
            (path / "src/a.txt").write_text("base\n")
            git(path, "add", ".")
            git(path, "commit", "-qm", "base")
            self.checkouts.append(path)
        self.store = Store(self.root / "state", self.checkouts)
        self.addCleanup(self.store.close)
        self.store.bind("ses_foreman", "term_foreman")
        self.runtime = FakeRuntime()
        self.supervisor = Supervisor(self.store, self.runtime)

    def submit(self, index=0, role="worker", **changes):
        request = dict(task_id=str(uuid.uuid4()), foreman="ses_foreman", role=role,
                       checkout=str(self.checkouts[index]), revision=git(self.checkouts[index], "rev-parse", "HEAD"),
                       scope=["src"], prompt="Make a small change" if role == "worker" else "Review this commit")
        request.update(changes)
        return self.store.submit(**request)

    def test_duplicate_uuid_returns_original_but_changed_payload_fails(self):
        task = self.submit()
        self.assertEqual(self.store.submit(**task["request"])["id"], task["id"])
        with self.assertRaises(Invalid):
            self.store.submit(**dict(task["request"], prompt="Different work"))

    def test_enforces_two_slots_for_each_role(self):
        for i in range(3):
            self.submit(i)
            self.submit(i + 3, "reviewer")
        self.supervisor.tick()
        states = self.store.tasks("ses_foreman")
        self.assertEqual(sum(t["state"] == "accepted" for t in states), 4)
        self.assertEqual(sum(t["state"] == "queued" for t in states), 2)

    def test_one_checkout_has_only_one_live_owner(self):
        self.submit()
        self.submit(role="reviewer")
        self.supervisor.tick()
        self.assertEqual([t["state"] for t in self.store.tasks("ses_foreman")],
                         ["accepted", "queued"])

    def test_concurrent_stores_cannot_overbook_slots(self):
        for i in range(6):
            self.submit(i)
        def claim(_):
            with Store(self.root / "state", self.checkouts) as other:
                return other.claim()
        with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool:
            claimed = list(pool.map(claim, range(6)))
        self.assertEqual(sum(t is not None for t in claimed), 2)

    def test_uncertain_submission_keeps_slot_and_never_replays(self):
        task = self.submit()
        self.runtime.fail_prompt = True
        self.supervisor.tick()
        self.assertEqual(self.store.task(task["id"])["state"], "uncertain")
        self.supervisor.tick()
        with Store(self.root / "state", self.checkouts) as reopened:
            Supervisor(reopened, self.runtime).tick()
        self.assertEqual(self.runtime.prompts, [task["prompt_id"]])

    def test_restart_reconciles_starting_without_new_submission(self):
        task = self.submit()
        claimed = self.store.claim()
        self.runtime.create(claimed)
        self.runtime.prompt(claimed)
        self.supervisor.tick()
        self.assertEqual(self.store.task(task["id"])["state"], "accepted")
        self.assertEqual(len(self.runtime.prompts), 1)

    def test_idle_without_acceptance_is_not_success(self):
        task = self.submit()
        self.supervisor.tick()
        self.runtime.sessions[task["session_id"]].update(state="uncertain")
        self.supervisor.tick()
        self.assertEqual(self.store.task(task["id"])["state"], "uncertain")
        self.assertEqual(self.store.events("ses_foreman"), [])

    def test_completion_persists_one_event_and_result(self):
        task = self.submit()
        self.supervisor.tick()
        self.runtime.sessions[task["session_id"]].update(state="succeeded", result="Changed src/a.txt")
        self.supervisor.tick()
        self.supervisor.tick()
        self.assertEqual(self.store.task(task["id"])["result"], "Changed src/a.txt")
        event, = self.store.events("ses_foreman")
        with self.assertRaises(Invalid):
            self.store.ack("ses_other", event["id"])
        self.store.ack("ses_foreman", event["id"])
        self.assertEqual(self.store.events("ses_foreman"), [])

    def test_native_failure_is_not_success(self):
        task = self.submit()
        self.supervisor.tick()
        self.runtime.sessions[task["session_id"]].update(state="failed", result="Provider failed")
        self.supervisor.tick()
        self.assertEqual(self.store.task(task["id"])["state"], "failed")

    def test_scope_and_checkout_are_explicit(self):
        for scope in (["../outside"], ["/tmp"], ["."], [".git"], ["src/*"], []):
            with self.subTest(scope=scope), self.assertRaises(Invalid):
                self.submit(scope=scope)
        with self.assertRaises(Invalid):
            self.submit(checkout=str(self.root))

    def test_reviewer_rejects_sparse_checkout(self):
        git(self.checkouts[0], "sparse-checkout", "init", "--cone")
        git(self.checkouts[0], "sparse-checkout", "set", "src")
        task = self.submit()
        self.assertTrue(task["sparse"])
        with self.assertRaises(Invalid):
            self.submit(role="reviewer")

    def test_reviewer_requires_exact_clean_revision(self):
        (self.checkouts[0] / "src/a.txt").write_text("dirty\n")
        with self.assertRaises(Invalid):
            self.submit(role="reviewer")
        with self.assertRaises(Invalid):
            self.submit(revision="0" * 40)

    def test_rechecks_checkout_before_launch(self):
        task = self.submit(role="reviewer")
        (self.checkouts[0] / "src/a.txt").write_text("changed after admission\n")
        self.supervisor.tick()
        self.assertEqual(self.store.task(task["id"])["state"], "failed")
        self.assertEqual(self.runtime.prompts, [])

    def test_revoked_checkout_authorisation_prevents_queued_launch(self):
        task = self.submit()
        with Store(self.root / "state", []) as reopened:
            Supervisor(reopened, self.runtime).tick()
        self.assertEqual(self.runtime.prompts, [])
        self.assertEqual(self.store.task(task["id"])["state"], "failed")

    def test_uuid_and_foreman_binding_are_required(self):
        with self.assertRaises(Invalid):
            self.submit(task_id="not-a-uuid")
        with self.assertRaises(Invalid):
            self.submit(foreman="ses_missing")

    def test_private_state(self):
        self.assertEqual((self.root / "state").stat().st_mode & 0o777, 0o700)
        self.assertEqual((self.root / "state/tasks.sqlite3").stat().st_mode & 0o777, 0o600)


if __name__ == "__main__":
    unittest.main()
