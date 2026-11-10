use anyhow::Result;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};
use teem::{
    approvals, checkouts, checks,
    common::*,
    plugin, progress,
    runtime::{self, Runtime},
    store::Store,
    workflows,
};

fn repo(path: &Path) -> String {
    fs::create_dir_all(path).unwrap();
    git(path, &["init", "-q"], None, None).unwrap();
    git(path, &["config", "user.name", "Test"], None, None).unwrap();
    git(
        path,
        &["config", "user.email", "test@example.invalid"],
        None,
        None,
    )
    .unwrap();
    fs::create_dir(path.join("src")).unwrap();
    fs::write(path.join("src/a"), "base\n").unwrap();
    git(path, &["add", "."], None, None).unwrap();
    git(path, &["commit", "-qm", "base"], None, None).unwrap();
    git_text(path, &["rev-parse", "HEAD"]).unwrap()
}
fn store(tmp: &Path, paths: &[String], projects: Value) -> Store {
    let s = Store::open(&tmp.join("state"), paths, projects).unwrap();
    s.bind("ses_test", "terminal").unwrap();
    s
}
fn req(path: &Path, base: &str, role: &str) -> Value {
    json!({"task_id":uuid::Uuid::new_v4().to_string(),"role":role,"checkout":path,"revision":base,"scope":["src"],"prompt":"Do the scoped work"})
}
fn runner() -> checks::Runner {
    checks::Runner::with_executable(PathBuf::from(env!("CARGO_BIN_EXE_teem")))
}
fn await_receipt(h: &Value) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(r) = checks::inspect(h).unwrap() {
            return r;
        }
        assert!(Instant::now() < deadline, "No check receipt");
        thread::sleep(Duration::from_millis(20));
    }
}
fn check_policy(argv: Value) -> Value {
    checks::normalise(
        &json!([{"name":"test","kind":"test","argv":argv,"timeout":2,"output_limit":1024}]),
    )
    .unwrap()
}
#[derive(Default)]
struct Fake {
    directory: String,
    calls: RefCell<Vec<(String, String, Value)>>,
    outcomes: RefCell<BTreeMap<String, Value>>,
    notifications: RefCell<usize>,
    fail_notify: bool,
    fail_model: bool,
    stop_live: RefCell<bool>,
    stop_lost: bool,
    pending: RefCell<Vec<Value>>,
    forms: RefCell<BTreeMap<String, Value>>,
}
impl Runtime for Fake {
    fn is_stopped(&self, _: &Value) -> Result<bool> {
        Ok(!*self.stop_live.borrow())
    }
    fn interrupt(&self, t: &Value) -> Result<()> {
        self.calls
            .borrow_mut()
            .push(("STOP".into(), text(t, "session_id"), Value::Null));
        if self.stop_lost {
            anyhow::bail!("Simulated unknown interrupt response")
        }
        *self.stop_live.borrow_mut() = false;
        Ok(())
    }
    fn request(&self, m: &str, path: &str, p: Option<&Value>) -> Result<Value> {
        self.calls
            .borrow_mut()
            .push((m.into(), path.into(), p.cloned().unwrap_or(Value::Null)));
        if path.ends_with("/reply") {
            anyhow::bail!("Simulated lost reply")
        }
        if m == "GET" {
            return self
                .forms
                .borrow()
                .get(path)
                .cloned()
                .ok_or_else(|| runtime::Missing.into());
        }
        if m == "POST" && path.ends_with("/form") {
            let mut v = p.unwrap().clone();
            v["sessionID"] = json!("ses_test");
            v["state"] = json!({"status":"pending"});
            self.forms
                .borrow_mut()
                .insert(format!("{path}/{}", text(&v, "id")), v.clone());
            return Ok(v);
        }
        Ok(json!({}))
    }
    fn directory(&self, _: &str) -> Result<String> {
        Ok(self.directory.clone())
    }
    fn dispatch_model(&self, _: &Value) -> Result<Value> {
        if self.fail_model {
            anyhow::bail!("Inherited model lookup unavailable");
        }
        Ok(json!({"providerID":"test","id":"model"}))
    }
    fn create(&self, t: &Value) -> Result<()> {
        self.calls
            .borrow_mut()
            .push(("CREATE".into(), text(t, "id"), t.clone()));
        Ok(())
    }
    fn prompt(&self, t: &Value) -> Result<()> {
        self.calls
            .borrow_mut()
            .push(("PROMPT".into(), text(t, "id"), t.clone()));
        Ok(())
    }
    fn inspect(&self, t: &Value) -> Result<Value> {
        if self.fail_model {
            return Err(runtime::Missing.into());
        }
        if !*self.stop_live.borrow() && self.calls.borrow().iter().any(|c| c.0 == "STOP") {
            return Ok(json!({"state":"interrupted"}));
        }
        Ok(self
            .outcomes
            .borrow()
            .get(&text(t, "id"))
            .cloned()
            .unwrap_or(json!({"state":"running"})))
    }
    fn notify(&self, _: &Value) -> Result<()> {
        *self.notifications.borrow_mut() += 1;
        if self.fail_notify {
            anyhow::bail!("Lost response")
        }
        Ok(())
    }
    fn accepted(&self, _: &Value) -> Result<bool> {
        Ok(false)
    }
    fn approvals(&self, _: &Value) -> Result<Vec<Value>> {
        Ok(self.pending.borrow().clone())
    }
}

#[test]
fn frozen_legacy_wire_and_digest_contracts_match() {
    let f: Value = serde_json::from_str(include_str!("fixtures/legacy-wire.json")).unwrap();
    let t = &f["task"];
    assert_eq!(runtime::permissions(t), f["permissions"]);
    assert_eq!(
        runtime::session_payload(t, &json!({})),
        f["session_payload"]
    );
    assert_eq!(runtime::prompt_text(t), f["prompt_text"]);
    for (event, key) in [
        ("event", "event_payload"),
        ("workflow_event", "workflow_payload"),
    ] {
        let mut payload = runtime::event_payload(&f[event]).unwrap();
        assert!(text(&payload, "text").starts_with(&text(&f[key], "text")));
        assert!(text(&payload, "text").contains("without ack"));
        payload["text"] = f[key]["text"].clone();
        assert_eq!(payload, f[key]);
    }
    assert_eq!(digest(&f["canonical_value"]), f["canonical_digest"]);
}

#[test]
fn historical_digest_encoding_handles_exponents_and_del() {
    let cases = [
        (json!({"timeout": 0.000001}), "{\"timeout\": 1e-06}"),
        (json!({"n": 1e16}), "{\"n\": 1e+16}"),
        (json!({"n": -0.0}), "{\"n\": -0.0}"),
        (json!({"text": "\u{7f}"}), "{\"text\": \"\\u007f\"}"),
    ];
    for (value, expected) in cases {
        assert_eq!(canonical(&value), expected);
    }
}

#[test]
fn historical_float_encoding_matches_frozen_legacy_sample() {
    let cases: Value = serde_json::from_str(include_str!("fixtures/legacy-floats.json")).unwrap();
    for case in cases.as_array().unwrap() {
        assert_eq!(canonical(&case[0]), case[1].as_str().unwrap());
    }
}
#[test]
fn immutable_task_and_binding_survive_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("repo");
    let base = repo(&path);
    let allowed = vec![path.to_str().unwrap().into()];
    let s = store(tmp.path(), &allowed, json!({}));
    let request = req(&path, &base, "worker");
    let original = s.submit("ses_test", request.clone()).unwrap();
    drop(s);
    let s = Store::open(&tmp.path().join("state"), &allowed, json!({})).unwrap();
    let adopted = s.submit("ses_test", request.clone()).unwrap();
    assert_eq!(original, adopted);
    let mut changed = request;
    changed["prompt"] = json!("Different request");
    assert!(s.submit("ses_test", changed).is_err());
    assert!(s.bind("ses_test", "different-terminal").is_err());
}
#[test]
fn admission_rejects_traversal_and_dirty_source() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("repo");
    let base = repo(&path);
    let s = store(tmp.path(), &[path.to_str().unwrap().into()], json!({}));
    for p in ["../src", "src/.git", "/tmp/a", "src/*", "src//x"] {
        let mut r = req(&path, &base, "worker");
        r["scope"] = json!([p]);
        assert!(s.submit("ses_test", r).is_err());
    }
    fs::write(path.join("src/a"), "dirty").unwrap();
    assert!(s.submit("ses_test", req(&path, &base, "worker")).is_err());
    assert!(checkouts::source_revision(&path).is_err());
}
#[test]
fn starting_or_uncertain_tasks_never_replay_after_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("repo");
    let base = repo(&path);
    let allowed = vec![path.to_str().unwrap().into()];
    let s = store(tmp.path(), &allowed, json!({}));
    let t = s.submit("ses_test", req(&path, &base, "worker")).unwrap();
    s.claim().unwrap().unwrap();
    drop(s);
    let s = Store::open(&tmp.path().join("state"), &allowed, json!({})).unwrap();
    let f = Fake::default();
    runtime::tick(&s, &f).unwrap();
    runtime::tick(&s, &f).unwrap();
    assert!(f.calls.borrow().is_empty());
    assert_eq!(s.task(&text(&t, "id")).unwrap()["state"], "running");
}
#[test]
fn uncertain_completion_notification_is_never_sent_twice() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("repo");
    let base = repo(&path);
    let s = store(tmp.path(), &[path.to_str().unwrap().into()], json!({}));
    let t = s.submit("ses_test", req(&path, &base, "worker")).unwrap();
    s.update(&text(&t, "id"), "succeeded", "done", "").unwrap();
    let f = Fake {
        fail_notify: true,
        ..Default::default()
    };
    runtime::notify(&s, &f).unwrap();
    s.exec("UPDATE events SET next_receipt=0", []).unwrap();
    runtime::notify(&s, &f).unwrap();
    assert_eq!(*f.notifications.borrow(), 1);
    assert_eq!(
        s.query("SELECT state FROM events", []).unwrap()[0]["state"],
        "uncertain"
    );
}
#[test]
fn role_caps_and_checkout_ownership_are_atomic_across_connections() {
    let tmp = tempfile::tempdir().unwrap();
    let mut allowed = Vec::new();
    let mut requests = Vec::new();
    for i in 0..3 {
        let p = tmp.path().join(format!("r{i}"));
        let base = repo(&p);
        allowed.push(p.to_str().unwrap().into());
        requests.push(req(&p, &base, "worker"));
    }
    let s = store(tmp.path(), &allowed, json!({}));
    for r in requests {
        s.submit("ses_test", r).unwrap();
    }
    let second = Store::open(&tmp.path().join("state"), &allowed, json!({})).unwrap();
    assert!(s.claim().unwrap().is_some());
    assert!(second.claim().unwrap().is_some());
    assert!(s.claim().unwrap().is_none());
}
#[test]
fn exact_snapshot_includes_binary_deletions_and_new_files_and_rejects_drift() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("source");
    let base = repo(&p);
    let checkout = checkouts::prepare(&p, &tmp.path().join("worker"), &base, None).unwrap();
    fs::remove_file(checkout.join("src/a")).unwrap();
    fs::write(checkout.join("src/b"), [0, 1, 255, 0]).unwrap();
    let snap = checkouts::capture(
        &checkout,
        &base,
        &["src".into()],
        &tmp.path().join("snapshot"),
    )
    .unwrap();
    let review =
        checkouts::prepare(&checkout, &tmp.path().join("review"), &base, Some(&snap)).unwrap();
    assert_eq!(fs::read(review.join("src/b")).unwrap(), [0, 1, 255, 0]);
    checkouts::validate(&review, &base, &text(&snap, "tree")).unwrap();
    fs::write(review.join("src/b"), "tampered").unwrap();
    assert!(checkouts::validate(&review, &base, &text(&snap, "tree")).is_err());
    assert!(checkouts::prepare(&p, &checkout, &base, None).is_err());
}
#[test]
fn out_of_scope_rename_and_symlink_destination_are_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("source");
    let base = repo(&p);
    fs::rename(p.join("src/a"), p.join("outside")).unwrap();
    assert!(checkouts::capture(&p, &base, &["src".into()], &tmp.path().join("snapshot")).is_err());
    std::os::unix::fs::symlink(tmp.path(), tmp.path().join("link")).unwrap();
    assert!(checkouts::prepare(&p, &tmp.path().join("link/new"), &base, None).is_err());
}
#[test]
fn checks_are_async_snapshot_bound_and_output_tampering_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("source");
    repo(&p);
    let mut runner = runner();
    let policy = check_policy(json!(["git", "status", "--porcelain"]));
    let handle = runner
        .start(&tmp.path().join("checks"), &p, &policy, "snapshot")
        .unwrap();
    assert!(
        runner
            .start(&tmp.path().join("checks"), &p, &policy, "snapshot")
            .is_err()
    );
    let receipt = await_receipt(&handle);
    assert_eq!(receipt["passed"], true);
    fs::write(tmp.path().join("checks/test.log"), "tampered").unwrap();
    assert!(checks::inspect(&handle).is_err());
}
#[test]
fn check_timeout_output_limit_and_early_closed_pipes_do_not_pass() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("repo");
    repo(&p);
    for (name, mode, value, flag) in [
        ("timeout", "sleep", "2000", "timed_out"),
        ("limit", "output", "10000", "output_limited"),
        ("closed", "closed", "2000", "timed_out"),
    ] {
        let mut policy = check_policy(json!(["native-fixture"]));
        policy[0]["argv"] = json!([env!("CARGO_BIN_EXE_teem-subprocess-fixture"), mode, value]);
        policy[0]["timeout"] = json!(0.15);
        let result = checks::execute(&policy[0], &p, &tmp.path().join(name)).unwrap();
        assert_eq!(result[flag], true, "{result}");
    }
}
#[test]
fn policy_disallows_shells_and_external_working_directories() {
    for argv in [
        json!(["sh", "-c", "true"]),
        json!(["env", "true"]),
        json!(["git", "push"]),
        json!(["/usr/bin/true"]),
    ] {
        assert!(checks::normalise(&json!([{"name":"test","kind":"test","argv":argv}])).is_err());
    }
    assert!(workflows::normalise_projects(&json!({"test":{"path":"/example","checks":[{"name":"inspect","kind":"inspect","argv":["git","status"]}]}})).is_err());
}
#[test]
fn manual_plan_does_not_claim_verification_and_progress_has_no_model_calls() {
    let tmp = tempfile::tempdir().unwrap();
    let s = store(tmp.path(), &[], json!({}));
    let snap = progress::set_plan(
        &s,
        "ses_test",
        &json!([{"id":"verify","content":"Run tests","status":"pending"}]),
    )
    .unwrap();
    assert_eq!(snap["todos"][0]["status"], "pending");
    progress::publish(&s).unwrap();
    assert_eq!(
        read_json(&s.directory.join("progress.json")).unwrap()["version"],
        1
    );
    assert!(s.query("SELECT * FROM events", []).unwrap().is_empty());
}

fn workflow_fixture(tmp: &Path, two: bool) -> (Store, Fake, Value) {
    workflow_fixture_revisions(tmp, two, Some(1))
}
fn workflow_fixture_revisions(
    tmp: &Path,
    two: bool,
    revisions: Option<u64>,
) -> (Store, Fake, Value) {
    let p = tmp.join("source");
    repo(&p);
    let mut config = json!({"project":{"path":p,"checks":[{"name":"tests","kind":"test","argv":["git","status","--porcelain"]}]}});
    if let Some(n) = revisions {
        config["project"]["max_revisions"] = json!(n);
    }
    let policy = workflows::normalise_projects(&config).unwrap();
    let s = store(tmp, &[], policy);
    let f = Fake {
        directory: p.to_str().unwrap().into(),
        ..Default::default()
    };
    let mut req = json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"project":"project","scope":["src"],"prompt":"Implement a scoped change","title":"A readable task"});
    if two {
        req["workers"] = json!([{"scope":["src/a"],"prompt":"First part"},{"scope":["src/b"],"prompt":"Second part"}]);
    }
    let w = s.workflow_submit("ses_test", req).unwrap();
    (s, f, w)
}
fn finish_workers(s: &Store) {
    for t in s.tasks(None).unwrap() {
        if t["role"] == "worker" && t["state"] == "queued" {
            let path = PathBuf::from(text(&t, "checkout"));
            let scopes = scope(&t["request"]["scope"]).unwrap();
            let file = if scopes[0] == "src" {
                path.join("src/a")
            } else {
                path.join(&scopes[0])
            };
            fs::write(file, "changed\n").unwrap();
            s.update(&text(&t, "id"), "succeeded", "implemented", "")
                .unwrap();
        }
    }
}

fn ready_operator_fixture(tmp: &Path) -> (Store, Value) {
    let (s, f, w) = workflow_fixture(tmp, false);
    let mut runner = runner();
    workflows::tick(&s, &f, &mut runner).unwrap();
    finish_workers(&s);
    workflows::tick(&s, &f, &mut runner).unwrap();
    let w = s.workflow(&text(&w, "id")).unwrap();
    await_receipt(&w["data"]["rounds"][0]["check_handle"]);
    workflows::tick(&s, &f, &mut runner).unwrap();
    let w = s.workflow(&text(&w, "id")).unwrap();
    for id in array(&w["data"]["rounds"][0]["review_ids"]) {
        s.update(id.as_str().unwrap(), "succeeded", &json!({"snapshot":w["data"]["rounds"][0]["snapshot"]["sha256"],"verdict":"pass","findings":[]}).to_string(), "").unwrap();
    }
    workflows::tick(&s, &f, &mut runner).unwrap();
    let w = s.workflow(&text(&w, "id")).unwrap();
    assert_eq!(w["state"], "ready");
    (s, w)
}

#[test]
fn operator_integration_applies_exact_candidate_and_updates_shared_status_without_replay() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, w) = ready_operator_fixture(tmp.path());
    let before = s.query("SELECT * FROM workflows", []).unwrap();
    let events = s.query("SELECT * FROM events", []).unwrap();
    let args = json!({"workflow_id":w["id"]});
    let receipt = plugin::call(&s, "ses_test", "foreman_integrate", &args).unwrap();
    assert_eq!(receipt["state"], "applied");
    assert_eq!(
        fs::read_to_string(tmp.path().join("source/src/a")).unwrap(),
        "changed\n"
    );
    assert_eq!(
        plugin::call(&s, "ses_test", "foreman_integrate", &args).unwrap(),
        receipt
    );
    assert_eq!(s.query("SELECT * FROM workflows", []).unwrap(), before);
    assert_eq!(s.query("SELECT * FROM events", []).unwrap(), events);
    assert_eq!(s.workflow(&text(&w, "id")).unwrap()["integration"], receipt);
    assert!(
        text(
            &progress::snapshot(&s, "ses_test").unwrap()["latest_result"],
            "content"
        )
        .contains("Applied locally")
    );
}

#[test]
fn revision_budget_change_does_not_revoke_qualified_integration() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, w) = ready_operator_fixture(tmp.path());
    s.projects["project"]["max_revisions"] = json!(3);
    let result = plugin::call(
        &s,
        "ses_test",
        "foreman_integrate",
        &json!({"workflow_id":w["id"]}),
    )
    .unwrap();
    assert_eq!(result["state"], "applied");
    assert_eq!(
        s.workflow(&text(&w, "id")).unwrap()["policy"]["max_revisions"],
        1
    );
}

#[test]
fn reconciliation_preserves_parent_and_requalifies_on_current_source() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, w) = ready_operator_fixture(tmp.path());
    let parent = s
        .query("SELECT * FROM workflows WHERE id=?", [text(&w, "id")])
        .unwrap();
    let source = tmp.path().join("source");
    fs::write(source.join("unrelated"), "new source\n").unwrap();
    git(&source, &["add", "unrelated"], None, None).unwrap();
    git(&source, &["commit", "-qm", "advance"], None, None).unwrap();
    let head = git_text(&source, &["rev-parse", "HEAD"]).unwrap();
    s.projects["project"]["max_revisions"] = json!(3);
    let id = uuid::Uuid::new_v4().to_string();
    let args = json!({"workflow_id":id,"source_workflow_id":w["id"]});
    let follow = plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap();
    assert_eq!(follow["state"], "queued");
    assert_eq!(follow["policy"]["max_revisions"], 3);
    assert_eq!(
        plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap()["id"],
        id
    );
    let f = Fake {
        directory: source.to_str().unwrap().into(),
        ..Default::default()
    };
    let mut runner = runner();
    workflows::tick(&s, &f, &mut runner).unwrap();
    let follow = s.workflow(&id).unwrap();
    assert_eq!(follow["state"], "working", "{follow}");
    assert_eq!(follow["base_revision"], head);
    let task = s
        .task(follow["data"]["rounds"][0]["worker_id"].as_str().unwrap())
        .unwrap();
    let checkout = PathBuf::from(text(&task, "checkout"));
    assert_eq!(
        fs::read_to_string(checkout.join("src/a")).unwrap(),
        "changed\n"
    );
    assert!(
        fs::read_to_string(checkout.join(".git/teem-reconciliation.patch"))
            .unwrap()
            .contains("+changed")
    );
    finish_workers(&s);
    workflows::tick(&s, &f, &mut runner).unwrap();
    let follow = s.workflow(&id).unwrap();
    await_receipt(&follow["data"]["rounds"][0]["check_handle"]);
    workflows::tick(&s, &f, &mut runner).unwrap();
    let follow = s.workflow(&id).unwrap();
    assert_eq!(follow["state"], "reviewing");
    for review in array(&follow["data"]["rounds"][0]["review_ids"]) {
        s.update(review.as_str().unwrap(),"succeeded",&json!({"snapshot":follow["data"]["rounds"][0]["snapshot"]["sha256"],"verdict":"pass","findings":[]}).to_string(),"").unwrap();
    }
    workflows::tick(&s, &f, &mut runner).unwrap();
    assert_eq!(s.workflow(&id).unwrap()["state"], "ready");
    assert_eq!(
        s.query("SELECT * FROM workflows WHERE id=?", [text(&w, "id")])
            .unwrap(),
        parent
    );
    assert_eq!(git_text(&source, &["rev-parse", "HEAD"]).unwrap(), head);
    assert_eq!(fs::read_to_string(source.join("src/a")).unwrap(), "base\n");
}

#[test]
fn approved_local_commit_records_exact_candidate_and_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, w) = ready_operator_fixture(tmp.path());
    let args = json!({"workflow_id":w["id"],"commit_local":true});
    let receipt = plugin::call(&s, "ses_test", "foreman_integrate", &args).unwrap();
    assert_eq!(receipt["state"], "committed");
    let root = tmp.path().join("source");
    assert_eq!(
        git_text(&root, &["rev-parse", "HEAD"]).unwrap(),
        receipt["commit"]
    );
    assert_eq!(
        checkouts::source_revision(&root).unwrap(),
        receipt["commit"]
    );
    assert_eq!(
        plugin::call(&s, "ses_test", "foreman_integrate", &args).unwrap(),
        receipt
    );
}

#[test]
fn local_commit_preserves_unrelated_staged_bytes_after_an_applied_integration() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, w) = ready_operator_fixture(tmp.path());
    plugin::call(
        &s,
        "ses_test",
        "foreman_integrate",
        &json!({"workflow_id":w["id"]}),
    )
    .unwrap();
    let root = tmp.path().join("source");
    fs::write(root.join("src/a"), "valuable staged work\n").unwrap();
    git(&root, &["add", "src/a"], None, None).unwrap();
    fs::write(root.join("src/a"), "changed\n").unwrap();
    let index = fs::read(root.join(".git/index")).unwrap();
    assert!(
        plugin::call(
            &s,
            "ses_test",
            "foreman_integrate",
            &json!({"workflow_id":w["id"],"commit_local":true})
        )
        .is_err()
    );
    assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
    assert_eq!(
        git_text(&root, &["show", ":src/a"]).unwrap(),
        "valuable staged work"
    );
    assert_eq!(
        git_text(&root, &["rev-parse", "HEAD"]).unwrap(),
        w["base_revision"]
    );
}

#[test]
fn blocked_continuation_with_live_worker_prevents_another_reconciliation() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, w) = ready_operator_fixture(tmp.path());
    let first = plugin::call(
        &s,
        "ses_test",
        "foreman_reconcile",
        &json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]}),
    )
    .unwrap();
    let f = Fake {
        directory: tmp.path().join("source").to_str().unwrap().into(),
        ..Default::default()
    };
    workflows::tick(&s, &f, &mut runner()).unwrap();
    runtime::tick(&s, &f).unwrap();
    let first = s.workflow(&text(&first, "id")).unwrap();
    let task = text(&first["data"]["rounds"][0], "worker_id");
    s.update(&task, "running", "", "").unwrap();
    s.projects["project"]["checks"][0]["timeout"] = json!(1);
    workflows::tick(&s, &f, &mut runner()).unwrap();
    assert_eq!(s.workflow(&text(&first, "id")).unwrap()["state"], "blocked");
    let another =
        json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]});
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &another).is_err());
    assert_eq!(s.task(&task).unwrap()["state"], "running");
}

#[test]
fn mcp_reloads_project_policy_without_reconnecting_and_fails_closed() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    let tmp = tempfile::tempdir().unwrap();
    let (s, _, _) = workflow_fixture(tmp.path(), false);
    let config = tmp.path().join("config.toml");
    let policy = |n| {
        format!(
            "enabled = false\nstate_dir = {:?}\n[projects.project]\npath = {:?}\nmax_revisions = {n}\n[[projects.project.checks]]\nname = 'tests'\nkind = 'test'\nargv = ['git','status','--porcelain']\n",
            s.directory,
            tmp.path().join("source")
        )
    };
    fs::write(&config, policy(1)).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_teem"))
        .args([
            "--config",
            config.to_str().unwrap(),
            "mcp",
            "--foreman",
            "ses_test",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    for n in [1, 3] {
        fs::write(&config, policy(n)).unwrap();
        writeln!(input,"{}",json!({"jsonrpc":"2.0","id":n,"method":"tools/call","params":{"name":"foreman_status","arguments":{"section":"projects","limit":5}}})).unwrap();
        input.flush().unwrap();
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        let response: Value = serde_json::from_str(&line).unwrap();
        let status: Value =
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap();
        assert_eq!(status["projects"][0]["max_revisions"], n);
    }
    fs::write(&config, "invalid TOML [").unwrap();
    writeln!(input,"{}",json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"foreman_status","arguments":{"section":"projects"}}})).unwrap();
    input.flush().unwrap();
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    let response: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(response["result"]["isError"], true);
    drop(input);
    assert!(child.wait().unwrap().success());
}

#[test]
fn reconciliation_conflicts_are_given_to_worker_without_forcing_old_code() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, w) = ready_operator_fixture(tmp.path());
    let source = tmp.path().join("source");
    fs::write(source.join("src/a"), "newer conflicting source\n").unwrap();
    git(&source, &["add", "src/a"], None, None).unwrap();
    git(&source, &["commit", "-qm", "conflict"], None, None).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    plugin::call(
        &s,
        "ses_test",
        "foreman_reconcile",
        &json!({"workflow_id":id,"source_workflow_id":w["id"]}),
    )
    .unwrap();
    let f = Fake {
        directory: source.to_str().unwrap().into(),
        ..Default::default()
    };
    workflows::tick(&s, &f, &mut runner()).unwrap();
    let follow = s.workflow(&id).unwrap();
    assert_eq!(follow["state"], "working", "{follow}");
    let checkout = PathBuf::from(text(&follow["data"]["rounds"][0], "checkout"));
    assert_eq!(
        fs::read_to_string(checkout.join("src/a")).unwrap(),
        "newer conflicting source\n"
    );
    assert_eq!(
        read_json(&checkout.join(".git/teem-reconciliation.json")).unwrap()["seeded"],
        false
    );
    assert_eq!(
        fs::read_to_string(source.join("src/a")).unwrap(),
        "newer conflicting source\n"
    );
}

#[test]
fn exhausted_candidate_continuation_uses_current_checks_not_old_qualification() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, mut w) = ready_operator_fixture(tmp.path());
    s.save_workflow(&mut w, "needs_attention", "Exhausted fixture")
        .unwrap();
    s.projects["project"]["checks"][0]["argv"] = json!(["git", "diff", "--exit-code"]);
    let args = json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]});
    let follow = plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap();
    assert_eq!(follow["policy"]["checks"], s.projects["project"]["checks"]);
    assert!(array(&follow["data"]["rounds"]).is_empty());
    assert_eq!(
        s.workflow(&text(&w, "id")).unwrap()["state"],
        "needs_attention"
    );
}

#[test]
fn reconciliation_reservation_covers_descendants_of_the_original_candidate() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, w) = ready_operator_fixture(tmp.path());
    let mut first = plugin::call(
        &s,
        "ses_test",
        "foreman_reconcile",
        &json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]}),
    )
    .unwrap();
    // Retained exhausted child fixture with the same immutable patch, and no
    // native tasks. The grandchild still reserves the original task lineage.
    first["data"] = w["data"].clone();
    first["base_revision"] = w["base_revision"].clone();
    s.exec(
        "UPDATE workflows SET data=?,base_revision=? WHERE id=?",
        rusqlite::params![
            canonical(&first["data"]),
            text(&first, "base_revision"),
            text(&first, "id")
        ],
    )
    .unwrap();
    s.save_workflow(&mut first, "needs_attention", "Retained fixture candidate")
        .unwrap();
    let grandchild = plugin::call(
        &s,
        "ses_test",
        "foreman_reconcile",
        &json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":first["id"]}),
    )
    .unwrap();
    assert_eq!(grandchild["request"]["reconcile_root"], w["id"]);
    assert!(
        plugin::call(
            &s,
            "ses_test",
            "foreman_reconcile",
            &json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]})
        )
        .is_err()
    );
}

#[test]
fn reconciliation_rejects_foreign_active_duplicate_and_tampered_candidates() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, w) = ready_operator_fixture(tmp.path());
    let args = json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]});
    s.bind("ses_other", "other-terminal").unwrap();
    assert!(plugin::call(&s, "ses_other", "foreman_reconcile", &args).is_err());
    let first = plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap();
    let other =
        json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]});
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &other).is_err());
    let active =
        json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":first["id"]});
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &active).is_err());
    let patch = text(&w["data"]["rounds"][0]["snapshot"], "patch");
    fs::remove_file(&patch).unwrap();
    fs::write(&patch, "tampered").unwrap();
    let source = tmp.path().join("source");
    let f = Fake {
        directory: source.to_str().unwrap().into(),
        ..Default::default()
    };
    workflows::tick(&s, &f, &mut runner()).unwrap();
    assert_eq!(
        s.workflow(first["id"].as_str().unwrap()).unwrap()["state"],
        "blocked"
    );
    assert!(
        s.tasks(None)
            .unwrap()
            .iter()
            .all(|t| t["request"]["workflow_id"] != first["id"])
    );
}

#[test]
fn integration_still_rejects_changed_checks_and_unknown_local_commit_outcomes() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, w) = ready_operator_fixture(tmp.path());
    let original = s.projects.clone();
    s.projects["project"]["checks"][0]["timeout"] = json!(1);
    assert!(
        plugin::call(
            &s,
            "ses_test",
            "foreman_integrate",
            &json!({"workflow_id":w["id"],"commit_local":true})
        )
        .is_err()
    );
    s.projects = original;
    assert!(
        plugin::call(
            &s,
            "ses_test",
            "foreman_integrate",
            &json!({"workflow_id":w["id"],"commit_local":true,"commit":w["base_revision"]})
        )
        .is_err()
    );
    let applied = plugin::call(
        &s,
        "ses_test",
        "foreman_integrate",
        &json!({"workflow_id":w["id"]}),
    )
    .unwrap();
    let mut reserved = applied;
    reserved["state"] = json!("committing");
    s.exec(
        "UPDATE integrations SET receipt=? WHERE workflow_id=?",
        rusqlite::params![canonical(&reserved), text(&w, "id")],
    )
    .unwrap();
    let source = tmp.path().join("source");
    assert!(
        plugin::call(
            &s,
            "ses_test",
            "foreman_integrate",
            &json!({"workflow_id":w["id"],"commit_local":true})
        )
        .is_err()
    );
    assert_eq!(
        git_text(&source, &["rev-parse", "HEAD"]).unwrap(),
        w["base_revision"]
    );
    // Simulate a commit whose response was lost: a read can record it, not
    // create a duplicate. Staging/commit here is test setup, not a tool retry.
    git(&source, &["add", "src/a"], None, None).unwrap();
    git(
        &source,
        &[
            "commit",
            "-m",
            &format!("Teem integrate workflow {}", text(&w, "id")),
        ],
        None,
        None,
    )
    .unwrap();
    let result = plugin::call(
        &s,
        "ses_test",
        "foreman_integrate",
        &json!({"workflow_id":w["id"],"commit_local":true}),
    )
    .unwrap();
    assert_eq!(result["state"], "committed");
    assert_eq!(
        git_text(&source, &["rev-list", "--count", "HEAD"]).unwrap(),
        "2"
    );
}

#[test]
fn operator_integration_rejects_dirty_source_foreign_owner_and_changed_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, w) = ready_operator_fixture(tmp.path());
    let args = json!({"workflow_id":w["id"]});
    s.bind("ses_other", "other-terminal").unwrap();
    assert!(plugin::call(&s, "ses_other", "foreman_integrate", &args).is_err());
    fs::write(tmp.path().join("source/unrelated"), "user data").unwrap();
    assert!(plugin::call(&s, "ses_test", "foreman_integrate", &args).is_err());
    assert_eq!(
        fs::read_to_string(tmp.path().join("source/src/a")).unwrap(),
        "base\n"
    );
    fs::remove_file(tmp.path().join("source/unrelated")).unwrap();
    let patch = text(&w["data"]["rounds"][0]["snapshot"], "patch");
    fs::remove_file(&patch).unwrap();
    fs::write(&patch, "tampered").unwrap();
    assert!(plugin::call(&s, "ses_test", "foreman_integrate", &args).is_err());
    assert_eq!(
        fs::read_to_string(tmp.path().join("source/src/a")).unwrap(),
        "base\n"
    );
}

#[test]
fn operator_can_record_an_exact_integrated_commit_without_changing_workflow_history() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, w) = ready_operator_fixture(tmp.path());
    let source = tmp.path().join("source");
    fs::write(source.join("src/a"), "changed\n").unwrap();
    git(&source, &["add", "src/a"], None, None).unwrap();
    git(&source, &["commit", "-qm", "integrated"], None, None).unwrap();
    let commit = git_text(&source, &["rev-parse", "HEAD"]).unwrap();
    let receipt = plugin::call(
        &s,
        "ses_test",
        "foreman_integrate",
        &json!({"workflow_id":w["id"],"commit":commit}),
    )
    .unwrap();
    assert_eq!(receipt["state"], "committed");
    assert_eq!(receipt["commit"], commit);
    assert_eq!(s.workflow(&text(&w, "id")).unwrap()["integration"], receipt);
    assert!(
        text(
            &progress::snapshot(&s, "ses_test").unwrap()["latest_result"],
            "content"
        )
        .contains("Recorded integration commit")
    );
    assert!(
        plugin::call(
            &s,
            "ses_test",
            "foreman_integrate",
            &json!({"workflow_id":w["id"],"commit":w["base_revision"]})
        )
        .is_err()
    );
}

#[test]
fn operator_color_changes_only_the_bound_foreman_color() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, _, _) = workflow_fixture(tmp.path(), false);
    let path = tmp.path().join("source/opencode.json");
    let mut config = json!({"agents":{"teem_foreman":{"color":"#C9956B","permissions":[{"action":"shell","effect":"ask","resource":"*"}]}},"mcp":{"servers":{"teem":{"command":["teem","mcp","--foreman","ses_test"]}}}});
    fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let args = json!({"project":"project","color":"#7D9DB8"});
    plugin::call(&s, "ses_test", "foreman_color", &args).unwrap();
    config["agents"]["teem_foreman"]["color"] = json!("#7D9DB8");
    assert_eq!(read_json(&path).unwrap(), config);
    assert!(
        plugin::call(
            &s,
            "ses_test",
            "foreman_color",
            &json!({"project":"project","color":"not a color"})
        )
        .is_err()
    );
    s.bind("ses_other", "other-terminal").unwrap();
    assert!(plugin::call(&s, "ses_other", "foreman_color", &args).is_err());
    assert_eq!(read_json(&path).unwrap(), config);
}

#[test]
fn operator_uncertain_apply_is_read_reconciled_but_never_repeated() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, w) = ready_operator_fixture(tmp.path());
    let id = text(&w, "id");
    let snapshot = &w["data"]["rounds"][0]["snapshot"];
    let receipt = json!({"workflow_id":id,"source":tmp.path().join("source"),"snapshot":snapshot["sha256"],"state":"attempting"});
    s.exec(
        "INSERT INTO integrations VALUES (?,?)",
        rusqlite::params![id, canonical(&receipt)],
    )
    .unwrap();
    let args = json!({"workflow_id":id});
    assert!(plugin::call(&s, "ses_test", "foreman_integrate", &args).is_err());
    assert_eq!(
        fs::read_to_string(tmp.path().join("source/src/a")).unwrap(),
        "base\n"
    );
    fs::write(tmp.path().join("source/src/a"), "changed\n").unwrap();
    assert_eq!(
        plugin::call(&s, "ses_test", "foreman_integrate", &args).unwrap()["state"],
        "applied"
    );
    let directory = s.directory.clone();
    let projects = s.projects.clone();
    drop(s);
    let s = Store::open(&directory, &[], projects).unwrap();
    assert_eq!(
        plugin::call(&s, "ses_test", "foreman_integrate", &args).unwrap()["state"],
        "applied"
    );
}

#[test]
fn operator_integration_rechecks_approved_log_bytes_and_ready_state() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, w) = ready_operator_fixture(tmp.path());
    let id = text(&w, "id");
    let args = json!({"workflow_id":id});
    s.exec("UPDATE workflows SET state='blocked' WHERE id=?", [&id])
        .unwrap();
    assert!(plugin::call(&s, "ses_test", "foreman_integrate", &args).is_err());
    s.exec("UPDATE workflows SET state='ready' WHERE id=?", [&id])
        .unwrap();
    let log = text(&w["data"]["rounds"][0]["check_result"]["results"][0], "log");
    fs::write(log, "tampered check output").unwrap();
    assert!(plugin::call(&s, "ses_test", "foreman_integrate", &args).is_err());
    assert_eq!(
        fs::read_to_string(tmp.path().join("source/src/a")).unwrap(),
        "base\n"
    );
    assert!(
        s.query("SELECT * FROM integrations", [])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn operator_integration_rejects_advanced_source_and_nonmatching_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, w) = ready_operator_fixture(tmp.path());
    let id = text(&w, "id");
    let root = tmp.path().join("source");
    fs::write(root.join("src/a"), "unrelated change\n").unwrap();
    git(&root, &["add", "src/a"], None, None).unwrap();
    git(&root, &["commit", "-qm", "unrelated"], None, None).unwrap();
    let commit = git_text(&root, &["rev-parse", "HEAD"]).unwrap();
    assert!(
        plugin::call(
            &s,
            "ses_test",
            "foreman_integrate",
            &json!({"workflow_id":id})
        )
        .is_err()
    );
    assert!(
        plugin::call(
            &s,
            "ses_test",
            "foreman_integrate",
            &json!({"workflow_id":id,"commit":commit})
        )
        .is_err()
    );
    assert_eq!(
        fs::read_to_string(root.join("src/a")).unwrap(),
        "unrelated change\n"
    );
    assert!(
        s.query("SELECT * FROM integrations", [])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn operator_color_rejects_symlinks_and_keeps_the_target_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, _, _) = workflow_fixture(tmp.path(), false);
    let target = tmp.path().join("private.json");
    fs::write(&target, "{}").unwrap();
    std::os::unix::fs::symlink(&target, tmp.path().join("source/opencode.json")).unwrap();
    assert!(
        plugin::call(
            &s,
            "ses_test",
            "foreman_color",
            &json!({"project":"project","color":"#7D9DB8"})
        )
        .is_err()
    );
    assert_eq!(fs::read_to_string(target).unwrap(), "{}");
}
#[test]
fn two_workers_checks_and_two_exact_reviews_gate_ready_and_one_final_notice() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), true);
    let id = text(&w, "id");
    let mut runner = runner();
    workflows::tick(&s, &f, &mut runner).unwrap();
    assert_eq!(s.tasks(None).unwrap().len(), 2);
    finish_workers(&s);
    workflows::tick(&s, &f, &mut runner).unwrap();
    let w = s.workflow(&id).unwrap();
    assert_eq!(w["state"], "checking", "{w}");
    let rd = &w["data"]["rounds"][0];
    await_receipt(&rd["check_handle"]);
    workflows::tick(&s, &f, &mut runner).unwrap();
    let w = s.workflow(&id).unwrap();
    assert_eq!(w["state"], "reviewing", "{w}");
    let hash = &w["data"]["rounds"][0]["snapshot"]["sha256"];
    for task in s.tasks(None).unwrap() {
        if task["role"] == "reviewer" {
            s.update(
                &text(&task, "id"),
                "succeeded",
                &json!({"snapshot":hash,"verdict":"pass","findings":[]}).to_string(),
                "",
            )
            .unwrap();
        }
    }
    workflows::tick(&s, &f, &mut runner).unwrap();
    assert_eq!(s.workflow(&id).unwrap()["state"], "ready");
    let snap = progress::snapshot(&s, "ses_test").unwrap();
    assert_eq!(snap["todos"], json!([]));
    assert_eq!(snap["latest_result"]["state"], "ready");
    assert_eq!(
        s.query("SELECT * FROM events WHERE workflow_id IS NOT NULL", [])
            .unwrap()
            .len(),
        1
    );
    runtime::notify(&s, &f).unwrap();
    assert_eq!(*f.notifications.borrow(), 1);
}
#[test]
fn interrupted_workflow_does_not_prepare_or_replay_agents() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), false);
    let id = text(&w, "id");
    s.exec("UPDATE workflows SET state='preparing' WHERE id=?", [&id])
        .unwrap();
    workflows::tick(&s, &f, &mut runner()).unwrap();
    assert_eq!(s.workflow(&id).unwrap()["state"], "uncertain");
    assert!(s.tasks(None).unwrap().is_empty());
    workflows::tick(&s, &f, &mut runner()).unwrap();
    assert!(s.tasks(None).unwrap().is_empty());
}

#[test]
fn requested_stop_freezes_workflow_and_queued_agents_without_discarding_work() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), false);
    let id = text(&w, "id");
    let mut runner = runner();
    workflows::tick(&s, &f, &mut runner).unwrap();
    let task = s.tasks(None).unwrap().remove(0);
    let file = PathBuf::from(text(&task, "checkout")).join("src/a");
    fs::write(&file, "unfinished work\n").unwrap();
    let args = json!({"workflow_id":id});
    let stop = plugin::call(&s, "ses_test", "foreman_stop", &args).unwrap();
    assert_eq!(stop["workflow_id"], id);
    workflows::tick(&s, &f, &mut runner).unwrap();
    runtime::tick(&s, &f).unwrap();
    assert!(f.calls.borrow().is_empty());
    assert_eq!(s.workflow(&id).unwrap()["state"], "blocked");
    assert_eq!(fs::read_to_string(&file).unwrap(), "unfinished work\n");
    assert_eq!(s.tasks(None).unwrap().len(), 1);
    assert!(plugin::call(&s, "ses_test", "foreman_stop", &args).is_ok());
    let detail = json!({"mode":"detail","section":"stopped_changes","task_id":task["id"],"file":"src/a","pointer":"/observed/patch","limit":4096});
    assert!(s.status("ses_test", &detail).is_err());
    teem::control::tick(&s, &f).unwrap();
    let chunk = s.status("ses_test", &detail).unwrap();
    let patch: String = serde_json::from_str(chunk["chunk"].as_str().unwrap()).unwrap();
    assert!(patch.contains("+unfinished work"));
    assert!(chunk["next_offset"].is_null());
    assert_eq!(fs::read_to_string(&file).unwrap(), "unfinished work\n");
}

#[test]
fn stop_confirmed_by_reads_survives_reopen_and_never_replaces_the_worker() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), false);
    let id = text(&w, "id");
    workflows::tick(&s, &f, &mut runner()).unwrap();
    runtime::tick(&s, &f).unwrap();
    let task = s.tasks(None).unwrap().remove(0);
    s.update(&text(&task, "id"), "running", "partial result", "")
        .unwrap();
    *f.stop_live.borrow_mut() = true;
    f.calls.borrow_mut().clear();
    plugin::call(
        &s,
        "ses_test",
        "foreman_stop",
        &json!({"workflow_id":id,"task_id":task["id"]}),
    )
    .unwrap();
    teem::control::tick(&s, &f).unwrap();
    assert_ne!(s.workflow(&id).unwrap()["stop"]["state"], "stopped");
    teem::control::tick(&s, &f).unwrap();
    assert_eq!(s.workflow(&id).unwrap()["stop"]["state"], "stopped");
    assert_eq!(s.task(&text(&task, "id")).unwrap()["state"], "interrupted");
    assert_eq!(
        s.task(&text(&task, "id")).unwrap()["result"],
        "partial result"
    );
    let directory = s.directory.clone();
    let projects = s.projects.clone();
    drop(s);
    let s = Store::open(&directory, &[], projects).unwrap();
    for _ in 0..2 {
        teem::control::tick(&s, &f).unwrap();
        workflows::tick(&s, &f, &mut runner()).unwrap();
        runtime::tick(&s, &f).unwrap();
    }
    assert_eq!(s.tasks(None).unwrap().len(), 1);
    assert_eq!(f.calls.borrow().iter().filter(|c| c.0 == "STOP").count(), 1);
    assert!(
        !f.calls
            .borrow()
            .iter()
            .any(|c| c.0 == "CREATE" || c.0 == "PROMPT")
    );
}

#[test]
fn unknown_stop_response_is_never_resent_and_idle_read_can_reconcile_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, mut f, w) = workflow_fixture(tmp.path(), false);
    let id = text(&w, "id");
    workflows::tick(&s, &f, &mut runner()).unwrap();
    runtime::tick(&s, &f).unwrap();
    f.stop_lost = true;
    *f.stop_live.borrow_mut() = true;
    f.calls.borrow_mut().clear();
    plugin::call(&s, "ses_test", "foreman_stop", &json!({"workflow_id":id})).unwrap();
    for _ in 0..3 {
        teem::control::tick(&s, &f).unwrap();
    }
    assert_eq!(s.workflow(&id).unwrap()["stop"]["state"], "uncertain");
    assert_eq!(f.calls.borrow().iter().filter(|c| c.0 == "STOP").count(), 1);
    *f.stop_live.borrow_mut() = false;
    teem::control::tick(&s, &f).unwrap();
    assert_eq!(s.workflow(&id).unwrap()["stop"]["state"], "stopped");
}

#[test]
fn stopped_workflow_rejects_stale_controller_saves_and_foreign_stop_requests() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, mut w) = workflow_fixture(tmp.path(), false);
    let id = text(&w, "id");
    workflows::tick(&s, &f, &mut runner()).unwrap();
    s.bind("ses_other", "other-terminal").unwrap();
    assert!(plugin::call(&s, "ses_other", "foreman_stop", &json!({"workflow_id":id})).is_err());
    assert!(
        s.query("SELECT * FROM workflow_stops", [])
            .unwrap()
            .is_empty()
    );
    assert!(
        plugin::call(
            &s,
            "ses_test",
            "foreman_stop",
            &json!({"workflow_id":id,"task_id":uuid::Uuid::new_v4().to_string()})
        )
        .is_err()
    );
    plugin::call(&s, "ses_test", "foreman_stop", &json!({"workflow_id":id})).unwrap();
    assert!(
        s.save_workflow(&mut w, "working", "stale progress")
            .is_err()
    );
    teem::control::tick(&s, &f).unwrap();
    assert_eq!(s.tasks(None).unwrap()[0]["state"], "interrupted");
    assert_eq!(s.workflow(&id).unwrap()["stop"]["state"], "stopped");
    assert_eq!(s.workflow(&id).unwrap()["state"], "blocked");
    assert!(s.claim().unwrap().is_none());
}

#[test]
fn queued_workflow_can_be_stopped_before_checkout_preparation() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), false);
    plugin::call(
        &s,
        "ses_test",
        "foreman_stop",
        &json!({"workflow_id":w["id"]}),
    )
    .unwrap();
    workflows::tick(&s, &f, &mut runner()).unwrap();
    runtime::tick(&s, &f).unwrap();
    assert!(s.tasks(None).unwrap().is_empty());
    assert!(!s.directory.join("managed").exists());
    assert_eq!(
        s.workflow(&text(&w, "id")).unwrap()["stop"]["state"],
        "stopped"
    );
}

#[test]
fn stop_cancels_permission_relay_without_answering_a_pending_worker_card() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), false);
    workflows::tick(&s, &f, &mut runner()).unwrap();
    let t = s.tasks(None).unwrap().remove(0);
    s.update(&text(&t, "id"), "running", "", "").unwrap();
    *f.pending.borrow_mut() = vec![
        json!({"permission":{"id":"per_stop","sessionID":t["session_id"],"action":"shell","resources":["git diff"]},"input":{"command":"git diff"}}),
    ];
    approvals::deliver(&s, &f).unwrap();
    for form in f.forms.borrow_mut().values_mut() {
        form["state"] = json!({"status":"answered","answer":{"decision":"once"}});
    }
    plugin::call(
        &s,
        "ses_test",
        "foreman_stop",
        &json!({"workflow_id":w["id"]}),
    )
    .unwrap();
    approvals::deliver(&s, &f).unwrap();
    assert!(
        !f.calls
            .borrow()
            .iter()
            .any(|(_, path, _)| path.ends_with("/reply"))
    );
    assert_eq!(
        s.query("SELECT reply_attempted FROM approvals", [])
            .unwrap()[0]["reply_attempted"],
        0
    );
}

#[test]
fn native_interrupt_transport_accepts_its_unwrapped_boolean_contract() {
    let tmp = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let registry = tmp.path().join("registry.json");
    fs::write(&registry,json!({"url":format!("http://{}",listener.local_addr().unwrap()),"password":"test-only-password"}).to_string()).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0; 4096];
        let n = stream.read(&mut buf).unwrap();
        assert!(
            String::from_utf8_lossy(&buf[..n]).starts_with("POST /api/session/ses_test/interrupt ")
        );
        let body = b"{\"interrupted\":true}";
        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).unwrap();
        stream.write_all(body).unwrap();
    });
    let r = runtime::OpenCode::new(&registry, "test/model").unwrap();
    assert_eq!(
        r.request("POST", "/api/session/ses_test/interrupt", None)
            .unwrap(),
        json!({"interrupted":true})
    );
    server.join().unwrap();
}

fn delayed_native_response(method: &str, path: &str) -> Result<Value> {
    let tmp = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let registry = tmp.path().join("registry.json");
    fs::write(&registry,json!({"url":format!("http://{}",listener.local_addr().unwrap()),"password":"test-only-password"}).to_string()).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut bytes = [0; 4096];
        let received = stream.read(&mut bytes).unwrap();
        assert!(received > 0, "Native request did not arrive");
        thread::sleep(Duration::from_secs(11));
        // A ten-second control request deliberately disconnects before this.
        let _=stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 20\r\nConnection: close\r\n\r\n{\"data\":{\"id\":\"ok\"}}");
    });
    let runtime = runtime::OpenCode::new(&registry, "test/model").unwrap();
    let result = runtime.request(method, path, None);
    server.join().unwrap();
    result
}

#[test]
fn native_prompt_admission_survives_more_than_ten_seconds_of_server_load() {
    assert_eq!(
        delayed_native_response("POST", "/api/session/ses_test/prompt").unwrap(),
        json!({"id":"ok"})
    );
}

#[test]
fn native_session_creation_survives_more_than_ten_seconds_of_server_load() {
    assert_eq!(
        delayed_native_response("POST", "/api/session").unwrap(),
        json!({"id":"ok"})
    );
}

#[test]
fn native_observation_calls_keep_the_short_timeout() {
    assert!(delayed_native_response("GET", "/api/session/ses_test").is_err());
}

#[test]
fn native_stop_requires_exact_identity_and_does_not_call_queued_input_stopped() {
    struct NativeStop {
        session: Value,
        inbox: Value,
        posts: RefCell<usize>,
    }
    impl Runtime for NativeStop {
        fn request(&self, m: &str, path: &str, _: Option<&Value>) -> Result<Value> {
            if m == "POST" {
                *self.posts.borrow_mut() += 1;
                return Ok(json!({"interrupted":true}));
            }
            Ok(if path.ends_with("/active") {
                json!({})
            } else if path.ends_with("/inbox") {
                self.inbox.clone()
            } else {
                self.session.clone()
            })
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, _) = workflow_fixture(tmp.path(), false);
    workflows::tick(&s, &f, &mut runner()).unwrap();
    let mut task = s.tasks(None).unwrap().remove(0);
    task["runtime_model"] = json!({"providerID":"test","id":"model"});
    let session = runtime::session_payload(&task, &task["runtime_model"]);
    let mut native = NativeStop {
        session,
        inbox: json!([]),
        posts: RefCell::new(0),
    };
    assert!(native.is_stopped(&task).unwrap());
    native.inbox = json!([{"id":task["prompt_id"]}]);
    assert!(!native.is_stopped(&task).unwrap());
    native.interrupt(&task).unwrap();
    assert_eq!(*native.posts.borrow(), 1);
    native.session["metadata"]["herdr_foreman"]["task_id"] = json!("foreign-task");
    assert!(native.is_stopped(&task).is_err());
    assert!(native.interrupt(&task).is_err());
    assert_eq!(*native.posts.borrow(), 1);
}

#[test]
fn live_changes_show_scoped_edits_and_new_files_without_touching_the_index() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, _w) = workflow_fixture(tmp.path(), false);
    workflows::tick(&s, &f, &mut runner()).unwrap();
    let t = s.tasks(None).unwrap().remove(0);
    let root = PathBuf::from(text(&t, "checkout"));
    let index = fs::read(root.join(".git/index")).unwrap();
    fs::write(root.join("src/a"), "real implementation\n").unwrap();
    fs::write(root.join("src/new file"), "new code\n").unwrap();
    fs::write(root.join("private-secret"), "not in task scope\n").unwrap();
    let args = json!({"task_id":t["id"],"section":"changes","mode":"detail","limit":4096});
    let result = plugin::call(&s, "ses_test", "foreman_status", &args).unwrap();
    let view: Value = serde_json::from_str(result["chunk"].as_str().unwrap()).unwrap();
    assert_eq!(view["files"].as_array().unwrap().len(), 2);
    assert_eq!(view["outside_scope_entries_omitted"], 1);
    assert!(!result.to_string().contains("private-secret"));
    let mut detail = args.clone();
    detail["file"] = json!("src/a");
    let result = plugin::call(&s, "ses_test", "foreman_status", &detail).unwrap();
    assert!(
        result["chunk"]
            .as_str()
            .unwrap()
            .contains("+real implementation")
    );
    detail["file"] = json!("src/new file");
    assert!(
        plugin::call(&s, "ses_test", "foreman_status", &detail).unwrap()["chunk"]
            .as_str()
            .unwrap()
            .contains("new code")
    );
    detail["file"] = json!("private-secret");
    assert!(plugin::call(&s, "ses_test", "foreman_status", &detail).is_err());
    detail["file"] = json!("../escape");
    assert!(plugin::call(&s, "ses_test", "foreman_status", &detail).is_err());
    assert_eq!(index, fs::read(root.join(".git/index")).unwrap());
    // A required broken clean filter would make worktree git diff fail.
    // Observation must read actual bytes without invoking that driver.
    fs::write(root.join(".git/info/attributes"), "* filter=must_not_run\n").unwrap();
    git(
        &root,
        &[
            "config",
            "filter.must_not_run.clean",
            "teem-missing-command-do-not-execute",
        ],
        None,
        None,
    )
    .unwrap();
    git(
        &root,
        &["config", "filter.must_not_run.required", "true"],
        None,
        None,
    )
    .unwrap();
    assert!(plugin::call(&s, "ses_test", "foreman_status", &args).is_ok());
    let previous = plugin::call(&s, "ses_test", "foreman_status", &args).unwrap()["sha256"].clone();
    fs::write(root.join("src/another"), "changed again\n").unwrap();
    assert_ne!(
        previous,
        plugin::call(&s, "ses_test", "foreman_status", &args).unwrap()["sha256"]
    );
    std::os::unix::fs::symlink(root.join("private-secret"), root.join("src/link")).unwrap();
    detail["file"] = json!("src/link");
    assert!(plugin::call(&s, "ses_test", "foreman_status", &detail).is_err());
    s.bind("ses_other", "other-terminal").unwrap();
    assert!(plugin::call(&s, "ses_other", "foreman_status", &args).is_err());
}

#[test]
fn consuming_inbox_does_not_falsely_confirm_stop_and_running_inspection_wins() {
    struct Consuming {
        session: Value,
        reads: RefCell<usize>,
    }
    impl Runtime for Consuming {
        fn request(&self, _: &str, path: &str, _: Option<&Value>) -> Result<Value> {
            if path.ends_with("/active") {
                let n = *self.reads.borrow();
                *self.reads.borrow_mut() += 1;
                return Ok(if n == 0 {
                    json!({})
                } else {
                    json!({text(&self.session,"id"):{"state":"running"}})
                });
            }
            Ok(if path.ends_with("/inbox") {
                json!([])
            } else {
                self.session.clone()
            })
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), false);
    workflows::tick(&s, &f, &mut runner()).unwrap();
    runtime::tick(&s, &f).unwrap();
    let task = s.tasks(None).unwrap().remove(0);
    let session = runtime::session_payload(&task, &task["runtime_model"]);
    let r = Consuming {
        session,
        reads: RefCell::new(0),
    };
    assert!(!r.is_stopped(&task).unwrap());
    plugin::call(
        &s,
        "ses_test",
        "foreman_stop",
        &json!({"workflow_id":w["id"]}),
    )
    .unwrap();
    // Fake says inactive, but its inspection says running: do not finalize.
    teem::control::tick(&s, &f).unwrap();
    assert_eq!(
        s.workflow(&text(&w, "id")).unwrap()["stop"]["state"],
        "requested"
    );
}

#[test]
fn native_prompt_is_serialized_with_stop_creation_across_connections() {
    use std::os::fd::AsRawFd;
    struct Admission<'a> {
        s: &'a Store,
        f: Fake,
    }
    impl Runtime for Admission<'_> {
        fn request(&self, m: &str, p: &str, v: Option<&Value>) -> Result<Value> {
            self.f.request(m, p, v)
        }
        fn dispatch_model(&self, t: &Value) -> Result<Value> {
            self.f.dispatch_model(t)
        }
        fn create(&self, t: &Value) -> Result<()> {
            self.f.create(t)
        }
        fn prompt(&self, t: &Value) -> Result<()> {
            let lock = open_lock(&self.s.directory.join("native-admission.lock"))?;
            assert_ne!(
                unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
                0
            );
            self.f.prompt(t)
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), false);
    workflows::tick(&s, &f, &mut runner()).unwrap();
    let r = Admission {
        s: &s,
        f: Fake::default(),
    };
    runtime::tick(&s, &r).unwrap();
    assert!(r.f.calls.borrow().iter().any(|c| c.0 == "PROMPT"));
    plugin::call(
        &s,
        "ses_test",
        "foreman_stop",
        &json!({"workflow_id":w["id"]}),
    )
    .unwrap();
}

#[test]
fn stop_committing_during_form_read_prevents_permission_reply() {
    struct StopOnRead<'a> {
        s: &'a Store,
        f: &'a Fake,
        w: String,
        armed: RefCell<bool>,
    }
    impl Runtime for StopOnRead<'_> {
        fn request(&self, m: &str, p: &str, v: Option<&Value>) -> Result<Value> {
            let value = self.f.request(m, p, v)?;
            if m == "GET" && p.contains("/form/") && *self.armed.borrow() {
                *self.armed.borrow_mut() = false;
                plugin::call(
                    self.s,
                    "ses_test",
                    "foreman_stop",
                    &json!({"workflow_id":self.w}),
                )?;
            }
            Ok(value)
        }
        fn approvals(&self, t: &Value) -> Result<Vec<Value>> {
            self.f.approvals(t)
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), false);
    workflows::tick(&s, &f, &mut runner()).unwrap();
    let t = s.tasks(None).unwrap().remove(0);
    s.update(&text(&t, "id"), "running", "", "").unwrap();
    *f.pending.borrow_mut() = vec![
        json!({"permission":{"id":"per_race","sessionID":t["session_id"],"action":"shell","resources":["git diff"]}}),
    ];
    approvals::deliver(&s, &f).unwrap();
    for form in f.forms.borrow_mut().values_mut() {
        form["state"] = json!({"status":"answered","answer":{"decision":"once"}});
    }
    let r = StopOnRead {
        s: &s,
        f: &f,
        w: text(&w, "id"),
        armed: RefCell::new(true),
    };
    approvals::deliver(&s, &r).unwrap();
    assert!(
        !f.calls
            .borrow()
            .iter()
            .any(|(_, p, _)| p.ends_with("/reply"))
    );
}

#[test]
fn inspection_includes_staged_additions_and_mode_only_edits_in_preserved_handoff() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), false);
    workflows::tick(&s, &f, &mut runner()).unwrap();
    let t = s.tasks(None).unwrap().remove(0);
    let root = PathBuf::from(text(&t, "checkout"));
    fs::write(root.join("src/staged"), "staged new bytes\n").unwrap();
    git(&root, &["add", "--", "src/staged"], None, None).unwrap();
    fs::set_permissions(root.join("src/a"), fs::Permissions::from_mode(0o755)).unwrap();
    let index = fs::read(root.join(".git/index")).unwrap();
    let args = json!({"task_id":t["id"],"section":"changes","mode":"detail","limit":4096});
    let result = s.status("ses_test", &args).unwrap();
    let manifest: Value = serde_json::from_str(result["chunk"].as_str().unwrap()).unwrap();
    assert_eq!(manifest["files"].as_array().unwrap().len(), 2);
    assert_eq!(manifest["files"][0]["before_executable"], false);
    assert_eq!(manifest["files"][0]["after_executable"], true);
    let mut selected = args.clone();
    selected["file"] = json!("src/staged");
    assert!(
        s.status("ses_test", &selected).unwrap()["chunk"]
            .as_str()
            .unwrap()
            .contains("staged new bytes")
    );
    plugin::call(
        &s,
        "ses_test",
        "foreman_stop",
        &json!({"workflow_id":w["id"]}),
    )
    .unwrap();
    teem::control::tick(&s, &f).unwrap();
    selected["section"] = json!("stopped_changes");
    assert!(
        s.status("ses_test", &selected).unwrap()["chunk"]
            .as_str()
            .unwrap()
            .contains("staged new bytes")
    );
    assert_eq!(index, fs::read(root.join(".git/index")).unwrap());
}

#[test]
fn stop_during_native_creation_prevents_the_paid_prompt() {
    struct StopDuringCreate<'a> {
        store: &'a Store,
        fake: Fake,
    }
    impl Runtime for StopDuringCreate<'_> {
        fn request(&self, m: &str, path: &str, p: Option<&Value>) -> Result<Value> {
            self.fake.request(m, path, p)
        }
        fn dispatch_model(&self, t: &Value) -> Result<Value> {
            self.fake.dispatch_model(t)
        }
        fn create(&self, t: &Value) -> Result<()> {
            self.fake.create(t)?;
            plugin::call(
                self.store,
                "ses_test",
                "foreman_stop",
                &json!({"workflow_id":t["request"]["workflow_id"]}),
            )?;
            Ok(())
        }
        fn prompt(&self, t: &Value) -> Result<()> {
            self.fake.prompt(t)
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), false);
    workflows::tick(&s, &f, &mut runner()).unwrap();
    let r = StopDuringCreate {
        store: &s,
        fake: Fake::default(),
    };
    runtime::tick(&s, &r).unwrap();
    assert!(r.fake.calls.borrow().iter().any(|c| c.0 == "CREATE"));
    assert!(!r.fake.calls.borrow().iter().any(|c| c.0 == "PROMPT"));
    assert_eq!(s.workflow(&text(&w, "id")).unwrap()["state"], "blocked");
    assert_eq!(s.tasks(None).unwrap()[0]["state"], "interrupted");
}
#[test]
fn policy_revocation_blocks_new_work_without_changing_source() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, f, w) = workflow_fixture(tmp.path(), false);
    s.projects = json!({});
    workflows::tick(&s, &f, &mut runner()).unwrap();
    assert_eq!(s.workflow(&text(&w, "id")).unwrap()["state"], "blocked");
    assert!(s.tasks(None).unwrap().is_empty());
}
#[test]
fn wrong_or_prose_review_verdict_never_qualifies() {
    let snap = json!({"sha256":"snapshot"});
    for v in [
        "PASS".to_owned(),
        json!({"snapshot":"wrong","verdict":"pass","findings":[]}).to_string(),
        json!({"snapshot":"snapshot","verdict":"changes_requested","findings":[]}).to_string(),
    ] {
        assert!(workflows::verdict(&v, &snap).is_err());
    }
    assert!(
        workflows::verdict(
            &json!({"snapshot":"snapshot","verdict":"pass","findings":[]}).to_string(),
            &snap
        )
        .is_ok()
    );
}
#[test]
fn permission_reply_is_attempted_once_after_a_lost_response() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("source");
    let base = repo(&p);
    let s = store(tmp.path(), &[p.to_str().unwrap().into()], json!({}));
    let t = s.submit("ses_test", req(&p, &base, "worker")).unwrap();
    s.update(&text(&t, "id"), "running", "", "").unwrap();
    let snapshot = json!({"permission":{"id":"per_one","sessionID":t["session_id"],"action":"shell","resources":["test"]},"input":{"command":"test"}});
    let f = Fake {
        pending: RefCell::new(vec![snapshot]),
        ..Default::default()
    };
    approvals::deliver(&s, &f).unwrap();
    for form in f.forms.borrow_mut().values_mut() {
        form["state"] = json!({"status":"answered","answer":{"decision":"once"}});
    }
    approvals::deliver(&s, &f).unwrap();
    approvals::deliver(&s, &f).unwrap();
    assert_eq!(
        f.calls
            .borrow()
            .iter()
            .filter(|(_, p, _)| p.ends_with("/reply"))
            .count(),
        1
    );
    assert_eq!(
        s.query("SELECT reply_attempted FROM approvals", [])
            .unwrap()[0]["reply_attempted"],
        1
    );
}
#[test]
fn mcp_oversized_line_is_drained_and_next_request_remains_usable() {
    let tmp = tempfile::tempdir().unwrap();
    let s = store(tmp.path(), &[], json!({}));
    let mut input = vec![b'x'; 70000];
    input.extend_from_slice(b"\n{\"id\":2,\"method\":\"ping\"}\n");
    let mut out = Vec::new();
    plugin::mcp(&s, "ses_test", std::io::Cursor::new(input), &mut out).unwrap();
    let lines = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str::<Value>(s).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["error"]["code"], -32700);
    assert_eq!(lines[1]["id"], 2);
}
#[test]
fn loopback_http_does_not_follow_redirects_or_disclose_credentials() {
    let tmp = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let path = tmp.path().join("registry.json");
    fs::write(
        &path,
        json!({"url":url,"password":"test-only-password"}).to_string(),
    )
    .unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = [0; 4096];
        let n = stream.read(&mut buf).unwrap();
        assert!(
            String::from_utf8_lossy(&buf[..n])
                .to_lowercase()
                .contains("authorization: basic ")
        );
        stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: http://example.invalid/steal\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    });
    let r = runtime::OpenCode::new(&path, "test/model").unwrap();
    let e = r
        .request("GET", "/api/session/test", None)
        .unwrap_err()
        .to_string();
    assert!(!e.contains("test-only-password"));
    server.join().unwrap();
}

struct Routes {
    values: BTreeMap<String, Value>,
}
impl Runtime for Routes {
    fn request(&self, method: &str, path: &str, _: Option<&Value>) -> Result<Value> {
        assert_eq!(method, "GET");
        self.values
            .get(path)
            .cloned()
            .ok_or_else(|| runtime::Missing.into())
    }
}
fn native_routes() -> (Value, Routes) {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/legacy-wire.json")).unwrap();
    let t = fixture["task"].clone();
    let prefix = format!("/api/session/{}", text(&t, "session_id"));
    let mut session = fixture["session_payload"].clone();
    session["time"] = json!({"idle":20});
    session["outcome"] = json!("succeeded");
    let mut values = BTreeMap::new();
    values.insert(prefix.clone(), session);
    values.insert(format!("{prefix}/inbox"), json!([]));
    values.insert(format!("{prefix}/message/{}",text(&t,"prompt_id")),json!({"id":t["prompt_id"],"type":"user","text":runtime::prompt_text(&t),"time":{"created":10}}));
    values.insert("/api/session/active".into(), json!({}));
    values.insert(format!("{prefix}/message?limit=20&order=desc"),json!([{"id":"msg_final","type":"assistant","time":{"created":11,"completed":12},"content":[{"type":"text","text":"Finished implementation"}]}]));
    (t, Routes { values })
}
#[test]
fn native_inspection_requires_exact_input_and_final_completed_text() {
    let (t, mut r) = native_routes();
    assert_eq!(r.inspect(&t).unwrap()["state"], "succeeded");
    let prefix = format!("/api/session/{}", text(&t, "session_id"));
    r.values
        .get_mut(&format!("{prefix}/message?limit=20&order=desc"))
        .unwrap()[0]["finish"] = json!("tool-call");
    assert_eq!(r.inspect(&t).unwrap()["state"], "uncertain");
    r.values
        .get_mut(&format!("{prefix}/message/{}", text(&t, "prompt_id")))
        .unwrap()["text"] = json!("another request");
    assert!(r.inspect(&t).is_err());
}
#[test]
fn native_failure_and_queued_admission_are_not_success() {
    let (t, mut r) = native_routes();
    let prefix = format!("/api/session/{}", text(&t, "session_id"));
    r.values.get_mut(&prefix).unwrap()["outcome"] = json!("failed");
    assert_eq!(r.inspect(&t).unwrap()["state"], "failed");
    r.values.insert(
        format!("{prefix}/inbox"),
        json!([{"id":t["prompt_id"],"type":"user","payload":{"text":runtime::prompt_text(&t)}}]),
    );
    assert_eq!(r.inspect(&t).unwrap()["state"], "accepted");
}
#[test]
fn inherited_model_and_variant_must_match_exact_native_session() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/legacy-wire.json")).unwrap();
    let t = &fixture["task"];
    let mut session = fixture["session_payload"].clone();
    session["model"]["variant"] = json!("default");
    runtime::validate_session(t, &session, &json!({})).unwrap();
    session["model"]["id"] = json!("other");
    assert!(runtime::validate_session(t, &session, &json!({})).is_err());
    session["model"] = fixture["session_payload"]["model"].clone();
    session["permissions"] = json!([]);
    assert!(runtime::validate_session(t, &session, &json!({})).is_err());
}
#[test]
fn old_schema_preserves_legacy_events_and_unknown_permission_reply_attempts() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("state");
    fs::create_dir(&dir).unwrap();
    let db = rusqlite::Connection::open(dir.join("tasks.sqlite3")).unwrap();
    db.execute_batch("CREATE TABLE events(id TEXT PRIMARY KEY,task_id TEXT UNIQUE NOT NULL,foreman TEXT NOT NULL,state TEXT DEFAULT 'pending',request_id TEXT NOT NULL UNIQUE,acked INTEGER DEFAULT 0,next_receipt REAL DEFAULT 0); INSERT INTO events VALUES ('old','task','ses_test','uncertain','request',0,0);CREATE TABLE approvals(task_id TEXT,permission_id TEXT,foreman TEXT,form_id TEXT UNIQUE,snapshot TEXT,payload TEXT,state TEXT,detail TEXT DEFAULT '',PRIMARY KEY(task_id,permission_id));INSERT INTO approvals VALUES ('task','permission','ses_test','form','{}','{}','uncertain','');").unwrap();
    drop(db);
    let s = Store::open(&dir, &[], json!({})).unwrap();
    assert_eq!(
        s.query("SELECT transport FROM events", []).unwrap()[0]["transport"],
        "legacy-codr"
    );
    assert_eq!(
        s.query("SELECT reply_attempted FROM approvals", [])
            .unwrap()[0]["reply_attempted"],
        1
    );
    let f = Fake::default();
    runtime::notify(&s, &f).unwrap();
    assert_eq!(*f.notifications.borrow(), 0);
}

#[test]
fn existing_state_database_permissions_are_tightened_on_open() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let s = store(tmp.path(), &[], json!({}));
    let path = s.directory.join("tasks.sqlite3");
    let directory = s.directory.clone();
    drop(s);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let _s = Store::open(&directory, &[], json!({})).unwrap();
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
#[test]
fn native_reported_todos_never_become_review_or_check_evidence() {
    let (t, mut r) = native_routes();
    let prefix = format!("/api/session/{}", text(&t, "session_id"));
    r.values.insert(format!("{prefix}/message?limit=50&order=desc&type=assistant"),json!([{"id":"msg_report","type":"assistant","content":[{"type":"tool","name":"todowrite","state":{"status":"completed","input":{"todos":[{"content":"  Tests  passed  ","status":"completed"}]}}}]}]));
    let report = r.todos(&t).unwrap().unwrap();
    assert_eq!(report["todos"][0]["content"], "Tests passed");
    assert_eq!(r.inspect(&t).unwrap()["state"], "succeeded");
}
#[test]
fn two_workflows_have_independent_worker_reservations() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), true);
    let request = w["request"].clone();
    let mut another = request;
    another["workflow_id"] = json!(uuid::Uuid::new_v4().to_string());
    s.workflow_submit("ses_test", another).unwrap();
    workflows::tick(&s, &f, &mut runner()).unwrap();
    for _ in 0..4 {
        assert!(s.claim().unwrap().is_some());
    }
    assert!(s.claim().unwrap().is_none());
    assert_eq!(s.tasks(None).unwrap().len(), 4);
}
#[test]
fn nonpassing_reviewer_forces_a_bounded_revision_with_new_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), false);
    let id = text(&w, "id");
    let mut runner = runner();
    workflows::tick(&s, &f, &mut runner).unwrap();
    finish_workers(&s);
    workflows::tick(&s, &f, &mut runner).unwrap();
    let w = s.workflow(&id).unwrap();
    await_receipt(&w["data"]["rounds"][0]["check_handle"]);
    workflows::tick(&s, &f, &mut runner).unwrap();
    let w = s.workflow(&id).unwrap();
    let hash = w["data"]["rounds"][0]["snapshot"]["sha256"].clone();
    for task in s.tasks(None).unwrap() {
        if task["role"] == "reviewer" {
            s.update(&text(&task,"id"),"succeeded",&json!({"snapshot":hash,"verdict":"changes_requested","findings":[{"path":"src/a","line":1,"message":"Demonstrated fixture issue"}]}).to_string(),"").unwrap();
        }
    }
    workflows::tick(&s, &f, &mut runner).unwrap();
    let w = s.workflow(&id).unwrap();
    assert_eq!(w["state"], "working", "{w}");
    assert_eq!(w["round"], 1);
    assert_eq!(w["data"]["rounds"].as_array().unwrap().len(), 2);
    assert_ne!(
        w["data"]["rounds"][0]["worker_id"],
        w["data"]["rounds"][1]["worker_id"]
    );
    assert_eq!(
        fs::read_to_string(PathBuf::from(text(&w["data"]["rounds"][1], "checkout")).join("src/a"))
            .unwrap(),
        "changed\n"
    );
}

#[test]
fn default_workflow_allows_three_corrections_with_fresh_checks_and_reviews() {
    exercise_default_revisions(true);
}

#[test]
fn default_workflow_stops_after_four_implementation_rounds_without_replay() {
    exercise_default_revisions(false);
}

fn exercise_default_revisions(pass_last: bool) {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, f, w) = workflow_fixture_revisions(tmp.path(), false, None);
    assert_eq!(w["policy"]["max_revisions"], 3);
    s.projects["project"]["max_revisions"] = json!(0);
    let id = text(&w, "id");
    let mut runner = runner();
    workflows::tick(&s, &f, &mut runner).unwrap();
    let mut workers = std::collections::BTreeSet::new();
    let mut reviewers = std::collections::BTreeSet::new();
    for round in 0..4 {
        let w = s.workflow(&id).unwrap();
        assert_eq!(w["state"], "working");
        assert_eq!(w["round"], round);
        let r = &w["data"]["rounds"][round];
        assert!(workers.insert(text(r, "worker_id")));
        finish_workers(&s);
        workflows::tick(&s, &f, &mut runner).unwrap();
        let w = s.workflow(&id).unwrap();
        assert_eq!(w["state"], "checking");
        await_receipt(&w["data"]["rounds"][round]["check_handle"]);
        workflows::tick(&s, &f, &mut runner).unwrap();
        let w = s.workflow(&id).unwrap();
        assert_eq!(w["state"], "reviewing");
        let r = &w["data"]["rounds"][round];
        let ids = array(&r["review_ids"]);
        assert_eq!(ids.len(), 2);
        for reviewer in ids {
            assert!(reviewers.insert(reviewer.as_str().unwrap().to_owned()));
            let verdict = if round == 3 && pass_last {
                "pass"
            } else {
                "changes_requested"
            };
            let findings = if verdict == "pass" {
                json!([])
            } else {
                json!([{"path":"src/a","line":1,"message":"Fix demonstrated fixture issue"}])
            };
            s.update(
                reviewer.as_str().unwrap(),
                "succeeded",
                &json!({"snapshot":r["snapshot"]["sha256"],"verdict":verdict,"findings":findings})
                    .to_string(),
                "",
            )
            .unwrap();
        }
        workflows::tick(&s, &f, &mut runner).unwrap();
    }
    let w = s.workflow(&id).unwrap();
    assert_eq!(
        w["state"],
        if pass_last {
            "ready"
        } else {
            "needs_attention"
        }
    );
    assert_eq!(w["data"]["rounds"].as_array().unwrap().len(), 4);
    assert_eq!(workers.len(), 4);
    assert_eq!(reviewers.len(), 8);
    let count = s.tasks(None).unwrap().len();
    workflows::tick(&s, &f, &mut runner).unwrap();
    assert_eq!(s.tasks(None).unwrap().len(), count);
}

#[test]
fn successful_check_cleans_up_its_background_descendants() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("source");
    repo(&p);
    let mut policy = check_policy(json!(["native-fixture"]));
    policy[0]["argv"] = json!([env!("CARGO_BIN_EXE_teem-subprocess-fixture"), "background"]);
    let log = tmp.path().join("process.log");
    let result = checks::execute(&policy[0], &p, &log).unwrap();
    assert_eq!(result["returncode"], 0);
    let pid = fs::read_to_string(log)
        .unwrap()
        .trim()
        .parse::<u32>()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match fs::read_to_string(format!("/proc/{pid}/stat")) {
            Err(_) => break,
            Ok(stat) if stat.split_whitespace().nth(2) == Some("Z") => break,
            _ => {
                assert!(
                    Instant::now() < deadline,
                    "Background check child is still running"
                );
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

#[test]
fn reopened_checking_and_reviewing_workflows_keep_original_evidence_and_ids() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), false);
    let id = text(&w, "id");
    let mut runner = runner();
    workflows::tick(&s, &f, &mut runner).unwrap();
    finish_workers(&s);
    workflows::tick(&s, &f, &mut runner).unwrap();
    let w = s.workflow(&id).unwrap();
    let handle = w["data"]["rounds"][0]["check_handle"].clone();
    let projects = s.projects.clone();
    let dir = s.directory.clone();
    drop(s);
    await_receipt(&handle);
    let s = Store::open(&dir, &[], projects.clone()).unwrap();
    workflows::tick(&s, &f, &mut runner).unwrap();
    let w = s.workflow(&id).unwrap();
    assert_eq!(w["state"], "reviewing");
    assert_eq!(w["data"]["rounds"][0]["check_handle"], handle);
    let ids = w["data"]["rounds"][0]["review_ids"].clone();
    let hash = w["data"]["rounds"][0]["snapshot"]["sha256"].clone();
    for id in array(&ids) {
        s.update(
            id.as_str().unwrap(),
            "succeeded",
            &json!({"snapshot":hash,"verdict":"pass","findings":[]}).to_string(),
            "",
        )
        .unwrap();
    }
    drop(s);
    let s = Store::open(&dir, &[], projects).unwrap();
    workflows::tick(&s, &f, &mut runner).unwrap();
    let w = s.workflow(&id).unwrap();
    assert_eq!(w["state"], "ready");
    assert_eq!(w["data"]["rounds"][0]["review_ids"], ids);
    assert_eq!(s.tasks(None).unwrap().len(), 3);
    assert!(f.calls.borrow().is_empty());
}

#[test]
fn dispatch_model_is_read_from_exact_parent_session() {
    let (t, mut r) = native_routes();
    let path = format!("/api/session/{}", text(&t, "foreman"));
    r.values.insert(
        path.clone(),
        json!({"id":t["foreman"],"model":{"providerID":"other","id":"selected","variant":"high"}}),
    );
    assert_eq!(
        r.dispatch_model(&t).unwrap(),
        json!({"providerID":"other","id":"selected","variant":"high"})
    );
    r.values.get_mut(&path).unwrap()["id"] = json!("ses_foreign");
    assert!(r.dispatch_model(&t).is_err());
}
#[test]
fn invalid_inherited_models_fail_closed() {
    for value in [
        json!({}),
        json!({"providerID":"","id":"model"}),
        json!({"providerID":"test","id":"x","variant":""}),
        json!({"providerID":"test","id":"x","extra":true}),
        json!({"providerID":"test","id":"x\n"}),
    ] {
        assert!(model(&value).is_err());
    }
}
#[test]
fn recorded_dispatch_model_survives_reopen_and_cannot_be_replaced() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("repo");
    let base = repo(&path);
    let allowed = vec![path.to_str().unwrap().into()];
    let s = store(tmp.path(), &allowed, json!({}));
    let t = s.submit("ses_test", req(&path, &base, "worker")).unwrap();
    s.claim().unwrap();
    let m = json!({"providerID":"test","id":"first"});
    s.record_model(&text(&t, "id"), &m).unwrap();
    let directory = s.directory.clone();
    drop(s);
    let s = Store::open(&directory, &allowed, json!({})).unwrap();
    assert_eq!(s.task(&text(&t, "id")).unwrap()["runtime_model"], m);
    assert!(
        s.record_model(
            &text(&t, "id"),
            &json!({"providerID":"test","id":"replacement"})
        )
        .is_err()
    );
}
#[test]
fn worker_and_reviewer_permissions_do_not_gain_unscoped_mutation() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/legacy-wire.json")).unwrap();
    let mut t = fixture["task"].clone();
    t["role"] = json!("reviewer");
    let permissions = array(&runtime::permissions(&t));
    assert!(!permissions.iter().any(|p| {
        ["edit", "shell", "external_directory", "execute", "subagent"]
            .contains(&text(p, "action").as_str())
            && p["effect"] == "allow"
    }));
    t["role"] = json!("worker");
    let permissions = array(&runtime::permissions(&t));
    assert!(
        permissions
            .iter()
            .any(|p| p["action"] == "shell" && p["effect"] == "ask")
    );
    assert!(
        !permissions
            .iter()
            .any(|p| p["action"] == "edit" && p["resource"] == "*" && p["effect"] == "allow")
    );
    assert!(
        permissions
            .iter()
            .rev()
            .take(4)
            .all(|p| p["action"] == "edit" && p["effect"] == "deny")
    );
}
#[test]
fn native_todo_capability_does_not_change_edit_shell_or_delegation_rules() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/legacy-wire.json")).unwrap();
    let mut t = fixture["task"].clone();
    t["request"].as_object_mut().unwrap().remove("native_todos");
    let before = runtime::permissions(&t);
    t["request"]["native_todos"] = json!(true);
    let mut after = array(&runtime::permissions(&t));
    after.retain(|r| r["action"] != "todowrite");
    assert_eq!(json!(after), before);
}
#[test]
fn native_input_absence_and_idle_without_terminal_outcome_are_uncertain() {
    let (t, mut r) = native_routes();
    let prefix = format!("/api/session/{}", text(&t, "session_id"));
    let prompt = format!("{prefix}/message/{}", text(&t, "prompt_id"));
    let original = r.values.remove(&prompt).unwrap();
    assert_eq!(r.inspect(&t).unwrap()["state"], "uncertain");
    r.values.insert(prompt, original);
    r.values.get_mut(&prefix).unwrap()["time"]["idle"] = json!(9);
    assert_eq!(r.inspect(&t).unwrap()["state"], "uncertain");
}
#[test]
fn active_session_still_requires_original_prompt() {
    let (t, mut r) = native_routes();
    let prefix = format!("/api/session/{}", text(&t, "session_id"));
    r.values.insert(
        "/api/session/active".into(),
        json!({text(&t,"session_id"): {}}),
    );
    r.values
        .remove(&format!("{prefix}/message/{}", text(&t, "prompt_id")));
    assert_eq!(r.inspect(&t).unwrap()["state"], "uncertain");
}
#[test]
fn native_receipt_uses_exact_message_or_inbox_identity_without_resending() {
    let f: Value = serde_json::from_str(include_str!("fixtures/legacy-wire.json")).unwrap();
    let event = &f["event"];
    let payload = runtime::event_payload(event).unwrap();
    let prefix = format!("/api/session/{}", text(event, "foreman"));
    let mut message = json!({"id":payload["id"],"type":"synthetic","text":payload["text"],"metadata":payload["metadata"]});
    let mut r = Routes {
        values: BTreeMap::new(),
    };
    r.values.insert(
        format!("{prefix}/message/{}", text(&payload, "id")),
        message.clone(),
    );
    assert!(r.accepted(event).unwrap());
    message["text"] = json!("changed payload");
    r.values.insert(
        format!("{prefix}/message/{}", text(&payload, "id")),
        message,
    );
    assert!(r.accepted(event).is_err());
    r.values.clear();
    r.values.insert(format!("{prefix}/inbox"),json!([{"id":payload["id"],"type":"synthetic","sessionID":event["foreman"],"payload":payload}]));
    assert!(r.accepted(event).unwrap());
    r.values.insert(format!("{prefix}/inbox"), json!([]));
    assert!(!r.accepted(event).unwrap());
}
#[test]
fn native_inbox_receipt_rejects_wrong_foreman() {
    let f: Value = serde_json::from_str(include_str!("fixtures/legacy-wire.json")).unwrap();
    let event = &f["event"];
    let payload = runtime::event_payload(event).unwrap();
    let mut r = Routes {
        values: BTreeMap::new(),
    };
    r.values.insert(
        format!("/api/session/{}/inbox", text(event, "foreman")),
        json!([{"id":payload["id"],"type":"synthetic","sessionID":"ses_wrong","payload":payload}]),
    );
    assert!(r.accepted(event).is_err());
}
#[test]
fn historical_completion_receipts_reconcile_without_replay() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/legacy-wire.json")).unwrap();
    for (event, payload) in [
        ("event", "event_payload"),
        ("workflow_event", "workflow_payload"),
    ] {
        let e = &fixture[event];
        let p = &fixture[payload];
        let prefix = format!("/api/session/{}", text(e, "foreman"));
        let mut r = Routes {
            values: BTreeMap::new(),
        };
        r.values.insert(
            format!("{prefix}/inbox"),
            json!([{"id":p["id"],"type":"synthetic","sessionID":e["foreman"],"payload":p}]),
        );
        assert!(r.accepted(e).unwrap());
        r.values.clear();
        r.values.insert(
            format!("{prefix}/message/{}", text(p, "id")),
            json!({"id":p["id"],"type":"synthetic","text":p["text"],"metadata":p["metadata"]}),
        );
        assert!(r.accepted(e).unwrap());
    }
}
#[test]
fn permission_observation_requires_exact_tool_input_and_worker_identity() {
    let (t, mut r) = native_routes();
    let prefix = format!("/api/session/{}", text(&t, "session_id"));
    r.values.insert(
        "/api/session/active".into(),
        json!({text(&t,"session_id"): {}}),
    );
    r.values.insert(format!("{prefix}/form"), json!([]));
    r.values.insert(format!("{prefix}/permission"),json!([{"id":"per_check","sessionID":t["session_id"],"action":"shell","resources":["git status"],"source":{"type":"tool","messageID":"msg_tool","id":"call_tool"}}]));
    r.values.insert(format!("{prefix}/message/msg_tool"),json!({"id":"msg_tool","type":"assistant","content":[{"id":"call_tool","type":"tool","state":{"input":{"command":"git status","workdir":t["checkout"]}}}]}));
    let p = r.approvals(&t).unwrap();
    assert_eq!(p[0]["input"]["command"], "git status");
    r.values.get_mut(&format!("{prefix}/permission")).unwrap()[0]["sessionID"] = json!("ses_wrong");
    assert!(r.approvals(&t).is_err());
}
#[test]
fn native_todo_reader_ignores_prose_failed_tools_and_honors_explicit_empty_list() {
    let (t, mut r) = native_routes();
    let path = format!(
        "/api/session/{}/message?limit=50&order=desc&type=assistant",
        text(&t, "session_id")
    );
    r.values.insert(path.clone(),json!([{"id":"msg_prose","type":"assistant","content":[{"type":"text","text":"Todo: finished"},{"type":"tool","name":"todowrite","state":{"status":"error","input":{"todos":[{"content":"not valid","status":"completed"}]}}}]}]));
    assert!(r.todos(&t).unwrap().is_none());
    r.values.insert(path,json!([{"id":"msg_todos","type":"assistant","content":[{"type":"tool","name":"todowrite","state":{"status":"completed","input":{"todos":[]}}}]}]));
    assert_eq!(r.todos(&t).unwrap().unwrap()["todos"], json!([]));
}

#[test]
fn check_early_closed_output_can_still_finish_successfully() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("source");
    repo(&p);
    let mut policy = check_policy(json!(["native-fixture"]));
    policy[0]["argv"] = json!([
        env!("CARGO_BIN_EXE_teem-subprocess-fixture"),
        "closed",
        "30"
    ]);
    let result = checks::execute(&policy[0], &p, &tmp.path().join("log")).unwrap();
    assert_eq!(result["returncode"], 0);
    assert_eq!(result["timed_out"], false);
}
#[test]
fn shell_metacharacters_remain_literal_child_arguments() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("source");
    repo(&p);
    let mut policy = check_policy(json!(["native-fixture"]));
    policy[0]["argv"] = json!([
        env!("CARGO_BIN_EXE_teem-subprocess-fixture"),
        "arguments",
        "$(touch owned); & > owned"
    ]);
    let log = tmp.path().join("log");
    let result = checks::execute(&policy[0], &p, &log).unwrap();
    assert_eq!(result["returncode"], 0);
    assert_eq!(
        read_json(&log).unwrap(),
        json!(["$(touch owned); & > owned"])
    );
    assert!(!p.join("owned").exists());
}
#[test]
fn approved_check_cwd_cannot_follow_symlinks_outside_candidate() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("source");
    repo(&p);
    let outside = tmp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, p.join("escape")).unwrap();
    let mut policy = check_policy(json!(["git", "status"]));
    policy[0]["cwd"] = json!("escape");
    assert!(
        runner()
            .start(&tmp.path().join("evidence"), &p, &policy, "snapshot")
            .is_err()
    );
}
#[test]
fn todo_configuration_never_overwrites_existing_project_files() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("source");
    let base = repo(&p);
    let checkout = checkouts::prepare(&p, &tmp.path().join("worker"), &base, None).unwrap();
    fs::write(checkout.join("opencode.json"), "{\"warming\":true}").unwrap();
    let before = fs::read(checkout.join("opencode.json")).unwrap();
    assert!(!checkouts::enable_todos(&checkout, &["src".into()]).unwrap());
    assert_eq!(fs::read(checkout.join("opencode.json")).unwrap(), before);
    assert!(!checkouts::enable_todos(&checkout, &["opencode.json".into()]).unwrap());
}
#[test]
fn complete_candidate_rejects_later_sparse_or_shallow_replacement() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("source");
    let base = repo(&p);
    let checkout = checkouts::prepare(&p, &tmp.path().join("worker"), &base, None).unwrap();
    let tree = checkouts::candidate_tree(&checkout, &base).unwrap();
    git(
        &checkout,
        &["config", "core.sparseCheckout", "true"],
        None,
        None,
    )
    .unwrap();
    assert!(checkouts::validate(&checkout, &base, &tree).is_err());
}
#[test]
fn managed_clone_does_not_copy_foreman_project_configuration() {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().join("source");
    let base = repo(&p);
    fs::write(p.join("AGENTS.md"), "Local instructions").unwrap();
    fs::write(p.join("opencode.json"), "private Foreman binding").unwrap();
    let checkout = checkouts::prepare(&p, &tmp.path().join("worker"), &base, None).unwrap();
    assert_eq!(
        fs::read_to_string(checkout.join("AGENTS.md")).unwrap(),
        "Local instructions"
    );
    assert!(!checkout.join("opencode.json").exists());
    assert!(
        git_text(&checkout, &["status", "--porcelain"])
            .unwrap()
            .is_empty()
    );
}

fn reviewing_fixture(tmp: &Path) -> (Store, Fake, String, checks::Runner) {
    let (s, f, w) = workflow_fixture(tmp, false);
    let id = text(&w, "id");
    let mut runner = runner();
    workflows::tick(&s, &f, &mut runner).unwrap();
    finish_workers(&s);
    workflows::tick(&s, &f, &mut runner).unwrap();
    let w = s.workflow(&id).unwrap();
    await_receipt(&w["data"]["rounds"][0]["check_handle"]);
    workflows::tick(&s, &f, &mut runner).unwrap();
    assert_eq!(s.workflow(&id).unwrap()["state"], "reviewing");
    (s, f, id, runner)
}
fn pass_reviews(s: &Store, id: &str) {
    let w = s.workflow(id).unwrap();
    let hash = &w["data"]["rounds"][0]["snapshot"]["sha256"];
    for task in s.tasks(None).unwrap() {
        if task["role"] == "reviewer" {
            s.update(
                &text(&task, "id"),
                "succeeded",
                &json!({"snapshot":hash,"verdict":"pass","findings":[]}).to_string(),
                "",
            )
            .unwrap();
        }
    }
}
#[test]
fn two_worker_assignments_must_be_disjoint_and_within_task_scope() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, _, w) = workflow_fixture(tmp.path(), false);
    for workers in [
        json!([{"scope":["src"],"prompt":"a"},{"scope":["src/a"],"prompt":"b"}]),
        json!([{"scope":["outside"],"prompt":"a"}]),
        json!([]),
    ] {
        let mut r = w["request"].clone();
        r["workflow_id"] = json!(uuid::Uuid::new_v4().to_string());
        r["workers"] = workers;
        assert!(s.workflow_submit("ses_test", r).is_err());
    }
}
#[test]
fn uncertain_worker_keeps_original_team_without_replacement() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), true);
    let id = text(&w, "id");
    let mut runner = runner();
    workflows::tick(&s, &f, &mut runner).unwrap();
    let tasks = s.tasks(None).unwrap();
    s.update(&text(&tasks[0], "id"), "uncertain", "", "")
        .unwrap();
    workflows::tick(&s, &f, &mut runner).unwrap();
    workflows::tick(&s, &f, &mut runner).unwrap();
    assert_eq!(s.workflow(&id).unwrap()["state"], "uncertain");
    assert_eq!(s.tasks(None).unwrap().len(), 2);
}
#[test]
fn completed_agent_events_are_passive_until_single_workflow_outcome() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), true);
    workflows::tick(&s, &f, &mut runner()).unwrap();
    finish_workers(&s);
    runtime::notify(&s, &f).unwrap();
    assert_eq!(*f.notifications.borrow(), 0);
    assert!(
        s.query("SELECT state FROM events", [])
            .unwrap()
            .iter()
            .all(|r| r["state"] == "passive")
    );
    assert_eq!(s.workflow(&text(&w, "id")).unwrap()["state"], "working");
}
#[test]
fn worker_cannot_modify_teammate_scope() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), true);
    let id = text(&w, "id");
    let mut runner = runner();
    workflows::tick(&s, &f, &mut runner).unwrap();
    finish_workers(&s);
    let tasks = s.tasks(None).unwrap();
    let first = &tasks[0];
    let own = scope(&first["request"]["scope"]).unwrap();
    let forbidden = if own[0] == "src/a" { "src/b" } else { "src/a" };
    fs::write(
        PathBuf::from(text(first, "checkout")).join(forbidden),
        "out of scope",
    )
    .unwrap();
    workflows::tick(&s, &f, &mut runner).unwrap();
    assert_eq!(s.workflow(&id).unwrap()["state"], "blocked");
}
#[test]
fn one_explicit_worker_cannot_take_the_entire_workflow_scope() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), false);
    let id = text(&w, "id");
    let mut request = w["request"].clone();
    request["workers"] = json!([{"scope":["src/a"],"prompt":"a only"}]);
    s.exec(
        "UPDATE workflows SET request=? WHERE id=?",
        rusqlite::params![canonical(&request), id],
    )
    .unwrap();
    let mut runner = runner();
    workflows::tick(&s, &f, &mut runner).unwrap();
    finish_workers(&s);
    let t = s.tasks(None).unwrap().remove(0);
    fs::write(
        PathBuf::from(text(&t, "checkout")).join("src/b"),
        "forbidden",
    )
    .unwrap();
    workflows::tick(&s, &f, &mut runner).unwrap();
    assert_eq!(s.workflow(&id).unwrap()["state"], "blocked");
}
#[test]
fn failed_approved_checks_start_bounded_revision_with_verified_diagnostics() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, f, w) = workflow_fixture(tmp.path(), false);
    let id = text(&w, "id");
    s.projects["project"]["checks"][0]["argv"] = json!(["git", "diff", "--exit-code", "HEAD"]);
    s.exec(
        "UPDATE workflows SET policy=? WHERE id=?",
        rusqlite::params![canonical(&s.projects["project"]), id],
    )
    .unwrap();
    let mut runner = runner();
    workflows::tick(&s, &f, &mut runner).unwrap();
    finish_workers(&s);
    workflows::tick(&s, &f, &mut runner).unwrap();
    let w = s.workflow(&id).unwrap();
    await_receipt(&w["data"]["rounds"][0]["check_handle"]);
    workflows::tick(&s, &f, &mut runner).unwrap();
    let w = s.workflow(&id).unwrap();
    assert_eq!(w["state"], "working");
    assert_eq!(w["round"], 1);
    let task = s
        .task(w["data"]["rounds"][1]["worker_id"].as_str().unwrap())
        .unwrap();
    assert!(text(&task["request"], "prompt").contains("untrusted data"));
    assert!(text(&task["request"], "prompt").contains("diff --git"));
    assert!(
        !s.tasks(None)
            .unwrap()
            .iter()
            .any(|t| t["role"] == "reviewer")
    );
}
#[test]
fn snapshot_or_check_candidate_drift_cannot_qualify_ready() {
    for drift in ["review", "check", "receipt"] {
        let tmp = tempfile::tempdir().unwrap();
        let (s, f, id, mut runner) = reviewing_fixture(tmp.path());
        pass_reviews(&s, &id);
        let w = s.workflow(&id).unwrap();
        let r = &w["data"]["rounds"][0];
        match drift {
            "review" => {
                let t = s.task(r["review_ids"][0].as_str().unwrap()).unwrap();
                fs::write(
                    PathBuf::from(text(&t, "checkout")).join("src/a"),
                    "tampered",
                )
                .unwrap();
            }
            "check" => {
                fs::write(
                    PathBuf::from(text(r, "check_checkout")).join("src/a"),
                    "tampered",
                )
                .unwrap();
            }
            _ => {
                fs::write(text(&r["check_handle"], "receipt"), "{}").unwrap();
            }
        }
        workflows::tick(&s, &f, &mut runner).unwrap();
        assert_eq!(s.workflow(&id).unwrap()["state"], "blocked", "{drift}");
    }
}
#[test]
fn wrong_snapshot_reviewer_execution_does_not_qualify_workflow() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, id, mut runner) = reviewing_fixture(tmp.path());
    for t in s.tasks(None).unwrap() {
        if t["role"] == "reviewer" {
            s.update(
                &text(&t, "id"),
                "succeeded",
                &json!({"snapshot":"wrong","verdict":"pass","findings":[]}).to_string(),
                "",
            )
            .unwrap();
        }
    }
    workflows::tick(&s, &f, &mut runner).unwrap();
    assert_eq!(s.workflow(&id).unwrap()["state"], "blocked");
}
#[test]
fn sidebar_metadata_is_additive_and_batch_checks_are_individually_pending() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, _, w) = workflow_fixture(tmp.path(), false);
    s.exec(
        "UPDATE workflows SET state='checking',data=? WHERE id=?",
        rusqlite::params![
            json!({"rounds":[{"check_handle":{"pending":true}}]}).to_string(),
            text(&w, "id")
        ],
    )
    .unwrap();
    let p = progress::snapshot(&s, "ses_test").unwrap();
    let items = array(&p["items"]);
    assert_eq!(items[0]["sidebar"]["project"], "project");
    assert_eq!(items[0]["sidebar"]["title"], progress::title(&w));
    assert_eq!(items[1]["sidebar"]["label"], "Implementation");
    let check = items
        .iter()
        .find(|r| r["sidebar"]["kind"] == "check")
        .unwrap();
    assert_eq!(check["status"], "pending");
    assert!(text(check, "content").contains("individual result pending"));
    assert!(text(check, "label").starts_with(&text(&items[0], "label")));
    assert_eq!(check["workflow_id"], w["id"]);
    for (item, todo) in items.iter().zip(array(&p["todos"])) {
        assert_eq!(item["content"], todo["content"]);
        assert_eq!(item["status"], todo["status"]);
    }
    assert_eq!(
        items.last().unwrap()["sidebar"]["label"],
        "Review · tests/scope"
    );
    assert_eq!(
        s.workflow(&text(&w, "id")).unwrap()["request"],
        w["request"]
    );
}

#[test]
fn sidebar_check_and_review_metadata_retains_authoritative_failure_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, _, w) = workflow_fixture(tmp.path(), false);
    let evidence = json!({"name":"tests","returncode":1,"timed_out":false,"output_limited":false});
    let data = json!({"rounds":[{"check_result":{"results":[evidence]},"review_results":[{"verdict":"changes_requested","findings":[{"message":"Keep evidence"}]}]}]});
    s.exec(
        "UPDATE workflows SET state='reviewing',data=? WHERE id=?",
        rusqlite::params![canonical(&data), text(&w, "id")],
    )
    .unwrap();
    let p = progress::snapshot(&s, "ses_test").unwrap();
    let items = array(&p["items"]);
    let check = items
        .iter()
        .find(|r| r["sidebar"]["kind"] == "check")
        .unwrap();
    assert_eq!(check["status"], "cancelled");
    assert!(text(check, "content").contains("failed; see recorded check evidence"));
    let review = items
        .iter()
        .find(|r| r["sidebar"]["label"] == "Review · correctness")
        .unwrap();
    assert_eq!(review["status"], "cancelled");
    assert!(text(review, "content").contains("changes requested"));
    assert_eq!(s.workflow(&text(&w, "id")).unwrap()["data"], data);
}

#[test]
fn review_execution_completion_is_not_a_passing_verdict_in_progress() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, _, id, _) = reviewing_fixture(tmp.path());
    pass_reviews(&s, &id);
    let p = progress::snapshot(&s, "ses_test").unwrap();
    let reviews = array(&p["items"])
        .into_iter()
        .filter(|r| text(r, "label").contains("review"))
        .collect::<Vec<_>>();
    assert_eq!(reviews.len(), 2);
    assert!(reviews.iter().all(|r|r["status"]=="in_progress"&&text(r,"content").contains("verdict pending")));
}
#[test]
fn latest_result_is_selected_by_finish_time_without_backfilled_notifications() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, _, w) = workflow_fixture(tmp.path(), false);
    s.exec(
        "UPDATE workflows SET state='ready',created=100,data=? WHERE id=?",
        rusqlite::params![
            json!({"rounds":[],"finished_at":400}).to_string(),
            text(&w, "id")
        ],
    )
    .unwrap();
    let mut other = w["request"].clone();
    other["workflow_id"] = json!(uuid::Uuid::new_v4().to_string());
    other["title"] = json!("Finished earlier");
    let second = s.workflow_submit("ses_test", other).unwrap();
    s.exec(
        "UPDATE workflows SET state='blocked',created=200,data=? WHERE id=?",
        rusqlite::params![
            json!({"rounds":[],"finished_at":300}).to_string(),
            text(&second, "id")
        ],
    )
    .unwrap();
    let p = progress::snapshot(&s, "ses_test").unwrap();
    assert_eq!(p["latest_result"]["workflow_id"], w["id"]);
    assert_eq!(p["todos"], json!([]));
    assert!(s.query("SELECT * FROM events", []).unwrap().is_empty());
}
#[test]
fn workflow_titles_are_readable_and_do_not_mutate_immutable_request() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, _, w) = workflow_fixture(tmp.path(), false);
    let mut request = w["request"].clone();
    request.as_object_mut().unwrap().remove("title");
    request["prompt"] = json!("Add group_labels_by_initial(items) and tests.");
    s.exec(
        "UPDATE workflows SET request=? WHERE id=?",
        rusqlite::params![canonical(&request), text(&w, "id")],
    )
    .unwrap();
    let p = progress::snapshot(&s, "ses_test").unwrap();
    assert!(text(&p["items"][0], "label").contains("Add group labels by initial"));
    assert!(!text(&p["items"][0], "label").contains(&text(&w, "id")));
    assert_eq!(s.workflow(&text(&w, "id")).unwrap()["request"], request);
}

#[test]
fn failed_inherited_model_read_retains_reservation_without_native_admission() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("source");
    let base = repo(&path);
    let s = store(tmp.path(), &[path.to_str().unwrap().into()], json!({}));
    let task = s.submit("ses_test", req(&path, &base, "worker")).unwrap();
    let f = Fake {
        fail_model: true,
        ..Default::default()
    };
    runtime::tick(&s, &f).unwrap();
    assert_eq!(s.task(&text(&task, "id")).unwrap()["state"], "uncertain");
    assert!(s.task(&text(&task, "id")).unwrap()["runtime_model"].is_null());
    assert!(f.calls.borrow().is_empty());
    runtime::tick(&s, &f).unwrap();
    assert_eq!(s.task(&text(&task, "id")).unwrap()["state"], "uncertain");
    assert!(f.calls.borrow().is_empty());
}

#[test]
fn both_worker_contributions_survive_combination_and_single_worker_revision() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), true);
    let id = text(&w, "id");
    let mut runner = runner();
    workflows::tick(&s, &f, &mut runner).unwrap();
    for task in s.tasks(None).unwrap() {
        let owned = scope(&task["request"]["scope"]).unwrap().remove(0);
        fs::write(
            PathBuf::from(text(&task, "checkout")).join(&owned),
            format!("contribution to {owned}\n"),
        )
        .unwrap();
        s.update(&text(&task, "id"), "succeeded", "implemented", "")
            .unwrap();
    }
    workflows::tick(&s, &f, &mut runner).unwrap();
    let w = s.workflow(&id).unwrap();
    let previous = &w["data"]["rounds"][0];
    let combined = PathBuf::from(text(previous, "checkout"));
    for name in ["src/a", "src/b"] {
        assert_eq!(
            fs::read_to_string(combined.join(name)).unwrap(),
            format!("contribution to {name}\n")
        );
    }
    await_receipt(&previous["check_handle"]);
    workflows::tick(&s, &f, &mut runner).unwrap();
    let w = s.workflow(&id).unwrap();
    let snapshot = w["data"]["rounds"][0]["snapshot"]["sha256"].clone();
    for task in s.tasks(None).unwrap() {
        if task["role"] == "reviewer" {
            s.update(&text(&task,"id"),"succeeded",&json!({"snapshot":snapshot,"verdict":"changes_requested","findings":[{"path":"src/a","line":1,"message":"Fixture revision requested"}]}).to_string(),"").unwrap();
        }
    }
    workflows::tick(&s, &f, &mut runner).unwrap();
    let w = s.workflow(&id).unwrap();
    assert_eq!(w["state"], "working");
    assert_eq!(w["round"], 1);
    let next = &w["data"]["rounds"][1];
    assert_eq!(next["worker_ids"].as_array().unwrap().len(), 1);
    let revision = PathBuf::from(text(next, "checkout"));
    for name in ["src/a", "src/b"] {
        assert_eq!(
            fs::read_to_string(revision.join(name)).unwrap(),
            format!("contribution to {name}\n")
        );
    }
}

#[test]
fn worker_ignore_rule_cannot_silently_discard_a_teammates_new_file() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), false);
    let id = text(&w, "id");
    let mut request = w["request"].clone();
    request["scope"] = json!(["src", ".gitignore"]);
    request["workers"] = json!([{"scope":["src/new"],"prompt":"New file"},{"scope":[".gitignore"],"prompt":"Ignore rules"}]);
    s.exec(
        "UPDATE workflows SET request=? WHERE id=?",
        rusqlite::params![canonical(&request), id],
    )
    .unwrap();
    let mut runner = runner();
    workflows::tick(&s, &f, &mut runner).unwrap();
    for task in s.tasks(None).unwrap() {
        let owned = scope(&task["request"]["scope"]).unwrap().remove(0);
        let contents = if owned == ".gitignore" {
            "src/new\n"
        } else {
            "teammate contribution\n"
        };
        fs::write(PathBuf::from(text(&task, "checkout")).join(owned), contents).unwrap();
        s.update(&text(&task, "id"), "succeeded", "implemented", "")
            .unwrap();
    }
    workflows::tick(&s, &f, &mut runner).unwrap();
    let w = s.workflow(&id).unwrap();
    assert_eq!(w["state"], "blocked");
    assert!(w["data"]["rounds"][0].get("snapshot").is_none());
    let combined = s
        .directory
        .join("managed")
        .join(id)
        .join("round-0/combined");
    assert_eq!(
        fs::read_to_string(combined.join("src/new")).unwrap(),
        "teammate contribution\n"
    );
    assert!(!combined.parent().unwrap().join("check-evidence").exists());
}
