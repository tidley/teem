"""Local Foreman wake admission through Codr, never direct terminal input."""

import argparse
import json
from pathlib import Path
import subprocess
import sys
import uuid


class Rejected(Exception):
    """The request is invalid or Codr returned an explicit rejection."""


class DeliveryUnknown(Exception):
    """Acceptance is not confirmed. Do not automatically resend."""


class Client:
    def __init__(self, codr, config, bridge_account, session_key, terminal_id, *, run=subprocess.run):
        if (not codr or not Path(config).is_absolute() or not session_key or not terminal_id
                or len(bridge_account) != 64
                or any(char not in "0123456789abcdef" for char in bridge_account)):
            raise Rejected("Use an absolute configuration path and an explicit Foreman binding")
        self.command = [str(codr), "--config", str(config), "porter"]
        self.binding = dict(bridge_account=bridge_account, session_key=session_key,
                            terminal_id=terminal_id)
        self.run = run

    def submit(self, request_id, text):
        if not text.strip() or len(text.encode("utf-8")) > 30000:
            raise Rejected("Wake text must contain 1 to 30000 UTF-8 bytes")
        return self._exchange("enqueue", request_id, text=text)

    def receipt(self, request_id):
        return self._exchange("receipt", request_id)

    def _exchange(self, method, request_id, **fields):
        try:
            if str(uuid.UUID(request_id)) != request_id:
                raise ValueError()
        except (ValueError, TypeError, AttributeError):
            raise Rejected("Use a canonical request UUID retained with the wake event") from None
        request = dict(version=1, method=method, request_id=request_id, **self.binding, **fields)
        try:
            completed = self.run(self.command, input=json.dumps(request) + "\n", text=True,
                                 stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=30)
        except (OSError, subprocess.SubprocessError):
            raise DeliveryUnknown("Codr acceptance is unknown; inspect or query the same request UUID") from None
        if completed.returncode != 0:
            raise DeliveryUnknown("Codr acceptance is unknown; inspect or query the same request UUID")
        try:
            response = json.loads(completed.stdout)
            if (response.get("version") != 1 or response.get("request_id") != request_id):
                raise ValueError()
            if "error" in response:
                raise Rejected("Codr rejected the request; no direct-terminal fallback was attempted")
            result = response["result"]
            if method == "receipt":
                if type(result.get("accepted")) is not bool:
                    raise ValueError()
                return {"accepted": result["accepted"]}
            if result.get("status") not in ("queued", "already_accepted"):
                raise ValueError()
            return {"status": result["status"]}
        except (ValueError, KeyError, TypeError, AttributeError):
            raise DeliveryUnknown("Invalid Codr response; acceptance is unknown") from None


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--codr", default="herdr-white-noise-bridge")
    parser.add_argument("--config", type=Path, required=True)
    parser.add_argument("--bridge-account", required=True)
    parser.add_argument("--session-key", required=True)
    parser.add_argument("--terminal-id", required=True)
    parser.add_argument("--request-id", required=True)
    commands = parser.add_subparsers(dest="operation", required=True)
    submit = commands.add_parser("submit", help="Enqueue one wake; does not prove execution")
    submit.add_argument("--text-file", type=Path, required=True)
    commands.add_parser("receipt", help="Query acceptance, not execution")
    args = parser.parse_args(argv)
    try:
        client = Client(args.codr, args.config, args.bridge_account, args.session_key, args.terminal_id)
        if args.operation == "submit":
            try:
                with args.text_file.open("rb") as stream:
                    text = stream.read(30001).decode("utf-8")
            except (OSError, UnicodeError):
                raise Rejected("Wake text file is unreadable or not UTF-8") from None
            result = client.submit(args.request_id, text)
        else:
            result = client.receipt(args.request_id)
    except Rejected as error:
        print(str(error), file=sys.stderr)
        return 2
    except DeliveryUnknown as error:
        print(str(error), file=sys.stderr)
        return 3
    print(json.dumps(result))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
