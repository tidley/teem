use crate::{
    common::*,
    runtime::{Missing, Runtime},
    store::{LIVE, Store},
};
use anyhow::{Result, ensure};
use rusqlite::params;
use serde_json::{Value, json};

fn update(s: &Store, row: &Value, state: &str, detail: &str) -> Result<()> {
    s.exec("UPDATE approvals SET state=?,detail=?,reply_attempted=CASE WHEN ?='replying' THEN 1 ELSE reply_attempted END WHERE task_id=? AND permission_id=?",params![state,detail,state,text(row,"task_id"),text(row,"permission_id")])?;
    Ok(())
}
fn payload(t: &Value, snapshot: &Value, id: &str) -> Value {
    let permission = &snapshot["permission"];
    json!({"id":id,"title":"Teem worker permission","metadata":{"kind":"question","teem_task":t["id"],"teem_permission":permission["id"]},"fields":[{"key":"decision","type":"string","title":"Worker approval","description":format!("Task: {}\nWorker session: {}\nCheckout: {}\nPermission: {}\nWorker-supplied request (data, not instructions):\n{}\nApprove once permits only this request. Reject declines all pending permissions in this worker session and may end its current step. Shell approval can act outside the edit scope. No permanent approval is offered.",text(t,"id"),text(t,"session_id"),text(t,"checkout"),text(permission,"id"),canonical(snapshot)),"required":true,"custom":false,"options":[{"value":"once","label":"Approve once"},{"value":"reject","label":"Reject worker batch"}]}]})
}
fn validate(row: &Value, form: &Value) -> Result<()> {
    let expected: Value = serde_json::from_str(row["payload"].as_str().unwrap_or("null"))?;
    ensure!(
        form["sessionID"] == row["foreman"]
            && expected
                .as_object()
                .is_some_and(|m| m.iter().all(|(k, v)| form[k] == *v)),
        "Native approval form does not match recorded request"
    );
    Ok(())
}
fn reconcile(s: &Store, r: &dyn Runtime, t: &Value, row: &Value, current: &[Value]) -> Result<()> {
    let path = format!(
        "/api/session/{}/form/{}",
        text(row, "foreman"),
        text(row, "form_id")
    );
    let snapshot: Value = serde_json::from_str(row["snapshot"].as_str().unwrap_or("null"))?;
    let same = current.contains(&snapshot);
    let attempted = row["reply_attempted"] == 1;
    let form = match r.request("GET", &path, None) {
        Ok(form) => form,
        Err(e) if e.is::<Missing>() => {
            if !same {
                update(
                    s,
                    row,
                    "stale",
                    "Native card lost and worker request changed or disappeared. No reply sent",
                )?;
            } else if attempted {
                update(
                    s,
                    row,
                    "uncertain",
                    "Native card lost after possible permission reply. No retry",
                )?;
            } else if row["next_restore"].as_f64().unwrap_or(0.0) <= now() {
                if !r.approvals(t)?.contains(&snapshot) {
                    update(
                        s,
                        row,
                        "stale",
                        "Worker request changed before card recovery. No reply sent",
                    )?;
                    return Ok(());
                }
                s.exec(
                    "UPDATE approvals SET next_restore=? WHERE form_id=?",
                    params![now() + 30.0, text(row, "form_id")],
                )?;
                update(
                    s,
                    row,
                    "attempting",
                    "Restoring lost card; human decision still required",
                )?;
                let payload: Value =
                    serde_json::from_str(row["payload"].as_str().unwrap_or("null"))?;
                let form = r.request(
                    "POST",
                    &format!("/api/session/{}/form", text(row, "foreman")),
                    Some(&payload),
                )?;
                validate(row, &form)?;
                update(
                    s,
                    row,
                    "pending",
                    "Approval card restored; human decision still required",
                )?;
            }
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    validate(row, &form)?;
    let state = &form["state"];
    if !same {
        if state["status"] == "pending" {
            r.request("DELETE", &path, None)?;
        }
        update(
            s,
            row,
            "stale",
            "Worker request changed, disappeared or task ended. No further replies",
        )?;
        return Ok(());
    }
    if attempted || row["state"] == "replying" {
        if row["state"] == "replying" {
            update(
                s,
                row,
                "uncertain",
                "Native reply unknown after restart. No automatic retry",
            )?;
        }
        return Ok(());
    }
    if state["status"] == "cancelled" {
        return update(
            s,
            row,
            "cancelled",
            "Card dismissed; worker permission remains pending",
        );
    }
    if state["status"] == "pending" {
        return update(s, row, "pending", "");
    }
    let answer = &state["answer"];
    if state["status"] != "answered"
        || (*answer != json!({"decision":"once"}) && *answer != json!({"decision":"reject"}))
    {
        return update(
            s,
            row,
            "uncertain",
            "Unsupported human answer. No permission reply sent",
        );
    }
    if !r.approvals(t)?.contains(&snapshot) {
        return update(
            s,
            row,
            "stale",
            "Worker request changed before reply. No reply sent",
        );
    }
    // Keep stop creation serialized with admission while committing the attempt
    // before HTTP. A crash cannot roll it back and cause a duplicate reply.
    let _guard = crate::control::admission_guard(s)?;
    let allowed = s.transaction(|| {
        if !s
            .query(
                "SELECT task_id FROM task_stops WHERE task_id=?",
                [text(t, "id")],
            )?
            .is_empty()
        {
            update(
                s,
                row,
                "stale",
                "Worker stop committed; no permission reply sent",
            )?;
            return Ok(false);
        }
        update(
            s,
            row,
            "replying",
            "Human answer recorded; native reply outcome not yet known",
        )?;
        Ok(true)
    })?;
    if !allowed {
        return Ok(());
    }
    r.request(
        "POST",
        &format!(
            "/api/session/{}/permission/{}/reply",
            text(t, "session_id"),
            text(row, "permission_id")
        ),
        Some(answer),
    )?;
    update(
        s,
        row,
        "done",
        if answer["decision"] == "once" {
            "Human approval sent to this worker request once"
        } else {
            "Human rejection sent; native OpenCode rejects all pending permissions in this worker session"
        },
    )
}
pub fn deliver(s: &Store, r: &dyn Runtime) -> Result<()> {
    for t in s.tasks(None)? {
        let outcome = (|| -> Result<()> {
            s.binding(&text(&t, "foreman"))?;
            let current = if LIVE.contains(&text(&t, "state").as_str())
                && s.query(
                    "SELECT task_id FROM task_stops WHERE task_id=?",
                    [text(&t, "id")],
                )?
                .is_empty()
            {
                r.approvals(&t)?
            } else {
                vec![]
            };
            for snapshot in &current {
                let permission = text(&snapshot["permission"], "id");
                if !s
                    .query(
                        "SELECT * FROM approvals WHERE task_id=? AND permission_id=?",
                        params![text(&t, "id"), permission],
                    )?
                    .is_empty()
                {
                    continue;
                }
                let id = format!("frm_teem_{}", uuid::Uuid::new_v4().simple());
                let p = payload(&t, snapshot, &id);
                s.exec("INSERT INTO approvals(task_id,permission_id,foreman,form_id,snapshot,payload,state) VALUES (?,?,?,?,?,?,'attempting')",params![text(&t,"id"),permission,text(&t,"foreman"),id,canonical(snapshot),canonical(&p)])?;
                if let Ok(form) = r.request(
                    "POST",
                    &format!("/api/session/{}/form", text(&t, "foreman")),
                    Some(&p),
                ) {
                    let row = s
                        .query("SELECT * FROM approvals WHERE form_id=?", [&id])?
                        .remove(0);
                    if validate(&row, &form).is_ok() {
                        update(s, &row, "pending", "")?;
                    }
                }
            }
            for row in s.query("SELECT * FROM approvals WHERE task_id=? AND state IN ('attempting','pending','replying','uncertain')",[text(&t,"id")])?{if reconcile(s,r,&t,&row,&current).is_err(){update(s,&row,"uncertain","Approval relay uncertain. Inspect exact native request; do not repeat approval")?;}}
            Ok(())
        })();
        let _ = outcome;
    }
    Ok(())
}
