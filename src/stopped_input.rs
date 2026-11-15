//! A stopped, unchanged failed-seed attempt may retain its parent's real patch.
use crate::{checkouts, checks, common::*, recovery, store::Store};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::path::Path;

fn worker(s: &Store, w: &Value) -> Result<Value> {
    let rounds = array(&w["data"]["rounds"]);
    ensure!(
        w["round"] == 0 && rounds.len() == 1,
        "Only an initial stopped input can use its earlier candidate"
    );
    let r = &rounds[0];
    ensure!(
        r.as_object().is_some_and(|m| m
            .keys()
            .all(|k| ["checkout", "worker_id", "worker_ids"].contains(&k.as_str()))),
        "Stopped attempt has output or qualification evidence; preserve its own candidate"
    );
    let ids = array(&r["worker_ids"]);
    ensure!(
        ids.len() == 1 && ids[0] == r["worker_id"],
        "Stopped input requires one exact implementation worker"
    );
    let t = s.task(ids[0].as_str().context("Missing stopped worker identity")?)?;
    let native_id = uuid(&text(&t, "id"))?.simple().to_string();
    ensure!(
        t["foreman"] == w["foreman"]
            && t["role"] == "worker"
            && t["state"] == "interrupted"
            && t["session_id"] == format!("ses_foreman_{native_id}")
            && t["prompt_id"] == format!("msg_foreman_{native_id}")
            && t["request"]["workflow_id"] == w["id"]
            && t["checkout"] == r["checkout"]
            && t["request"]["revision"] == w["base_revision"]
            && t["request"]["scope"] == w["request"]["scope"]
            && t["request"]["issue"] == w["request"]["issue"],
        "Stopped implementation provenance changed"
    );
    ensure!(
        s.query(
            "SELECT id FROM tasks WHERE json_extract(request,'$.workflow_id')=?",
            [text(w, "id")]
        )?
        .len()
            == 1,
        "Stopped attempt owns additional native work"
    );
    let stops = s.query(
        "SELECT state FROM task_stops WHERE task_id=?",
        [text(&t, "id")],
    )?;
    ensure!(
        stops.len() == 1 && stops[0]["state"] == "stopped",
        "Native stop is not confirmed"
    );
    Ok(t)
}

fn lineage(s: &Store, stopped: &Value, successor: Option<&str>) -> Result<()> {
    let root = stopped["request"]["reconcile_root"]
        .as_str()
        .context("Missing continuation root")?;
    for row in s.query("SELECT id FROM workflows WHERE id=? OR COALESCE(json_extract(request,'$.reconcile_root'),json_extract(request,'$.reconcile_from'))=?", [root, root])? {
        let id = text(&row,"id");
        let w = s.workflow(&id)?;
        if successor == Some(id.as_str()) {
            ensure!(["queued","preparing"].contains(&text(&w,"state").as_str()) && w["stop"].is_null()
                && s.query("SELECT id FROM tasks WHERE json_extract(request,'$.workflow_id')=?", [&id])?.is_empty(),
                "Stopped-input successor already owns native work or is paused");
            continue;
        }
        ensure!(["ready","blocked","needs_attention"].contains(&text(&w,"state").as_str()), "Lineage still owns an active workflow");
        if w["id"] != stopped["id"] {
            ensure!(w["stop"].is_null() && s.query("SELECT task_id FROM task_stops JOIN tasks ON tasks.id=task_stops.task_id WHERE json_extract(tasks.request,'$.workflow_id')=?", [&id])?.is_empty(),
                "Another lineage member is paused or stopped; no implicit handoff");
        }
        ensure!(s.query("SELECT id FROM tasks WHERE json_extract(request,'$.workflow_id')=? AND (state NOT IN ('succeeded','failed','interrupted') OR EXISTS (SELECT 1 FROM task_stops WHERE task_id=tasks.id AND state!='stopped'))", [&id])?.is_empty(),
            "Lineage owns active, uncertain or unconfirmed-stop native work");
        for r in array(&w["data"]["rounds"]) {
            if let Some(handle) = r.get("check_handle") {
                let receipt = checks::inspect(handle)?.context("Lineage check launch is unresolved")?;
                ensure!(Some(&receipt) == r.get("check_result") && receipt["snapshot"] == r["snapshot"]["sha256"], "Lineage check evidence changed");
            } else if r.get("check_checkout").is_some() {
                recovery::stopped_input_candidate(s, &w)?;
            }
        }
    }
    Ok(())
}

pub(crate) fn candidate(
    s: &Store,
    f: &str,
    w: &Value,
    successor: Option<&str>,
) -> Result<(Value, Value)> {
    ensure!(
        w["foreman"] == f
            && w["state"] == "blocked"
            && w["stop"]["workflow_paused"] == true
            && w["stop"]["state"] == "stopped"
            && w["integration"].is_null(),
        "Stopped input requires a confirmed paused, unintegrated source"
    );
    let t = worker(s, w)?;
    let checkout = Path::new(t["checkout"].as_str().context("Missing stopped checkout")?);
    let base = text(w, "base_revision");
    let input = text(&t["request"], "input_tree");
    ensure!(
        input == git_text(checkout, &["rev-parse", "HEAD^{tree}"])?,
        "Stopped input was seeded or modified; preserve its own work"
    );
    git(
        checkout,
        &[
            "diff",
            "--cached",
            "--quiet",
            "--no-ext-diff",
            "--no-textconv",
            &input,
            "--",
        ],
        None,
        None,
    )
    .context("Stopped worker index has changes; do not replace its staged work")?;
    checkouts::validate(checkout, &base, &input)
        .context("Stopped worker has changes; do not replace its work")?;
    let parent_id = text(&w["request"], "reconcile_from");
    uuid(&parent_id).context(
        "Stopped attempt has no captured snapshot or pinned earlier input; preserve its checkout",
    )?;
    ensure!(parent_id != w["id"], "Stopped input cannot refer to itself");
    let parent = s.workflow(&parent_id)?;
    ensure!(
        parent["stop"].is_null() && parent["integration"].is_null(),
        "Earlier candidate is paused or integrated"
    );
    for field in ["project", "issue", "scope", "prompt"] {
        ensure!(
            parent["request"][field] == w["request"][field],
            "Earlier candidate changed task {field}"
        );
    }
    let root = parent["request"]["reconcile_root"]
        .as_str()
        .unwrap_or(&parent_id);
    ensure!(
        parent["foreman"] == f
            && parent["policy"]["path"] == w["policy"]["path"]
            && w["request"]["reconcile_root"] == root,
        "Earlier candidate identity or lineage changed"
    );
    let (parent, r) = if parent["state"] == "blocked" {
        let r = recovery::stopped_input_candidate(s, &parent)?;
        (parent, r)
    } else {
        // Require a real immediate-parent snapshot before using the usual contract.
        ensure!(
            array(&parent["data"]["rounds"])
                .last()
                .is_some_and(|r| r["snapshot"].is_object()),
            "Earlier workflow has no retained candidate snapshot"
        );
        let (parent, r, _) = crate::reconcile::candidate(s, f, &parent_id, None, None, successor)?;
        (parent, r)
    };
    ensure!(
        r["snapshot"]["sha256"] == w["request"]["reconcile_snapshot"],
        "Earlier candidate digest differs from stopped input"
    );
    let patch = checkouts::verify(&r["snapshot"])?;
    ensure!(
        regular(
            &checkout.join(".git/teem-reconciliation.patch"),
            64 * 1024 * 1024
        )? == patch,
        "Stopped input patch changed"
    );
    let notes = read_json(&checkout.join(".git/teem-reconciliation.json"))?;
    ensure!(
        notes["source_workflow_id"] == parent["id"]
            && notes["snapshot"] == r["snapshot"]["sha256"]
            && notes["new_base"] == w["base_revision"]
            && notes["old_base"] == parent["base_revision"]
            && notes["seeded"] == false,
        "Stopped input does not retain the exact failed-seed provenance"
    );
    lineage(s, w, successor)?;
    Ok((parent, r))
}

pub(crate) fn proof(s: &Store, w: &Value) -> Result<Value> {
    Ok(serde_json::json!({"workflow_sha256":digest(w),"worker_sha256":digest(&worker(s,w)?)}))
}
