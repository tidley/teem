//! Durable user stop requests. Never discard a checkout or replay an interrupt.
use crate::{
    common::*,
    runtime::Runtime,
    store::{LIVE, Store, TERMINAL},
};
use anyhow::{Context, Result, ensure};
use rusqlite::params;
use serde_json::{Value, json};
use std::{fs::File, os::fd::AsRawFd};

/// Serialize stop creation and native input/approval admission across processes.
/// The guard outlives committed attempt reservations and unknown HTTP responses.
pub fn admission_guard(s: &Store) -> Result<File> {
    let lock = open_lock(&s.directory.join("native-admission.lock"))?;
    ensure!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } == 0,
        "Cannot serialize native admission"
    );
    Ok(lock)
}

pub fn paused(s: &Store, id: &str) -> Result<bool> {
    Ok(!s
        .query(
            "SELECT workflow_id FROM workflow_stops WHERE workflow_id=?",
            [id],
        )?
        .is_empty())
}

pub fn snapshot(s: &Store, id: &str) -> Result<Value> {
    let rows = s.query(
        "SELECT requested FROM workflow_stops WHERE workflow_id=?",
        [id],
    )?;
    let Some(row) = rows.first() else {
        return Ok(Value::Null);
    };
    let targets=s.query("SELECT task_stops.*,tasks.session_id,tasks.checkout FROM task_stops JOIN tasks ON tasks.id=task_stops.task_id WHERE json_extract(tasks.request,'$.workflow_id')=? ORDER BY tasks.created,tasks.id",[id])?;
    let state = if targets.iter().all(|t| t["state"] == "stopped") {
        "stopped"
    } else if targets.iter().any(|t| t["state"] == "uncertain") {
        "uncertain"
    } else {
        "requested"
    };
    Ok(
        json!({"workflow_id":id,"requested_at":row["requested"],"state":state,"workflow_paused":true,"targets":targets,"instruction":"No automatic resume or replacement. Preserve checkouts; confirm stopped before handing off. Running checks are not cancelled by this native-agent stop."}),
    )
}

pub fn request(s: &Store, f: &str, args: &Value) -> Result<Value> {
    s.binding(f)?;
    let id = text(args, "workflow_id");
    uuid(&id)?;
    let _guard = admission_guard(s)?;
    s.transaction(||{
        let mut w=s.workflow(&id)?;
        ensure!(w["foreman"]==f,"Workflow does not belong to this Foreman");
        let tasks=s.tasks(Some(f))?.into_iter().filter(|t|t["request"]["workflow_id"]==id).collect::<Vec<_>>();
        let selected=if let Some(task)=args.get("task_id") {
            let task=task.as_str().context("Use an exact task UUID")?;uuid(task)?;
            let t=tasks.iter().find(|t|t["id"]==task).context("Task does not belong to this workflow")?;
            vec![t.clone()]
        }else{tasks};
        let targets=selected.into_iter().filter(|t|t["state"]=="queued"||LIVE.contains(&text(t,"state").as_str())).collect::<Vec<_>>();
        if targets.is_empty() {
            if paused(s,&id)? {return snapshot(s,&id)}
            ensure!(!crate::store::FINAL.contains(&text(&w,"state").as_str()) && w["state"]!="checking" && args.get("task_id").is_none(),"No active or queued native agent to stop; check batches are not stopped by this tool");
        }
        s.exec("INSERT OR IGNORE INTO workflow_stops VALUES (?,?)",params![id,now()])?;
        if w["data"]["finished_at"].is_null(){w["data"]["finished_at"]=json!(now());}
        s.exec("UPDATE workflows SET state='blocked',detail='Stop requested by user; workflow paused, checkouts and evidence preserved',data=? WHERE id=?",params![canonical(&w["data"]),id])?;
        for t in targets {
            s.exec("INSERT OR IGNORE INTO task_stops(task_id,state) VALUES (?,'requested')",[text(&t,"id")])?;
        }
        snapshot(s,&id)
    })
}

fn finished(s: &Store, r: &dyn Runtime, t: &Value) -> Result<()> {
    let id = text(t, "id");
    if !TERMINAL.contains(&text(t, "state").as_str()) {
        let observation = r.inspect(t).ok();
        if observation
            .as_ref()
            .is_some_and(|o| o["state"] == "running" || o["state"] == "accepted")
        {
            return Ok(());
        }
        let observed = observation.filter(|o| TERMINAL.contains(&text(o, "state").as_str()));
        if let Some(o) = observed {
            s.update(
                &id,
                &text(&o, "state"),
                &if text(&o, "result").is_empty() {
                    text(t, "result")
                } else {
                    text(&o, "result")
                },
                &text(&o, "detail"),
            )?;
        } else {
            s.update(
                &id,
                "interrupted",
                &text(t, "result"),
                "Stopped by user; native execution inactive and inbox empty; checkout preserved",
            )?;
        }
    }
    s.exec("UPDATE task_stops SET state='stopped',detail='Native inactivity confirmed; preserved checkout; no replacement' WHERE task_id=?",[id])?;
    Ok(())
}

pub fn tick(s: &Store, r: &dyn Runtime) -> Result<()> {
    for stop in s.query(
        "SELECT * FROM task_stops WHERE state!='stopped' ORDER BY rowid",
        [],
    )? {
        let id = text(&stop, "task_id");
        let t = s.task(&id)?;
        if t["state"] == "queued"
            || (t["state"] == "interrupted"
                && text(&t, "detail") == "Stopped before native session creation")
        {
            s.update(
                &id,
                "interrupted",
                &text(&t, "result"),
                "Stopped before native session creation",
            )?;
            s.exec("UPDATE task_stops SET state='stopped',detail='Cancelled before native admission; checkout preserved' WHERE task_id=?",[&id])?;
            continue;
        }
        match r.is_stopped(&t) {
            Ok(true) => {
                finished(s, r, &t)?;
                continue;
            }
            Err(_) => {
                s.exec("UPDATE task_stops SET detail='Native identity/activity unavailable; stop unconfirmed; no unchecked interrupt issued' WHERE task_id=?",[&id])?;
                continue;
            }
            Ok(false) => {}
        }
        if s.exec("UPDATE task_stops SET state='attempting',detail='Native interrupt attempt reserved' WHERE task_id=? AND state='requested'",[&id])?==0 {continue}
        let result = r.interrupt(&t);
        s.exec("UPDATE task_stops SET state=?,detail=? WHERE task_id=?",params![if result.is_ok(){"waiting"}else{"uncertain"},if result.is_ok(){"Interrupt requested; waiting for inactive execution and empty inbox"}else{"Native interrupt response unknown; reconcile by reads only, never resend automatically"},id])?;
    }
    for row in s.query("SELECT workflow_id FROM workflow_stops", [])? {
        let id = text(&row, "workflow_id");
        if snapshot(s, &id)?["state"] == "stopped" {
            s.exec("UPDATE workflows SET detail='Stopped by user; selected native agents inactive; workflow paused; preserved work, no replacement' WHERE id=?",[&id])?;
            let w = s.workflow(&id)?;
            s.exec("INSERT OR IGNORE INTO events(id,task_id,foreman,request_id,transport,refresh_todos,workflow_id) VALUES (?,?,?,?,'native',0,?)",params![uuid::Uuid::new_v4().to_string(),format!("workflow:{id}"),text(&w,"foreman"),uuid::Uuid::new_v4().to_string(),id])?;
        }
    }
    Ok(())
}
