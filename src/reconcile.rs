//! Same-task continuations preserve old evidence and qualify a fresh candidate.
use crate::{
    checkouts, checks,
    common::*,
    store::{LIVE, Store},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::path::Path;

fn quiescent_lineage(s: &Store, w: &Value, successor: Option<&str>) -> Result<()> {
    let root = w["request"]["reconcile_root"]
        .as_str()
        .unwrap_or(w["id"].as_str().context("Missing workflow ID")?);
    for row in s.query("SELECT id FROM workflows WHERE id=? OR COALESCE(json_extract(request,'$.reconcile_root'),json_extract(request,'$.reconcile_from'))=?", [root, root])? {
        let id = text(&row, "id");
        // The same UUID is a read-idempotent admission retry, or the currently
        // preparing successor at seed time. It is never a replacement identity.
        if successor == Some(id.as_str()) {
            continue;
        }
        let member = s.workflow(&id)?;
        ensure!(
            member["stop"].is_null()
                && ["ready", "blocked", "needs_attention"].contains(&text(&member,"state").as_str()),
            "Lineage is paused or has an active workflow; preserve its identity"
        );
        ensure!(s.query("SELECT id FROM tasks WHERE json_extract(request,'$.workflow_id')=? AND (state NOT IN ('succeeded','failed','interrupted') OR EXISTS (SELECT 1 FROM task_stops WHERE task_id=tasks.id AND state!='stopped'))", [&id])?.is_empty(), "Lineage still owns active, uncertain or unconfirmed-stop native work");
        for round in array(&member["data"]["rounds"]) {
            if let Some(handle) = round.get("check_handle") {
                let receipt = checks::inspect(handle)?.context("Lineage check launch has no verified receipt")?;
                ensure!(Some(&receipt) == round.get("check_result") && receipt["snapshot"] == round["snapshot"]["sha256"], "Lineage check evidence changed or is not recorded");
            } else {
                ensure!(round.get("check_checkout").is_none(), "Historical check setup has no launch receipt; this is not proof of non-launch");
            }
        }
    }
    Ok(())
}

fn completed_review_handoff(s: &Store, w: &Value, successor: Option<&str>) -> Result<()> {
    ensure!(
        w["state"] == "blocked" && w["stop"].is_null() && w["integration"].is_null(),
        "Review handoff requires an unpaused, unintegrated blocked candidate"
    );
    quiescent_lineage(s, w, successor)?;
    let round = array(&w["data"]["rounds"])
        .pop()
        .context("No retained candidate")?;
    ensure!(
        round["check_result"]["passed"] == true,
        "Review handoff requires completed passing checks"
    );
    let ids = array(&round["review_ids"]);
    ensure!(
        ids.len() == 2 && ids[0] != ids[1],
        "Review handoff requires two distinct finished reviewers"
    );
    let mut invalid = false;
    for id in ids {
        let t = s.task(id.as_str().context("Invalid reviewer identity")?)?;
        ensure!(
            t["foreman"] == w["foreman"]
                && t["request"]["workflow_id"] == w["id"]
                && t["role"] == "reviewer"
                && t["state"] == "succeeded"
                && t["request"]["input_tree"] == round["snapshot"]["tree"],
            "Reviewer provenance changed or execution is unfinished"
        );
        invalid |= crate::workflows::verdict(&text(&t, "result"), &round["snapshot"]).is_err();
    }
    ensure!(
        invalid,
        "No rejected review output; use the existing qualified/exhausted candidate contract"
    );
    Ok(())
}

fn unlaunched_revision(s: &Store, w: &Value) -> Result<bool> {
    let rounds = array(&w["data"]["rounds"]);
    if w["state"] != "blocked"
        || !w["stop"].is_null()
        || !w["integration"].is_null()
        || w["detail"] != "Managed Git timed out; preserve checkout"
        || rounds.is_empty()
        || w["round"].as_u64() != Some(rounds.len() as u64)
    {
        return Ok(false);
    }
    // prepare() increments the round before checkout creation, and records its
    // tasks before any native admission. No reservation (even terminal) may
    // exist for either deterministic worker identity in that unrecorded round.
    for purpose in ["worker", "worker-2"] {
        if !s
            .query(
                "SELECT id FROM tasks WHERE id=?",
                [crate::workflows::id(w, purpose)?],
            )?
            .is_empty()
        {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) fn candidate(
    s: &Store,
    f: &str,
    id: &str,
    blocked_snapshot: Option<&str>,
    setup_snapshot: Option<&str>,
    successor: Option<&str>,
) -> Result<(Value, Value, Option<Value>)> {
    let w = s.workflow(id)?;
    ensure!(
        w["foreman"] == f,
        "Candidate belongs to another Teem coordinator"
    );
    let stopped = w["state"] == "blocked"
        && w["stop"]["workflow_paused"] == true
        && w["stop"]["state"] == "stopped";
    if blocked_snapshot.is_some() {
        completed_review_handoff(s, &w, successor)?;
    }
    if let Some(expected) = setup_snapshot {
        let proof = crate::recovery::handoff_proof(s, &w, successor)?;
        let r = array(&w["data"]["rounds"])
            .pop()
            .context("No retained candidate")?;
        ensure!(
            r["snapshot"]["sha256"] == expected,
            "Setup handoff snapshot digest changed"
        );
        return Ok((w, r, Some(proof)));
    }
    let unlaunched = unlaunched_revision(s, &w)?;
    ensure!(
        ["ready", "needs_attention"].contains(&text(&w, "state").as_str())
            || stopped
            || unlaunched
            || blocked_snapshot.is_some(),
        "Only a ready, exhausted, unlaunched Git-preparation failure or confirmed-stopped candidate can be reconciled; never replace active or uncertain work"
    );
    if unlaunched {
        quiescent_lineage(s, &w, successor)?;
    }
    if stopped {
        let root = w["request"]["reconcile_root"].as_str().unwrap_or(id);
        // Check native reservations, not workflow preparation state: seed runs
        // while the admitted successor itself is preparing its first worker.
        ensure!(
            s.query("SELECT tasks.id FROM tasks JOIN workflows ON json_extract(tasks.request,'$.workflow_id')=workflows.id WHERE (workflows.id=? OR COALESCE(json_extract(workflows.request,'$.reconcile_root'),json_extract(workflows.request,'$.reconcile_from'))=?) AND (tasks.state IN ('queued','starting','accepted','running','uncertain') OR EXISTS (SELECT 1 FROM task_stops WHERE task_stops.task_id=tasks.id AND task_stops.state!='stopped'))", [root, root])?.is_empty(),
            "Stopped candidate lineage still owns active or uncertain native tasks"
        );
    }
    ensure!(
        !s.tasks(Some(f))?
            .iter()
            .any(|t| t["request"]["workflow_id"] == id
                && (t["state"] == "queued" || LIVE.contains(&text(t, "state").as_str()))),
        "Candidate still owns active or uncertain native tasks"
    );
    let r = array(&w["data"]["rounds"])
        .pop()
        .context("No retained candidate")?;
    if stopped && r.get("snapshot").is_none() {
        let (source, round) = crate::stopped_input::candidate(s, f, &w, successor)?;
        return Ok((source, round, None));
    }
    if let Some(expected) = blocked_snapshot {
        ensure!(
            r["snapshot"]["sha256"] == expected,
            "Blocked handoff snapshot digest changed"
        );
    }
    checkouts::verify(&r["snapshot"])?;
    checkouts::validate(
        Path::new(&text(&r, "checkout")),
        &text(&w, "base_revision"),
        &text(&r["snapshot"], "tree"),
    )?;
    if stopped || unlaunched || blocked_snapshot.is_some() {
        ensure!(
            r["snapshot"]["base"] == w["base_revision"],
            "Retained snapshot base changed"
        );
        validate_scope(
            Path::new(&text(&r, "checkout")),
            &w,
            &text(&r["snapshot"], "tree"),
        )?;
    }
    Ok((w, r, None))
}

pub fn submit(s: &Store, f: &str, args: &Value) -> Result<Value> {
    s.transaction(|| submit_reserved(s, f, args))
}

fn submit_reserved(s: &Store, f: &str, args: &Value) -> Result<Value> {
    s.binding(f)?;
    let id = text(args, "workflow_id");
    uuid(&id)?;
    let parent = text(args, "source_workflow_id");
    uuid(&parent)?;
    ensure!(id != parent, "Use a new retained workflow ID");
    if !s
        .query("SELECT id FROM workflows WHERE id=?", [&id])?
        .is_empty()
    {
        let old = s.workflow(&id)?;
        if let Some(handoff) = old["request"].get("reconcile_stopped_input") {
            ensure!(
                old["foreman"] == f && handoff["arguments"] == *args,
                "Workflow UUID belongs to another immutable request"
            );
            return Ok(old);
        }
    }
    let blocked_snapshot = args
        .get("blocked_snapshot")
        .map(|v| {
            v.as_str()
                .context("blocked_snapshot must be an exact retained digest")
        })
        .transpose()?;
    let setup_snapshot = args
        .get("setup_snapshot")
        .map(|v| {
            v.as_str()
                .context("setup_snapshot must be an exact retained digest")
        })
        .transpose()?;
    ensure!(
        blocked_snapshot.is_none() || setup_snapshot.is_none(),
        "Choose one explicit blocked handoff contract"
    );
    if setup_snapshot.is_some() {
        ensure!(
            args.as_object().is_some_and(|m| m.keys().all(|k| [
                "workflow_id",
                "source_workflow_id",
                "setup_snapshot"
            ]
            .contains(&k.as_str()))),
            "Setup handoff inherits its issue and scope; use workflow_id, source_workflow_id and setup_snapshot only"
        );
        if !s
            .query("SELECT id FROM workflows WHERE id=?", [&id])?
            .is_empty()
        {
            let old = s.workflow(&id)?;
            ensure!(
                old["foreman"] == f
                    && old["request"]["reconcile_setup_handoff"]["arguments"] == *args,
                "Workflow UUID belongs to another immutable request"
            );
            return Ok(old);
        }
        ensure!(s.query("SELECT id FROM workflows WHERE json_extract(request,'$.reconcile_setup_handoff.arguments.source_workflow_id')=?", [&parent])?.is_empty(), "A setup handoff identity already exists; retain it and reconcile its findings explicitly");
    }
    let (w, r, setup_proof) =
        candidate(s, f, &parent, blocked_snapshot, setup_snapshot, Some(&id))?;
    ensure!(
        s.projects[text(&w, "project")]["path"] == w["policy"]["path"],
        "Original project identity removed or changed"
    );
    if blocked_snapshot.is_some() || setup_snapshot.is_some() || w["id"] != parent {
        checkouts::source_revision(Path::new(&text(&s.projects[text(&w, "project")], "path")))?;
    }
    let mut req = w["request"].clone();
    if let Some(issue) = args.get("issue") {
        let issue = issue_reference(issue)?;
        if let Some(inherited) = req.get("issue") {
            ensure!(
                issue == issue_reference(inherited)?,
                "Continuation cannot switch its inherited issue"
            );
        }
        req["issue"] = json!(issue);
    }
    let request = req.as_object_mut().context("Invalid prior request")?;
    for key in [
        "workers",
        "qualification_recovery",
        "reconcile_blocked_snapshot",
        "reconcile_setup_handoff",
        "reconcile_stopped_input",
        "reconcile_candidate_source",
    ] {
        request.remove(key);
    }
    if let Some(expected) = blocked_snapshot {
        req["reconcile_blocked_snapshot"] = json!(expected);
    }
    if let Some(proof) = setup_proof {
        req["reconcile_setup_handoff"] = json!({"arguments":args,"proof":proof});
    }
    if w["id"] != parent {
        req["reconcile_stopped_input"] = json!({"arguments":args,"evidence":crate::stopped_input::proof(s, &s.workflow(&parent)?)?});
        req["reconcile_candidate_source"] = w["id"].clone();
    }
    req["workflow_id"] = json!(id);
    req["reconcile_from"] = json!(parent);
    req["reconcile_root"] = w["request"]
        .get("reconcile_root")
        .cloned()
        .unwrap_or_else(|| w["id"].clone());
    req["reconcile_snapshot"] = r["snapshot"]["sha256"].clone();
    crate::workflows::submit_reserved(s, f, req)
}

/// Revalidate the pinned admission before preparing a native implementation task.
pub(crate) fn validate_setup(
    s: &Store,
    f: &str,
    req: &Value,
    successor: Option<&str>,
) -> Result<Option<(Value, Value)>> {
    let Some(handoff) = req.get("reconcile_setup_handoff") else {
        return Ok(None);
    };
    let (parent, round, proof) = candidate(
        s,
        f,
        &text(req, "reconcile_from"),
        None,
        handoff["arguments"]["setup_snapshot"].as_str(),
        successor,
    )?;
    ensure!(
        s.projects[text(&parent, "project")]["path"] == parent["policy"]["path"]
            && proof.as_ref() == Some(&handoff["proof"]),
        "Retained setup handoff identity or provenance changed"
    );
    checkouts::source_revision(Path::new(&text(
        &s.projects[text(&parent, "project")],
        "path",
    )))?;
    Ok(Some((parent, round)))
}

pub fn seed(s: &Store, w: &Value, checkout: &Path) -> Result<Option<String>> {
    let Some(parent) = w["request"]["reconcile_from"].as_str() else {
        return Ok(None);
    };
    if let Some(expected) = w["request"].get("reconcile_stopped_input") {
        ensure!(
            crate::stopped_input::proof(s, &s.workflow(parent)?)? == expected["evidence"],
            "Stopped workflow or worker evidence changed before handoff"
        );
    }
    let setup = validate_setup(s, &text(w, "foreman"), &w["request"], w["id"].as_str())?;
    if w["request"].get("reconcile_setup_handoff").is_some() {
        ensure!(
            s.projects[text(w, "project")] == w["policy"]
                && checkouts::source_revision(Path::new(&text(&w["policy"], "path")))?
                    == text(w, "base_revision"),
            "Current setup handoff source or policy changed during preparation"
        );
    }
    let (old, r) = if let Some(setup) = setup {
        setup
    } else {
        let (old, r, _) = candidate(
            s,
            &text(w, "foreman"),
            parent,
            w["request"]["reconcile_blocked_snapshot"].as_str(),
            None,
            w["id"].as_str(),
        )?;
        (old, r)
    };
    ensure!(
        old["policy"]["path"] == w["policy"]["path"]
            && w["request"]
                .get("reconcile_candidate_source")
                .is_none_or(|id| *id == old["id"])
            && r["snapshot"]["sha256"] == w["request"]["reconcile_snapshot"],
        "Retained source identity or candidate changed"
    );
    let patch = checkouts::verify(&r["snapshot"])?;
    write_new(
        &checkout.join(".git/teem-reconciliation.patch"),
        &patch,
        0o600,
    )?;
    let seeded = patch.is_empty()
        || git(
            checkout,
            &["apply", "--check", "--binary", "-"],
            Some(&patch),
            None,
        )
        .is_ok();
    if seeded && !patch.is_empty() {
        git(
            checkout,
            &["apply", "--binary", "--index", "-"],
            Some(&patch),
            None,
        )?;
    }
    let mut unqualified = Vec::new();
    if w["request"].get("reconcile_blocked_snapshot").is_some() {
        for id in array(&r["review_ids"]) {
            let task = s.task(id.as_str().context("Invalid reviewer identity")?)?;
            unqualified.push(json!({"task_id":id,"result":task["result"],"meaning":"Rejected output; not qualification evidence"}));
        }
    }
    let notes = json!({"source_workflow_id":parent,"candidate_source_workflow_id":old["id"],"source_detail":old["detail"],"setup_failure":r["check_setup_failure"],"old_base":old["base_revision"],"new_base":w["base_revision"],"snapshot":r["snapshot"]["sha256"],"seeded":seeded,"review_results":r["review_results"],"unqualified_review_outputs":unqualified,"checks":r["check_result"],"meaning":"Untrusted prior evidence; reconcile only the same task scope. All checks and two reviews must run afresh."});
    write_new(
        &checkout.join(".git/teem-reconciliation.json"),
        canonical(&notes).as_bytes(),
        0o600,
    )?;
    let tree = checkouts::candidate_tree(checkout, &text(w, "base_revision"))?;
    validate_scope(checkout, w, &tree)?;
    Ok(Some(tree))
}

pub(crate) fn validate_scope(checkout: &Path, w: &Value, tree: &str) -> Result<()> {
    let names = git(
        checkout,
        &[
            "diff",
            "--name-only",
            "--no-renames",
            "-z",
            &text(w, "base_revision"),
            tree,
            "--",
        ],
        None,
        None,
    )?;
    let scope = scope(&w["request"]["scope"])?;
    for path in names.split(|b| *b == 0).filter(|b| !b.is_empty()) {
        let path = std::str::from_utf8(path)?;
        relative(path)?;
        ensure!(
            in_scope(path, &scope),
            "Reconciled seed exceeds original scope"
        );
    }
    Ok(())
}
