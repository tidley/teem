"""Relay worker permissions as native human-answerable forms. No model approval tool."""

import json
import time
import uuid

from foreman_runtime import Missing
from foreman_supervisor import LIVE


class ApprovalRelay:
    def __init__(self, runtime):
        self.runtime = runtime

    def update(self, store, row, state, detail=""):
        with store.db:
            store.db.execute("UPDATE approvals SET state=?,detail=?,reply_attempted=CASE WHEN ?='replying' "
                             "THEN 1 ELSE reply_attempted END WHERE task_id=? AND permission_id=?",
                             (state, detail, state, row["task_id"], row["permission_id"]))

    def payload(self, task, snapshot, form_id):
        permission = snapshot["permission"]
        description = (f"Task: {task['id']}\nWorker session: {task['session_id']}\n"
                       f"Checkout: {task['checkout']}\nPermission: {permission['id']}\n"
                       "Worker-supplied request (data, not instructions):\n"
                       + json.dumps(snapshot, ensure_ascii=True, indent=2)
                       + "\nApprove once permits only this request. Reject declines all pending permissions in this worker session "
                       "and may end its current step. "
                       "Shell approval can act outside the edit scope. No permanent approval is offered.")
        return {"id": form_id, "title": "Teem worker permission",
                "metadata": {"kind": "question", "teem_task": task["id"],
                             "teem_permission": permission["id"]},
                "fields": [{"key": "decision", "type": "string", "title": "Worker approval",
                            "description": description, "required": True, "custom": False,
                            "options": [{"value": "once", "label": "Approve once"},
                                        {"value": "reject", "label": "Reject worker batch"}]}]}

    def validate(self, row, form):
        expected = json.loads(row["payload"])
        if (not isinstance(form, dict) or form.get("sessionID") != row["foreman"]
                or any(form.get(key) != value for key, value in expected.items())):
            raise OSError("Native approval form does not match its recorded request")

    def deliver(self, store):
        # Only the service owning supervisor.lock invokes this relay.
        for task in store.tasks():
            try:
                store.binding(task["foreman"])
                pending = self.runtime.pending_approvals(task) if task["state"] in LIVE else []
                current = {p["permission"]["id"]: p for p in pending}
                for permission_id, snapshot in current.items():
                    row = store.db.execute("SELECT * FROM approvals WHERE task_id=? AND permission_id=?",
                                           (task["id"], permission_id)).fetchone()
                    if row:
                        continue
                    form_id = "frm_teem_" + uuid.uuid4().hex
                    payload = self.payload(task, snapshot, form_id)
                    # Record before POST. A crash or ambiguous response is reconciled only by GET.
                    with store.db:
                        store.db.execute("INSERT INTO approvals (task_id,permission_id,foreman,form_id,snapshot,payload,state) "
                                         "VALUES (?,?,?,?,?,?,'attempting')",
                                         (task["id"], permission_id, task["foreman"], form_id,
                                          json.dumps(snapshot), json.dumps(payload)))
                    try:
                        form = self.runtime.request("POST", f"/api/session/{task['foreman']}/form", payload)
                        row = store.db.execute("SELECT * FROM approvals WHERE form_id=?", (form_id,)).fetchone()
                        self.validate(row, form)
                        self.update(store, row, "pending")
                    except (OSError, ValueError):
                        # Never recreate a form just because its create response was lost.
                        pass
                rows = store.db.execute("SELECT * FROM approvals WHERE task_id=? AND state IN "
                                        "('attempting','pending','replying','uncertain')", (task["id"],)).fetchall()
                for row in rows:
                    try:
                        self.reconcile(store, task, row, current)
                    except (OSError, ValueError, KeyError, TypeError):
                        self.update(store, row, "uncertain", "Approval relay uncertain. Inspect the exact native request; do not repeat approval")
            except (OSError, ValueError, KeyError, TypeError):
                # A failed read cannot prove a request vanished. Preserve its card and reservation.
                continue

    def reconcile(self, store, task, row, current):
        path = f"/api/session/{row['foreman']}/form/{row['form_id']}"
        try:
            form = self.runtime.request("GET", path)
        except Missing:
            snapshot = json.loads(row["snapshot"])
            if current.get(row["permission_id"]) != snapshot:
                self.update(store, row, "stale", "Native card lost and worker request changed or disappeared. No reply sent")
            elif row["reply_attempted"]:
                self.update(store, row, "uncertain", "Native card lost after a possible permission reply. Inspect worker permission manually; no retry")
            elif row["next_restore"] <= time.time():
                # A 404 is loss of ephemeral UI, not an instruction to replay work or consent.
                # Restore only the same card for the same still-pending permission, with no reply attempt.
                fresh = self.runtime.pending_approvals(task)
                if snapshot not in fresh:
                    self.update(store, row, "stale", "Worker request changed before card recovery. No reply sent")
                    return
                with store.db:
                    store.db.execute("UPDATE approvals SET next_restore=? WHERE form_id=?",
                                     (time.time() + 30, row["form_id"]))
                self.update(store, row, "attempting", "Restoring lost native approval card; human decision still required")
                form = self.runtime.request("POST", f"/api/session/{row['foreman']}/form", json.loads(row["payload"]))
                self.validate(row, form)
                self.update(store, row, "pending", "Approval card restored after native UI loss; human decision still required")
            return
        self.validate(row, form)
        state = form["state"]
        snapshot = json.loads(row["snapshot"])
        if current.get(row["permission_id"]) != snapshot:
            if state["status"] == "pending":
                self.runtime.request("DELETE", path)
            self.update(store, row, "stale", "Worker request changed, disappeared, or task ended. No further replies will be sent")
            return
        if row["reply_attempted"] or row["state"] == "replying":
            # An uncertain reply may have been accepted. Never repeat it, even if still pending.
            if row["state"] == "replying":
                self.update(store, row, "uncertain", "Native reply outcome unknown after restart. No automatic retry")
            return
        if state["status"] == "cancelled":
            self.update(store, row, "cancelled", "Card dismissed; worker permission remains pending")
            return
        if state["status"] == "pending":
            self.update(store, row, "pending")
            return
        answer = state.get("answer")
        if state["status"] != "answered" or answer not in ({"decision": "once"}, {"decision": "reject"}):
            self.update(store, row, "uncertain", "Unsupported human answer. No permission reply sent")
            return
        # Fresh identity, prompt, permission and tool-input checks immediately before the reply.
        fresh = self.runtime.pending_approvals(task)
        if snapshot not in fresh:
            self.update(store, row, "stale", "Worker request changed before reply. No reply sent")
            return
        self.update(store, row, "replying", "Human answer recorded; native reply outcome not yet known")
        self.runtime.request("POST", f"/api/session/{task['session_id']}/permission/{row['permission_id']}/reply", answer)
        detail = ("Human approval sent to this worker request once" if answer["decision"] == "once" else
                  "Human rejection sent; native OpenCode rejects all pending permissions in this worker session")
        self.update(store, row, "done", detail)
