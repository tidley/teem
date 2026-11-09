use crate::{checkouts, checks, common::*, store::Store, workflows};
use anyhow::{Context, Result, ensure};
use rusqlite::params;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub fn receipt(s: &Store, id: &str) -> Result<Value> {
    s.query("SELECT receipt FROM integrations WHERE workflow_id=?", [id])?
        .first()
        .map(|r| {
            serde_json::from_str(
                r["receipt"]
                    .as_str()
                    .context("Invalid integration receipt")?,
            )
            .map_err(Into::into)
        })
        .unwrap_or(Ok(Value::Null))
}

fn save(s: &Store, id: &str, value: &Value) -> Result<()> {
    s.exec("INSERT INTO integrations(workflow_id,receipt) VALUES (?,?) ON CONFLICT(workflow_id) DO UPDATE SET receipt=excluded.receipt",params![id,canonical(value)])?;
    Ok(())
}

pub fn description(value: &Value) -> String {
    match text(value, "state").as_str() {
        "committed" => format!(
            "Recorded integration commit {}; publication not checked",
            text(value, "commit")
        ),
        "applied" => "Applied locally; commit and publication not recorded".into(),
        _ => "Integration attempt uncertain; inspect source without replay".into(),
    }
}

fn source(s: &Store, w: &Value) -> Result<PathBuf> {
    ensure!(
        s.projects[text(w, "project")] == w["policy"],
        "Project policy changed; integration is not authorised"
    );
    let root = PathBuf::from(text(&w["policy"], "path"));
    ensure!(
        root.canonicalize()? == root
            && git_text(&root, &["rev-parse", "--show-toplevel"])? == root.to_string_lossy(),
        "Configured source root changed"
    );
    Ok(root)
}

fn qualified(s: &Store, w: &Value) -> Result<Value> {
    ensure!(
        w["state"] == "ready",
        "Only a ready workflow can be integrated"
    );
    let round = w["data"]["rounds"]
        .as_array()
        .and_then(|a| a.last())
        .context("Candidate snapshot missing")?;
    checkouts::verify(&round["snapshot"])?;
    ensure!(
        checks::inspect(&round["check_handle"])? == Some(round["check_result"].clone())
            && round["check_result"]["passed"] == true,
        "Approved check evidence no longer qualifies"
    );
    checkouts::validate(
        Path::new(&text(round, "check_checkout")),
        &text(w, "base_revision"),
        &text(&round["snapshot"], "tree"),
    )?;
    let ids = array(&round["review_ids"]);
    ensure!(
        ids.len() == 2 && ids[0] != ids[1],
        "Two independent reviewers are required"
    );
    let mut results = Vec::new();
    for id in ids {
        let task = s.task(id.as_str().context("Invalid review ID")?)?;
        ensure!(
            task["foreman"] == w["foreman"]
                && task["request"]["workflow_id"] == w["id"]
                && task["role"] == "reviewer"
                && task["state"] == "succeeded"
                && task["request"]["input_tree"] == round["snapshot"]["tree"],
            "Review execution does not qualify this candidate"
        );
        checkouts::validate(
            Path::new(&text(&task, "checkout")),
            &text(w, "base_revision"),
            &text(&round["snapshot"], "tree"),
        )?;
        let verdict = workflows::verdict(&text(&task, "result"), &round["snapshot"])?;
        ensure!(verdict["verdict"] == "pass", "Both reviewers must pass");
        results.push(verdict);
    }
    ensure!(
        json!(results) == round["review_results"],
        "Recorded reviews changed"
    );
    Ok(round["snapshot"].clone())
}

pub fn integrate(s: &Store, f: &str, args: &Value) -> Result<Value> {
    s.binding(f)?;
    let id = text(args, "workflow_id");
    uuid(&id)?;
    let w = s.workflow(&id)?;
    ensure!(
        w["foreman"] == f,
        "Workflow does not belong to this Foreman"
    );
    let root = source(s, &w)?;
    let lock = open_lock(&s.directory.join("integration.lock"))?;
    // SAFETY: lock stays owned for the complete filesystem operation.
    use std::os::fd::AsRawFd;
    ensure!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "Another integration is in progress"
    );
    let snapshot = qualified(s, &w)?;
    let old = receipt(s, &id)?;
    let mut value = if old.is_null() {
        json!({"workflow_id":id,"source":root,"base":w["base_revision"],"snapshot":snapshot["sha256"],"tree":snapshot["tree"],"state":"attempting","started":now()})
    } else {
        old
    };
    ensure!(
        value["snapshot"] == snapshot["sha256"] && value["source"] == json!(root),
        "Integration receipt does not match candidate"
    );
    if let Some(commit) = args.get("commit") {
        let commit = commit.as_str().context("Use a full commit ID")?;
        ensure!(oid(commit), "Use a full commit ID");
        ensure!(
            git_text(&root, &["rev-parse", &format!("{commit}^{{tree}}")])? == snapshot["tree"],
            "Commit is not the exact reviewed candidate"
        );
        git(
            &root,
            &[
                "merge-base",
                "--is-ancestor",
                &text(&w, "base_revision"),
                commit,
            ],
            None,
            None,
        )?;
        git(
            &root,
            &["merge-base", "--is-ancestor", commit, "HEAD"],
            None,
            None,
        )?;
        ensure!(
            value["state"] != "committed" || value["commit"] == commit,
            "A different integration commit is already recorded"
        );
        if value["state"] != "committed" {
            value["state"] = json!("committed");
            value["commit"] = json!(commit);
            value["recorded_at"] = json!(now());
            save(s, &id, &value)?;
        }
        return Ok(value);
    }
    if value["state"] == "committed" {
        return Ok(value);
    }
    if !receipt(s, &id)?.is_null() {
        checkouts::validate(&root, &text(&w, "base_revision"), &text(&snapshot, "tree")).context(
            "Previous integration needs manual inspection; never repeat an uncertain apply",
        )?;
    } else {
        ensure!(
            checkouts::source_revision(&root)? == w["base_revision"],
            "Source HEAD changed; rebase and requalify instead of applying an old candidate"
        );
        let patch = checkouts::verify(&snapshot)?;
        if !patch.is_empty() {
            git(
                &root,
                &["apply", "--check", "--binary", "-"],
                Some(&patch),
                None,
            )?;
        }
        // Commit the marker before touching source. A crash can only reconcile by reads.
        save(s, &id, &value)?;
        if !patch.is_empty() {
            git(&root, &["apply", "--binary", "-"], Some(&patch), None)?;
        }
        checkouts::validate(&root, &text(&w, "base_revision"), &text(&snapshot, "tree"))?;
    }
    if value["state"] != "applied" {
        value["state"] = json!("applied");
        value["applied_at"] = json!(now());
        save(s, &id, &value)?;
    }
    Ok(value)
}

pub fn color(s: &Store, f: &str, args: &Value) -> Result<Value> {
    s.binding(f)?;
    let color = text(args, "color");
    ensure!(
        color.len() == 7
            && color.starts_with('#')
            && color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit),
        "Use a six-digit hex color such as #7D9DB8"
    );
    let project = text(args, "project");
    let root = s.projects[&project]["path"]
        .as_str()
        .context("Select a configured project")?;
    let path = Path::new(root).join("opencode.json");
    let mut config: Value = serde_json::from_slice(&regular(&path, 1024 * 1024)?)?;
    ensure!(
        path.ancestors().all(|p| !p.is_symlink()),
        "Profile follows a symlink"
    );
    let cmd = array(&config["mcp"]["servers"]["teem"]["command"]);
    ensure!(
        cmd.windows(2).any(|a| a[0] == "--foreman" && a[1] == f),
        "Project profile belongs to another Foreman"
    );
    ensure!(
        config["agents"]["teem_foreman"].is_object(),
        "Foreman profile is missing"
    );
    config["agents"]["teem_foreman"]["color"] = json!(color);
    atomic_json(&path, &config)?;
    Ok(json!({"project":project,"color":color,"scope":"local Foreman profile only"}))
}
