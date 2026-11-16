//! Explicit, scoped management handoffs. Native task/provenance owners never change.
use crate::{common::*, store::Store};
use anyhow::{Context, Result, ensure};
use serde_json::Value;

fn parent(s: &Store, current: &Value, root: &Value) -> Result<Option<Value>> {
    let id = current["request"]["qualification_recovery"]["source_workflow_id"]
        .as_str()
        .or(current["request"]["reconcile_from"].as_str());
    let Some(id) = id else {
        return Ok(None);
    };
    if s.query("SELECT id FROM workflows WHERE id=?", [id])?
        .is_empty()
    {
        return Ok(None);
    }
    let next = s.workflow(id)?;
    Ok((next["foreman"] == root["foreman"]
        && next["project"] == root["project"]
        && (next["request"]["issue"].is_null()
            || next["request"]["issue"] == root["request"]["issue"]))
        .then_some(next))
}

pub fn record(s: &Store, w: &Value) -> Result<Option<Value>> {
    if s.query(
        "SELECT seq FROM workflow_handoffs WHERE original_foreman=? LIMIT 1",
        [text(w, "foreman")],
    )?
    .is_empty()
    {
        return Ok(None);
    }
    let mut current = w.clone();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..64 {
        let id = text(&current, "id");
        ensure!(
            seen.insert(id.clone()),
            "Handoff lineage cycle; preserve records for inspection"
        );
        if let Some(row) = s
            .query(
                "SELECT * FROM workflow_handoffs WHERE workflow_id=? ORDER BY seq DESC LIMIT 1",
                [&id],
            )?
            .into_iter()
            .next()
        {
            ensure!(
                row["original_foreman"] == w["foreman"],
                "Handoff provenance changed"
            );
            return Ok(Some(row));
        }
        let Some(next) = parent(s, &current, w)? else {
            return Ok(None);
        };
        current = next;
    }
    anyhow::bail!("Handoff lineage exceeds inspection bound")
}

pub fn manager(s: &Store, w: &Value) -> Result<String> {
    Ok(record(s, w)?
        .map(|r| text(&r, "target_foreman"))
        .unwrap_or_else(|| text(w, "foreman")))
}

pub fn authorize(s: &Store, f: &str, w: &Value, read_only: bool) -> Result<String> {
    let manager = manager(s, w)?;
    ensure!(
        manager == f || (read_only && can_read(s, f, w)?),
        "Workflow is managed by another Teem coordinator ({manager}); use an explicitly user-approved teem_handoff for this workflow, not task replay"
    );
    Ok(text(w, "foreman"))
}

pub fn can_read(s: &Store, f: &str, w: &Value) -> Result<bool> {
    if w["foreman"] == f {
        return Ok(true);
    }
    let mut current = w.clone();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..64 {
        let id = text(&current, "id");
        ensure!(
            seen.insert(id.clone()),
            "Handoff read lineage contains a cycle"
        );
        if !s.query("SELECT seq FROM workflow_handoffs WHERE workflow_id=? AND (source_foreman=? OR target_foreman=?) LIMIT 1",[id,f.into(),f.into()])?.is_empty() {return Ok(true);}
        let Some(next) = parent(s, &current, w)? else {
            return Ok(false);
        };
        current = next;
    }
    anyhow::bail!("Handoff read lineage exceeds inspection bound")
}

pub fn claim(s: &Store, f: &str, args: &Value) -> Result<Value> {
    s.binding(f)?;
    let id = text(args, "workflow_id");
    uuid(&id)?;
    let source = text(args, "source_foreman");
    s.binding(&source)?;
    let reason = text(args, "reason");
    ensure!(
        !reason.trim().is_empty()
            && reason.chars().count() <= 500
            && !reason.chars().any(char::is_control),
        "A bounded single-line handoff reason is required"
    );
    s.transaction(||{
        let w=s.workflow(&id)?;
        if let Some(old)=record(s,&w)? {
            if old["workflow_id"]==id && old["target_foreman"]==f && old["source_foreman"]==source && old["reason"]==reason {return Ok(old);}
        }
        ensure!(manager(s,&w)?==source && source!=f,"Expected current manager differs; inspect the workflow before a scoped handoff");
        s.exec("INSERT INTO workflow_handoffs(workflow_id,original_foreman,source_foreman,target_foreman,reason,created) VALUES (?,?,?,?,?,?)",rusqlite::params![id,text(&w,"foreman"),source,f,reason,now()])?;
        record(s,&w)?.context("Missing handoff receipt")
    })
}

/// Resolve only a named workflow/task/event. Unscoped tools never gain another chat's authority.
pub fn caller(s: &Store, f: &str, name: &str, args: &Value) -> Result<String> {
    let id = match name {
        "foreman_reconcile" | "foreman_recover_qualification" => text(args, "source_workflow_id"),
        "foreman_status"
        | "foreman_integrate"
        | "foreman_stop"
        | "foreman_result_notice"
        | "foreman_repair_prerequisites" => {
            let id = text(args, "workflow_id");
            if !id.is_empty() {
                id
            } else if !text(args, "task_id").is_empty() {
                text(&s.task(&text(args, "task_id"))?["request"], "workflow_id")
            } else {
                String::new()
            }
        }
        "foreman_ack" => {
            let e = s.query("SELECT * FROM events WHERE id=?", [text(args, "event_id")])?;
            let e = e.first().context("Event not found")?;
            if !text(e, "workflow_id").is_empty() {
                let w = s.workflow(&text(e, "workflow_id"))?;
                authorize(s, f, &w, false)?;
            } else {
                ensure!(
                    e["foreman"] == f,
                    "Event belongs to another Teem coordinator"
                );
            }
            return Ok(text(e, "foreman"));
        }
        _ => String::new(),
    };
    if id.is_empty() {
        return Ok(f.into());
    }
    authorize(s, f, &s.workflow(&id)?, name == "foreman_status")
}
