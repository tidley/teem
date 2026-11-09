"""Teem Herdr plugin commands and fixed-binding Foreman tools."""

import argparse
import fcntl
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
import tomllib

from foreman_runtime import OpenCode
from foreman_approvals import ApprovalRelay
from foreman_progress import ProgressPublisher
from foreman_workflows import WorkflowEngine, normalise_projects
from foreman_checks import CheckRunner
from foreman_agent_progress import AgentTodoReader
from foreman_supervisor import Invalid, Store, Supervisor, Notifier


def load_config(path):
    with Path(path).expanduser().open("rb") as stream:
        config = tomllib.load(stream)
    if type(config.get("enabled", False)) is not bool:
        raise Invalid("Configuration enabled must be true or false")
    config.setdefault("enabled", False)
    if not isinstance(config.get("state_dir"), str):
        raise Invalid("Configuration must select a private absolute state_dir")
    config["state_dir"] = str(Path(config["state_dir"]).expanduser())
    if not Path(config["state_dir"]).is_absolute():
        raise Invalid("state_dir must be absolute")
    paths = config.setdefault("allowed_checkouts", [])
    if not isinstance(paths, list) or any(not isinstance(p, str) or not Path(p).expanduser().is_absolute() for p in paths):
        raise Invalid("List each authorised checkout root explicitly")
    if config["enabled"] and (not isinstance(config.get("registry"), str) or not config.get("model")):
        raise Invalid("Enabled supervision needs an explicit registry path and model")
    config['projects'] = normalise_projects(config.get('projects', {}))
    return config


def bind_from_herdr(store, *, run=subprocess.run):
    pane = os.environ.get("HERDR_PANE_ID")
    if os.environ.get("HERDR_ENV") != "1" or not pane:
        raise Invalid("Bind from the focused Foreman pane inside Herdr")
    try:
        response = run([os.environ.get("HERDR_BIN_PATH", "herdr"), "agent", "get", pane],
                       text=True, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=10)
        if response.returncode:
            raise ValueError()
        agent = json.loads(response.stdout)["result"]["agent"]
        native = agent["agent_session"]
        if (agent["agent"] != "opencode" or native["agent"] != "opencode"
                or native["kind"] != "id" or native["source"] != "herdr:opencode"):
            raise ValueError()
        store.bind(native["value"], agent["terminal_id"])
        return {"foreman": native["value"], "terminal_id": agent["terminal_id"],
                "completion_transport": "native-runtime"}
    except (OSError, subprocess.SubprocessError, ValueError, KeyError, TypeError):
        raise Invalid("Cannot bind the focused pane to an exact native OpenCode session") from None


def open_status(*, run=subprocess.run):
    if os.environ.get("HERDR_ENV") != "1":
        raise Invalid("Open task status from inside Herdr")
    return run([os.environ.get("HERDR_BIN_PATH", "herdr"), "plugin", "pane", "open",
                "--plugin", "local.teem", "--entrypoint", "status"], check=False).returncode


def status(store, foreman, task_id=None, workflow_id=None):
    binding = store.binding(foreman)
    tasks = store.tasks(foreman)
    workflows = store.workflows(foreman)
    if task_id and workflow_id:
        raise Invalid('Select a task or a workflow, not both')
    if workflow_id:
        workflow = store.workflow(workflow_id)
        if workflow['foreman'] != foreman:
            raise Invalid('Workflow does not belong to this Foreman')
        workflows = [workflow]
        tasks = [t for t in tasks if t['request'].get('workflow_id') == workflow_id]
    if task_id:
        task = store.task(task_id)
        if task["foreman"] != foreman:
            raise Invalid("Task does not belong to this Foreman")
        tasks = [task]
    return {"foreman": foreman, "terminal_id": binding["terminal_id"],
            "limits": {"worker": 2, "reviewer": 2}, "limit_scope": "per managed task; legacy tasks share two slots per role",
            "completion_transport": "native-runtime",
            "tasks": tasks[-100:], "events": store.events(foreman),
            "workflows": workflows[-100:],
            "projects": store.projects,
            "agent_progress": [dict(row) for row in store.db.execute(
                'SELECT agent_todos.* FROM agent_todos JOIN tasks ON tasks.id=agent_todos.task_id WHERE tasks.foreman=?', (foreman,))],
            "approvals": [row for row in store.approvals(foreman) if not task_id or row["task_id"] == task_id],
            "progress": store.progress(foreman),
            "progress_delivery": [dict(row) for row in store.db.execute(
                "SELECT seq,state,detail FROM progress_events WHERE foreman=? ORDER BY seq DESC LIMIT 1", (foreman,))]}


def tools():
    properties = {"task_id": {"type": "string", "description": "Retain one canonical UUID for this immutable request"},
                  "role": {"type": "string", "enum": ["worker", "reviewer"]},
                  "checkout": {"type": "string", "description": "An explicitly authorised absolute Git checkout root"},
                  "revision": {"type": "string", "description": "Full Git commit ID matching checkout HEAD"},
                  "scope": {"type": "array", "items": {"type": "string"}, "minItems": 1},
                  "prompt": {"type": "string", "description": "Bounded assignment, acceptance criteria, and required checks"}}
    return [
        {"name": "foreman_submit", "description":
         "Queue one legacy headless task and return immediately. Legacy tasks share two slots per role; managed tasks have independent teams. "
         "Queue acceptance is not execution. Reviews require a clean, complete checkout at a fixed commit. "
         "Workers may use sparse checkouts, whose tests cannot qualify integration. Never retry with a new UUID after an uncertain response.",
         "inputSchema": {"type": "object", "properties": properties, "required": list(properties), "additionalProperties": False}},
        {"name": "foreman_status", "description":
         "Read this Foreman's task state, results, and unacknowledged completion events. "
         "Worker permissions appear as native cards here for the human to Approve once or Reject worker batch. "
         "Native rejection declines all pending permissions in that worker session. "
         "Do not answer permission cards or interpret conversational consent as a permission reply. "
         "Success means native execution ended, not that review or integration checks passed.",
          "inputSchema": {"type": "object", "properties": {"task_id": {"type": "string"}, "workflow_id": {"type": "string"}}, "additionalProperties": False}},
        {"name": "foreman_ack", "description": "Acknowledge one completion event after reading its result. Does not approve or merge code.",
          "inputSchema": {"type": "object", "properties": {"event_id": {"type": "string"}}, "required": ["event_id"], "additionalProperties": False}},
        {"name": "foreman_plan", "description":
         "Define the current ordered checklist for this Foreman. Replace old completed work, preserving unrelated active work. "
         "Link execution items to retained task UUIDs before submission; the supervisor owns their status. "
         "Unlinked planning/verification items need an explicit status, based on evidence, not worker success. "
         "The passive Teem TUI displays supervisor progress. Do not copy it to native todowrite or poll to render it. "
         "No worker is dispatched and no Foreman turn is triggered by progress changes.",
         "inputSchema": {"type": "object", "properties": {"items": {"type": "array", "maxItems": 100, "items": {
             "type": "object", "properties": {"id": {"type": "string", "description": "Stable short plan item ID"},
                 "content": {"type": "string", "description": "Short single-line checklist label"},
                 "task_id": {"type": "string", "description": "Canonical UUID; do not supply status for linked execution"},
                 "status": {"type": "string", "enum": ["pending", "in_progress", "completed", "cancelled"]}},
              "required": ["id", "content"], "additionalProperties": False}}}, "required": ["items"], "additionalProperties": False}},
        {"name": "foreman_workflow", "description":
         "Delegate implementation, approved checks, two independent exact-snapshot reviews and bounded revisions. "
         "Use a project alias from foreman_status.projects and acceptance criteria. The supervisor chooses separate "
         "checkouts and base revisions; never ask the human for paths or hashes. No model override, arbitrary commands, "
         "commits, integration or publication. Optionally split a task into up to two workers with disjoint file scopes. "
         "Each task has its own two implementation and two review slots; additional tasks do not share them. "
         "Their combined candidate must pass checks and both reviews. Revisions use one worker on that candidate; "
         "stay within the user's authorised scope. Retain one UUID per immutable workflow, never replace uncertain work. "
         "The workflow appears automatically in passive progress. Do not call todowrite for Teem or wait/poll to update it. "
         "Finish your dispatch response so the human can keep talking. Progress and individual agent completions do not wake you. "
         "One final workflow outcome asks for a concise result summary, not checklist maintenance.",
         "inputSchema": {"type": "object", "properties": {
             "workflow_id": {"type": "string"}, "project": {"type": "string"},
             "title": {"type": "string", "description": "Short human-readable task title; no workflow codes", "maxLength": 100},
              "scope": {"type": "array", "items": {"type": "string"}, "minItems": 1},
              "workers": {"type": "array", "minItems": 1, "maxItems": 2, "items": {
                  "type": "object", "properties": {
                      "scope": {"type": "array", "items": {"type": "string"}, "minItems": 1},
                      "prompt": {"type": "string"}}, "required": ["scope", "prompt"], "additionalProperties": False}},
             "prompt": {"type": "string"}}, "required": ["workflow_id", "project", "scope", "prompt"], "additionalProperties": False}},
    ]


def call_tool(store, foreman, name, arguments):
    definitions = {tool["name"]: tool for tool in tools()}
    if name not in definitions or not isinstance(arguments, dict):
        raise Invalid("Unknown tool or invalid arguments")
    schema = definitions[name]["inputSchema"]
    if set(arguments) - set(schema["properties"]) or set(schema.get("required", [])) - set(arguments):
        raise Invalid("Use exactly the documented tool arguments")
    if name == "foreman_submit":
        return store.submit(foreman=foreman, **arguments)
    if name == "foreman_status":
        return status(store, foreman, **arguments)
    if name == "foreman_plan":
        return store.set_plan(foreman, arguments["items"])
    if name == 'foreman_workflow':
        return store.submit_workflow(foreman, **arguments)
    store.ack(foreman, arguments["event_id"])
    return {"acknowledged": True}


def mcp(store, foreman, source=sys.stdin, sink=sys.stdout):
    store.binding(foreman)
    while line := source.readline(65537):
        request_id = None
        try:
            if len(line.encode()) > 65536:
                raise ValueError()
            request = json.loads(line)
            if not isinstance(request, dict):
                raise ValueError()
            request_id = request.get("id")
            if "id" not in request:
                continue
            method = request.get("method")
            params = request.get("params", {})
            if not isinstance(params, dict):
                raise Invalid("Parameters must be an object")
            if method == "initialize":
                version = params.get("protocolVersion")
                result = {"protocolVersion": version if version in (
                    "2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25") else "2024-11-05",
                          "capabilities": {"tools": {}}, "serverInfo": {"name": "Teem", "version": "0.1.0"}}
            elif method == "ping":
                result = {}
            elif method == "tools/list":
                result = {"tools": tools()}
            elif method == "tools/call":
                try:
                    data = call_tool(store, foreman, params.get("name"), params.get("arguments", {}))
                    result = {"content": [{"type": "text", "text": json.dumps(data)}]}
                except (Invalid, TypeError, KeyError) as error:
                    result = {"isError": True, "content": [{"type": "text", "text": str(error)}]}
            else:
                sink.write(json.dumps({"jsonrpc": "2.0", "id": request_id,
                                       "error": {"code": -32601, "message": "Method not found"}}) + "\n")
                sink.flush()
                continue
            response = {"jsonrpc": "2.0", "id": request_id, "result": result}
        except (ValueError, TypeError) as error:
            response = {"jsonrpc": "2.0", "id": request_id, "error": {
                "code": -32602 if isinstance(error, Invalid) else -32700,
                "message": str(error) if isinstance(error, Invalid) else "Invalid JSON request"}}
        sink.write(json.dumps(response) + "\n")
        sink.flush()


def service_unit(config):
    def quote(value):
        value = str(value)
        if any(char in value for char in "\n\r\0"):
            raise Invalid("Service paths cannot contain line breaks")
        return '"' + value.replace("%", "%%").replace("\\", "\\\\").replace('"', '\\"') + '"'
    command = " ".join(quote(value) for value in (
        sys.executable, "-B", Path(__file__).resolve(), "--config", Path(config).expanduser().resolve()))
    return ("[Unit]\nDescription=Teem headless task supervisor\nAfter=network-online.target\n\n"
            "[Service]\nType=simple\nExecStart=" + command + " serve\nRestart=on-failure\nRestartSec=5\n"
            "UMask=0077\nTimeoutStopSec=20\n\n[Install]\nWantedBy=default.target\n")


def serve(config):
    if not config["enabled"]:
        print("Foreman supervision is disabled")
        return
    with Store(config["state_dir"], config["allowed_checkouts"], projects=config.get('projects')) as store:
        with (store.directory / "supervisor.lock").open("a") as lock:
            os.chmod(lock.name, 0o600)
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                raise Invalid("Another supervisor owns this state directory") from None
            stopping = False
            def stop(*_):
                nonlocal stopping
                stopping = True
            signal.signal(signal.SIGTERM, stop)
            signal.signal(signal.SIGINT, stop)
            checks = CheckRunner()
            while not stopping:
                try:
                    checks.reap()
                    runtime = OpenCode(config["registry"], config["model"])
                    WorkflowEngine(store, runtime, checks).tick()
                    Supervisor(store, runtime, Notifier(runtime)).tick()
                    ApprovalRelay(runtime).deliver(store)
                    AgentTodoReader(runtime).deliver(store)
                    ProgressPublisher(runtime).deliver(store)
                except (OSError, ValueError):
                    print("Foreman runtime unavailable. Reservations retained", file=sys.stderr, flush=True)
                for _ in range(20):
                    if stopping:
                        break
                    time.sleep(0.1)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    default = Path(os.environ.get("HERDR_PLUGIN_CONFIG_DIR",
                   str(Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")) / "teem"))) / "config.toml"
    parser.add_argument("--config", type=Path, default=default)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("bind", help="Register the focused native Foreman. No agent execution")
    for name in ("status", "mcp"):
        command = commands.add_parser(name)
        command.add_argument("--foreman", required=True)
    commands.add_parser("board")
    commands.add_parser("open-status")
    commands.add_parser("serve")
    commands.add_parser("service-start")
    commands.add_parser("service-unit", help="Print a unit for explicit installation. Does not install or start it")
    args = parser.parse_args(argv)
    try:
        if args.command == "open-status":
            return open_status()
        if args.command == "service-unit":
            print(service_unit(args.config), end="")
            return 0
        if args.command == "service-start" and not args.config.expanduser().exists():
            print("Foreman is not configured. No service was started")
            return 0
        config = load_config(args.config)
        if args.command == "serve":
            serve(config)
            return 0
        if args.command == "service-start":
            if not config["enabled"]:
                print("Foreman supervision is disabled")
                return 0
            return subprocess.run(["systemctl", "--user", "start", "teem.service"], check=False).returncode
        with Store(config["state_dir"], config["allowed_checkouts"], projects=config.get('projects')) as store:
            if args.command == "bind":
                print(json.dumps(bind_from_herdr(store)))
            elif args.command == "mcp":
                mcp(store, args.foreman)
            elif args.command == "status":
                print(json.dumps(status(store, args.foreman), indent=2))
            else:
                bindings = store.db.execute("SELECT id FROM foremen ORDER BY id").fetchall()
                print(json.dumps([status(store, b["id"]) for b in bindings], indent=2))
                if sys.stdin.isatty():
                    input("\nPress Enter to close. Worker sessions remain headless. ")
    except (OSError, ValueError, subprocess.SubprocessError, EOFError) as error:
        print(str(error), file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
