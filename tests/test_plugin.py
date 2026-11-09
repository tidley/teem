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
from foreman_plugin import load_config, mcp, service_unit, bind_from_herdr
from foreman_supervisor import Store, Invalid


class PluginTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="foreman-plugin-")
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
        self.assertEqual(responses[0]["result"]["serverInfo"]["name"], "Teem")
        tools = {t["name"]: t for t in responses[1]["result"]["tools"]}
        self.assertEqual(set(tools), {"foreman_submit", "foreman_status", "foreman_ack", "foreman_plan", "foreman_workflow"})
        self.assertNotIn("foreman", tools["foreman_submit"]["inputSchema"]["properties"])
        workflow_args = tools['foreman_workflow']['inputSchema']['properties']
        self.assertEqual(set(workflow_args), {'workflow_id', 'project', 'scope', 'prompt', 'title', 'workers'})

    def test_plan_tool_is_bound_and_returns_supervisor_progress(self):
        response, = self.exchange({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {
            "name": "foreman_plan", "arguments": {"items": [{"id": "check", "content": "Check results", "status": "pending"}]}}})
        data = json.loads(response["result"]["content"][0]["text"])
        self.assertEqual(data["todos"], [{"content": "Check results", "status": "pending"}])
        self.assertEqual(self.store.db.execute("SELECT foreman FROM plans").fetchone()["foreman"], "ses_foreman")
        response, = self.exchange({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {
            "name": "foreman_plan", "arguments": {"foreman": "ses_other", "items": []}}})
        self.assertTrue(response["result"]["isError"])

    def test_status_uses_fixed_foreman_binding(self):
        result, = self.exchange({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                                 "params": {"name": "foreman_status", "arguments": {}}})
        data = json.loads(result["result"]["content"][0]["text"])
        self.assertEqual(data["foreman"], "ses_foreman")
        self.assertEqual(data["limits"], {"worker": 2, "reviewer": 2})
        self.assertEqual(data["completion_transport"], "native-runtime")
        self.assertNotIn("notifications_configured", data)

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

    def test_binding_does_not_silently_replace_a_terminal(self):
        with self.assertRaises(Invalid):
            self.store.bind("ses_foreman", "term_other")

    def test_service_unit_preserves_explicit_paths_and_has_no_enable_side_effect(self):
        unit = service_unit(self.root / "space % config.toml")
        self.assertIn("Restart=on-failure", unit)
        self.assertIn("Description=Teem headless task supervisor", unit)
        self.assertIn("UMask=0077", unit)
        self.assertIn("space %% config.toml", unit)
        self.assertIn(" serve", unit)
        self.assertNotIn("--auto", unit)

    def test_manifest_has_status_popup_and_no_worker_panes(self):
        root = Path(__file__).resolve().parents[1]
        manifest = tomllib.loads((root / "herdr-plugin.toml").read_text())
        self.assertEqual(manifest["id"], "local.teem")
        self.assertEqual(manifest["name"], "Teem")
        action = next(a for a in manifest["actions"] if a["id"] == "status")
        self.assertEqual(action["command"][-1], "open-status")
        self.assertEqual([p["placement"] for p in manifest["panes"]], ["popup"])
        for entry in manifest["actions"] + manifest["startup"] + manifest["panes"]:
            self.assertEqual(entry["command"][0], "bin/teem")
        self.assertEqual(manifest["build"][0]["command"],
                         ["cargo", "run", "--locked", "--release", "--", "install-plugin"])

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
        result = subprocess.run([sys.executable, "-B", str(root / "tests/reference/foreman_plugin.py"),
                                 "--config", str(config), "serve"], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("disabled", result.stdout)


    def test_teem_imports_without_codr_modules(self):
        root = Path(__file__).resolve().parents[1]
        result = subprocess.run([sys.executable, "-B", "-c",
                                 "import sys; sys.modules['codr_foreman'] = None; "
                                 "import foreman_plugin, foreman_supervisor, foreman_runtime"],
                                cwd=root, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse((root / "codr_foreman.py").exists())

    def test_cli_no_longer_accepts_codr_notification_configuration(self):
        root = Path(__file__).resolve().parents[1]
        result = subprocess.run([sys.executable, "-B", str(root / "tests/reference/foreman_plugin.py"), "bind", "--help"],
                                capture_output=True, text=True, check=True)
        self.assertNotIn("notification-file", result.stdout)


if __name__ == "__main__":
    unittest.main()
