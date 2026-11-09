"""Durable task admission and bounded scheduling. No terminal input or source Git mutations."""

import json
import os
from pathlib import Path, PurePosixPath
import re
import sqlite3
import subprocess
import time
import uuid


LIVE = ("starting", "accepted", "running", "uncertain")
TERMINAL = ("succeeded", "failed", "interrupted")


class Invalid(ValueError):
    """An explicit request or binding is invalid."""


class RuntimeFailure(OSError):
    """A sanitised runtime diagnostic safe to include in task status. Never include secrets."""


def model_reference(value):
    if (not isinstance(value, dict) or not {"providerID", "id"} <= value.keys()
            or set(value) - {"providerID", "id", "variant"}
            or any(not isinstance(v, str) or not v or len(v) > 300
                   or any(c in v for c in "\n\r\0") for v in value.values())):
        raise Invalid("Foreman must have an explicit provider, model, and optional variant")
    return dict(value)


def git(checkout, *args):
    # Inherited Git overrides must not redirect an assigned checkout.
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    try:
        return subprocess.check_output(
            ["git", "--no-optional-locks", "-C", str(checkout), *args],
            text=True, stderr=subprocess.DEVNULL, env=env, timeout=15).strip()
    except (OSError, subprocess.SubprocessError):
        raise Invalid("Cannot inspect the assigned Git checkout") from None


def checkout_info(checkout, revision, role):
    if git(checkout, "rev-parse", "--show-toplevel") != str(checkout):
        raise Invalid("Assign a Git checkout root, not a subdirectory")
    if not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", revision):
        raise Invalid("Use the full immutable Git commit ID")
    if git(checkout, "rev-parse", "HEAD") != revision:
        raise Invalid("Checkout HEAD does not match the requested revision")
    if git(checkout, "status", "--porcelain", "--untracked-files=all"):
        raise Invalid("Assign a clean checkout without unrelated changes")
    try:
        sparse = git(checkout, "config", "--bool", "core.sparseCheckout") == "true"
    except Invalid:
        sparse = False
    if role == "reviewer" and sparse:
        raise Invalid("Review requires a complete checkout, not sparse checkout")
    return sparse


def task_checkout_info(task):
    if task['request'].get('input_tree'):
        from foreman_checkouts import validate_candidate
        validate_candidate(task['checkout'], task['request']['revision'], task['request']['input_tree'])
        return False
    return checkout_info(task['checkout'], task['request']['revision'], task['role'])


class Store:
    def __init__(self, directory, allowed_checkouts, *, projects=None):
        self.directory = Path(directory).expanduser().resolve()
        self.directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        os.chmod(self.directory, 0o700)
        self.allowed = {str(Path(p).expanduser().resolve()) for p in allowed_checkouts}
        self.projects = projects or {}
        path = self.directory / "tasks.sqlite3"
        fd = os.open(path, os.O_CREAT | os.O_RDWR, 0o600)
        os.close(fd)
        os.chmod(path, 0o600)
        self.db = sqlite3.connect(path, timeout=15)
        self.db.row_factory = sqlite3.Row
        self.db.executescript("""
            CREATE TABLE IF NOT EXISTS foremen (
                id TEXT PRIMARY KEY, terminal_id TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY, foreman TEXT NOT NULL, role TEXT NOT NULL,
                checkout TEXT NOT NULL, state TEXT NOT NULL, request TEXT NOT NULL,
                session_id TEXT NOT NULL UNIQUE, prompt_id TEXT NOT NULL UNIQUE,
                sparse INTEGER NOT NULL, result TEXT NOT NULL DEFAULT '',
                detail TEXT NOT NULL DEFAULT '', created REAL NOT NULL);
            CREATE TABLE IF NOT EXISTS events (
                id TEXT PRIMARY KEY, task_id TEXT NOT NULL UNIQUE, foreman TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'pending', request_id TEXT NOT NULL UNIQUE,
                acked INTEGER NOT NULL DEFAULT 0, next_receipt REAL NOT NULL DEFAULT 0,
                transport TEXT NOT NULL DEFAULT 'native', detail TEXT NOT NULL DEFAULT '');
            CREATE TABLE IF NOT EXISTS approvals (
                task_id TEXT NOT NULL, permission_id TEXT NOT NULL, foreman TEXT NOT NULL,
                form_id TEXT NOT NULL UNIQUE, snapshot TEXT NOT NULL, payload TEXT NOT NULL,
                state TEXT NOT NULL, detail TEXT NOT NULL DEFAULT '',
                PRIMARY KEY (task_id, permission_id));
            CREATE TABLE IF NOT EXISTS plans (foreman TEXT PRIMARY KEY, items TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS progress_events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT, foreman TEXT NOT NULL,
                fingerprint TEXT NOT NULL, snapshot TEXT NOT NULL, request_id TEXT NOT NULL UNIQUE,
                state TEXT NOT NULL DEFAULT 'pending', sent_at REAL NOT NULL DEFAULT 0,
                next_receipt REAL NOT NULL DEFAULT 0, detail TEXT NOT NULL DEFAULT '');
            CREATE TABLE IF NOT EXISTS workflows (
                id TEXT PRIMARY KEY,foreman TEXT NOT NULL,project TEXT NOT NULL,
                request TEXT NOT NULL,policy TEXT NOT NULL,base_revision TEXT,
                state TEXT NOT NULL DEFAULT 'queued',round INTEGER NOT NULL DEFAULT 0,
                data TEXT NOT NULL DEFAULT '{"rounds":[]}',detail TEXT NOT NULL DEFAULT '',created REAL NOT NULL);
            CREATE TABLE IF NOT EXISTS managed_checkouts (path TEXT PRIMARY KEY,workflow_id TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS agent_todos (
                task_id TEXT PRIMARY KEY,todos TEXT,message_id TEXT,next_read REAL NOT NULL DEFAULT 0,
                final INTEGER NOT NULL DEFAULT 0,detail TEXT NOT NULL DEFAULT '');
        """)
        with self.db:
            self.db.execute("BEGIN IMMEDIATE")
            task_columns = {row["name"] for row in self.db.execute("PRAGMA table_info(tasks)")}
            if "runtime_model" not in task_columns:
                self.db.execute("ALTER TABLE tasks ADD COLUMN runtime_model TEXT")
            columns = {row["name"] for row in self.db.execute("PRAGMA table_info(events)")}
            if "transport" not in columns:
                # Existing attempts may already have reached Codr. Never replay them natively.
                self.db.execute("ALTER TABLE events ADD COLUMN transport TEXT NOT NULL DEFAULT 'legacy-codr'")
            if "detail" not in columns:
                self.db.execute("ALTER TABLE events ADD COLUMN detail TEXT NOT NULL DEFAULT ''")
            if "refresh_todos" not in columns:
                self.db.execute("ALTER TABLE events ADD COLUMN refresh_todos INTEGER NOT NULL DEFAULT 0")
            if "workflow_id" not in columns:
                self.db.execute("ALTER TABLE events ADD COLUMN workflow_id TEXT")
            columns = {row["name"] for row in self.db.execute("PRAGMA table_info(approvals)")}
            if "reply_attempted" not in columns:
                self.db.execute("ALTER TABLE approvals ADD COLUMN reply_attempted INTEGER NOT NULL DEFAULT 0")
                # Old uncertain rows do not distinguish lost UI from a lost permission reply.
                self.db.execute("UPDATE approvals SET reply_attempted=1 WHERE state IN ('replying','uncertain','done')")
            if "next_restore" not in columns:
                self.db.execute("ALTER TABLE approvals ADD COLUMN next_restore REAL NOT NULL DEFAULT 0")

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()

    def close(self):
        self.db.close()

    def bind(self, foreman, terminal_id):
        if not re.fullmatch(r"ses_[A-Za-z0-9_-]+", foreman) or not terminal_id:
            raise Invalid("Use an exact native session and terminal ID")
        with self.db:
            old = self.db.execute("SELECT * FROM foremen WHERE id=?", (foreman,)).fetchone()
            if old and old["terminal_id"] != terminal_id:
                raise Invalid("Foreman terminal changed. Inspect the binding before replacing it")
            self.db.execute("INSERT INTO foremen (id,terminal_id) VALUES (?,?) ON CONFLICT(id) DO NOTHING",
                            (foreman, terminal_id))

    def binding(self, foreman):
        row = self.db.execute("SELECT * FROM foremen WHERE id=?", (foreman,)).fetchone()
        if not row:
            raise Invalid("Register this Foreman explicitly before delegation")
        return dict(row)

    def submit(self, *, task_id, foreman, role, checkout, revision, scope, prompt, workflow_id=None, input_tree=None, native_todos=False):
        try:
            if str(uuid.UUID(task_id)) != task_id:
                raise ValueError()
        except (ValueError, TypeError, AttributeError):
            raise Invalid("Use a canonical task UUID") from None
        self.binding(foreman)
        if role not in ("worker", "reviewer"):
            raise Invalid("Task role must be worker or reviewer")
        if not isinstance(prompt, str) or not prompt.strip() or len(prompt.encode()) > 30000:
            raise Invalid("Task prompt must contain 1 to 30000 UTF-8 bytes")
        if (not isinstance(scope, list) or not scope or len(scope) > 100
                or any(not isinstance(p, str) or not p or PurePosixPath(p).is_absolute()
                       or any(part in (".", "..", ".git", "") for part in p.split("/"))
                       or any(c in p for c in "*?[]\\\n\r\0") for p in scope)):
            raise Invalid("Use explicit relative file or directory scopes without patterns")
        if not isinstance(checkout, str) or not Path(checkout).is_absolute():
            raise Invalid("Use an absolute authorised checkout path")
        checkout = str(Path(checkout).resolve())
        if not self.is_allowed(checkout, foreman):
            raise Invalid("Checkout is not explicitly authorised in configuration")
        if checkout not in self.allowed and workflow_id is None:
            raise Invalid('Managed checkouts require a controller-owned workflow task')
        if not isinstance(revision, str):
            raise Invalid("Use the full immutable Git commit ID")
        request = dict(task_id=task_id, foreman=foreman, role=role, checkout=checkout,
                        revision=revision, scope=scope, prompt=prompt)
        if workflow_id is not None:
            owner = self.db.execute('SELECT workflow_id FROM managed_checkouts WHERE path=?', (checkout,)).fetchone()
            if not owner or owner['workflow_id'] != workflow_id:
                raise Invalid('Managed task is not bound to its owned workflow checkout')
            request['workflow_id'] = workflow_id
        if native_todos:
            if workflow_id is None or native_todos is not True:
                raise Invalid('Native agent progress is only available for controller-owned managed tasks')
            request['native_todos'] = True
        if input_tree is not None:
            if workflow_id is None or not isinstance(input_tree, str) or not re.fullmatch(r'[0-9a-f]{40}|[0-9a-f]{64}', input_tree):
                raise Invalid('Snapshot input requires an exact managed workflow tree')
            request['input_tree'] = input_tree
        encoded = json.dumps(request, sort_keys=True)
        with self.db:
            self.db.execute("BEGIN IMMEDIATE")
            old = self.db.execute("SELECT * FROM tasks WHERE id=?", (task_id,)).fetchone()
            if old:
                if old["request"] != encoded:
                    raise Invalid("Task UUID already belongs to a different request")
                return self._task(old)
            sparse = task_checkout_info({'checkout': checkout, 'role': role, 'request': request})
            self.db.execute("INSERT INTO tasks (id,foreman,role,checkout,state,request,session_id,prompt_id,sparse,created) VALUES (?,?,?,?,?,?,?,?,?,?)",
                            (task_id, foreman, role, checkout, "queued", encoded,
                             "ses_foreman_" + uuid.UUID(task_id).hex,
                             "msg_foreman_" + uuid.UUID(task_id).hex, sparse, time.time()))
        return self.task(task_id)

    def _task(self, row):
        result = dict(row)
        result["request"] = json.loads(result["request"])
        result["sparse"] = bool(result["sparse"])
        result["runtime_model"] = json.loads(result["runtime_model"]) if result["runtime_model"] else None
        return result

    def record_model(self, task_id, model):
        model = model_reference(model)
        with self.db:
            self.db.execute("BEGIN IMMEDIATE")
            task = self.task(task_id)
            if task["runtime_model"] is not None and task["runtime_model"] != model:
                raise Invalid("A task's recorded dispatch model cannot change")
            if task["state"] != "starting":
                raise Invalid("Record the model only before the first worker admission")
            self.db.execute("UPDATE tasks SET runtime_model=? WHERE id=?", (json.dumps(model), task_id))
        return self.task(task_id)

    def task(self, task_id):
        row = self.db.execute("SELECT * FROM tasks WHERE id=?", (task_id,)).fetchone()
        if not row:
            raise Invalid("Task not found")
        return self._task(row)

    def is_allowed(self, checkout, foreman=None):
        if str(checkout) in self.allowed:
            return True
        row = self.db.execute('SELECT workflows.* FROM managed_checkouts JOIN workflows ON workflows.id=managed_checkouts.workflow_id WHERE path=?',
                              (str(checkout),)).fetchone()
        return bool(row and row['state'] in ('preparing', 'working', 'snapshotting', 'checking', 'review_preparing', 'reviewing')
                    and (foreman is None or row['foreman'] == foreman)
                    and self.projects.get(row['project']) == json.loads(row['policy']))

    def submit_workflow(self, foreman, **arguments):
        from foreman_workflows import submit
        return submit(self, foreman, **arguments)

    def workflow(self, workflow_id):
        row = self.db.execute('SELECT * FROM workflows WHERE id=?', (workflow_id,)).fetchone()
        if not row:
            raise Invalid('Workflow not found')
        result = dict(row)
        for key in ('request', 'policy', 'data'):
            result[key] = json.loads(result[key])
        return result

    def workflows(self, foreman):
        return [self.workflow(row['id']) for row in self.db.execute('SELECT id FROM workflows WHERE foreman=? ORDER BY created,id', (foreman,))]

    def tasks(self, foreman=None):
        rows = (self.db.execute("SELECT * FROM tasks ORDER BY created,id") if foreman is None else
                self.db.execute("SELECT * FROM tasks WHERE foreman=? ORDER BY created,id", (foreman,)))
        return [self._task(row) for row in rows]

    def claim(self):
        with self.db:
            self.db.execute("BEGIN IMMEDIATE")
            live = [t for t in self.tasks() if t["state"] in LIVE]
            for task in self.tasks():
                team = task['request'].get('workflow_id')
                if (task["state"] != "queued"
                        or sum(t["role"] == task["role"] and
                               t['request'].get('workflow_id') == team for t in live) >= 2
                        or any(t["checkout"] == task["checkout"] for t in live)):
                    continue
                self.db.execute("UPDATE tasks SET state='starting' WHERE id=?", (task["id"],))
                task["state"] = "starting"
                return task
        return None

    def update(self, task_id, state, *, result="", detail=""):
        if state not in (*LIVE, *TERMINAL):
            raise Invalid("Unknown task state")
        with self.db:
            self.db.execute("BEGIN IMMEDIATE")
            task = self.task(task_id)
            if task["state"] in TERMINAL:
                return
            self.db.execute("UPDATE tasks SET state=?,result=?,detail=? WHERE id=?",
                            (state, result[:100000], detail[:2000], task_id))
            if state in TERMINAL:
                passive = bool(task['request'].get('workflow_id'))
                self.db.execute("INSERT OR IGNORE INTO events (id,task_id,foreman,request_id,transport,refresh_todos,state) VALUES (?,?,?,?,?,?,?)",
                                (str(uuid.uuid4()), task_id, task["foreman"], str(uuid.uuid4()), "native", 0,
                                 'passive' if passive else 'pending'))

    def events(self, foreman):
        return [dict(row) for row in self.db.execute(
            "SELECT * FROM events WHERE foreman=? AND acked=0 ORDER BY rowid", (foreman,))]

    def approvals(self, foreman):
        return [dict(row) for row in self.db.execute(
            "SELECT task_id,permission_id,form_id,state,detail FROM approvals WHERE foreman=? ORDER BY rowid",
            (foreman,))]

    def set_plan(self, foreman, items):
        from foreman_progress import set_plan
        return set_plan(self, foreman, items)

    def progress(self, foreman):
        from foreman_progress import snapshot
        return snapshot(self, foreman)

    def ack(self, foreman, event_id):
        with self.db:
            row = self.db.execute("SELECT * FROM events WHERE id=? AND foreman=?", (event_id, foreman)).fetchone()
            if not row:
                raise Invalid("Event does not belong to this Foreman")
            self.db.execute("UPDATE events SET acked=1 WHERE id=?", (event_id,))


class Supervisor:
    def __init__(self, store, runtime, notifier=None):
        self.store = store
        self.runtime = runtime
        self.notifier = notifier

    def tick(self):
        # Existing reservations are reconciled by reads only, including after a crash.
        for task in self.store.tasks():
            if task["state"] not in LIVE:
                continue
            try:
                outcome = self.runtime.inspect(task)
                state = outcome["state"]
                if state in TERMINAL and task["role"] == "reviewer":
                    task_checkout_info(task)
                self.store.update(task["id"], state, result=outcome.get("result", ""),
                                  detail=outcome.get("detail", ""))
            except Invalid as error:
                self.store.update(task["id"], "uncertain", detail=str(error))
            except RuntimeFailure as error:
                self.store.update(task["id"], "uncertain", detail=f"Runtime inspection failed: {error}. Reservation retained")
            except (OSError, ValueError, KeyError):
                self.store.update(task["id"], "uncertain", detail="Runtime inspection failed. Reservation retained")
        while (task := self.store.claim()) is not None:
            try:
                if not self.store.is_allowed(str(Path(task["checkout"]).resolve()), task['foreman']):
                    raise Invalid("Checkout authorisation was removed before launch")
                task_checkout_info(task)
            except Invalid as error:
                self.store.update(task["id"], "failed", detail=str(error))
                continue
            try:
                task = self.store.record_model(task["id"], self.runtime.dispatch_model(task))
                self.runtime.create(task)
                self.runtime.prompt(task)
                self.store.update(task["id"], "accepted")
            except RuntimeFailure as error:
                self.store.update(task["id"], "uncertain", detail=f"Worker setup/admission failed: {error}. No automatic replay")
            except (OSError, ValueError, KeyError):
                self.store.update(task["id"], "uncertain", detail="Submission uncertain. No automatic replay")
        if self.notifier:
            self.notifier.deliver(self.store)


class Notifier:
    """Deliver once through the agent runtime. Reconcile uncertain attempts by reads only."""

    def __init__(self, runtime):
        self.runtime = runtime

    def deliver(self, store):
        rows = store.db.execute("""
            SELECT events.* FROM events JOIN foremen ON foremen.id=events.foreman
            WHERE events.acked=0 AND events.transport='native'
              AND (events.state='pending' OR
                   (events.state IN ('attempting','uncertain') AND events.next_receipt<=?))
            ORDER BY CASE WHEN events.state='pending' THEN 0 ELSE 1 END,
                     events.next_receipt, events.rowid LIMIT 8
        """, (time.time(),)).fetchall()
        for row in rows:
            event = dict(row)
            task = store.db.execute('SELECT request FROM tasks WHERE id=?', (event['task_id'],)).fetchone()
            if event['state'] == 'pending' and task and json.loads(task['request']).get('workflow_id'):
                with store.db:
                    store.db.execute("UPDATE events SET state='passive' WHERE id=? AND state='pending'", (event['id'],))
                continue
            try:
                if event["state"] == "pending":
                    with store.db:
                        claimed = store.db.execute("""
                            UPDATE events SET state='attempting'
                            WHERE id=? AND acked=0 AND state='pending' AND transport='native'
                        """, (event["id"],))
                    if not claimed.rowcount:
                        continue
                    self.runtime.notify(event)
                    state = "queued"
                else:
                    state = "queued" if self.runtime.event_accepted(event) else "uncertain"
                detail = "" if state == "queued" else "No matching native input yet. No automatic resend"
            except (OSError, ValueError, KeyError, TypeError):
                state, detail = "uncertain", "Native completion delivery uncertain. No automatic resend"
            with store.db:
                store.db.execute("UPDATE events SET state=?,detail=?,next_receipt=? WHERE id=? AND acked=0",
                                 (state, detail, time.time() + 60, event["id"]))
