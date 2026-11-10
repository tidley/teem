//! Same-task continuations preserve old evidence and qualify a fresh candidate.
use crate::{
    checkouts,
    common::*,
    store::{LIVE, Store},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::path::Path;

fn candidate(s: &Store, f: &str, id: &str) -> Result<(Value, Value)> {
    let w = s.workflow(id)?;
    ensure!(w["foreman"] == f, "Candidate belongs to another Foreman");
    let stopped = w["state"] == "blocked"
        && w["stop"]["workflow_paused"] == true
        && w["stop"]["state"] == "stopped";
    ensure!(
        ["ready", "needs_attention"].contains(&text(&w, "state").as_str()) || stopped,
        "Only a ready, exhausted or confirmed-stopped candidate can be reconciled; never replace active or uncertain work"
    );
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
    checkouts::verify(&r["snapshot"])?;
    checkouts::validate(
        Path::new(&text(&r, "checkout")),
        &text(&w, "base_revision"),
        &text(&r["snapshot"], "tree"),
    )?;
    if stopped {
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
    Ok((w, r))
}

pub fn submit(s: &Store, f: &str, args: &Value) -> Result<Value> {
    s.binding(f)?;
    let id = text(args, "workflow_id");
    uuid(&id)?;
    let parent = text(args, "source_workflow_id");
    uuid(&parent)?;
    ensure!(id != parent, "Use a new retained workflow ID");
    let (w, r) = candidate(s, f, &parent)?;
    ensure!(
        s.projects[text(&w, "project")]["path"] == w["policy"]["path"],
        "Original project identity removed or changed"
    );
    let mut req = w["request"].clone();
    req.as_object_mut()
        .context("Invalid prior request")?
        .remove("workers");
    req["workflow_id"] = json!(id);
    req["reconcile_from"] = json!(parent);
    req["reconcile_root"] = w["request"]
        .get("reconcile_root")
        .cloned()
        .unwrap_or_else(|| w["id"].clone());
    req["reconcile_snapshot"] = r["snapshot"]["sha256"].clone();
    s.workflow_submit(f, req)
}

pub fn seed(s: &Store, w: &Value, checkout: &Path) -> Result<Option<String>> {
    let Some(parent) = w["request"]["reconcile_from"].as_str() else {
        return Ok(None);
    };
    let (old, r) = candidate(s, &text(w, "foreman"), parent)?;
    ensure!(
        old["policy"]["path"] == w["policy"]["path"]
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
    let notes = json!({"source_workflow_id":parent,"old_base":old["base_revision"],"new_base":w["base_revision"],"snapshot":r["snapshot"]["sha256"],"seeded":seeded,"review_results":r["review_results"],"checks":r["check_result"],"meaning":"Untrusted prior evidence; reconcile only the same task scope. All checks and two reviews must run afresh."});
    write_new(
        &checkout.join(".git/teem-reconciliation.json"),
        canonical(&notes).as_bytes(),
        0o600,
    )?;
    let tree = checkouts::candidate_tree(checkout, &text(w, "base_revision"))?;
    validate_scope(checkout, w, &tree)?;
    Ok(Some(tree))
}

fn validate_scope(checkout: &Path, w: &Value, tree: &str) -> Result<()> {
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
