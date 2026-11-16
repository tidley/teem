//! Explicit read-only prerequisites, pinned before paid work and supplied locally.
use crate::common::*;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::path::Path;

pub fn effective_policy(w: &Value) -> Value {
    let mut policy = w["policy"].clone();
    if let Some(update) = w["data"]["prerequisite_updates"]
        .as_array()
        .and_then(|a| a.last())
    {
        policy["review_prerequisites"] = update["files"].clone();
    }
    policy
}

/// Adopt delivery-only repairs, retaining admission policy and every prior pin.
pub fn refresh(s: &crate::store::Store, w: &mut Value) -> Result<bool> {
    let current = &s.projects[text(w, "project")];
    ensure!(
        crate::workflows::compatible_policy(current, &w["policy"]),
        "Execution policy changed; prerequisite repair cannot change source, checks or authority"
    );
    let previous = effective_policy(w);
    if current.get("review_prerequisites") == previous.get("review_prerequisites") {
        return Ok(false);
    }
    let files = array(&current["review_prerequisites"]);
    ensure!(
        array(&previous["review_prerequisites"])
            .iter()
            .all(|old| files.iter().any(|new| new["path"] == old["path"])),
        "Mandatory prerequisite declaration removed; explicit policy reconciliation required"
    );
    validate(current)?;
    if !w["data"]["prerequisite_updates"].is_array() {
        w["data"]["prerequisite_updates"] = json!([]);
    }
    w["data"]["prerequisite_updates"].as_array_mut().unwrap().push(json!({"at":now(),"previous":previous["review_prerequisites"],"files":current["review_prerequisites"]}));
    Ok(true)
}

/// Explicit recovery of the old controller's prerequisite-only policy stop.
/// Continues observing the SAME admitted worker; never creates/replays one.
pub fn repair(s: &crate::store::Store, f: &str, args: &Value) -> Result<Value> {
    s.binding(f)?;
    let id = text(args, "workflow_id");
    uuid(&id)?;
    s.transaction(|| {
        let mut w = s.workflow(&id)?;
        ensure!(w["foreman"] == f,"Workflow belongs to another Teem coordinator");
        if w["data"]["prerequisite_policy_recovery"].is_object() { return Ok(w); }
        ensure!(w["state"]=="blocked" && w["detail"]=="Project policy removed or changed; progression stopped" && w["stop"].is_null() && w["integration"].is_null() && w["request"].get("qualification_recovery").is_none(),"Only an unpaused prerequisite-only policy stop during implementation can be repaired");
        let round = array(&w["data"]["rounds"]).pop().context("No admitted implementation")?;
        ensure!(["snapshot","check_checkout","check_handle","check_result","review_ids","review_results"].iter().all(|k|round.get(*k).is_none()) && w["data"].get("resume_phase").is_none(),"Candidate/check/review admission or uncertain progression already exists; use its retained qualification contract");
        let round_dir = s.directory.join("managed").join(&id).join(format!("round-{}",w["round"].as_u64().context("Invalid workflow round")?));
        for entry in ["snapshot","checks-candidate","check-evidence","combined","worker-snapshot-0","worker-snapshot-1"] {
            ensure!(std::fs::symlink_metadata(round_dir.join(entry)).is_err_and(|e|e.kind()==std::io::ErrorKind::NotFound),"Partial snapshot/check preparation exists; preserve it without replay");
        }
        let ids = round.get("worker_ids").map(array).unwrap_or_else(||vec![round["worker_id"].clone()]);
        ensure!(!ids.is_empty() && ids.len()<=2 && (ids.len()==1 || ids[0]!=ids[1]),"Distinct existing worker identities required");
        for task_id in &ids {
            let t = s.task(task_id.as_str().context("Invalid worker identity")?)?;
            ensure!(t["role"]=="worker" && t["foreman"]==f && t["request"]["workflow_id"]==id && t["request"]["revision"]==w["base_revision"] && ["queued","starting","accepted","running","succeeded"].contains(&text(&t,"state").as_str()),"Worker is failed, interrupted, uncertain or has changed provenance; never replay it");
        }
        ensure!(s.query("SELECT task_stops.task_id FROM task_stops JOIN tasks ON tasks.id=task_stops.task_id WHERE json_extract(tasks.request,'$.workflow_id')=?",[&id])?.is_empty(),"Preserve stopped work without resuming it");
        ensure!(s.query("SELECT id FROM tasks WHERE json_extract(request,'$.workflow_id')=? AND state NOT IN ('succeeded','failed','interrupted')",[&id])?.iter().all(|r|ids.contains(&r["id"])),"Workflow owns other active or uncertain work");
        let prior = digest(&w);
        ensure!(refresh(s,&mut w)?,"No prerequisite-only repair to apply");
        w["data"]["prerequisite_policy_recovery"] = json!({"at":now(),"prior_workflow_sha256":prior,"prior_finished_at":w["data"]["finished_at"],"prior_state":"blocked","prior_detail":w["detail"],"worker_ids":ids});
        w["data"].as_object_mut().unwrap().remove("finished_at");
        s.exec("UPDATE workflows SET state='working',detail=?,data=? WHERE id=?",rusqlite::params!["Prerequisite-only policy stop repaired; observing original worker identities without replay or budget renewal",canonical(&w["data"]),id])?;
        s.workflow(&id)
    })
}

pub fn normalise(value: &Value, project: &Path) -> Result<Value> {
    ensure!(
        value.as_array().is_some_and(|a| a.len() <= 8),
        "Use at most eight review prerequisite files"
    );
    let mut paths = crate::workflows::reference_paths(value, project)?;
    for entry in paths.as_array_mut().unwrap() {
        ensure!(
            entry["kind"] == "file",
            "Review prerequisites must be explicit regular files, not directories"
        );
        let path = text(entry, "path");
        let data =
            regular(Path::new(&path), 131072).context("Review prerequisite must be readable")?;
        std::str::from_utf8(&data).context("Review prerequisite must be UTF-8")?;
        entry["sha256"] = json!(sha(&data));
    }
    validate(&json!({"review_prerequisites":paths}))?;
    Ok(paths)
}

fn files(policy: &Value) -> Result<Vec<(Value, Vec<u8>)>> {
    let mut files = Vec::new();
    let mut total = 0;
    let mut skills = 0;
    for entry in array(&policy["review_prerequisites"]) {
        let path = text(&entry, "path");
        let path = Path::new(&path);
        ensure!(
            path.canonicalize().ok().as_deref() == Some(path),
            "Review prerequisite path changed: {}",
            path.display()
        );
        let data = regular(path, 131072)
            .with_context(|| format!("Review prerequisite unavailable: {}", path.display()))?;
        ensure!(
            sha(&data) == entry["sha256"],
            "Review prerequisite changed: {}",
            path.display()
        );
        std::str::from_utf8(&data).context("Review prerequisite must be UTF-8")?;
        total += data.len();
        if path.file_name().is_some_and(|n| n == "SKILL.md") {
            skills += data.len();
        }
        ensure!(
            total <= 131072 && skills <= 12000,
            "Review prerequisites exceed bounded file/guidance limits"
        );
        files.push((entry, data));
    }
    Ok(files)
}

pub fn validate(policy: &Value) -> Result<()> {
    files(policy).map(|_| ())
}

fn local(entry: &Value, index: usize) -> Result<String> {
    let name = Path::new(
        entry["path"]
            .as_str()
            .context("Missing prerequisite path")?,
    )
    .file_name()
    .context("Missing prerequisite filename")?
    .to_str()
    .context("Invalid prerequisite filename")?;
    Ok(format!(".git/teem-prerequisites/{index}-{name}"))
}

pub fn prepare(checkout: &Path, policy: &Value) -> Result<String> {
    let files = files(policy)?;
    if files.is_empty() {
        return Ok(String::new());
    }
    private_dir(&checkout.join(".git/teem-prerequisites"))?;
    let mut prompt = String::from(
        "\nMandatory approved prerequisites: the supervisor pinned these copies from the configured originals. Read the checkout-local copies FIRST, instead of retrying inaccessible original paths. Copies and preloaded guidance are read-only; never change permissions. Additional unavailable mandatory material is a blocked review, not a source defect.\n",
    );
    for (index, (entry, data)) in files.iter().enumerate() {
        let path = local(entry, index)?;
        write_new(&checkout.join(&path), data, 0o400)?;
        prompt.push_str(&format!(
            "{} -> {path} (SHA-256 {})\n",
            text(entry, "path"),
            text(entry, "sha256")
        ));
        if Path::new(&text(entry, "path"))
            .file_name()
            .is_some_and(|n| n == "SKILL.md")
        {
            let name = Path::new(entry["path"].as_str().unwrap())
                .parent()
                .and_then(Path::file_name)
                .and_then(|n| n.to_str())
                .context("Invalid skill prerequisite")?;
            ensure!(
                name.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_-".contains(c)),
                "Use a literal skill directory name"
            );
            prompt.push_str(&format!(
                "\n<skill_content name=\"{name}\">\n{}\n</skill_content>\n",
                std::str::from_utf8(data)?
            ));
        }
    }
    Ok(prompt)
}

pub fn validate_task(task: &Value) -> Result<()> {
    for (index, (entry, _)) in files(&task["request"])?.iter().enumerate() {
        let data = regular(
            &Path::new(&text(task, "checkout")).join(local(entry, index)?),
            131072,
        )
        .context("Review prerequisite copy unavailable")?;
        ensure!(
            sha(&data) == entry["sha256"],
            "Review prerequisite copy changed"
        );
    }
    Ok(())
}
