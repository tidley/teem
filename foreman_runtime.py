"""Headless OpenCode current-API transport. Never starts or replaces a server."""

import base64
import json
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, *_):
        return None


class Missing(OSError):
    """The exact native resource does not exist."""


def task_permissions(task):
    rules = [{"action": "*", "resource": "*", "effect": "deny"}]
    rules.extend({"action": action, "resource": "*", "effect": "allow"}
                 for action in ("read", "glob", "grep"))
    if task["role"] == "worker":
        for path in task["request"]["scope"]:
            rules.extend({"action": "edit", "resource": resource, "effect": "allow"}
                         for resource in (path, path + "/**"))
        rules.append({"action": "shell", "resource": "*", "effect": "ask"})
    # These last-match rules also protect nested Git metadata within an edit scope.
    rules.extend({"action": "edit", "resource": resource, "effect": "deny"}
                 for resource in (".git", ".git/**", "**/.git", "**/.git/**"))
    return rules


class OpenCode:
    def __init__(self, registry, model):
        try:
            record = json.loads(Path(registry).expanduser().read_text())
            url = urlsplit(record["url"])
            if (url.scheme != "http" or url.hostname not in ("127.0.0.1", "::1")
                    or not url.port or url.username or url.password
                    or url.path not in ("", "/") or url.query or url.fragment
                    or not isinstance(record.get("password"), str) or not record["password"]):
                raise ValueError()
            provider, name = model.split("/", 1)
            name, separator, variant = name.partition("#")
            if not provider or not name or (separator and not variant):
                raise ValueError()
        except (OSError, ValueError, KeyError, TypeError, AttributeError):
            raise ValueError("Use a local OpenCode service registry with credentials and an explicit provider/model") from None
        self.url = record["url"].rstrip("/")
        self.auth = "Basic " + base64.b64encode(("opencode:" + record["password"]).encode()).decode()
        self.model = {"providerID": provider, "id": name}
        if separator:
            self.model["variant"] = variant
        # Do not send local credentials through environment-configured proxies.
        self.opener = build_opener(ProxyHandler({}), NoRedirect())

    def request(self, method, path, payload=None):
        data = None if payload is None else json.dumps(payload).encode()
        request = Request(self.url + path, data=data, method=method,
                          headers={"Authorization": self.auth, "Content-Type": "application/json"})
        try:
            with self.opener.open(request, timeout=10) as response:
                raw = response.read(1048577)
            if len(raw) > 1048576:
                raise ValueError()
            decoded = json.loads(raw)
            return decoded["data"]
        except HTTPError as error:
            if error.code == 404:
                raise Missing("Native resource not found") from None
            raise OSError("OpenCode rejected the request. Inspect native state before retrying") from None
        except (URLError, TimeoutError, ValueError, KeyError, TypeError):
            raise OSError("OpenCode response unavailable or invalid. Acceptance is not confirmed") from None

    def session_payload(self, task):
        return dict(id=task["session_id"], title=f"Foreman {task['role']} {task['id']}",
                    agent="build", model=self.model, location={"directory": task["checkout"]},
                    permissions=task_permissions(task), metadata={"herdr_foreman": {
                        "task_id": task["id"], "role": task["role"],
                        "revision": task["request"]["revision"], "scope": task["request"]["scope"]}})

    def validate_session(self, task, session):
        expected = self.session_payload(task)
        if (not isinstance(session, dict)
                or any(session.get(key) != expected[key]
                       for key in ("id", "location", "permissions", "model", "agent"))
                or session.get("metadata", {}).get("herdr_foreman") != expected["metadata"]["herdr_foreman"]):
            raise OSError("Native session does not match the reserved task identity or permissions")

    def create(self, task):
        self.validate_session(task, self.request("POST", "/api/session", self.session_payload(task)))

    def prompt_text(self, task):
        scope = ", ".join(task["request"]["scope"])
        return (f"Assigned {task['role']} task {task['id']}. Base commit: {task['request']['revision']}.\n"
                f"File scope: {scope}. Sparse checkout: {task['sparse']}.\n"
                "Do not delegate, merge, publish, deploy, or change runtime permissions. "
                "Report results, checks, and unresolved limitations. Execution success is not review approval.\n\n"
                + task["request"]["prompt"])

    def prompt(self, task):
        response = self.request("POST", f"/api/session/{task['session_id']}/prompt", {
            "id": task["prompt_id"], "text": self.prompt_text(task), "delivery": "queue", "resume": True})
        if not isinstance(response, dict) or response.get("id") != task["prompt_id"]:
            raise OSError("Native prompt admission did not return the expected input ID")

    def inspect(self, task):
        prefix = f"/api/session/{task['session_id']}"
        session = self.request("GET", prefix)
        self.validate_session(task, session)
        inbox = self.request("GET", prefix + "/inbox")
        if not isinstance(inbox, list):
            raise OSError("Invalid native inbox")
        for item in inbox:
            if item.get("id") == task["prompt_id"]:
                if item.get("type") != "user" or item.get("payload", {}).get("text") != self.prompt_text(task):
                    raise OSError("Native inbox payload does not match the assigned task")
                return {"state": "accepted", "detail": "Native input queued. Execution not yet confirmed"}
        try:
            prompt = self.request("GET", prefix + "/message/" + task["prompt_id"])
        except Missing:
            return {"state": "uncertain", "detail": "No matching native input. Do not replay automatically"}
        if (prompt.get("id") != task["prompt_id"] or prompt.get("type") != "user"
                or prompt.get("text") != self.prompt_text(task)):
            raise OSError("Native message does not match the assigned task")
        active = self.request("GET", "/api/session/active")
        if not isinstance(active, dict):
            raise OSError("Invalid native activity response")
        if task["session_id"] in active:
            permissions = self.request("GET", prefix + "/permission")
            forms = self.request("GET", prefix + "/form")
            return {"state": "running", "detail":
                    "Requires human permission or answer. Inspect the native session" if permissions or forms else "Native execution active"}
        idle = session.get("time", {}).get("idle", 0)
        if idle <= prompt["time"]["created"] or session.get("outcome") not in ("succeeded", "failed", "interrupted"):
            return {"state": "uncertain", "detail": "No terminal outcome after this input. Reservation retained"}
        if session["outcome"] != "succeeded":
            return {"state": session["outcome"], "detail": "Native execution did not succeed"}
        messages = self.request("GET", prefix + "/message?limit=20&order=desc")
        for message in messages:
            if (message.get("type") != "assistant" or message.get("error")
                    or message.get("finish") == "tool-call"
                    or not prompt["time"]["created"] <= message.get("time", {}).get("created", 0) <= idle
                    or not message.get("time", {}).get("completed")):
                continue
            result = "\n".join(part["text"] for part in message.get("content", []) if part.get("type") == "text")
            if result.strip():
                return {"state": "succeeded", "result": result[:100000]}
        return {"state": "uncertain", "detail": "Native success has no final text result in the latest 20 messages"}
