import json
import subprocess
import unittest
from pathlib import Path

from codr_foreman import Client, DeliveryUnknown, Rejected

REQUEST = "00112233-4455-4677-8899-aabbccddeeff"


class ClientTests(unittest.TestCase):
    def client(self, result=None, *, failure=None):
        self.calls = []

        def run(argv, **kwargs):
            self.calls.append((argv, kwargs))
            if failure:
                raise failure
            request = json.loads(kwargs["input"])
            response = {"version": 1, "request_id": request["request_id"], "result": result}
            return subprocess.CompletedProcess(argv, 0, json.dumps(response), "")

        return Client("/unused/codr", Path("/unused/config.toml"), "aa" * 32,
                      '["herdr:codex","codex","id","foreman-session"]', "terminal-1", run=run)

    def test_submit_uses_local_queue_and_stable_request_id(self):
        client = self.client({"status": "queued"})
        self.assertEqual(client.submit(REQUEST, "Worker finished"), {"status": "queued"})
        argv, options = self.calls[0]
        self.assertEqual(argv, ["/unused/codr", "--config", "/unused/config.toml", "porter"])
        request = json.loads(options["input"])
        self.assertEqual(request["method"], "enqueue")
        self.assertEqual(request["request_id"], REQUEST)
        self.assertEqual(request["terminal_id"], "terminal-1")
        self.assertEqual(request["text"], "Worker finished")
        self.assertNotIn("Worker finished", argv)
        self.assertEqual(options["stderr"], subprocess.DEVNULL)

    def test_receipt_reports_acceptance_only(self):
        client = self.client({"accepted": False})
        self.assertEqual(client.receipt(REQUEST), {"accepted": False})
        self.assertEqual(json.loads(self.calls[0][1]["input"])["method"], "receipt")

    def test_duplicate_acceptance_is_not_execution(self):
        client = self.client({"status": "already_accepted"})
        self.assertEqual(client.submit(REQUEST, "Wake"), {"status": "already_accepted"})

    def test_timeout_is_uncertain_and_never_retried(self):
        client = self.client(failure=subprocess.TimeoutExpired("codr", 30))
        with self.assertRaises(DeliveryUnknown):
            client.submit(REQUEST, "Wake")
        self.assertEqual(len(self.calls), 1)

    def test_invalid_request_is_rejected_without_transport(self):
        for request_id, text in [("bad", "Wake"), (REQUEST, " "), (REQUEST, "é" * 15001)]:
            client = self.client({"status": "queued"})
            with self.assertRaises(Rejected):
                client.submit(request_id, text)
            self.assertEqual(self.calls, [])

    def test_wrong_or_malformed_response_cannot_confirm_acceptance(self):
        for value in ["invalid JSON", {"version": 1, "request_id": "different", "result": {"status": "queued"}},
                      {"version": 1, "request_id": REQUEST, "result": {"status": "executed"}}]:
            client = self.client()
            client.run = lambda argv, **kwargs: subprocess.CompletedProcess(
                argv, 0, value if isinstance(value, str) else json.dumps(value))
            with self.assertRaises(DeliveryUnknown):
                client.submit(REQUEST, "Wake")

    def test_process_failure_is_uncertain_not_safe_to_resend(self):
        client = self.client()
        client.run = lambda argv, **kwargs: subprocess.CompletedProcess(argv, 1, "private details")
        with self.assertRaises(DeliveryUnknown) as error:
            client.submit(REQUEST, "Wake")
        self.assertNotIn("private details", str(error.exception))

    def test_explicit_rejection_is_not_reported_as_acceptance(self):
        client = self.client()
        client.run = lambda argv, **kwargs: subprocess.CompletedProcess(argv, 0, json.dumps({
            "version": 1, "request_id": REQUEST,
            "error": {"code": "porter_request_rejected", "message": "private details"}
        }))
        with self.assertRaises(Rejected) as error:
            client.submit(REQUEST, "Wake")
        self.assertNotIn("private details", str(error.exception))

    def test_malformed_receipt_is_unknown_not_false(self):
        for result in [{}, {"accepted": "false"}, {"accepted": 0}, []]:
            client = self.client(result)
            with self.assertRaises(DeliveryUnknown):
                client.receipt(REQUEST)


if __name__ == "__main__":
    unittest.main()
