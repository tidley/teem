import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
import unittest
from unittest.mock import patch
import uuid

from codr_foreman import DeliveryUnknown, Rejected
from foreman_plugin import load_config, mcp, service_unit, bind_from_herdr, Notifier
from foreman_supervisor import Store, Invalid


class PluginTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="foreman-plugin-", dir="/tmp/opencode")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.store = Store(self.root / "state", [])
        self.addCleanup(self.store.close)
        self.store.bind("ses_foreman", "term_foreman")

    def exchange(self, *requests):
        output = io.StringIO()
        mcp(self.store, "ses_foreman", io.StringIO("\n".join(json.dumps(r) for r in requests) + "\n"), output)
        return [json.loads(line) for line in output.getvalue().splitlines()]

    def test_mcp_initialises_and_lists_structured_tools(self):
        responses = self.exchange(
            {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2024-11-05"}},
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list"})
        self.assertEqual(len(responses), 2)
        self.assertEqual(responses[0]["result"]["protocolVersion"], "2024-11-05")
        self.assertEqual(responses[0]["result"]["serverInfo"]["name"], "teem")
        tools = {t["name"]: t for t in responses[1]["result"]["tools"]}
        self.assertEqual(set(tools), {"foreman_submit", "foreman_status", "foreman_ack"})
        self.assertNotIn("foreman", tools["foreman_submit"]["inputSchema"]["properties"])

    def test_status_uses_fixed_foreman_binding(self):
        result, = self.exchange({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                                 "params": {"name": "foreman_status", "arguments": {}}})
        data = json.loads(result["result"]["content"][0]["text"])
        self.assertEqual(data["foreman"], "ses_foreman")
        self.assertEqual(data["limits"], {"worker": 2, "reviewer": 2})

    def test_mcp_rejects_unregistered_foreman(self):
        with self.assertRaises(Invalid):
            mcp(self.store, "ses_missing", io.StringIO(""), io.StringIO())

    def test_malformed_tool_arguments_return_error_without_crash(self):
        for arguments in ({}, [], {"foreman": "ses_other"}):
            response, = self.exchange({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                                      "params": {"name": "foreman_submit", "arguments": arguments}})
            self.assertTrue(response["result"]["isError"])

    def test_invalid_json_returns_parse_error(self):
        output = io.StringIO()
        mcp(self.store, "ses_foreman", io.StringIO("not-json\n"), output)
        self.assertEqual(json.loads(output.getvalue())["error"]["code"], -32700)

    def test_binding_checks_native_identity_and_uses_running_herdr(self):
        agent = dict(agent="opencode", terminal_id="term_test",
                     agent_session=dict(agent="opencode", kind="id", source="herdr:opencode", value="ses_test"))
        calls = []
        def run(command, **kwargs):
            calls.append(command)
            return subprocess.CompletedProcess(command, 0, json.dumps({"result": {"agent": agent}}))
        with patch.dict(os.environ, HERDR_BIN_PATH="/example/herdr", HERDR_PANE_ID="w3:p8", HERDR_ENV="1"):
            bind_from_herdr(self.store, run=run)
        self.assertEqual(calls, [["/example/herdr", "agent", "get", "w3:p8"]])
        self.assertEqual(self.store.binding("ses_test")["terminal_id"], "term_test")

    def test_notification_binding_must_match_native_session_and_terminal(self):
        with self.assertRaises(Invalid):
            self.store.bind("ses_foreman", "term_foreman", dict(
                codr="codr", config="/example/config", bridge_account="a" * 64,
                session_key=json.dumps(["herdr:opencode", "opencode", "id", "ses_other"]), terminal_id="term_foreman"))

    def test_service_unit_preserves_explicit_paths_and_has_no_enable_side_effect(self):
        unit = service_unit(self.root / "space % config.toml")
        self.assertIn("Restart=on-failure", unit)
        self.assertIn("Description=TEEM headless task supervisor", unit)
        self.assertIn("UMask=0077", unit)
        self.assertIn("space %% config.toml", unit)
        self.assertIn(" serve", unit)
        self.assertNotIn("--auto", unit)

    def test_manifest_has_status_popup_and_no_worker_panes(self):
        root = Path(__file__).resolve().parents[1]
        manifest = tomllib.loads((root / "herdr-plugin.toml").read_text())
        self.assertEqual(manifest["id"], "local.teem")
        self.assertEqual(manifest["name"], "TEEM")
        action = next(a for a in manifest["actions"] if a["id"] == "status")
        self.assertEqual(action["command"][-1], "open-status")
        self.assertEqual([p["placement"] for p in manifest["panes"]], ["popup"])
        for entry in manifest["actions"] + manifest["startup"] + manifest["panes"]:
            self.assertEqual(entry["command"][0], "python3")
            self.assertTrue((root / entry["command"][1]).is_file())

    def test_status_action_opens_popup_through_running_herdr(self):
        from foreman_plugin import open_status
        calls = []
        def run(command, **kwargs):
            calls.append(command)
            return subprocess.CompletedProcess(command, 0)
        with patch.dict(os.environ, HERDR_BIN_PATH="/example/herdr", HERDR_ENV="1"):
            self.assertEqual(open_status(run=run), 0)
        self.assertEqual(calls, [["/example/herdr", "plugin", "pane", "open",
                                 "--plugin", "local.teem", "--entrypoint", "status"]])

    def test_disabled_config_needs_no_runtime_credentials(self):
        config = self.root / "config.toml"
        config.write_text('enabled = false\nstate_dir = "' + str(self.root / "state") + '"\nallowed_checkouts = []\n')
        self.assertFalse(load_config(config)["enabled"])
        root = Path(__file__).resolve().parents[1]
        result = subprocess.run([sys.executable, "-B", str(root / "foreman_plugin.py"),
                                 "--config", str(config), "serve"], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("disabled", result.stdout)


class NotificationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="foreman-event-", dir="/tmp/opencode")
        self.addCleanup(self.temp.cleanup)
        self.store = Store(Path(self.temp.name) / "state", [])
        self.addCleanup(self.store.close)
        self.store.bind("ses_foreman", "term_foreman", dict(
            codr="codr", config="/example/config.toml", bridge_account="a" * 64,
            session_key=json.dumps(["herdr:opencode", "opencode", "id", "ses_foreman"]), terminal_id="term_foreman"))
        self.event = str(uuid.uuid4())
        with self.store.db:
            self.store.db.execute("INSERT INTO events (id,task_id,foreman,request_id) VALUES (?,?,?,?)",
                                  (self.event, "task-test", "ses_foreman", str(uuid.uuid4())))
        self.submits = []
        self.receipts = []
        self.mode = "accepted"
        case = self

        class Client:
            def __init__(self, **kwargs):
                pass

            def submit(self, request_id, text):
                case.submits.append((request_id, text))
                if case.mode == "unknown":
                    raise DeliveryUnknown("Lost response")
                if case.mode == "rejected":
                    raise Rejected("Rejected")
                return {"status": "queued"}

            def receipt(self, request_id):
                case.receipts.append(request_id)
                return {"accepted": case.mode == "accepted"}

        self.notifier = Notifier(client_factory=Client)

    def test_accepted_wake_is_not_event_acknowledgement(self):
        self.notifier.deliver(self.store)
        self.notifier.deliver(self.store)
        self.assertEqual(len(self.submits), 1)
        event, = self.store.events("ses_foreman")
        self.assertEqual(event["state"], "queued")
        self.assertFalse(event["acked"])

    def test_unknown_send_is_never_automatically_repeated(self):
        self.mode = "unknown"
        self.notifier.deliver(self.store)
        self.notifier.deliver(self.store)
        self.assertEqual(len(self.submits), 1)
        self.assertEqual(self.store.events("ses_foreman")[0]["state"], "uncertain")

    def test_crash_after_attempt_reconciles_receipt_only(self):
        with self.store.db:
            self.store.db.execute("UPDATE events SET state='attempting' WHERE id=?", (self.event,))
        self.notifier.deliver(self.store)
        self.assertEqual(self.submits, [])
        self.assertEqual(len(self.receipts), 1)
        self.assertEqual(self.store.events("ses_foreman")[0]["state"], "queued")

    def test_explicit_rejection_is_visible_and_not_retried(self):
        self.mode = "rejected"
        self.notifier.deliver(self.store)
        self.notifier.deliver(self.store)
        self.assertEqual(len(self.submits), 1)
        self.assertEqual(self.store.events("ses_foreman")[0]["state"], "rejected")

    def test_old_uncertain_events_cannot_starve_new_completions(self):
        self.mode = "unknown"
        with self.store.db:
            self.store.db.execute("UPDATE events SET state='uncertain'")
            for i in range(32):
                self.store.db.execute("INSERT INTO events (id,task_id,foreman,request_id,state) VALUES (?,?,?,?,?)",
                                      (str(uuid.uuid4()), f"old-{i}", "ses_foreman", str(uuid.uuid4()), "uncertain"))
            self.store.db.execute("INSERT INTO events (id,task_id,foreman,request_id) VALUES (?,?,?,?)",
                                  (str(uuid.uuid4()), "new-completion", "ses_foreman", str(uuid.uuid4())))
        self.notifier.deliver(self.store)
        self.assertEqual(len(self.submits), 1)
        self.assertIn("new-completion", self.submits[0][1])

    def test_unconfigured_foreman_events_do_not_starve_configured_foreman(self):
        self.store.bind("ses_unconfigured", "term_unconfigured")
        with self.store.db:
            self.store.db.execute("UPDATE events SET foreman='ses_unconfigured'")
            for i in range(40):
                self.store.db.execute("INSERT INTO events (id,task_id,foreman,request_id) VALUES (?,?,?,?)",
                                      (str(uuid.uuid4()), f"unconfigured-{i}", "ses_unconfigured", str(uuid.uuid4())))
            self.store.db.execute("INSERT INTO events (id,task_id,foreman,request_id) VALUES (?,?,?,?)",
                                  (str(uuid.uuid4()), "configured-task", "ses_foreman", str(uuid.uuid4())))
        self.notifier.deliver(self.store)
        self.assertEqual(len(self.submits), 1)

    def test_uncertain_receipt_checks_have_persisted_backoff(self):
        self.mode = "unknown"
        with self.store.db:
            self.store.db.execute("UPDATE events SET state='uncertain'")
        self.notifier.deliver(self.store)
        self.notifier.deliver(self.store)
        self.assertEqual(len(self.receipts), 1)


if __name__ == "__main__":
    unittest.main()
