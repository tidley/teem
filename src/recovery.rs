//! Explicit qualification-only successors. Never infer non-launch from error prose alone.
use crate::{checkouts, checks, common::*, store::Store};
use anyhow::{Context, Result, ensure};
use rusqlite::params;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

fn last(w: &Value) -> Result<Value> {
    array(&w["data"]["rounds"])
        .pop()
        .context("No retained candidate")
}

/// Historical Runner::start had no receipt, but did have a synchronous-return
/// contract: after spawn succeeds it cannot return Err, and worker_finished puts
/// the handle in the in-memory round before its next fallible database write.
/// tick's blocked write saves that same round. A crash instead leaves snapshotting
/// (read-reconciled to uncertain), not a synchronous ENOENT terminal outcome.
/// Require the complete controller/artifact conjunction, not merely error prose.
fn non_launch_proof(s: &Store, w: &Value, r: &Value) -> Result<Value> {
    let checkout = PathBuf::from(text(r, "check_checkout"));
    let dir = s
        .directory
        .join("managed")
        .join(text(w, "id"))
        .join("round-0/check-evidence");
    // A partial or changed modern receipt must never downgrade to historical proof.
    if r.get("check_setup_failure").is_some()
        || std::fs::symlink_metadata(dir.join("launch-failure.json")).is_ok()
    {
        let failure = checks::setup_failure(
            &dir,
            &checkout,
            &w["policy"]["checks"],
            &text(&r["snapshot"], "sha256"),
        )?;
        ensure!(
            r["check_setup_failure"] == failure,
            "Retained structured setup-failure receipt changed or is missing"
        );
        return Ok(json!({"kind":"synchronous-spawn-rejection","receipt":failure}));
    }
    let finished = w["data"]["finished_at"]
        .as_f64()
        .context("Missing historical controller completion timestamp")?;
    let created = w["created"]
        .as_f64()
        .context("Missing historical workflow creation timestamp")?;
    ensure!(
        w["request"].get("qualification_recovery").is_none()
            && w["data"].get("resume_phase").is_none()
            && w["detail"] == "No such file or directory (os error 2)"
            && finished.is_finite()
            && created.is_finite()
            && finished >= created,
        "No historical synchronous ENOENT terminal outcome; reconcile uncertain launches by reads"
    );
    let terminal = s.query(
        "SELECT id,task_id,foreman,request_id,transport,workflow_id FROM events WHERE task_id=?",
        [format!("workflow:{}", text(w, "id"))],
    )?;
    ensure!(
        terminal.len() == 1
            && terminal[0]["foreman"] == w["foreman"]
            && terminal[0]["workflow_id"] == w["id"]
            && terminal[0]["transport"] == "native",
        "Missing historical controller terminal event; non-launch provenance is ambiguous"
    );
    uuid(&text(&terminal[0], "id"))?;
    uuid(&text(&terminal[0], "request_id"))?;
    let spec: Value = serde_json::from_slice(&regular(&dir.join("spec.json"), 1048576)?)?;
    ensure!(
        spec.as_object().is_some_and(|m| m.len() == 5)
            && spec["checkout"] == json!(checkout.canonicalize()?)
            && spec["checks"] == w["policy"]["checks"]
            && checks::normalise(&spec["checks"])? == spec["checks"]
            && spec["snapshot"] == r["snapshot"]["sha256"]
            && spec["directory"] == json!(dir)
            && regular(&dir.join("helper.log"), 1048576)?.is_empty(),
        "Historical check specification/log does not match retained admission"
    );
    uuid(&text(&spec, "token"))?;
    for entry in std::fs::read_dir(&dir)? {
        ensure!(
            ["spec.json", "helper.log"]
                .iter()
                .any(|name| entry.as_ref().is_ok_and(|e| e.file_name() == *name)),
            "Historical evidence contains a possible launch/result; read-reconcile, never replay"
        );
    }
    Ok(
        json!({"kind":"historical-controller-enoent-v1","spec_sha256":digest(&spec),"terminal":terminal[0]}),
    )
}

fn lineage(s: &Store, root: &str, successor: Option<&str>, same_source: bool) -> Result<()> {
    let members = s.query(
        "SELECT id FROM workflows WHERE id=? OR COALESCE(json_extract(request,'$.reconcile_root'),json_extract(request,'$.reconcile_from'))=?",
        [root, root],
    )?;
    for row in members {
        let id = text(&row, "id");
        if successor == Some(id.as_str()) {
            let own = s.workflow(&id)?;
            ensure!(
                ["queued", "preparing"].contains(&text(&own, "state").as_str())
                    && own["stop"].is_null()
                    && array(&parent_tasks(s, &own)?).is_empty(),
                "Setup handoff successor already owns native work or is paused"
            );
            continue;
        }
        let w = s.workflow(&id)?;
        ensure!(
            w["stop"].is_null()
                && s.query("SELECT task_id FROM task_stops JOIN tasks ON tasks.id=task_stops.task_id WHERE json_extract(tasks.request,'$.workflow_id')=?", [&id])?.is_empty(),
            "Paused or stopped lineage is not qualification recovery; preserve the confirmed-stop handoff contract"
        );
        ensure!(
            ["ready", "blocked", "needs_attention"].contains(&text(&w, "state").as_str()),
            "Lineage has an active or uncertain workflow; reconcile its existing identity"
        );
        ensure!(
            s.query("SELECT id FROM tasks WHERE json_extract(request,'$.workflow_id')=? AND state NOT IN ('succeeded','failed','interrupted')", [&id])?.is_empty(),
            "Lineage owns a queued, active or uncertain native reservation"
        );
        for round in array(&w["data"]["rounds"]) {
            if let Some(handle) = round.get("check_handle") {
                ensure!(
                    checks::inspect(handle)? == round.get("check_result").cloned()
                        && round.get("check_result").is_some()
                        && handle["snapshot"] == round["snapshot"]["sha256"],
                    "Lineage check launch is active, uncertain or changed; read-reconcile it"
                );
            } else if round.get("check_checkout").is_some() {
                retained_at(s, &w, same_source)?;
            }
        }
    }
    Ok(())
}

fn retained(s: &Store, w: &Value) -> Result<Value> {
    retained_at(s, w, true)
}

/// Only the explicit stopped-input path may supply its own lineage validation.
/// Snapshot, native provenance and positive non-launch requirements stay intact.
pub(crate) fn stopped_input_candidate(s: &Store, w: &Value) -> Result<Value> {
    retained_at(s, w, false)
}

fn retained_at(s: &Store, w: &Value, same_source: bool) -> Result<Value> {
    ensure!(
        w["state"] == "blocked" && w["integration"].is_null(),
        "Only a setup-blocked, unintegrated candidate is eligible"
    );
    let r = last(w)?;
    ensure!(
        r.get("check_handle").is_none()
            && r.get("check_result").is_none()
            && r.get("review_ids").is_none()
            && r.get("review_results").is_none()
            && w["data"].get("resume_phase").is_none(),
        "Prior check/reviewer admission exists; read-reconcile it instead of launching recovery"
    );
    ensure!(
        w["round"].as_u64().context("Invalid round")? == 0
            && array(&w["data"]["rounds"]).len() == 1,
        "Recovery cannot renew a consumed correction budget; explicit scoped reconciliation is required"
    );
    checkouts::verify(&r["snapshot"])?;
    ensure!(
        r["snapshot"]["base"] == w["base_revision"],
        "Retained snapshot base changed"
    );
    let checkout = PathBuf::from(text(&r, "checkout"));
    let base = text(w, "base_revision");
    let tree = text(&r["snapshot"], "tree");
    checkouts::validate(&checkout, &base, &tree)?;
    crate::reconcile::validate_scope(&checkout, w, &tree)?;
    let check_checkout = PathBuf::from(text(&r, "check_checkout"));
    checkouts::validate(&check_checkout, &base, &tree)?;
    non_launch_proof(s, w, &r)?;
    if w["request"].get("qualification_recovery").is_none() {
        let ids = r
            .get("worker_ids")
            .map(array)
            .unwrap_or_else(|| vec![r["worker_id"].clone()]);
        ensure!(
            !ids.is_empty() && ids.len() <= 2,
            "Missing implementation provenance"
        );
        let tasks = parent_tasks(s, w)?;
        let assignments = w["request"]
            .get("workers")
            .map(array)
            .unwrap_or_else(|| vec![json!({"scope":w["request"]["scope"]})]);
        ensure!(
            array(&tasks).len() == ids.len()
                && assignments.len() == ids.len()
                && (ids.len() == 1 || ids[0] != ids[1]),
            "Missing, duplicate or additional prior native launch evidence"
        );
        for (id, assignment) in ids.iter().zip(assignments) {
            let task = s.task(id.as_str().context("Missing worker identity")?)?;
            let native_id = uuid(&text(&task, "id"))?.simple().to_string();
            ensure!(
                task["foreman"] == w["foreman"]
                    && task["session_id"] == format!("ses_foreman_{native_id}")
                    && task["prompt_id"] == format!("msg_foreman_{native_id}")
                    && task["role"] == "worker"
                    && task["state"] == "succeeded"
                    && task["request"]["workflow_id"] == w["id"]
                    && task["request"]["scope"] == assignment["scope"]
                    && task["request"]["issue"] == w["request"]["issue"]
                    && task["request"]["revision"] == w["base_revision"],
                "Implementation provenance changed or is incomplete"
            );
        }
    } else {
        ensure!(
            array(&parent_tasks(s, w)?).is_empty(),
            "Qualification attempt already owns native launch evidence"
        );
        if same_source {
            validate_current(s, w)?;
        } else {
            validate_parent(s, w, false)?;
        }
    }
    Ok(r)
}

pub fn validate_current(s: &Store, w: &Value) -> Result<()> {
    ensure!(
        s.projects[text(w, "project")] == w["policy"],
        "Project policy changed; explicit reconciliation/requalification is required"
    );
    ensure!(
        checkouts::source_revision(Path::new(&text(&w["policy"], "path")))?
            == text(w, "base_revision"),
        "Source advanced; explicit reconciliation and fresh qualification are required"
    );
    validate_parent(s, w, true)
}

// Explicit implementation handoff may advance source/policy, but never bypass
// the immutable qualification ancestry, non-launch proof or stop guards.
fn validate_parent(s: &Store, w: &Value, same_source: bool) -> Result<()> {
    let recovery = &w["request"]["qualification_recovery"];
    let parent = s.workflow(&text(recovery, "source_workflow_id"))?;
    ensure!(
        digest(&parent) == recovery["parent_sha256"],
        "Retained parent evidence changed; recovery refused"
    );
    ensure!(
        digest(&parent_tasks(s, &parent)?) == recovery["parent_tasks_sha256"],
        "Retained native identities or results changed; recovery refused"
    );
    let root = text(&w["request"], "reconcile_root");
    ensure!(s.query("SELECT workflow_stops.workflow_id FROM workflow_stops JOIN workflows ON workflows.id=workflow_stops.workflow_id WHERE workflows.id=? OR COALESCE(json_extract(workflows.request,'$.reconcile_root'),json_extract(workflows.request,'$.reconcile_from'))=?", [&root, &root])?.is_empty(), "Lineage was paused; preserve it without recovery progression");
    ensure!(s.query("SELECT task_stops.task_id FROM task_stops JOIN tasks ON tasks.id=task_stops.task_id JOIN workflows ON workflows.id=json_extract(tasks.request,'$.workflow_id') WHERE workflows.id=? OR COALESCE(json_extract(workflows.request,'$.reconcile_root'),json_extract(workflows.request,'$.reconcile_from'))=?", [&root, &root])?.is_empty(), "Lineage has a task-stop record; preserve it without recovery progression");
    let prior = retained_at(s, &parent, same_source)?;
    ensure!(
        non_launch_proof(s, &parent, &prior)? == recovery["non_launch_proof"],
        "Retained non-launch evidence changed; recovery refused"
    );
    Ok(())
}

fn parent_tasks(s: &Store, parent: &Value) -> Result<Value> {
    Ok(json!(
        s.tasks(Some(&text(parent, "foreman")))?
            .into_iter()
            .filter(|t| t["request"]["workflow_id"] == parent["id"])
            .collect::<Vec<_>>()
    ))
}

/// The same retained/non-launch checks as recovery, without changing recovery's
/// current-source/current-policy admission contract.
pub(crate) fn handoff_proof(s: &Store, parent: &Value, successor: Option<&str>) -> Result<Value> {
    issue_reference(&parent["request"]["issue"])?;
    let root = parent["request"]["reconcile_root"]
        .as_str()
        .unwrap_or(parent["id"].as_str().context("Missing workflow ID")?);
    lineage(s, root, successor, false)?;
    let r = retained_at(s, parent, false)?;
    Ok(
        json!({"parent_sha256":digest(parent),"parent_tasks_sha256":digest(&parent_tasks(s, parent)?),"non_launch_proof":non_launch_proof(s, parent, &r)?}),
    )
}

pub fn submit(s: &Store, f: &str, args: &Value) -> Result<Value> {
    s.binding(f)?;
    ensure!(
        args.as_object().is_some_and(|m| m.len() == 3
            && ["recovery_id", "source_workflow_id", "expected_snapshot"]
                .iter()
                .all(|k| m.contains_key(*k))),
        "Use recovery_id, source_workflow_id and expected_snapshot only"
    );
    let id = text(args, "recovery_id");
    let parent_id = text(args, "source_workflow_id");
    uuid(&id)?;
    uuid(&parent_id)?;
    ensure!(id != parent_id, "Retain a new recovery UUID");
    s.transaction(|| {
        if !s.query("SELECT id FROM workflows WHERE id=?", [&id])?.is_empty() {
            let old = s.workflow(&id)?;
            ensure!(old["foreman"] == f && old["request"]["qualification_recovery"]["arguments"] == *args, "Recovery UUID belongs to another immutable request");
            return Ok(old);
        }
        let parent = s.workflow(&parent_id)?;
        ensure!(parent["foreman"] == f, "Candidate belongs to another Foreman");
        issue_reference(&parent["request"]["issue"])?;
        let root = parent["request"]["reconcile_root"].as_str().unwrap_or(&parent_id);
        lineage(s, root, None, true)?;
        ensure!(s.query("SELECT id FROM workflows WHERE json_extract(request,'$.qualification_recovery.source_workflow_id')=?", [&parent_id])?.is_empty(), "A recovery identity already exists for this candidate; retain it, do not bypass its findings");
        let r = retained(s, &parent)?;
        ensure!(r["snapshot"]["sha256"] == args["expected_snapshot"], "Expected snapshot digest differs from retained candidate");
        let mut req = parent["request"].clone();
        req.as_object_mut().context("Invalid parent request")?.remove("workers");
        // A qualification successor is not an implementation reconciliation.
        req.as_object_mut().unwrap().remove("reconcile_from");
        req.as_object_mut().unwrap().remove("reconcile_blocked_snapshot");
        req.as_object_mut().unwrap().remove("reconcile_setup_handoff");
        req["workflow_id"] = json!(id);
        req["reconcile_root"] = json!(root);
        req["qualification_recovery"] = json!({"arguments":args,"source_workflow_id":parent_id,"parent_sha256":digest(&parent),"parent_tasks_sha256":digest(&parent_tasks(s, &parent)?),"non_launch_proof":non_launch_proof(s, &parent, &r)?});
        let w = json!({"request":req,"project":parent["project"],"policy":parent["policy"],"base_revision":parent["base_revision"]});
        validate_current(s, &w)?;
        let reserved = s.query("SELECT count(*) AS n FROM workflows WHERE foreman=? AND state NOT IN ('ready','blocked','needs_attention')", [f])?;
        ensure!(reserved[0]["n"].as_u64().unwrap_or(32) < 32, "Too many reserved workflows");
        s.exec("INSERT INTO workflows(id,foreman,project,request,policy,base_revision,state,created) VALUES (?,?,?,?,?,?,'qualification_queued',?)", params![id,f,text(&parent,"project"),canonical(&req),canonical(&parent["policy"]),text(&parent,"base_revision"),now()])?;
        s.workflow(&id)
    })
}

pub fn start(s: &Store, w: &mut Value, runner: &mut checks::Runner) -> Result<()> {
    if s.query("SELECT id FROM workflows WHERE state='checking'", [])?
        .len()
        >= 2
    {
        return Ok(());
    }
    let parent = s.workflow(&text(
        &w["request"]["qualification_recovery"],
        "source_workflow_id",
    ))?;
    let prior = retained(s, &parent)?;
    s.save_workflow(
        w,
        "snapshotting",
        "Qualification preparation reserved; interrupted operations are never replayed",
    )?;
    let dir = s
        .directory
        .join("managed")
        .join(text(w, "id"))
        .join("round-0");
    let base = text(w, "base_revision");
    let checkout = checkouts::prepare(
        Path::new(&text(&prior, "checkout")),
        &dir.join("checks-candidate"),
        &base,
        Some(&prior["snapshot"]),
    )?;
    let snapshot = checkouts::capture(
        &checkout,
        &base,
        &scope(&w["request"]["scope"])?,
        &dir.join("snapshot"),
    )?;
    ensure!(
        snapshot["sha256"] == prior["snapshot"]["sha256"]
            && snapshot["tree"] == prior["snapshot"]["tree"],
        "Recovery cannot alter the retained candidate"
    );
    w["data"]["rounds"] =
        json!([{"checkout":checkout,"check_checkout":checkout,"snapshot":snapshot}]);
    s.save_workflow(
        w,
        "snapshotting",
        "Fresh qualification check launch reserved; no automatic replay",
    )?;
    match runner.start(
        &dir.join("check-evidence"),
        &checkout,
        &w["policy"]["checks"],
        &text(&snapshot, "sha256"),
    ) {
        Ok(handle) => w["data"]["rounds"][0]["check_handle"] = handle,
        Err(error) => {
            if let Ok(failure) = checks::setup_failure(
                &dir.join("check-evidence"),
                &checkout,
                &w["policy"]["checks"],
                &text(&snapshot, "sha256"),
            ) {
                w["data"]["rounds"][0]["check_setup_failure"] = failure;
            }
            return Err(error);
        }
    }
    s.save_workflow(
        w,
        "checking",
        "Fresh qualification-only checks running; no implementation or correction worker",
    )
}
