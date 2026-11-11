//! Permission payload/state contracts, not a substitute for native tool qualification.
use anyhow::Result;
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::symlink,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
use teem::{
    checkouts, checks,
    common::*,
    reconcile,
    runtime::{self, Runtime},
    store::Store,
    workflows,
};

fn policy(source: &std::path::Path) -> Value {
    json!({"path":source,"checks":[{"name":"tests","kind":"test","argv":["git","status"],"cwd":"."}]})
}

struct Local(PathBuf);
impl Runtime for Local {
    fn request(&self, method: &str, path: &str, _: Option<&Value>) -> Result<Value> {
        assert_eq!((method, path), ("GET", "/api/session/ses_test"));
        Ok(json!({"id":"ses_test","location":{"directory":self.0}}))
    }
}

fn runner() -> checks::Runner {
    checks::Runner::with_executable(PathBuf::from(env!("CARGO_BIN_EXE_teem")))
}

#[test]
fn paths_are_existing_absolute_literals_and_aliases_have_one_frozen_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    let directory = tmp.path().join("references");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&directory).unwrap();
    let file = directory.join("guide.md");
    fs::write(&file, "reference").unwrap();
    let alias = tmp.path().join("alias");
    symlink(&directory, &alias).unwrap();
    let mut p = policy(&source);
    let default = workflows::normalise_projects(&json!({"project":p})).unwrap();
    assert!(default["project"].get("reference_paths").is_none());
    p["reference_paths"] = json!([]);
    assert_eq!(
        workflows::normalise_projects(&json!({"project":p})).unwrap(),
        default
    );
    p["reference_paths"] = json!([
        directory,
        alias,
        alias.join("./guide.md"),
        file,
        directory.join("../references")
    ]);
    let normal = workflows::normalise_projects(&json!({"project":p})).unwrap();
    assert_eq!(
        normal["project"]["reference_paths"],
        json!([
            {"path":directory.canonicalize().unwrap(),"kind":"directory"},
            {"path":file.canonicalize().unwrap(),"kind":"file"}
        ])
    );
    p["reference_paths"] = json!([file, directory]);
    assert_eq!(
        workflows::normalise_projects(&json!({"project":p})).unwrap(),
        normal
    );
    assert!(!workflows::compatible_policy(
        &default["project"],
        &normal["project"]
    ));
    let mut budget = normal["project"].clone();
    budget["max_revisions"] = json!(0);
    assert!(workflows::compatible_policy(&budget, &normal["project"]));

    let broad_alias = tmp.path().join("broad");
    symlink("/", &broad_alias).unwrap();
    let socket = tmp.path().join("socket");
    let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    for invalid in [
        Value::Null,
        json!("not a list"),
        json!([1]),
        json!([""]),
        json!(["relative/file"]),
        json!(["~/guide.md"]),
        json!(["/"]),
        json!(["/mnt"]),
        json!([home()]),
        json!([source]),
        json!([tmp.path()]),
        json!([broad_alias]),
        json!([socket]),
        json!([tmp.path().join("missing")]),
        json!([format!("{}/*", directory.display())]),
        json!([format!("{}/[guide]", directory.display())]),
        json!([format!("{}/{{guide}}", directory.display())]),
        json!([format!("{}/@(guide)", directory.display())]),
        json!([format!("{}/bad\npath", directory.display())]),
        json!(vec![file.clone(); 33]),
    ] {
        p["reference_paths"] = invalid.clone();
        assert!(
            workflows::normalise_projects(&json!({"project":p})).is_err(),
            "{invalid}"
        );
    }
    // Canonical targets must also be literal, even when the input is innocuous.
    let patterned = tmp.path().join("pattern*");
    fs::create_dir(&patterned).unwrap();
    let innocent = tmp.path().join("innocent");
    symlink(patterned, &innocent).unwrap();
    p["reference_paths"] = json!([innocent]);
    assert!(workflows::normalise_projects(&json!({"project":p})).is_err());
}

#[test]
fn reference_rules_do_not_widen_files_edits_shell_reviewers_or_legacy_sessions() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/legacy-wire.json")).unwrap();
    let old = &fixture["task"];
    assert_eq!(runtime::permissions(old), fixture["permissions"]);
    let default = array(&runtime::permissions(old));
    assert_eq!(
        default[0],
        json!({"action":"*","resource":"*","effect":"deny"})
    );
    assert_eq!(
        default
            .iter()
            .filter(|r| r["action"] == "external_directory")
            .count(),
        1
    );
    runtime::validate_session(old, &fixture["session_payload"], &Value::Null).unwrap();
    let mut task = old.clone();
    task["request"]["reference_paths"] = json!([
        {"path":"/approved/reference/guide.md","kind":"file"},
        {"path":"/approved/library","kind":"directory"}
    ]);
    for trusted in [0, 1] {
        task["trusted_worker_shell"] = json!(trusted);
        let mut before = old.clone();
        before["trusted_worker_shell"] = json!(trusted);
        let mut actual = array(&runtime::permissions(&task));
        let added: Vec<_> = actual
            .iter()
            .filter(|r| r["action"] == "external_directory" && r["resource"] != "/mnt/fastbuild/**")
            .cloned()
            .collect();
        assert_eq!(
            added,
            vec![
                json!({"action":"external_directory","resource":"/approved/reference/guide.md","effect":"allow"}),
                json!({"action":"external_directory","resource":"/approved/library/**","effect":"allow"})
            ]
        );
        // Assert exact payloads, not an invented native resource matcher. In
        // particular no /approved/reference/* or sibling grant is emitted.
        actual.retain(|r| !added.contains(r));
        assert_eq!(json!(actual), runtime::permissions(&before));
    }
    let session = runtime::session_payload(&task, &Value::Null);
    runtime::validate_session(&task, &session, &Value::Null).unwrap();
    let mut widened = session.clone();
    let rules = widened["permissions"].as_array_mut().unwrap();
    rules.push(json!({
        "action":"external_directory","resource":"/approved/reference/**","effect":"allow"
    }));
    assert!(runtime::validate_session(&task, &widened, &Value::Null).is_err());
    assert!(runtime::validate_session(old, &session, &Value::Null).is_err());
    assert!(runtime::validate_session(&task, &fixture["session_payload"], &Value::Null).is_err());
    task["role"] = json!("reviewer");
    let mut reviewer = old.clone();
    reviewer["role"] = json!("reviewer");
    assert_eq!(runtime::permissions(&task), runtime::permissions(&reviewer));
}

#[test]
fn managed_policy_is_project_local_retained_and_reconciliation_uses_current_policy() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    fs::create_dir_all(source.join("src")).unwrap();
    fs::write(source.join("src/a"), "base\n").unwrap();
    fs::write(source.join("src/b"), "base\n").unwrap();
    git(&source, &["init", "-q"], None, None).unwrap();
    git(&source, &["config", "user.name", "Test"], None, None).unwrap();
    git(
        &source,
        &["config", "user.email", "test@example.invalid"],
        None,
        None,
    )
    .unwrap();
    git(&source, &["add", "."], None, None).unwrap();
    git(&source, &["commit", "-qm", "base"], None, None).unwrap();
    let file = tmp.path().join("guide.md");
    let other = tmp.path().join("other");
    fs::write(&file, "guide").unwrap();
    fs::create_dir(&other).unwrap();
    let mut config = json!({"project":policy(&source),"other":policy(&source)});
    config["project"]["reference_paths"] = json!([file]);
    config["other"]["reference_paths"] = json!([other]);
    let projects = workflows::normalise_projects(&config).unwrap();
    let s = Store::open(&tmp.path().join("state"), &[], projects.clone()).unwrap();
    s.bind("ses_test", "terminal").unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let request = json!({"workflow_id":id,"project":"project","issue":"tidley/teem#12","scope":["src"],"prompt":"Implement scoped change","workers":[{"scope":["src/a"],"prompt":"Part a"},{"scope":["src/b"],"prompt":"Part b"}]});
    s.workflow_submit("ses_test", request).unwrap();
    let local = Local(source.clone());
    workflows::tick(&s, &local, &mut runner()).unwrap();
    let mut w = s.workflow(&id).unwrap();
    assert_eq!(w["state"], "working", "{w}");
    let task = s
        .task(w["data"]["rounds"][0]["worker_id"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        task["request"]["reference_paths"],
        projects["project"]["reference_paths"]
    );
    assert_ne!(
        task["request"]["reference_paths"],
        projects["other"]["reference_paths"]
    );
    let workers = s.tasks(None).unwrap();
    assert_eq!(workers.len(), 2);
    for worker in &workers {
        assert_eq!(
            worker["request"]["reference_paths"],
            task["request"]["reference_paths"]
        );
    }
    // Check revocation while the workflow is still eligible to launch, not
    // merely after its terminal state has independently blocked the checkout.
    assert!(s.allowed(&text(&task, "checkout"), "ses_test").unwrap());
    for replacement in [None, Some(projects["other"]["reference_paths"].clone())] {
        let mut changed = projects.clone();
        changed["project"]
            .as_object_mut()
            .unwrap()
            .remove("reference_paths");
        if let Some(paths) = replacement {
            changed["project"]["reference_paths"] = paths;
        }
        let reloaded = Store::open(&tmp.path().join("state"), &[], changed).unwrap();
        assert!(
            !reloaded
                .allowed(&text(&task, "checkout"), "ses_test")
                .unwrap()
        );
        assert_eq!(reloaded.task(&text(&task, "id")).unwrap(), task);
    }
    let manual = Store::open(
        &tmp.path().join("manual-state"),
        &[source.to_str().unwrap().into()],
        json!({}),
    )
    .unwrap();
    manual.bind("ses_test", "terminal").unwrap();
    let mut legacy = task["request"].clone();
    legacy.as_object_mut().unwrap().remove("workflow_id");
    legacy["checkout"] = json!(source);
    assert!(manual.submit("ses_test", legacy).is_err());
    let mut req = task["request"].clone();
    req["task_id"] = json!(uuid::Uuid::new_v4().to_string());
    req["reference_paths"] = projects["other"]["reference_paths"].clone();
    assert!(s.submit("ses_test", req.clone()).is_err());
    req["role"] = json!("reviewer");
    assert!(s.submit("ses_test", req.clone()).is_err());
    req.as_object_mut().unwrap().remove("reference_paths");
    let reviewer = s.submit("ses_test", req).unwrap();
    assert!(reviewer["request"].get("reference_paths").is_none());
    s.update(&text(&reviewer, "id"), "succeeded", "review fixture", "")
        .unwrap();

    let checkout = PathBuf::from(text(&task, "checkout"));
    fs::write(checkout.join("src/a"), "changed\n").unwrap();
    let snapshot = checkouts::capture(
        &checkout,
        &text(&w, "base_revision"),
        &["src".into()],
        &tmp.path().join("snapshot"),
    )
    .unwrap();
    for worker in &workers {
        s.update(&text(worker, "id"), "succeeded", "implemented", "")
            .unwrap();
    }

    // Exercise the real snapshot/check/review/revision path using only local
    // Git and fake native sessions. No model or external reference read runs.
    workflows::tick(&s, &local, &mut runner()).unwrap();
    w = s.workflow(&id).unwrap();
    assert_eq!(w["state"], "checking", "{w}");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(result) = checks::inspect(&w["data"]["rounds"][0]["check_handle"]).unwrap() {
            assert_eq!(result["passed"], true, "{result}");
            break;
        }
        assert!(Instant::now() < deadline, "No local check receipt");
        thread::sleep(Duration::from_millis(20));
    }
    workflows::tick(&s, &local, &mut runner()).unwrap();
    w = s.workflow(&id).unwrap();
    assert_eq!(w["state"], "reviewing", "{w}");
    for review in array(&w["data"]["rounds"][0]["review_ids"]) {
        let review = s.task(review.as_str().unwrap()).unwrap();
        assert!(review["request"].get("reference_paths").is_none());
        assert!(
            !array(&runtime::permissions(&review))
                .iter()
                .any(|r| r["action"] == "external_directory")
        );
        let verdict = json!({"snapshot":w["data"]["rounds"][0]["snapshot"]["sha256"],"verdict":"changes_requested","findings":[{"path":"src/a","line":1,"message":"Fixture correction"}]});
        s.update(&text(&review, "id"), "succeeded", &verdict.to_string(), "")
            .unwrap();
    }
    workflows::tick(&s, &local, &mut runner()).unwrap();
    w = s.workflow(&id).unwrap();
    assert_eq!(w["state"], "working", "{w}");
    assert_eq!(w["round"], 1);
    let revision = s
        .task(w["data"]["rounds"][1]["worker_id"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        revision["request"]["reference_paths"],
        task["request"]["reference_paths"]
    );
    assert_eq!(revision["request"]["input_tree"], snapshot["tree"]);
    let revised_checkout = PathBuf::from(text(&revision, "checkout"));
    w["data"]["rounds"][1]["snapshot"] = checkouts::capture(
        &revised_checkout,
        &text(&w, "base_revision"),
        &["src".into()],
        &tmp.path().join("revision-snapshot"),
    )
    .unwrap();
    s.update(&text(&revision, "id"), "succeeded", "corrected", "")
        .unwrap();
    s.save_workflow(&mut w, "needs_attention", "Retained test candidate")
        .unwrap();
    let task = s.task(&text(&task, "id")).unwrap();
    let parent = s.workflow(&id).unwrap();
    let permissions = runtime::permissions(&task);
    drop(s);

    // A later type change cannot change the recorded session permissions.
    fs::remove_file(&file).unwrap();
    fs::create_dir(&file).unwrap();
    config["project"]["reference_paths"] = json!([other]);
    let current = workflows::normalise_projects(&config).unwrap();
    let s = Store::open(&tmp.path().join("state"), &[], current.clone()).unwrap();
    assert_eq!(s.task(&text(&task, "id")).unwrap(), task);
    assert_eq!(runtime::permissions(&task), permissions);
    assert!(!s.allowed(&text(&task, "checkout"), "ses_test").unwrap());
    let successor = uuid::Uuid::new_v4().to_string();
    reconcile::submit(
        &s,
        "ses_test",
        &json!({"workflow_id":successor,"source_workflow_id":id}),
    )
    .unwrap();
    workflows::tick(&s, &local, &mut runner()).unwrap();
    let follow = s.workflow(&successor).unwrap();
    assert_eq!(follow["state"], "working", "{follow}");
    let worker = s
        .task(follow["data"]["rounds"][0]["worker_id"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        worker["request"]["reference_paths"],
        current["project"]["reference_paths"]
    );
    assert!(worker["request"]["input_tree"].is_string());
    assert_eq!(s.workflow(&id).unwrap(), parent);
    let patch = PathBuf::from(text(&worker, "checkout")).join(".git/teem-reconciliation.patch");
    assert_eq!(
        checkouts::verify(&snapshot).unwrap(),
        fs::read(patch).unwrap()
    );
}
