"""Durable plans and passive, private UI projections. Never resumes a model."""

import json
import os
import re
import tempfile
import time
import uuid

from foreman_supervisor import Invalid


def set_plan(store, foreman, items):
    store.binding(foreman)
    if not isinstance(items, list) or len(items) > 100:
        raise Invalid("Plan must be an ordered list of at most 100 items")
    ids = set()
    for item in items:
        if (not isinstance(item, dict) or not {"id", "content"} <= item.keys()
                or set(item) - {"id", "content", "task_id", "status"}
                or not isinstance(item["id"], str) or not re.fullmatch(r"[A-Za-z0-9_-]{1,64}", item["id"])
                or item["id"] in ids or not isinstance(item["content"], str)
                or not item["content"].strip() or len(item["content"]) > 300
                or any(c in item["content"] for c in "\n\r\0")):
            raise Invalid("Use unique plan IDs, short single-line labels, and documented fields")
        ids.add(item["id"])
        if "task_id" in item:
            try:
                if str(uuid.UUID(item["task_id"])) != item["task_id"] or "status" in item:
                    raise ValueError()
            except (ValueError, TypeError, AttributeError):
                raise Invalid("Linked items require a canonical task UUID and no model-supplied status") from None
            task = store.db.execute("SELECT foreman FROM tasks WHERE id=?", (item["task_id"],)).fetchone()
            if task and task["foreman"] != foreman:
                raise Invalid("Plan task does not belong to this Foreman")
        elif item.get("status") not in ("pending", "in_progress", "completed", "cancelled"):
            raise Invalid("Unlinked planning items require an explicit manual status")
    with store.db:
        store.db.execute("INSERT INTO plans VALUES (?,?) ON CONFLICT(foreman) DO UPDATE SET items=excluded.items",
                         (foreman, json.dumps(items)))
    return snapshot(store, foreman)


def snapshot(store, foreman):
    store.binding(foreman)
    plan = store.db.execute("SELECT items FROM plans WHERE foreman=?", (foreman,)).fetchone()
    if not plan:
        return None
    items = []
    for item in json.loads(plan["items"]):
        row = dict(item, source="foreman-plan", label=item["content"])
        if "task_id" in item:
            row["source"] = "supervisor"
            task = store.db.execute("SELECT state,detail,role FROM tasks WHERE id=? AND foreman=?",
                                    (item["task_id"], foreman)).fetchone()
            state = task["state"] if task else "unavailable"
            row["task_state"] = state
            row["status"] = ("completed" if state == "succeeded" else
                             "cancelled" if state in ("failed", "interrupted") else
                             "pending" if state in ("queued", "unavailable") else "in_progress")
            description = {"unavailable": "not submitted or unavailable", "queued": "queued",
                           "starting": "reserved; starting", "accepted": "native input accepted; execution pending",
                           "running": "running", "uncertain": "uncertain; reservation retained",
                           "succeeded": "execution ended; tests/review not implied",
                           "failed": "execution failed", "interrupted": "execution interrupted"}[state]
            if task and state == "running" and "human permission or answer" in task["detail"]:
                description = "waiting for human approval or answer"
            row["content"] += " — " + description
        items.append(row)
    from foreman_workflow_progress import workflow_rows, latest_result
    items.extend(workflow_rows(store, foreman))
    result = {"items": items, "todos": [{"content": item["content"], "status": item["status"]} for item in items]}
    outcome = latest_result(store, foreman)
    if outcome:
        result['latest_result'] = outcome
    return result


class ProgressPublisher:
    def __init__(self, runtime=None):
        # Retain the constructor contract; UI projection needs no runtime access.
        pass

    def deliver(self, store, *, now=None):
        now = time.time() if now is None else now
        sessions = {}
        for plan in store.db.execute('SELECT foreman FROM plans ORDER BY foreman').fetchall():
            sessions[plan['foreman']] = snapshot(store, plan['foreman'])
        with store.db:
            # Unsent old projections are obsolete. Already admitted/uncertain inputs
            # remain history; never delete, replay or fabricate their receipts.
            store.db.execute("UPDATE progress_events SET state='superseded' WHERE state='pending'")
        data = json.dumps({'version': 1, 'updated_at': now, 'sessions': sessions}) + '\n'
        with tempfile.NamedTemporaryFile(mode='w', dir=store.directory, prefix='.progress-', delete=False) as stream:
            temporary = stream.name
            try:
                stream.write(data)
                stream.flush()
                os.replace(temporary, store.directory / 'progress.json')
            finally:
                if os.path.exists(temporary):
                    os.unlink(temporary)
