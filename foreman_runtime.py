"""Headless OpenCode current-API transport. Never starts or replaces a server."""

import base64
import json
from pathlib import Path
import uuid
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener

from foreman_supervisor import RuntimeFailure, Invalid, model_reference


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, *_):
        return None


class Missing(OSError):
    """The exact native resource does not exist."""


def task_permissions(task):
    rules = [{"action": "*", "resource": "*", "effect": "deny"}]
    rules.extend({"action": action, "resource": "*", "effect": "allow"}
                  for action in ("read", "glob", "grep"))
    if task['request'].get('native_todos') is True:
        rules.append({'action': 'todowrite', 'resource': '*', 'effect': 'allow'})
    if task["role"] == "worker":
        rules.append({"action": "external_directory", "resource": "/mnt/fastbuild/**", "effect": "allow"})
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
                if response.status == 204:
                    return None
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
        payload = dict(id=task["session_id"], title=f"Foreman {task['role']} {task['id']}",
                    agent="build", model=task.get("runtime_model") or self.model, location={"directory": task["checkout"]},
                    permissions=task_permissions(task), metadata={"herdr_foreman": {
                        "task_id": task["id"], "role": task["role"],
                        "revision": task["request"]["revision"], "scope": task["request"]["scope"]}})
        if task['request'].get('workflow_id'):
            payload['metadata']['herdr_foreman'].update(workflow_id=task['request']['workflow_id'], input_tree=task['request'].get('input_tree'))
            if task['request'].get('native_todos') is True:
                payload['metadata']['herdr_foreman']['native_todos'] = True
        return payload

    def foreman_directory(self, foreman):
        session = self.request('GET', f'/api/session/{foreman}')
        if (not isinstance(session, dict) or session.get('id') != foreman
                or not isinstance(session.get('location'), dict)
                or not isinstance(session['location'].get('directory'), str)
                or not Path(session['location']['directory']).is_absolute()):
            raise RuntimeFailure('Exact native Foreman location is unavailable')
        return str(Path(session['location']['directory']).resolve())

    def dispatch_model(self, task):
        session = self.request("GET", f"/api/session/{task['foreman']}")
        try:
            if not isinstance(session, dict) or session.get("id") != task["foreman"]:
                raise Invalid("Foreman session mismatch")
            return model_reference(session.get("model"))
        except Invalid:
            raise RuntimeFailure("Cannot read an explicit model from the exact bound Foreman session") from None

    def validate_session(self, task, session):
        expected = self.session_payload(task)
        if not isinstance(session, dict):
            raise RuntimeFailure("Native session information is invalid")
        for key in ("id", "location", "permissions", "agent"):
            if session.get(key) != expected[key]:
                raise RuntimeFailure(f"Native session {key} mismatch")
        model = session.get("model")
        # OpenCode records an omitted variant as the explicit canonical value "default".
        if (not isinstance(model, dict)
                or {**model, "variant": model.get("variant", "default")} !=
                   {**expected["model"], "variant": expected["model"].get("variant", "default")}):
            raise RuntimeFailure("Native session model mismatch")
        metadata = session.get("metadata")
        if (not isinstance(metadata, dict)
                or metadata.get("herdr_foreman") != expected["metadata"]["herdr_foreman"]):
            raise RuntimeFailure("Native session metadata mismatch")

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

    def event_payload(self, event):
        if event.get('workflow_id'):
            return {
                'id': 'msg_teem_workflow_' + uuid.UUID(event['request_id']).hex,
                'text': (f"Teem workflow outcome event {event['id']} for workflow {event['workflow_id']}. "
                         'Read foreman_status with this workflow_id and give the human one concise result summary: '
                         'task title, completed or blocked/needs attention, recorded check/review outcomes, and any limitations. '
                         'Ready means the exact candidate passed its configured checks and both reviews, not integrated. '
                         'Then foreman_ack this event after consuming it. Do not refresh the checklist, dispatch agents, '
                         'replay work, approve permissions, integrate, merge or publish. '
                         'Progress is displayed independently; this is the final result notification, not a progress update.'),
                'description': 'Teem workflow finished',
                'metadata': {'teem_event': event['id'], 'teem_workflow': event['workflow_id']},
                'delivery': 'queue', 'resume': True,
            }
        if event.get("kind") == "progress":
            return {
                "id": "msg_teem_progress_" + uuid.UUID(event["request_id"]).hex,
                "text": (f"Teem progress revision {event['revision']}. Read foreman_status now and use its progress.todos "
                         "to refresh the native todowrite checklist with a real tool call. Use current status, not this "
                         "event's age. Preserve unrelated planning items through foreman_plan. Do not dispatch work, "
                         "replay tasks, approve permissions, or claim tests/review passed merely to handle this event. "
                         "For newly terminal managed workflows, report their recorded candidate/gate evidence once; ready is not integrated. "
                         "No chat reply is needed just to announce a checklist refresh."),
                "description": "Teem progress changed",
                "metadata": {"teem_progress": event["id"], "teem_revision": event["revision"]},
                "delivery": "queue", "resume": True,
            }
        return {
            "id": "msg_teem_event_" + uuid.UUID(event["request_id"]).hex,
            "text": (f"Teem completion event {event['id']} for task {event['task_id']}. "
                      "Use foreman_status to read the recorded result, then foreman_ack after consuming this event. "
                      + ("If status has progress.todos, refresh the native todowrite checklist with that current snapshot. "
                         if event.get("refresh_todos") else "")
                      + "This event is not engineering approval or permission to merge."),
            "description": "Teem task completed",
            "metadata": {"teem_event": event["id"], "teem_task": event["task_id"]},
            "delivery": "queue", "resume": True,
        }

    def validate_event(self, event, message, *, queued):
        expected = self.event_payload(event)
        body = message.get("payload") if queued and isinstance(message, dict) else message
        if (not isinstance(message, dict) or not isinstance(body, dict)
                or message.get("id") != expected["id"] or message.get("type") != "synthetic"
                or (queued and message.get("sessionID") != event["foreman"])
                or body.get("text") != expected["text"] or body.get("metadata") != expected["metadata"]):
            raise OSError("Native completion input does not match the expected Foreman event")

    def notify(self, event):
        response = self.request("POST", f"/api/session/{event['foreman']}/synthetic", self.event_payload(event))
        self.validate_event(event, response, queued=True)

    def event_accepted(self, event):
        prefix = f"/api/session/{event['foreman']}"
        expected = self.event_payload(event)
        try:
            message = self.request("GET", prefix + "/message/" + expected["id"])
        except Missing:
            inbox = self.request("GET", prefix + "/inbox")
            if not isinstance(inbox, list) or any(not isinstance(item, dict) for item in inbox):
                raise OSError("Invalid native Foreman inbox")
            for item in inbox:
                if item.get("id") == expected["id"]:
                    self.validate_event(event, item, queued=True)
                    return True
            return False
        self.validate_event(event, message, queued=False)
        return True

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
                    "Requires human permission or answer. Check Foreman approval cards and approval status; "
                    "inspect the native worker session for other questions" if permissions or forms else "Native execution active"}
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

    def pending_approvals(self, task):
        # inspect also verifies the original task input, not only the session name.
        if self.inspect(task)["state"] != "running":
            return []
        prefix = f"/api/session/{task['session_id']}"
        permissions = self.request("GET", prefix + "/permission")
        if not isinstance(permissions, list):
            raise RuntimeFailure("Invalid native permission list")
        result = []
        for permission in permissions:
            if (not isinstance(permission, dict) or permission.get("sessionID") != task["session_id"]
                    or not isinstance(permission.get("id"), str)
                    or not permission["id"].startswith("per_")
                    or not permission["id"].replace("_", "").isalnum()
                    or not isinstance(permission.get("action"), str)
                    or not isinstance(permission.get("resources"), list)):
                raise RuntimeFailure("Invalid native permission identity")
            source = permission.get("source", {})
            if not isinstance(source, dict) or source.get("type") != "tool":
                raise RuntimeFailure("Native permission has no tool source; inspect it manually")
            message_id = source.get("messageID", "")
            if (not isinstance(message_id, str) or not message_id.startswith("msg_")
                    or not message_id.replace("_", "").isalnum()):
                raise RuntimeFailure("Invalid native permission source")
            message = self.request("GET", prefix + "/message/" + message_id)
            if (not isinstance(message, dict) or message.get("id") != message_id
                    or message.get("type") != "assistant" or not isinstance(message.get("content"), list)):
                raise RuntimeFailure("Invalid native permission tool message")
            parts = [part for part in message.get("content", [])
                     if isinstance(part, dict) and part.get("type") == "tool" and part.get("id") == source.get("id")]
            if (len(parts) != 1 or not isinstance(parts[0].get("state"), dict)
                    or not isinstance(parts[0]["state"].get("input"), dict)):
                raise RuntimeFailure("Native permission tool input unavailable")
            result.append({"permission": permission, "input": parts[0]["state"]["input"]})
        return result

    def read_todos(self, task):
        prefix = f"/api/session/{task['session_id']}"
        self.validate_session(task, self.request('GET', prefix))
        messages = self.request('GET', prefix + '/message?limit=50&order=desc&type=assistant')
        if not isinstance(messages, list):
            raise RuntimeFailure('Invalid native agent progress messages')
        for message in messages:
            if not isinstance(message, dict) or message.get('type') != 'assistant' or message.get('error'):
                continue
            content = message.get('content')
            if not isinstance(content, list):
                raise RuntimeFailure('Invalid native agent progress content')
            for part in reversed(content):
                if not isinstance(part, dict) or part.get('type') != 'tool' or part.get('name') != 'todowrite':
                    continue
                state = part.get('state')
                if not isinstance(state, dict) or state.get('status') != 'completed':
                    continue
                value = state.get('input')
                todos = value.get('todos') if isinstance(value, dict) else None
                if not isinstance(todos, list) or not isinstance(message.get('id'), str):
                    raise RuntimeFailure('Invalid native todo report')
                normalised = []
                for todo in todos[:100]:
                    if (not isinstance(todo, dict) or not isinstance(todo.get('content'), str)
                            or todo.get('status') not in ('pending', 'in_progress', 'completed', 'cancelled')):
                        raise RuntimeFailure('Invalid native todo entry')
                    normalised.append({'content': ' '.join(todo['content'].split())[:300], 'status': todo['status']})
                return {'message_id': message['id'], 'todos': normalised}
        return None
