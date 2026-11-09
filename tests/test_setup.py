import json
from pathlib import Path
import unittest


class SetupTests(unittest.TestCase):
    def test_foreman_profile_is_project_local_and_cannot_implement(self):
        root = Path(__file__).resolve().parents[1]
        config = json.loads((root / "examples/pilot-foreman.json").read_text())
        self.assertEqual(config["$schema"], "https://opencode.ai/config.json")
        self.assertEqual(config["default_agent"], "teem_foreman")
        self.assertFalse(config["warming"])
        self.assertNotIn("model", config)
        self.assertNotIn("mcp", config)
        agent = config["agents"]["teem_foreman"]
        self.assertEqual(agent["mode"], "primary")
        self.assertLessEqual(agent["steps"], 12)
        self.assertEqual(agent["permissions"][0], {"action": "*", "resource": "*", "effect": "deny"})
        allowed = {r["action"] for r in agent["permissions"] if r["effect"] == "allow"}
        self.assertFalse(allowed & {"edit", "shell", "subagent", "opencode_session_move"})
        self.assertTrue({"teem_foreman_submit", "teem_foreman_status", "teem_foreman_ack"} <= allowed)
        self.assertIn("approval", agent["system"])


if __name__ == "__main__":
    unittest.main()
