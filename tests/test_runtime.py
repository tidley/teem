import base64
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import sqlite3
from pathlib import Path
import tempfile
import threading
import unittest
import uuid

from foreman_runtime import OpenCode, task_permissions
from foreman_supervisor import Store


class RuntimeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="foreman-http-")
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

    def test_workflow_outcome_requests_one_summary_not_a_todo_refresh_or_worker_replay(self):
        event = {'id': str(uuid.uuid4()), 'request_id': str(uuid.uuid4()), 'workflow_id': str(uuid.uuid4()),
                 'foreman': 'ses_foreman', 'task_id': 'workflow-result'}
        payload = self.runtime.event_payload(event)
        self.assertIn('workflow', payload['text'])
        self.assertIn(event['workflow_id'], payload['text'])
        self.assertIn('concise', payload['text'])
        self.assertIn('Do not', payload['text'])
        self.assertNotIn('todowrite', payload['text'])
        self.assertEqual(payload['metadata']['teem_workflow'], event['workflow_id'])
        native = {'id': payload['id'], 'type': 'synthetic', 'sessionID': event['foreman'], 'payload': payload}
        self.runtime.validate_event(event, native, queued=True)

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

    def test_runtime_explicit_default_variant_matches_omitted_variant(self):
        self.session["model"] = {**self.session["model"], "variant": "default"}
        self.responses[("POST", "/api/session")] = (200, {"data": self.session})
        self.runtime.create(self.task)
        self.assertEqual(self.runtime.inspect(self.task)["state"], "uncertain")

    def test_default_variant_normalisation_does_not_accept_another_model(self):
        self.session["model"] = {**self.session["model"], "variant": "default", "id": "different-model"}
        self.responses[("POST", "/api/session")] = (200, {"data": self.session})
        with self.assertRaisesRegex(OSError, "model"):
            self.runtime.create(self.task)

    def test_explicit_nondefault_variant_is_still_required(self):
        self.runtime = OpenCode(self.registry, "openai/test-model#high")
        self.responses[("POST", "/api/session")] = (200, {"data": self.session})
        with self.assertRaises(OSError):
            self.runtime.create(self.task)
        self.session["model"] = {**self.session["model"], "variant": "high"}
        self.runtime.create(self.task)

    def test_prompt_has_persisted_id_and_queue_delivery(self):
        self.responses[("POST", self.prefix + "/prompt")] = (200, {"data": {"id": self.task["prompt_id"]}})
        self.runtime.prompt(self.task)
        payload = self.calls[0][2]
        self.assertEqual(payload["id"], self.task["prompt_id"])
        self.assertEqual(payload["delivery"], "queue")
        self.assertTrue(payload["resume"])

    def test_dispatch_model_is_read_from_exact_foreman_session(self):
        self.task["foreman"] = "ses_foreman_parent"
        model = {"providerID": "another-provider", "id": "foreman-model", "variant": "high"}
        self.responses[("GET", "/api/session/ses_foreman_parent")] = (200, {"data": {"id": "ses_foreman_parent", "model": model}})
        self.assertEqual(self.runtime.dispatch_model(self.task), model)
        self.responses[("GET", "/api/session/ses_foreman_parent")] = (200, {"data": {"id": "ses_other", "model": model}})
        with self.assertRaises(OSError):
            self.runtime.dispatch_model(self.task)

    def test_recorded_model_is_used_even_after_runtime_default_changes(self):
        model = {"providerID": "other", "id": "recorded", "variant": "default"}
        task = dict(self.task, runtime_model=model)
        payload = self.runtime.session_payload(task)
        self.assertEqual(payload["model"], model)
        self.runtime.model = {"providerID": "changed", "id": "later"}
        self.runtime.validate_session(task, payload)

    def test_recorded_model_survives_store_reopen_and_parent_model_switch(self):
        directory = Path(self.temp.name) / "state"
        recorded = {"providerID": "original", "id": "chosen", "variant": "high"}
        with Store(directory, []) as store:
            store.db.execute("""INSERT INTO tasks
                (id,foreman,role,checkout,state,request,session_id,prompt_id,sparse,created,runtime_model)
                VALUES (?,?,?,?,?,?,?,?,?,?,?)""", (self.task["id"], "ses_parent", "worker", self.task["checkout"],
                "accepted", json.dumps(self.task["request"]), self.task["session_id"], self.task["prompt_id"], 0, 0, json.dumps(recorded)))
            store.db.commit()
        self.responses[("GET", "/api/session/ses_parent")] = (200, {"data": {
            "id": "ses_parent", "model": {"providerID": "later", "id": "different"}}})
        self.runtime.model = {"providerID": "later", "id": "different"}
        with Store(directory, []) as reopened:
            task = reopened.task(self.task["id"])
        self.assertEqual(self.runtime.session_payload(task)["model"], recorded)
        self.assertFalse(any(path == "/api/session/ses_parent" for _, path, *_ in self.calls))

    def test_invalid_inherited_models_fail_closed(self):
        self.task["foreman"] = "ses_parent"
        for model in (None, {}, {"providerID": "p", "id": ""}, {"providerID": "p", "id": "m", "variant": None}):
            self.responses[("GET", "/api/session/ses_parent")] = (200, {"data": {"id": "ses_parent", "model": model}})
            with self.subTest(model=model), self.assertRaises(OSError):
                self.runtime.dispatch_model(self.task)

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

    def test_pending_approval_contains_exact_tool_input_and_worker_identity(self):
        self.accepted()
        self.responses[("GET", "/api/session/active")] = (200, {"data": {self.task["session_id"]: {}}})
        permission = dict(id="per_test", sessionID=self.task["session_id"], action="shell", resources=["git status"],
                          source={"type": "tool", "messageID": "msg_tool", "id": "call_shell"})
        self.responses[("GET", self.prefix + "/permission")] = (200, {"data": [permission]})
        self.responses[("GET", self.prefix + "/form")] = (200, {"data": []})
        inputs = {"command": "git status", "workdir": "/example/repo"}
        self.responses[("GET", self.prefix + "/message/msg_tool")] = (200, {"data": {
            "id": "msg_tool", "type": "assistant", "content": [dict(type="tool", id="call_shell", state={"input": inputs})]}})
        self.assertEqual(self.runtime.pending_approvals(self.task), [dict(permission=permission, input=inputs)])
        for source in (None, [], {"type": "tool", "messageID": None},
                       {"type": "tool", "messageID": 7}):
            with self.subTest(source=source):
                malformed = dict(permission, source=source)
                self.responses[("GET", self.prefix + "/permission")] = (200, {"data": [malformed]})
                with self.assertRaises(OSError):
                    self.runtime.pending_approvals(self.task)
        self.responses[("GET", self.prefix + "/permission")] = (200, {"data": [permission]})
        permission["sessionID"] = "ses_other"
        with self.assertRaises(OSError):
            self.runtime.pending_approvals(self.task)

    def test_no_content_permission_reply_is_supported(self):
        path = self.prefix + "/permission/per_test/reply"
        self.responses[("POST", path)] = (204, None)
        self.assertIsNone(self.runtime.request("POST", path, {"decision": "once"}))

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

    def test_worker_allows_only_fastbuild_as_an_external_directory(self):
        rules = task_permissions(self.task)
        self.assertEqual([r for r in rules if r["action"] == "external_directory"],
                         [{"action": "external_directory", "resource": "/mnt/fastbuild/**", "effect": "allow"}])
        self.assertEqual(self.runtime.session_payload(self.task)["permissions"], rules)

    def test_agent_todowrite_capability_is_opt_in_and_not_a_shell_or_edit_grant(self):
        self.assertFalse(any(r['action'] == 'todowrite' for r in task_permissions(self.task)))
        task = dict(self.task, role='reviewer', request=dict(self.task['request'], workflow_id='workflow', native_todos=True))
        rules = task_permissions(task)
        self.assertIn({'action': 'todowrite', 'resource': '*', 'effect': 'allow'}, rules)
        self.assertFalse(any(r['action'] in ('shell', 'edit') and r['effect'] == 'allow' for r in rules))

    def test_agent_todo_reader_uses_completed_native_calls_not_prose_or_failed_tools(self):
        messages = [dict(id='msg_latest', type='assistant', content=[
            dict(type='text', text='All work is complete'),
            dict(type='tool', name='todowrite', state={'status': 'completed', 'input': {'todos': [
                {'content': 'Read code', 'status': 'completed'}, {'content': 'Implement change', 'status': 'in_progress'}]}}),
            dict(type='tool', name='todowrite', state={'status': 'error', 'input': {'todos': []}})])]
        self.responses[('GET', self.prefix + '/message?limit=50&order=desc&type=assistant')] = (200, {'data': messages})
        result = self.runtime.read_todos(self.task)
        self.assertEqual(result['message_id'], 'msg_latest')
        self.assertEqual(result['todos'][1]['status'], 'in_progress')
        messages[0]['content'] = [dict(type='text', text='Todos: completed')]
        self.assertIsNone(self.runtime.read_todos(self.task))

    def test_fastbuild_access_does_not_allow_unscoped_edits_or_unattended_shell(self):
        rules = task_permissions(self.task)
        self.assertEqual([r["resource"] for r in rules if r["action"] == "edit" and r["effect"] == "allow"],
                         ["src", "src/**"])
        self.assertEqual([r for r in rules if r["action"] == "shell"],
                         [{"action": "shell", "resource": "*", "effect": "ask"}])

    def test_reviewer_has_no_fastbuild_external_directory_exception(self):
        rules = task_permissions(dict(self.task, role="reviewer"))
        self.assertFalse(any(r["action"] == "external_directory" for r in rules))

    def event(self):
        return dict(id="c0a151b1-45cb-4ba5-a40c-31f246bfe1f3", task_id="task-test", foreman="ses_foreman",
                    request_id="76249e19-eef9-44b9-b7e5-26a0caa740c6")

    def test_event_uses_native_synthetic_admission_with_stable_id(self):
        event = self.event()
        payload = self.runtime.event_payload(event)
        native = dict(id=payload["id"], sessionID=event["foreman"], type="synthetic",
                      delivery="queue", payload={k: payload[k] for k in ("text", "description", "metadata")})
        self.responses[("POST", "/api/session/ses_foreman/synthetic")] = (200, {"data": native})
        self.runtime.notify(event)
        method, path, sent, _ = self.calls[0]
        self.assertEqual((method, path), ("POST", "/api/session/ses_foreman/synthetic"))
        self.assertEqual(sent["id"], "msg_teem_event_76249e19eef944b9b7e526a0caa740c6")
        self.assertEqual(sent["delivery"], "queue")
        self.assertTrue(sent["resume"])
        self.assertEqual(sent["metadata"], {"teem_event": event["id"], "teem_task": event["task_id"]})
        self.assertIn("foreman_status", sent["text"])

    def test_progress_event_requests_real_native_todo_refresh_not_fake_tool_history(self):
        event = dict(self.event(), kind="progress", revision=4)
        payload = self.runtime.event_payload(event)
        self.assertEqual(payload["metadata"], {"teem_progress": event["id"], "teem_revision": 4})
        self.assertIn("todowrite", payload["text"])
        self.assertIn("foreman_status", payload["text"])
        self.assertNotIn("foreman_ack", payload["text"])
        native = dict(id=payload["id"], sessionID=event["foreman"], type="synthetic",
                      payload={k: payload[k] for k in ("text", "description", "metadata")})
        self.responses[("POST", "/api/session/ses_foreman/synthetic")] = (200, {"data": native})
        self.runtime.notify(event)
        self.assertEqual(self.calls[0][1], "/api/session/ses_foreman/synthetic")

    def test_event_response_must_match_foreman_and_payload(self):
        event = self.event()
        payload = self.runtime.event_payload(event)
        native = dict(id=payload["id"], sessionID="ses_other", type="synthetic", delivery="queue",
                      payload={k: payload[k] for k in ("text", "description", "metadata")})
        self.responses[("POST", "/api/session/ses_foreman/synthetic")] = (200, {"data": native})
        with self.assertRaises(OSError):
            self.runtime.notify(event)
        native["sessionID"] = event["foreman"]
        native["payload"]["metadata"] = {"teem_event": "wrong"}
        with self.assertRaises(OSError):
            self.runtime.notify(event)

    def test_event_acceptance_reads_exact_projected_message(self):
        event = self.event()
        payload = self.runtime.event_payload(event)
        message = dict(id=payload["id"], type="synthetic", text=payload["text"], metadata=payload["metadata"])
        path = "/api/session/ses_foreman/message/" + payload["id"]
        self.responses[("GET", path)] = (200, {"data": message})
        self.assertTrue(self.runtime.event_accepted(event))
        self.assertEqual([(m, p) for m, p, *_ in self.calls], [("GET", path)])

    def test_preupgrade_native_completion_keeps_original_payload_and_receipt_validation(self):
        directory = Path(self.temp.name) / "old-state"
        directory.mkdir()
        event = self.event()
        original = self.runtime.event_payload(event)
        with sqlite3.connect(directory / "tasks.sqlite3") as db:
            db.execute("""CREATE TABLE events (id TEXT PRIMARY KEY,task_id TEXT NOT NULL UNIQUE,
                foreman TEXT NOT NULL,state TEXT NOT NULL DEFAULT 'pending',request_id TEXT NOT NULL UNIQUE,
                acked INTEGER NOT NULL DEFAULT 0,next_receipt REAL NOT NULL DEFAULT 0,
                transport TEXT NOT NULL DEFAULT 'native',detail TEXT NOT NULL DEFAULT '')""")
            db.execute("INSERT INTO events(id,task_id,foreman,request_id) VALUES (?,?,?,?)",
                       tuple(event[k] for k in ("id", "task_id", "foreman", "request_id")))
        with Store(directory, []) as store:
            row = dict(store.db.execute("SELECT * FROM events").fetchone())
        self.assertEqual(row["refresh_todos"], 0)
        self.assertEqual(self.runtime.event_payload(row), original)
        path = "/api/session/ses_foreman/message/" + original["id"]
        self.responses[("GET", path)] = (200, {"data": dict(id=original["id"], type="synthetic", text=original["text"], metadata=original["metadata"])})
        self.assertTrue(self.runtime.event_accepted(row))

    def test_event_acceptance_reads_inbox_without_resending(self):
        event = self.event()
        payload = self.runtime.event_payload(event)
        native = dict(id=payload["id"], sessionID=event["foreman"], type="synthetic",
                      payload={k: payload[k] for k in ("text", "description", "metadata")})
        self.responses[("GET", "/api/session/ses_foreman/inbox")] = (200, {"data": [native]})
        self.assertTrue(self.runtime.event_accepted(event))
        self.assertTrue(all(method == "GET" for method, *_ in self.calls))

    def test_missing_native_event_is_not_proof_it_can_be_resent(self):
        self.responses[("GET", "/api/session/ses_foreman/inbox")] = (200, {"data": []})
        self.assertFalse(self.runtime.event_accepted(self.event()))
        self.assertTrue(all(method == "GET" for method, *_ in self.calls))

    def test_mismatched_native_event_cannot_confirm_delivery(self):
        event = self.event()
        payload = self.runtime.event_payload(event)
        self.responses[("GET", "/api/session/ses_foreman/message/" + payload["id"])] = (
            200, {"data": dict(id=payload["id"], type="user", text=payload["text"], metadata=payload["metadata"])})
        with self.assertRaises(OSError):
            self.runtime.event_accepted(event)


if __name__ == "__main__":
    unittest.main()
