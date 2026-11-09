"""Durable task admission and bounded scheduling. No terminal input or Git mutations."""

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


class Store:
    def __init__(self, directory, allowed_checkouts):
        self.directory = Path(directory).expanduser().resolve()
        self.directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        os.chmod(self.directory, 0o700)
        self.allowed = {str(Path(p).expanduser().resolve()) for p in allowed_checkouts}
        path = self.directory / "tasks.sqlite3"
        fd = os.open(path, os.O_CREAT | os.O_RDWR, 0o600)
        os.close(fd)
        os.chmod(path, 0o600)
        self.db = sqlite3.connect(path, timeout=15)
        self.db.row_factory = sqlite3.Row
        self.db.executescript("""
            CREATE TABLE IF NOT EXISTS foremen (
                id TEXT PRIMARY KEY, terminal_id TEXT NOT NULL, notification TEXT);
            CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY, foreman TEXT NOT NULL, role TEXT NOT NULL,
                checkout TEXT NOT NULL, state TEXT NOT NULL, request TEXT NOT NULL,
                session_id TEXT NOT NULL UNIQUE, prompt_id TEXT NOT NULL UNIQUE,
                sparse INTEGER NOT NULL, result TEXT NOT NULL DEFAULT '',
                detail TEXT NOT NULL DEFAULT '', created REAL NOT NULL);
            CREATE TABLE IF NOT EXISTS events (
                id TEXT PRIMARY KEY, task_id TEXT NOT NULL UNIQUE, foreman TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'pending', request_id TEXT NOT NULL UNIQUE,
                acked INTEGER NOT NULL DEFAULT 0, next_receipt REAL NOT NULL DEFAULT 0);
        """)

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()

    def close(self):
        self.db.close()

    def bind(self, foreman, terminal_id, notification=None):
        if not re.fullmatch(r"ses_[A-Za-z0-9_-]+", foreman) or not terminal_id:
            raise Invalid("Use an exact native session and terminal ID")
        if notification:
            from codr_foreman import Client, Rejected
            try:
                Client(**notification)
                if (json.loads(notification["session_key"]) != ["herdr:opencode", "opencode", "id", foreman]
                        or notification["terminal_id"] != terminal_id):
                    raise ValueError()
            except (Rejected, ValueError, KeyError, TypeError):
                raise Invalid("Codr notification binding must match this exact native session and terminal") from None
        with self.db:
            old = self.db.execute("SELECT * FROM foremen WHERE id=?", (foreman,)).fetchone()
            if old and old["terminal_id"] != terminal_id:
                raise Invalid("Foreman terminal changed. Inspect the binding before replacing it")
            self.db.execute("INSERT INTO foremen VALUES (?,?,?) ON CONFLICT(id) DO UPDATE SET notification=excluded.notification",
                            (foreman, terminal_id, json.dumps(notification) if notification else None))

    def binding(self, foreman):
        row = self.db.execute("SELECT * FROM foremen WHERE id=?", (foreman,)).fetchone()
        if not row:
            raise Invalid("Register this Foreman explicitly before delegation")
        return dict(row)

    def submit(self, *, task_id, foreman, role, checkout, revision, scope, prompt):
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
        if checkout not in self.allowed:
            raise Invalid("Checkout is not explicitly authorised in configuration")
        if not isinstance(revision, str):
            raise Invalid("Use the full immutable Git commit ID")
        request = dict(task_id=task_id, foreman=foreman, role=role, checkout=checkout,
                       revision=revision, scope=scope, prompt=prompt)
        encoded = json.dumps(request, sort_keys=True)
        with self.db:
            self.db.execute("BEGIN IMMEDIATE")
            old = self.db.execute("SELECT * FROM tasks WHERE id=?", (task_id,)).fetchone()
            if old:
                if old["request"] != encoded:
                    raise Invalid("Task UUID already belongs to a different request")
                return self._task(old)
            sparse = checkout_info(checkout, revision, role)
            self.db.execute("INSERT INTO tasks (id,foreman,role,checkout,state,request,session_id,prompt_id,sparse,created) VALUES (?,?,?,?,?,?,?,?,?,?)",
                            (task_id, foreman, role, checkout, "queued", encoded,
                             "ses_foreman_" + uuid.UUID(task_id).hex,
                             "msg_foreman_" + uuid.UUID(task_id).hex, sparse, time.time()))
        return self.task(task_id)

    def _task(self, row):
        result = dict(row)
        result["request"] = json.loads(result["request"])
        result["sparse"] = bool(result["sparse"])
        return result

    def task(self, task_id):
        row = self.db.execute("SELECT * FROM tasks WHERE id=?", (task_id,)).fetchone()
        if not row:
            raise Invalid("Task not found")
        return self._task(row)

    def tasks(self, foreman=None):
        rows = (self.db.execute("SELECT * FROM tasks ORDER BY created,id") if foreman is None else
                self.db.execute("SELECT * FROM tasks WHERE foreman=? ORDER BY created,id", (foreman,)))
        return [self._task(row) for row in rows]

    def claim(self):
        with self.db:
            self.db.execute("BEGIN IMMEDIATE")
            live = [t for t in self.tasks() if t["state"] in LIVE]
            for task in self.tasks():
                if (task["state"] != "queued"
                        or sum(t["role"] == task["role"] for t in live) >= 2
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
                self.db.execute("INSERT OR IGNORE INTO events (id,task_id,foreman,request_id) VALUES (?,?,?,?)",
                                (str(uuid.uuid4()), task_id, task["foreman"], str(uuid.uuid4())))

    def events(self, foreman):
        return [dict(row) for row in self.db.execute(
            "SELECT * FROM events WHERE foreman=? AND acked=0 ORDER BY rowid", (foreman,))]

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
                    checkout_info(task["checkout"], task["request"]["revision"], "reviewer")
                self.store.update(task["id"], state, result=outcome.get("result", ""),
                                  detail=outcome.get("detail", ""))
            except Invalid as error:
                self.store.update(task["id"], "uncertain", detail=str(error))
            except (OSError, ValueError, KeyError):
                self.store.update(task["id"], "uncertain", detail="Runtime inspection failed. Reservation retained")
        while (task := self.store.claim()) is not None:
            try:
                if str(Path(task["checkout"]).resolve()) not in self.store.allowed:
                    raise Invalid("Checkout authorisation was removed before launch")
                checkout_info(task["checkout"], task["request"]["revision"], task["role"])
            except Invalid as error:
                self.store.update(task["id"], "failed", detail=str(error))
                continue
            try:
                self.runtime.create(task)
                self.runtime.prompt(task)
                self.store.update(task["id"], "accepted")
            except (OSError, ValueError, KeyError):
                self.store.update(task["id"], "uncertain", detail="Submission uncertain. No automatic replay")
        if self.notifier:
            self.notifier.deliver(self.store)
