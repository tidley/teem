import base64
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import tempfile
import threading
import unittest
import uuid

from foreman_runtime import OpenCode, task_permissions


class RuntimeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="foreman-http-", dir="/tmp/opencode")
        self.addCleanup(self.temp.cleanup)
        self.registry = Path(self.temp.name) / "registry.json"
        self.calls = []
        self.responses = {}
        case = self

        class Handler(BaseHTTPRequestHandler):
            def handle_request(self):
                payload = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))) or "null")
                case.calls.append((self.command, self.path, payload, self.headers.get("Authorization")))
                status, response = case.responses.get((self.command, self.path), (404, {"error": "missing"}))
                self.send_response(status)
                if status == 302:
                    self.send_header("Location", "/must-not-follow")
                self.end_headers()
                self.wfile.write(json.dumps(response).encode())

            do_GET = handle_request
            do_POST = handle_request

            def log_message(self, *_):
                pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)
        self.registry.write_text(json.dumps(dict(url=f"http://127.0.0.1:{self.server.server_port}", password="test-only")))
        self.runtime = OpenCode(self.registry, "openai/test-model")
        uid = uuid.uuid4().hex
        self.task = dict(id=str(uuid.UUID(uid)), session_id="ses_foreman_" + uid,
                         prompt_id="msg_foreman_" + uid, role="worker", checkout="/example/repo",
                         sparse=False, request=dict(revision="a" * 40, scope=["src"], prompt="Edit src"))
        self.session = self.runtime.session_payload(self.task)
        self.prefix = "/api/session/" + self.task["session_id"]
        self.responses[("GET", self.prefix)] = (200, {"data": self.session})
        self.responses[("GET", "/api/session/active")] = (200, {"data": {}})
        self.responses[("GET", self.prefix + "/inbox")] = (200, {"data": []})

    def accepted(self):
        self.responses[("GET", self.prefix + "/message/" + self.task["prompt_id"])] = (
            200, {"data": dict(id=self.task["prompt_id"], type="user",
                               text=self.runtime.prompt_text(self.task), time=dict(created=10))})

    def test_creates_exact_native_session_without_terminal(self):
        self.responses[("POST", "/api/session")] = (200, {"data": self.session})
        self.runtime.create(self.task)
        method, path, payload, auth = self.calls[0]
        self.assertEqual((method, path), ("POST", "/api/session"))
        self.assertEqual(payload["id"], self.task["session_id"])
        self.assertEqual(payload["location"], {"directory": "/example/repo"})
        self.assertEqual(auth, "Basic " + base64.b64encode(b"opencode:test-only").decode())

    def test_mismatched_adopted_session_is_rejected(self):
        other = dict(self.session, metadata={})
        self.responses[("POST", "/api/session")] = (200, {"data": other})
        with self.assertRaises(OSError):
            self.runtime.create(self.task)

    def test_prompt_has_persisted_id_and_queue_delivery(self):
        self.responses[("POST", self.prefix + "/prompt")] = (200, {"data": {"id": self.task["prompt_id"]}})
        self.runtime.prompt(self.task)
        payload = self.calls[0][2]
        self.assertEqual(payload["id"], self.task["prompt_id"])
        self.assertEqual(payload["delivery"], "queue")
        self.assertTrue(payload["resume"])

    def test_idle_missing_prompt_is_uncertain_not_success(self):
        self.session.update(outcome="succeeded", time={"idle": 20})
        self.assertEqual(self.runtime.inspect(self.task)["state"], "uncertain")

    def test_native_inbox_is_accepted_not_executed(self):
        self.responses[("GET", self.prefix + "/inbox")] = (
            200, {"data": [{"id": self.task["prompt_id"], "type": "user",
                             "payload": {"text": self.runtime.prompt_text(self.task)}}]})
        self.assertEqual(self.runtime.inspect(self.task)["state"], "accepted")

    def test_active_session_requires_own_prompt(self):
        self.responses[("GET", "/api/session/active")] = (200, {"data": {self.task["session_id"]: {"type": "running"}}})
        self.assertEqual(self.runtime.inspect(self.task)["state"], "uncertain")
        self.accepted()
        self.responses[("GET", self.prefix + "/permission")] = (200, {"data": []})
        self.responses[("GET", self.prefix + "/form")] = (200, {"data": []})
        self.assertEqual(self.runtime.inspect(self.task)["state"], "running")

    def test_completion_requires_final_text_and_terminal_time(self):
        self.accepted()
        self.session.update(outcome="succeeded", time={"idle": 20})
        self.responses[("GET", self.prefix + "/message?limit=20&order=desc")] = (
            200, {"data": [{"type": "assistant", "time": {"created": 11, "completed": 19},
                             "finish": "stop", "content": [{"type": "text", "text": "Done"}]}]})
        self.assertEqual(self.runtime.inspect(self.task), {"state": "succeeded", "result": "Done"})
        self.session["time"]["idle"] = 5
        self.assertEqual(self.runtime.inspect(self.task)["state"], "uncertain")

    def test_failure_cannot_be_reported_as_success(self):
        self.accepted()
        self.session.update(outcome="failed", time={"idle": 20})
        self.assertEqual(self.runtime.inspect(self.task)["state"], "failed")

    def test_redirects_are_not_followed_with_credentials(self):
        self.responses[("POST", "/api/session")] = (302, {})
        with self.assertRaises(OSError):
            self.runtime.create(self.task)
        self.assertEqual(len(self.calls), 1)

    def test_registry_rejects_remote_urls_and_missing_password(self):
        for url in ("https://example.com", "http://127.0.0.1:80/path", "http://user@127.0.0.1", "http://localhost:80"):
            self.registry.write_text(json.dumps({"url": url, "password": "secret"}))
            with self.subTest(url=url), self.assertRaises(ValueError):
                OpenCode(self.registry, "openai/test-model")
        self.registry.write_text(json.dumps({"url": "http://127.0.0.1:1"}))
        with self.assertRaises(ValueError):
            OpenCode(self.registry, "openai/test-model")

    def test_permissions_deny_reviewer_writes_shell_and_delegation(self):
        rules = task_permissions(dict(self.task, role="reviewer"))
        self.assertEqual(rules[0], {"action": "*", "resource": "*", "effect": "deny"})
        self.assertEqual({r["action"] for r in rules if r["effect"] == "allow"}, {"read", "glob", "grep"})

    def test_worker_edits_are_scoped_and_shell_needs_approval(self):
        rules = task_permissions(self.task)
        self.assertIn({"action": "edit", "resource": "src/**", "effect": "allow"}, rules)
        self.assertIn({"action": "shell", "resource": "*", "effect": "ask"}, rules)
        self.assertNotIn({"action": "edit", "resource": "*", "effect": "allow"}, rules)


if __name__ == "__main__":
    unittest.main()
