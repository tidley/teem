//! Result presentation lifecycle. Never changes execution, reviews or acknowledgement.
use crate::{
    common::*,
    store::{FINAL, Store},
};
use anyhow::{Context, Result, ensure};
use rusqlite::params;
use serde_json::{Value, json};

pub fn record(s: &Store, id: &str) -> Result<Option<Value>> {
    Ok(s.query(
        "SELECT * FROM result_notices WHERE workflow_id=? ORDER BY seq DESC LIMIT 1",
        [id],
    )?
    .into_iter()
    .next())
}

pub fn update(s: &Store, f: &str, args: &Value) -> Result<Value> {
    s.binding(f)?;
    ensure!(
        args.as_object().is_some_and(|m| m.len() == 3
            && ["workflow_id", "action", "reason"]
                .iter()
                .all(|k| m.contains_key(*k))),
        "Use workflow_id, action and reason only"
    );
    let id = text(args, "workflow_id");
    uuid(&id)?;
    let action = text(args, "action");
    let reason = text(args, "reason");
    ensure!(
        ["archive", "restore"].contains(&action.as_str())
            && !reason.trim().is_empty()
            && reason.chars().count() <= 500
            && !reason.chars().any(char::is_control),
        "Choose archive or restore with a bounded single-line reason"
    );
    s.transaction(|| {
        let w = s.workflow(&id)?;
        ensure!(w["foreman"] == f,"Result belongs to another Teem coordinator");
        ensure!(FINAL.contains(&text(&w,"state").as_str()),"Do not archive active or uncertain work");
        ensure!(s.query("SELECT id FROM tasks WHERE json_extract(request,'$.workflow_id')=? AND (state NOT IN ('succeeded','failed','interrupted') OR EXISTS (SELECT 1 FROM task_stops WHERE task_stops.task_id=tasks.id AND state!='stopped'))",[&id])?.is_empty(),"Result still owns queued, active or uncertain native work");
        for round in array(&w["data"]["rounds"]) {
            if let Some(handle) = round.get("check_handle") {
                ensure!(round.get("check_result").is_some() && crate::checks::inspect(handle)? == round.get("check_result").cloned(),"Check execution is active, uncertain or its evidence changed; reconcile it before archiving");
            } else {
                ensure!(round.get("check_checkout").is_none() || round["check_setup_failure"]["not_started"] == true,"Historical check launch is unproven; preserve its notice until reconciled");
            }
        }
        if let Some(old) = record(s,&id)? {
            if old["action"] == action && old["reason"] == reason { return Ok(old); }
        }
        s.exec("INSERT INTO result_notices(workflow_id,foreman,action,reason,created) VALUES (?,?,?,?,?)",params![id,f,action,reason,now()])?;
        record(s,&id)?.context("Missing saved notice disposition")
    })
}

/// Prefer the newest attempt; follow only explicit same-owner/project lineage.
/// Clearing its notice must never resurrect an older unrelated failure.
pub fn select<'a>(s: &Store, works: &'a [Value]) -> Result<(Option<&'a Value>, Option<Value>)> {
    let time = |w: &Value| {
        w["data"]["finished_at"]
            .as_f64()
            .or(w["created"].as_f64())
            .unwrap_or(0.0)
    };
    let Some(mut current) = works
        .iter()
        .filter(|w| FINAL.contains(&text(w, "state").as_str()))
        .max_by(|a, b| {
            time(a)
                .total_cmp(&time(b))
                .then_with(|| text(a, "id").cmp(&text(b, "id")))
        })
    else {
        return Ok((None, None));
    };
    for _ in 0..=works.len() {
        let successor = works
            .iter()
            .filter(|w| {
                w["id"] != current["id"]
                    && w["foreman"] == current["foreman"]
                    && w["project"] == current["project"]
                    && (w["request"]["reconcile_from"] == current["id"]
                        || w["request"]["qualification_recovery"]["source_workflow_id"]
                            == current["id"])
            })
            .max_by(|a, b| {
                a["created"]
                    .as_f64()
                    .unwrap_or(0.0)
                    .total_cmp(&b["created"].as_f64().unwrap_or(0.0))
                    .then_with(|| text(a, "id").cmp(&text(b, "id")))
            });
        if let Some(next) = successor {
            if !FINAL.contains(&text(next, "state").as_str()) {
                return Ok((
                    None,
                    Some(
                        json!({"workflow_id":current["id"],"action":"superseded","successor_workflow_id":next["id"],"reason":"An explicit same-task successor now owns the work; original result retained in history"}),
                    ),
                ));
            }
            current = next;
            continue;
        }
        if let Some(notice) = record(s, &text(current, "id"))? {
            if notice["action"] == "archive" {
                return Ok((None, Some(notice)));
            }
        }
        return Ok((Some(current), None));
    }
    anyhow::bail!("Result successor lineage contains a cycle; inspect retained history")
}

pub fn blocker(w: &Value) -> Value {
    if w["state"] == "ready" {
        return Value::Null;
    }
    let detail = w
        .get("candidate_detail")
        .unwrap_or(&w["detail"])
        .as_str()
        .unwrap_or("");
    let (kind, reason, action) = if detail == "Source has unfinished changes; none discarded" {
        ("source_changes",detail.to_owned(),"Finish or commit the existing work in this source checkout; preserve unrelated changes. Start a new authorised attempt only if still needed. If handled outside this attempt, archive this saved notice with a reason.".to_owned())
    } else if detail == "Project policy removed or changed; progression stopped" {
        ("policy_changed",detail.to_owned(),"Do not replay the worker. For a prerequisite-only change during implementation, use teem_repair_prerequisites to continue observing its existing identity. Source, check, reference-authority or other policy changes require separate reconciliation; preserve active/uncertain work.".to_owned())
    } else if let Some(review) = w["data"]["rounds"]
        .as_array()
        .and_then(|r| r.last())
        .and_then(|r| r["review_results"].as_array().map(|reviews| (r, reviews)))
        .and_then(|(round, reviews)| {
            reviews
                .iter()
                .filter(|v| {
                    v["snapshot"].is_string()
                        && v["snapshot"] == round["snapshot"]["sha256"]
                        && ["blocked", "changes_requested"].contains(&text(v, "verdict").as_str())
                })
                .min_by_key(|v| if v["verdict"] == "blocked" { 0 } else { 1 })
        })
    {
        let blocked = review["verdict"] == "blocked";
        (if blocked { "review_blocked" } else { "review_not_approved" }, if blocked {text(review,"reason")} else {text(&review["findings"][0],"message")}, "Teem must resolve the recorded review reason and requalify the retained candidate. Do not replay workers for a setup failure. If superseded by verified work elsewhere, archive this saved notice with a reason.".into())
    } else {
        ("stopped_attempt", if detail.is_empty() {"No detailed controller reason recorded".into()} else {short(detail,800)}, "Inspect the recorded task/check evidence and resolve the stated blocker. If resolved or superseded outside Teem, archive this saved notice with a reason; do not mark execution or reviews passed.".into())
    };
    json!({"kind":kind,"project":w["project"],"source":w["policy"]["path"],"reason":short(&reason,800),"action":action,"current_source_checked":false})
}
