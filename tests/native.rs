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
    // Operator-commit tests must never invoke a real user's configured signer.
    git(path, &["config", "commit.gpgsign", "false"], None, None).unwrap();
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
    let mut session = runtime::session_payload(t, &json!({}));
    assert!(text(&session, "title").starts_with("Teem "));
    session["title"] = f["session_payload"]["title"].clone();
    assert_eq!(session, f["session_payload"]);
    runtime::validate_session(t, &f["session_payload"], &json!({})).unwrap();
    assert_eq!(runtime::prompt_text(t), f["prompt_text"]);
    for (event, key) in [
        ("event", "event_payload"),
        ("workflow_event", "workflow_payload"),
    ] {
        let mut payload = runtime::event_payload(&f[event]).unwrap();
        if event == "event" {
            assert!(
                text(&payload, "text")
                    .starts_with(&text(&f[key], "text").replace("foreman_", "teem_"))
            );
        } else {
            assert!(
                !text(&payload, "text").contains("Do not refresh the checklist, dispatch agents")
            );
            assert!(text(&payload, "text").contains("eligible bounded same-task continuation"));
            assert!(text(&payload, "text").contains("briefly report engineering decisions"));
        }
        assert!(text(&payload, "text").contains("without ack"));
        payload["text"] = f[key]["text"].clone();
        assert_eq!(payload, f[key]);
    }
    assert_eq!(digest(&f["canonical_value"]), f["canonical_digest"]);
}

#[test]
fn completion_respects_existing_authority_without_granting_new_actions() {
    let f: Value = serde_json::from_str(include_str!("fixtures/legacy-wire.json")).unwrap();
    let payload = runtime::event_payload(&f["workflow_event"]).unwrap();
    let instruction = text(&payload, "text");
    assert!(instruction.contains("Existing explicit task approval remains usable"));
    assert!(instruction.contains("This notification grants no new authority"));
    assert!(instruction.contains("integration, local commit, publication or deployment needs its own existing explicit authority"));
    assert!(!instruction.contains("Do not invent tasks, silently renew budgets, replay uncertain work, approve permissions, integrate, merge or publish."));
    assert!(instruction.contains("proposed, implemented and verified fixes"));
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
    let mut req = json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"issue":"tidley/teem#8","project":"project","scope":["src"],"prompt":"Implement a scoped change","title":"A readable task"});
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
    let outcome = progress::snapshot(&s, "ses_test").unwrap()["latest_result"].clone();
    let details = text(&outcome, "details");
    assert!(details.contains("Applied locally"));
    assert!(details.contains("Checks"));
    assert!(details.contains("Reviews"));
    assert!(details.contains("Publication: not checked"));
    assert!(details.contains("Deployment: not checked"));
    let status = plugin::call(
        &s,
        "ses_test",
        "foreman_status",
        &json!({"section":"progress"}),
    )
    .unwrap();
    assert_eq!(
        status["progress"]["latest_result"]["details"],
        outcome["details"]
    );
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
fn issue8_unlinked_legacy_progress_remains_usable_without_backfill() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), false);
    let id = text(&w, "id");
    // Historical records are inserted/adjusted only in this temporary fixture.
    let mut request = w["request"].clone();
    request.as_object_mut().unwrap().remove("issue");
    s.exec(
        "UPDATE workflows SET request=? WHERE id=?",
        rusqlite::params![canonical(&request), id],
    )
    .unwrap();
    let mut runner = runner();
    workflows::tick(&s, &f, &mut runner).unwrap();
    assert_eq!(s.workflow(&id).unwrap()["state"], "working");
    assert!(s.tasks(None).unwrap()[0]["request"].get("issue").is_none());
    assert!(
        progress::snapshot(&s, "ses_test").unwrap()["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row.get("issue").is_none())
    );
    assert_eq!(s.workflow(&id).unwrap()["request"], request);
}

#[test]
fn issue8_legacy_continuation_needs_explicit_issue_and_cannot_switch_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, w) = ready_operator_fixture(tmp.path());
    let parent_id = text(&w, "id");
    let mut request = w["request"].clone();
    request.as_object_mut().unwrap().remove("issue");
    s.exec(
        "UPDATE workflows SET request=? WHERE id=?",
        rusqlite::params![canonical(&request), parent_id],
    )
    .unwrap();
    let before = s
        .query("SELECT * FROM workflows WHERE id=?", [&parent_id])
        .unwrap();
    let mut args =
        json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":parent_id});
    assert!(
        plugin::call(&s, "ses_test", "foreman_reconcile", &args)
            .unwrap_err()
            .to_string()
            .contains("owner/repo#positive-number")
    );
    assert!(s.workflow(args["workflow_id"].as_str().unwrap()).is_err());
    args["issue"] = json!("Tidley/Teem#8");
    let follow = plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap();
    assert_eq!(follow["request"]["issue"], "tidley/teem#8");
    assert_eq!(follow["request"]["scope"], request["scope"]);
    assert_eq!(follow["request"]["prompt"], request["prompt"]);
    assert_eq!(
        plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap(),
        follow
    );
    assert_eq!(
        s.query("SELECT * FROM workflows WHERE id=?", [&parent_id])
            .unwrap(),
        before
    );
    args["issue"] = json!("tidley/teem#9");
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err());
    args["source_workflow_id"] = follow["id"].clone();
    // A terminal linked source must reject an explicit issue switch as well.
    s.exec(
        "UPDATE workflows SET state='ready',base_revision=?,data=? WHERE id=?",
        rusqlite::params![
            text(&w, "base_revision"),
            canonical(&w["data"]),
            text(&follow, "id")
        ],
    )
    .unwrap();
    args["workflow_id"] = json!(uuid::Uuid::new_v4().to_string());
    assert!(
        plugin::call(&s, "ses_test", "foreman_reconcile", &args)
            .unwrap_err()
            .to_string()
            .contains("cannot switch")
    );
    args["issue"] = json!("tidley/teem#8");
    args["scope"] = json!(["outside"]);
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err());
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
    assert_eq!(follow["request"]["issue"], w["request"]["issue"]);
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

fn operator_config_case(case: &str) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir(&home).unwrap();
    let config = if case == "missing" {
        "[user]\nuseConfigOnly = true\n"
    } else if case == "signing" {
        "[user]\nname = Operator Fixture\nemail = operator@example.invalid\n[commit]\ngpgSign\n[gpg]\nprogram = /usr/bin/false\n"
    } else {
        "[user]\nname = Operator Fixture\nemail = operator@example.invalid\n[commit]\ngpgSign = false\n"
    };
    fs::write(home.join(".gitconfig"), config).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "operator_commit_config_child", "--nocapture"])
        .env_clear()
        .current_dir(tmp.path())
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .env("XDG_DATA_HOME", tmp.path().join("data"))
        .env("XDG_STATE_HOME", tmp.path().join("state"))
        .env("XDG_CACHE_HOME", tmp.path().join("cache"))
        .env("HERDR_PLUGIN_CONFIG_DIR", tmp.path().join("herdr"))
        .env("TMPDIR", std::env::temp_dir())
        .env("GIT_AUTHOR_NAME", "Untrusted ambient author")
        .env("GIT_AUTHOR_EMAIL", "ambient@example.invalid")
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "user.name")
        .env("GIT_CONFIG_VALUE_0", "Untrusted ambient config")
        .env("TEEM_OPERATOR_CONFIG_CASE", case)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "case {case}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn operator_commit_uses_configured_global_identity_and_keeps_local_precedence() {
    for case in ["global", "local"] {
        operator_config_case(case);
    }
}

#[test]
fn missing_operator_identity_refuses_before_apply_or_staging() {
    operator_config_case("missing");
}

#[test]
fn operator_commit_does_not_bypass_configured_signing() {
    operator_config_case("signing");
}

#[test]
fn operator_commit_rejects_identity_added_by_candidate_config_include() {
    operator_config_case("snapshot");
}

#[test]
fn operator_commit_config_child() {
    let Ok(case) = std::env::var("TEEM_OPERATOR_CONFIG_CASE") else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    if case == "snapshot" {
        let root = tmp.path().join("source");
        repo(&root);
        let included = root.join("src/operator.conf");
        fs::write(&included, "# original operator include\n").unwrap();
        git(
            &root,
            &["config", "include.path", "../src/operator.conf"],
            None,
            None,
        )
        .unwrap();
        git(&root, &["add", "."], None, None).unwrap();
        git(&root, &["commit", "-qm", "include fixture"], None, None).unwrap();
        let head = git_text(&root, &["rev-parse", "HEAD"]).unwrap();
        let config = git_commit_identity(&root).unwrap();
        fs::write(&included, "[author]\nname = Candidate\nemail = candidate@example.invalid\n[committer]\nname = Candidate\nemail = candidate@example.invalid\n").unwrap();
        git(&root, &["add", "."], None, None).unwrap();
        assert!(git_commit(&root, "must not adopt candidate identity", &config).is_err());
        assert_eq!(git_text(&root, &["rev-parse", "HEAD"]).unwrap(), head);
        return;
    }
    let (s, w) = ready_operator_fixture(tmp.path());
    let root = tmp.path().join("source");
    if !["local", "signing"].contains(&case.as_str()) {
        for key in ["user.name", "user.email"] {
            git(&root, &["config", "--unset-all", key], None, None).unwrap();
        }
    }
    if case == "signing" {
        git(
            &root,
            &["config", "--unset-all", "commit.gpgsign"],
            None,
            None,
        )
        .unwrap();
    }
    let base = git_text(&root, &["rev-parse", "HEAD"]).unwrap();
    let index = fs::read(root.join(".git/index")).unwrap();
    let result = plugin::call(
        &s,
        "ses_test",
        "foreman_integrate",
        &json!({"workflow_id":w["id"],"commit_local":true}),
    );
    if case == "missing" {
        assert!(result.is_err());
        assert!(format!("{:#}", result.unwrap_err()).contains("identity"));
        assert!(
            teem::operator::receipt(&s, &text(&w, "id"))
                .unwrap()
                .is_null()
        );
        assert_eq!(fs::read_to_string(root.join("src/a")).unwrap(), "base\n");
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
        assert_eq!(git_text(&root, &["rev-parse", "HEAD"]).unwrap(), base);
    } else if case == "signing" {
        assert!(
            result.is_err(),
            "configured signing must not be silently disabled"
        );
        assert_eq!(
            teem::operator::receipt(&s, &text(&w, "id")).unwrap()["state"],
            "committing"
        );
        assert_eq!(git_text(&root, &["rev-parse", "HEAD"]).unwrap(), base);
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
        assert_eq!(git_text(&root, &["rev-parse", "HEAD"]).unwrap(), base);
        assert_eq!(
            teem::operator::receipt(&s, &text(&w, "id")).unwrap()["state"],
            "committing"
        );
    } else {
        assert_eq!(result.unwrap()["state"], "committed");
        let expected = if case == "local" {
            "Test <test@example.invalid>"
        } else {
            "Operator Fixture <operator@example.invalid>"
        };
        assert_eq!(
            git_text(&root, &["log", "-1", "--format=%an <%ae>"]).unwrap(),
            expected
        );
        assert_eq!(
            git_text(&root, &["log", "-1", "--format=%cn <%ce>"]).unwrap(),
            expected
        );
    }
}

fn blocked_review_fixture(tmp: &Path) -> (Store, Fake, Value) {
    let (s, f, w) = workflow_fixture(tmp, false);
    let id = text(&w, "id");
    let mut runner = runner();
    workflows::tick(&s, &f, &mut runner).unwrap();
    finish_workers(&s);
    workflows::tick(&s, &f, &mut runner).unwrap();
    let w = s.workflow(&id).unwrap();
    await_receipt(&w["data"]["rounds"][0]["check_handle"]);
    workflows::tick(&s, &f, &mut runner).unwrap();
    let w = s.workflow(&id).unwrap();
    for review in array(&w["data"]["rounds"][0]["review_ids"]) {
        let output = json!({"snapshot":w["data"]["rounds"][0]["snapshot"]["sha256"],"verdict":"changes_requested","findings":[{"path":"src/a","line":1,"message":"x".repeat(501)}]});
        s.update(
            review.as_str().unwrap(),
            "succeeded",
            &canonical(&output),
            "",
        )
        .unwrap();
    }
    workflows::tick(&s, &f, &mut runner).unwrap();
    let w = s.workflow(&id).unwrap();
    assert_eq!(w["state"], "blocked");
    assert!(w["data"]["rounds"][0].get("review_results").is_none());
    (s, f, w)
}

#[test]
fn blocked_review_reports_the_exact_format_error_without_accepting_it() {
    let snapshot = json!({"sha256":"a".repeat(64)});
    let output = json!({"snapshot":snapshot["sha256"],"verdict":"changes_requested","findings":[{"path":"src/a","line":1,"message":"x".repeat(501)}]});
    let error = workflows::verdict(&canonical(&output), &snapshot)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("Finding 1 message has 501 characters; maximum is 500"),
        "{error}"
    );
}

#[test]
fn blocked_review_handoff_preserves_evidence_and_runs_fresh_pipeline() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = blocked_review_fixture(tmp.path());
    let parent = s.workflow(&text(&w, "id")).unwrap();
    let old_tasks = s.tasks(None).unwrap();
    let mut args =
        json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]});
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err());
    let mut setup = args.clone();
    setup["setup_snapshot"] = w["data"]["rounds"][0]["snapshot"]["sha256"].clone();
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &setup).is_err());
    args["blocked_snapshot"] = w["data"]["rounds"][0]["snapshot"]["sha256"].clone();
    let child = plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap();
    assert_eq!(child["state"], "queued");
    assert_eq!(
        plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap(),
        child
    );
    let mut runner = runner();
    workflows::tick(&s, &f, &mut runner).unwrap();
    let child = s.workflow(&text(&child, "id")).unwrap();
    assert_eq!(child["state"], "working", "{child}");
    let checkout = PathBuf::from(text(&child["data"]["rounds"][0], "checkout"));
    let notes = read_json(&checkout.join(".git/teem-reconciliation.json")).unwrap();
    assert_eq!(
        notes["unqualified_review_outputs"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(notes["review_results"].is_null());
    assert!(
        notes["unqualified_review_outputs"][0]["result"]
            .as_str()
            .unwrap()
            .contains(&"x".repeat(501))
    );
    finish_workers(&s);
    workflows::tick(&s, &f, &mut runner).unwrap();
    let child = s.workflow(&text(&child, "id")).unwrap();
    await_receipt(&child["data"]["rounds"][0]["check_handle"]);
    workflows::tick(&s, &f, &mut runner).unwrap();
    let child = s.workflow(&text(&child, "id")).unwrap();
    assert_eq!(child["state"], "reviewing");
    assert_ne!(
        child["data"]["rounds"][0]["check_handle"],
        w["data"]["rounds"][0]["check_handle"]
    );
    assert_ne!(
        child["data"]["rounds"][0]["review_ids"],
        w["data"]["rounds"][0]["review_ids"]
    );
    for review in array(&child["data"]["rounds"][0]["review_ids"]) {
        s.update(review.as_str().unwrap(), "succeeded", &canonical(&json!({"snapshot":child["data"]["rounds"][0]["snapshot"]["sha256"],"verdict":"pass","findings":[]})), "").unwrap();
    }
    workflows::tick(&s, &f, &mut runner).unwrap();
    assert_eq!(s.workflow(&text(&child, "id")).unwrap()["state"], "ready");
    assert_eq!(s.workflow(&text(&w, "id")).unwrap(), parent);
    for task in old_tasks {
        assert_eq!(s.task(&text(&task, "id")).unwrap(), task);
    }
}

#[test]
fn blocked_review_handoff_removes_qualification_only_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, mut w) = blocked_review_fixture(tmp.path());
    w["request"]["qualification_recovery"] =
        json!({"source_workflow_id":uuid::Uuid::new_v4().to_string()});
    s.exec(
        "UPDATE workflows SET request=? WHERE id=?",
        rusqlite::params![canonical(&w["request"]), text(&w, "id")],
    )
    .unwrap();
    let args = json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"],"blocked_snapshot":w["data"]["rounds"][0]["snapshot"]["sha256"]});
    let child = plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap();
    assert!(child["request"].get("qualification_recovery").is_none());
    workflows::tick(&s, &f, &mut runner()).unwrap();
    assert_eq!(s.workflow(&text(&child, "id")).unwrap()["state"], "working");
}

#[test]
fn blocked_review_handoff_rejects_unsafe_sources() {
    for case in [
        "digest",
        "active",
        "uncertain",
        "paused",
        "receipt",
        "missing-receipt",
        "missing-handle",
        "missing-review",
        "foreign-review",
        "wrong-tree",
        "scope",
        "integrated",
        "source-drift",
        "lineage-task",
        "lineage-workflow",
        "pending-stop",
        "non-string",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let (s, _, mut w) = blocked_review_fixture(tmp.path());
        let mut args = json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"],"blocked_snapshot":w["data"]["rounds"][0]["snapshot"]["sha256"]});
        let review = w["data"]["rounds"][0]["review_ids"][0]
            .as_str()
            .unwrap()
            .to_owned();
        match case {
            "digest" => args["blocked_snapshot"] = json!("0".repeat(64)),
            "active" | "uncertain" => {
                s.exec(
                    "UPDATE tasks SET state=? WHERE id=?",
                    rusqlite::params![
                        if case == "active" {
                            "running"
                        } else {
                            "uncertain"
                        },
                        review
                    ],
                )
                .unwrap();
            }
            "paused" => {
                s.exec("INSERT INTO workflow_stops VALUES (?,0)", [text(&w, "id")])
                    .unwrap();
            }
            "receipt" => {
                fs::write(
                    text(&w["data"]["rounds"][0]["check_handle"], "receipt"),
                    "{}",
                )
                .unwrap();
            }
            "missing-receipt" => {
                fs::remove_file(text(&w["data"]["rounds"][0]["check_handle"], "receipt")).unwrap();
            }
            "missing-handle" => {
                w["data"]["rounds"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("check_handle");
            }
            "missing-review" => {
                w["data"]["rounds"][0]["review_ids"] = json!([]);
            }
            "foreign-review" => {
                s.exec(
                    "UPDATE tasks SET foreman='ses_foreign' WHERE id=?",
                    [&review],
                )
                .unwrap();
            }
            "wrong-tree" => {
                s.exec(
                    "UPDATE tasks SET request=json_set(request,'$.input_tree','wrong') WHERE id=?",
                    [&review],
                )
                .unwrap();
            }
            "scope" => {
                w["request"]["scope"] = json!(["outside"]);
                s.exec(
                    "UPDATE workflows SET request=? WHERE id=?",
                    rusqlite::params![canonical(&w["request"]), text(&w, "id")],
                )
                .unwrap();
            }
            "integrated" => {
                s.exec(
                    "INSERT INTO integrations(workflow_id,receipt) VALUES (?,'{}')",
                    [text(&w, "id")],
                )
                .unwrap();
            }
            "source-drift" => {
                fs::write(tmp.path().join("source/src/a"), "dirty\n").unwrap();
            }
            "lineage-task" | "lineage-workflow" => {
                let mut req = w["request"].clone();
                let sibling = uuid::Uuid::new_v4().to_string();
                req["workflow_id"] = json!(sibling);
                req["reconcile_root"] = w["id"].clone();
                s.workflow_submit("ses_test", req).unwrap();
                if case == "lineage-task" {
                    s.exec(
                        "UPDATE workflows SET state='blocked' WHERE id=?",
                        [&sibling],
                    )
                    .unwrap();
                    s.exec("UPDATE tasks SET request=json_set(request,'$.workflow_id',?),state='uncertain' WHERE id=?",rusqlite::params![sibling,review]).unwrap();
                }
            }
            "pending-stop" => {
                s.exec(
                    "INSERT INTO task_stops VALUES (?,'uncertain','')",
                    [&review],
                )
                .unwrap();
            }
            "non-string" => args["blocked_snapshot"] = json!(true),
            _ => unreachable!(),
        }
        s.exec(
            "UPDATE workflows SET data=? WHERE id=?",
            rusqlite::params![canonical(&w["data"]), text(&w, "id")],
        )
        .unwrap();
        assert!(
            plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err(),
            "admitted {case}"
        );
        assert_eq!(
            s.workflows("ses_test").unwrap().len(),
            if case.starts_with("lineage-") { 2 } else { 1 },
            "{case}"
        );
    }
}

#[test]
fn blocked_review_handoff_revalidates_before_seeding() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = blocked_review_fixture(tmp.path());
    let args = json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"],"blocked_snapshot":w["data"]["rounds"][0]["snapshot"]["sha256"]});
    let child = plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap();
    let count = s.tasks(None).unwrap().len();
    fs::write(
        text(&w["data"]["rounds"][0]["check_handle"], "receipt"),
        "{}",
    )
    .unwrap();
    workflows::tick(&s, &f, &mut runner()).unwrap();
    assert_eq!(s.workflow(&text(&child, "id")).unwrap()["state"], "blocked");
    assert_eq!(s.tasks(None).unwrap().len(), count);
    assert_eq!(s.workflow(&text(&w, "id")).unwrap(), w);
}

// Retain an actual checked candidate with one snapshot-bound pass and one
// uncertain native reviewer, then stop the latter through the managed path.
fn stopped_candidate_fixture(tmp: &Path) -> (Store, Fake, Value) {
    let (s, f, w) = workflow_fixture(tmp, false);
    let id = text(&w, "id");
    let mut runner = runner();
    workflows::tick(&s, &f, &mut runner).unwrap();
    finish_workers(&s);
    workflows::tick(&s, &f, &mut runner).unwrap();
    let w = s.workflow(&id).unwrap();
    await_receipt(&w["data"]["rounds"][0]["check_handle"]);
    workflows::tick(&s, &f, &mut runner).unwrap();
    let w = s.workflow(&id).unwrap();
    let reviews = array(&w["data"]["rounds"][0]["review_ids"]);
    s.update(reviews[1].as_str().unwrap(), "succeeded", &json!({"snapshot":w["data"]["rounds"][0]["snapshot"]["sha256"],"verdict":"pass","findings":[]}).to_string(), "").unwrap();
    runtime::tick(&s, &f).unwrap();
    s.update(
        reviews[0].as_str().unwrap(),
        "uncertain",
        "retained partial result",
        "No matching native input",
    )
    .unwrap();
    *f.stop_live.borrow_mut() = true;
    plugin::call(
        &s,
        "ses_test",
        "foreman_stop",
        &json!({"workflow_id":id,"task_id":reviews[0]}),
    )
    .unwrap();
    let args = json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":id});
    assert_eq!(s.workflow(&id).unwrap()["stop"]["state"], "requested");
    let reviewer = s.task(reviews[0].as_str().unwrap()).unwrap();
    assert!(!f.is_stopped(&reviewer).unwrap());
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err());
    teem::control::tick(&s, &f).unwrap();
    assert_eq!(
        s.workflow(&id).unwrap()["stop"]["targets"][0]["state"],
        "waiting"
    );
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err());
    teem::control::tick(&s, &f).unwrap();
    let w = s.workflow(&id).unwrap();
    assert_eq!(w["state"], "blocked");
    assert_eq!(w["stop"]["state"], "stopped");
    let reviewer = s.task(reviews[0].as_str().unwrap()).unwrap();
    assert!(f.is_stopped(&reviewer).unwrap());
    assert_eq!(f.calls.borrow().iter().filter(|c| c.0 == "STOP").count(), 1);
    assert_eq!(
        s.task(reviews[0].as_str().unwrap()).unwrap()["state"],
        "interrupted"
    );
    (s, f, w)
}

fn stopped_unseeded_fixture(tmp: &Path) -> (Store, Fake, Value, Value) {
    let (s, origin) = ready_operator_fixture(tmp);
    stop_unseeded_input(tmp, s, origin)
}

fn stop_unseeded_input(tmp: &Path, s: Store, origin: Value) -> (Store, Fake, Value, Value) {
    let source = tmp.join("source");
    fs::write(source.join("src/a"), "newer conflicting source\n").unwrap();
    git(&source, &["add", "."], None, None).unwrap();
    git(&source, &["commit", "-qm", "advance"], None, None).unwrap();
    let f = Fake {
        directory: source.to_str().unwrap().into(),
        ..Default::default()
    };
    let mut args =
        json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":origin["id"]});
    if origin["state"] == "blocked" {
        args["setup_snapshot"] = origin["data"]["rounds"][0]["snapshot"]["sha256"].clone();
    }
    let child = plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap();
    workflows::tick(&s, &f, &mut runner()).unwrap();
    let child = s.workflow(&text(&child, "id")).unwrap();
    let round = &child["data"]["rounds"][0];
    let checkout = PathBuf::from(text(round, "checkout"));
    assert_eq!(
        read_json(&checkout.join(".git/teem-reconciliation.json")).unwrap()["seeded"],
        false
    );
    runtime::tick(&s, &f).unwrap();
    *f.stop_live.borrow_mut() = true;
    plugin::call(
        &s,
        "ses_test",
        "foreman_stop",
        &json!({"workflow_id":child["id"],"task_id":round["worker_id"]}),
    )
    .unwrap();
    teem::control::tick(&s, &f).unwrap();
    teem::control::tick(&s, &f).unwrap();
    let stopped = s.workflow(&text(&child, "id")).unwrap();
    assert_eq!(stopped["stop"]["state"], "stopped");
    assert!(stopped["data"]["rounds"][0].get("snapshot").is_none());
    (s, f, origin, stopped)
}

#[test]
fn stopped_unseeded_extension_preserves_legacy_reconciliation_retries() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, origin) = ready_operator_fixture(tmp.path());
    let args =
        json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":origin["id"]});
    let child = plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap();
    let mut legacy = child["request"].clone();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("reconcile_candidate_source");
    s.exec(
        "UPDATE workflows SET request=? WHERE id=?",
        rusqlite::params![canonical(&legacy), text(&child, "id")],
    )
    .unwrap();
    assert_eq!(
        plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap(),
        s.workflow(&text(&child, "id")).unwrap()
    );
}

#[test]
fn stopped_unseeded_handoff_retains_real_ancestor_patch_and_requalifies() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, origin, stopped) = stopped_unseeded_fixture(tmp.path());
    let tasks = s.tasks(None).unwrap();
    let args =
        json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":stopped["id"]});
    let child = plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap();
    assert_eq!(child["request"]["reconcile_from"], stopped["id"]);
    assert_eq!(child["request"]["reconcile_candidate_source"], origin["id"]);
    assert_eq!(
        child["request"]["reconcile_snapshot"],
        origin["data"]["rounds"][0]["snapshot"]["sha256"]
    );
    assert_eq!(
        plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap(),
        child
    );
    workflows::tick(&s, &f, &mut runner()).unwrap();
    let working = s.workflow(&text(&child, "id")).unwrap();
    assert_eq!(working["state"], "working", "{working}");
    assert_eq!(
        plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap(),
        working
    );
    let checkout = PathBuf::from(text(&working["data"]["rounds"][0], "checkout"));
    let notes = read_json(&checkout.join(".git/teem-reconciliation.json")).unwrap();
    assert_eq!(notes["source_workflow_id"], stopped["id"]);
    assert_eq!(notes["candidate_source_workflow_id"], origin["id"]);
    assert_eq!(notes["seeded"], false);
    assert_eq!(
        fs::read(checkout.join(".git/teem-reconciliation.patch")).unwrap(),
        checkouts::verify(&origin["data"]["rounds"][0]["snapshot"]).unwrap()
    );
    fs::write(checkout.join("src/a"), "reconciled newer source\n").unwrap();
    s.update(
        working["data"]["rounds"][0]["worker_id"].as_str().unwrap(),
        "succeeded",
        "scoped reconciliation",
        "",
    )
    .unwrap();
    workflows::tick(&s, &f, &mut runner()).unwrap();
    let checking = s.workflow(&text(&child, "id")).unwrap();
    assert_eq!(checking["state"], "checking");
    await_receipt(&checking["data"]["rounds"][0]["check_handle"]);
    workflows::tick(&s, &f, &mut runner()).unwrap();
    let reviewing = s.workflow(&text(&child, "id")).unwrap();
    assert_eq!(reviewing["state"], "reviewing");
    let ids = array(&reviewing["data"]["rounds"][0]["review_ids"]);
    assert_eq!(ids.len(), 2);
    for id in ids {
        s.update(id.as_str().unwrap(), "succeeded", &canonical(&json!({"snapshot":reviewing["data"]["rounds"][0]["snapshot"]["sha256"],"verdict":"pass","findings":[]})), "").unwrap();
    }
    workflows::tick(&s, &f, &mut runner()).unwrap();
    assert_eq!(s.workflow(&text(&child, "id")).unwrap()["state"], "ready");
    assert_eq!(
        plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap()["state"],
        "ready"
    );
    assert_eq!(s.workflow(&text(&origin, "id")).unwrap(), origin);
    assert_eq!(s.workflow(&text(&stopped, "id")).unwrap(), stopped);
    for task in tasks {
        assert_eq!(s.task(&text(&task, "id")).unwrap(), task);
    }
}

#[test]
fn stopped_unseeded_handoff_rejects_changed_or_uncertain_inputs() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, _, origin, stopped) = stopped_unseeded_fixture(tmp.path());
    let round = &stopped["data"]["rounds"][0];
    let checkout = PathBuf::from(text(round, "checkout"));
    let worker = s.task(round["worker_id"].as_str().unwrap()).unwrap();
    let patch = checkout.join(".git/teem-reconciliation.patch");
    let original_patch = fs::read(&patch).unwrap();
    for case in [
        "edit",
        "staged-only",
        "untracked",
        "patch",
        "notes",
        "unknown-stop",
        "active-worker",
        "wrong-input",
        "wrong-scope",
        "wrong-owner",
        "source-drift",
    ] {
        let notes = checkout.join(".git/teem-reconciliation.json");
        let old_notes = fs::read(&notes).unwrap();
        match case {
            "edit" => fs::write(checkout.join("src/a"), "new worker edits\n").unwrap(),
            "staged-only" => {
                fs::write(checkout.join("src/a"), "retained staged edits\n").unwrap();
                git(&checkout, &["add", "src/a"], None, None).unwrap();
                fs::write(checkout.join("src/a"), "newer conflicting source\n").unwrap();
            }
            "untracked" => fs::write(checkout.join("src/untracked"), "keep me\n").unwrap(),
            "patch" => fs::write(&patch, "changed").unwrap(),
            "notes" => {
                let mut n: Value = serde_json::from_slice(&old_notes).unwrap();
                n["seeded"] = json!(true);
                fs::write(&notes, canonical(&n)).unwrap();
            }
            "unknown-stop" => {
                s.exec(
                    "UPDATE task_stops SET state='uncertain' WHERE task_id=?",
                    [text(&worker, "id")],
                )
                .unwrap();
            }
            "active-worker" => {
                s.exec(
                    "UPDATE tasks SET state='running' WHERE id=?",
                    [text(&worker, "id")],
                )
                .unwrap();
            }
            "wrong-input" | "wrong-scope" | "wrong-owner" => {
                let mut q = worker["request"].clone();
                if case == "wrong-input" {
                    q["input_tree"] = json!("0".repeat(40));
                }
                if case == "wrong-scope" {
                    q["scope"] = json!(["outside"]);
                }
                s.exec(
                    "UPDATE tasks SET request=?,foreman=? WHERE id=?",
                    rusqlite::params![
                        canonical(&q),
                        if case == "wrong-owner" {
                            "ses_foreign"
                        } else {
                            "ses_test"
                        },
                        text(&worker, "id")
                    ],
                )
                .unwrap();
            }
            "source-drift" => fs::write(tmp.path().join("source/src/a"), "dirty source\n").unwrap(),
            _ => unreachable!(),
        }
        let args = json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":stopped["id"]});
        assert!(
            plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err(),
            "admitted {case}"
        );
        if case == "staged-only" {
            assert_eq!(
                git_text(&checkout, &["show", ":src/a"]).unwrap(),
                "retained staged edits"
            );
        }
        // Restore only files/rows created in this private fixture.
        fs::write(checkout.join("src/a"), "newer conflicting source\n").unwrap();
        if case == "staged-only" {
            git(&checkout, &["add", "src/a"], None, None).unwrap();
        }
        if case == "untracked" {
            fs::remove_file(checkout.join("src/untracked")).unwrap();
        }
        fs::write(&patch, &original_patch).unwrap();
        fs::write(&notes, &old_notes).unwrap();
        fs::write(
            tmp.path().join("source/src/a"),
            "newer conflicting source\n",
        )
        .unwrap();
        s.exec(
            "UPDATE tasks SET request=?,foreman=?,state=? WHERE id=?",
            rusqlite::params![
                canonical(&worker["request"]),
                text(&worker, "foreman"),
                text(&worker, "state"),
                text(&worker, "id")
            ],
        )
        .unwrap();
        s.exec(
            "UPDATE task_stops SET state='stopped' WHERE task_id=?",
            [text(&worker, "id")],
        )
        .unwrap();
    }
    assert_eq!(s.workflow(&text(&origin, "id")).unwrap(), origin);
    let valid =
        json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":stopped["id"]});
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &valid).is_ok());
}

#[test]
fn stopped_unseeded_handoff_rechecks_stop_and_input_before_seeding() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, _, stopped) = stopped_unseeded_fixture(tmp.path());
    let args =
        json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":stopped["id"]});
    let child = plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap();
    let count = s.tasks(None).unwrap().len();
    let checkout = PathBuf::from(text(&stopped["data"]["rounds"][0], "checkout"));
    fs::write(checkout.join("src/a"), "late edit, must not lose\n").unwrap();
    workflows::tick(&s, &f, &mut runner()).unwrap();
    assert_eq!(s.workflow(&text(&child, "id")).unwrap()["state"], "blocked");
    assert_eq!(s.tasks(None).unwrap().len(), count);
}

#[test]
fn stopped_unseeded_handoff_rechecks_staged_only_edits_before_seeding() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, _, stopped) = stopped_unseeded_fixture(tmp.path());
    let args =
        json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":stopped["id"]});
    let child = plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap();
    let count = s.tasks(None).unwrap().len();
    let checkout = PathBuf::from(text(&stopped["data"]["rounds"][0], "checkout"));
    fs::write(checkout.join("src/a"), "late staged edit\n").unwrap();
    git(&checkout, &["add", "src/a"], None, None).unwrap();
    fs::write(checkout.join("src/a"), "newer conflicting source\n").unwrap();
    let index = fs::read(checkout.join(".git/index")).unwrap();
    workflows::tick(&s, &f, &mut runner()).unwrap();
    assert_eq!(s.workflow(&text(&child, "id")).unwrap()["state"], "blocked");
    assert_eq!(s.tasks(None).unwrap().len(), count);
    assert_eq!(fs::read(checkout.join(".git/index")).unwrap(), index);
}

#[test]
fn stopped_unseeded_handoff_retains_setup_blocked_source_proof() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), false);
    workflows::tick(&s, &f, &mut runner()).unwrap();
    finish_workers(&s);
    let mut missing = checks::Runner::with_executable(tmp.path().join("missing-helper"));
    workflows::tick(&s, &f, &mut missing).unwrap();
    let origin = s.workflow(&text(&w, "id")).unwrap();
    assert_eq!(origin["state"], "blocked");
    let (s, f, origin, stopped) = stop_unseeded_input(tmp.path(), s, origin);
    let args =
        json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":stopped["id"]});
    let child = plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap();
    workflows::tick(&s, &f, &mut runner()).unwrap();
    assert_eq!(s.workflow(&text(&child, "id")).unwrap()["state"], "working");
    assert_eq!(s.workflow(&text(&origin, "id")).unwrap(), origin);
    assert_eq!(s.workflow(&text(&stopped, "id")).unwrap(), stopped);
}

#[test]
fn stopped_unseeded_handoff_rejects_live_siblings_and_unresolved_checks() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, _, origin, stopped) = stopped_unseeded_fixture(tmp.path());
    let mut req = origin["request"].clone();
    req["workflow_id"] = json!(uuid::Uuid::new_v4().to_string());
    req["reconcile_root"] = origin["id"].clone();
    let sibling = s.workflow_submit("ses_test", req).unwrap();
    let args =
        json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":stopped["id"]});
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err());
    s.exec(
        "UPDATE workflows SET state='blocked' WHERE id=?",
        [text(&sibling, "id")],
    )
    .unwrap();
    let reviewer = s
        .task(
            origin["data"]["rounds"][0]["review_ids"][0]
                .as_str()
                .unwrap(),
        )
        .unwrap();
    s.exec(
        "UPDATE tasks SET state='uncertain' WHERE id=?",
        [text(&reviewer, "id")],
    )
    .unwrap();
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err());
    s.exec(
        "UPDATE tasks SET state='succeeded' WHERE id=?",
        [text(&reviewer, "id")],
    )
    .unwrap();
    let receipt = PathBuf::from(text(
        &origin["data"]["rounds"][0]["check_handle"],
        "receipt",
    ));
    let bytes = fs::read(&receipt).unwrap();
    fs::remove_file(&receipt).unwrap();
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err());
    fs::write(&receipt, bytes).unwrap();
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_ok());
}

#[test]
fn confirmed_stop_handoff_seeds_while_preparing_and_requires_fresh_qualification() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, f, w) = stopped_candidate_fixture(tmp.path());
    let parent = s.workflow(&text(&w, "id")).unwrap();
    let tasks = s.tasks(None).unwrap();
    let stops = s.query("SELECT * FROM task_stops", []).unwrap();
    let events = s.query("SELECT * FROM events", []).unwrap();
    let source = tmp.path().join("source");
    fs::write(source.join("unrelated"), "advanced source\n").unwrap();
    git(&source, &["add", "unrelated"], None, None).unwrap();
    git(&source, &["commit", "-qm", "advance"], None, None).unwrap();
    let head = git_text(&source, &["rev-parse", "HEAD"]).unwrap();
    s.projects["project"]["max_revisions"] = json!(3);
    s.projects["project"]["checks"][0]["argv"] = json!(["git", "diff", "--check"]);
    let id = uuid::Uuid::new_v4().to_string();
    let args = json!({"workflow_id":id,"source_workflow_id":w["id"]});
    let follow = plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap();
    assert_eq!(follow["state"], "queued");
    assert_eq!(follow["request"]["scope"], w["request"]["scope"]);
    assert_eq!(follow["request"]["prompt"], w["request"]["prompt"]);
    assert_eq!(follow["request"]["reconcile_root"], w["id"]);
    assert_eq!(follow["policy"], s.projects["project"]);
    assert!(array(&follow["data"]["rounds"]).is_empty());
    assert_eq!(
        plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap()["id"],
        id
    );
    let duplicate =
        json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]});
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &duplicate).is_err());
    let mut runner = runner();
    workflows::tick(&s, &f, &mut runner).unwrap();
    let follow = s.workflow(&id).unwrap();
    assert_eq!(follow["state"], "working", "{follow}");
    assert_eq!(follow["base_revision"], head);
    let round = &follow["data"]["rounds"][0];
    assert_eq!(array(&round["worker_ids"]).len(), 1);
    assert!(round["check_result"].is_null());
    assert!(array(&round["review_ids"]).is_empty());
    let checkout = PathBuf::from(text(round, "checkout"));
    assert_eq!(
        fs::read_to_string(checkout.join("src/a")).unwrap(),
        "changed\n"
    );
    let notes = read_json(&checkout.join(".git/teem-reconciliation.json")).unwrap();
    assert_eq!(notes["source_workflow_id"], w["id"]);
    assert_eq!(notes["seeded"], true);
    assert_eq!(
        notes["snapshot"],
        w["data"]["rounds"][0]["snapshot"]["sha256"]
    );
    assert_eq!(
        fs::read(checkout.join(".git/teem-reconciliation.patch")).unwrap(),
        fs::read(text(&w["data"]["rounds"][0]["snapshot"], "patch")).unwrap()
    );
    finish_workers(&s);
    workflows::tick(&s, &f, &mut runner).unwrap();
    let follow = s.workflow(&id).unwrap();
    assert_eq!(follow["state"], "checking");
    assert_ne!(
        follow["data"]["rounds"][0]["check_handle"],
        w["data"]["rounds"][0]["check_handle"]
    );
    await_receipt(&follow["data"]["rounds"][0]["check_handle"]);
    workflows::tick(&s, &f, &mut runner).unwrap();
    let follow = s.workflow(&id).unwrap();
    assert_eq!(follow["state"], "reviewing");
    let reviews = array(&follow["data"]["rounds"][0]["review_ids"]);
    assert_eq!(reviews.len(), 2);
    assert_ne!(reviews[0], reviews[1]);
    for (index, review) in reviews.iter().enumerate() {
        assert!(!array(&w["data"]["rounds"][0]["review_ids"]).contains(review));
        s.update(review.as_str().unwrap(), "succeeded", &json!({"snapshot":follow["data"]["rounds"][0]["snapshot"]["sha256"],"verdict":"pass","findings":[]}).to_string(), "").unwrap();
        if index == 0 {
            workflows::tick(&s, &f, &mut runner).unwrap();
            assert_eq!(s.workflow(&id).unwrap()["state"], "reviewing");
        }
    }
    workflows::tick(&s, &f, &mut runner).unwrap();
    assert_eq!(s.workflow(&id).unwrap()["state"], "ready");
    assert_eq!(s.workflow(&text(&w, "id")).unwrap(), parent);
    assert_eq!(s.query("SELECT * FROM task_stops", []).unwrap(), stops);
    for task in tasks {
        assert_eq!(s.task(&text(&task, "id")).unwrap(), task);
    }
    for event in events {
        assert_eq!(
            s.query("SELECT * FROM events WHERE id=?", [text(&event, "id")])
                .unwrap(),
            vec![event]
        );
    }
    assert_eq!(git_text(&source, &["rev-parse", "HEAD"]).unwrap(), head);
    assert_eq!(fs::read_to_string(source.join("src/a")).unwrap(), "base\n");
}

#[test]
fn stopped_handoff_rejects_unconfirmed_foreign_and_invalid_sources() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, _, w) = stopped_candidate_fixture(tmp.path());
    let args = json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]});
    let stopped_task = text(&w["stop"]["targets"][0], "task_id");
    let sibling = s
        .task(w["data"]["rounds"][0]["review_ids"][1].as_str().unwrap())
        .unwrap();
    // Model observed reservations directly: update rejects queued and preserves
    // terminal outcomes, including this already-succeeded sibling.
    for state in ["queued", "starting", "accepted", "running", "uncertain"] {
        s.exec(
            "UPDATE tasks SET state=? WHERE id=?",
            [state, &text(&sibling, "id")],
        )
        .unwrap();
        assert_eq!(s.task(&text(&sibling, "id")).unwrap()["state"], state);
        assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err());
    }
    s.exec(
        "UPDATE tasks SET state=? WHERE id=?",
        [text(&sibling, "state"), text(&sibling, "id")],
    )
    .unwrap();
    assert_eq!(s.task(&text(&sibling, "id")).unwrap(), sibling);
    for state in ["requested", "attempting", "waiting", "uncertain"] {
        s.exec(
            "UPDATE task_stops SET state=? WHERE task_id=?",
            [state, &stopped_task],
        )
        .unwrap();
        assert!(
            plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err(),
            "{state}"
        );
    }
    s.exec(
        "UPDATE task_stops SET state='stopped' WHERE task_id=?",
        [&stopped_task],
    )
    .unwrap();
    s.bind("ses_other", "other-terminal").unwrap();
    assert!(plugin::call(&s, "ses_other", "foreman_reconcile", &args).is_err());
    for state in ["failed", "working"] {
        s.exec(
            "UPDATE workflows SET state=? WHERE id=?",
            [state, &text(&w, "id")],
        )
        .unwrap();
        assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err());
    }
    s.exec(
        "UPDATE workflows SET state='blocked' WHERE id=?",
        [text(&w, "id")],
    )
    .unwrap();
    s.exec(
        "DELETE FROM workflow_stops WHERE workflow_id=?",
        [text(&w, "id")],
    )
    .unwrap();
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err());
    s.exec(
        "INSERT INTO workflow_stops VALUES (?,?)",
        rusqlite::params![text(&w, "id"), now()],
    )
    .unwrap();
    let checkout = PathBuf::from(text(&w["data"]["rounds"][0], "checkout"));
    fs::write(checkout.join("src/a"), "tampered checkout\n").unwrap();
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err());
    fs::write(checkout.join("src/a"), "changed\n").unwrap();
    let mut data = w["data"].clone();
    data["rounds"][0]["snapshot"] = Value::Null;
    s.exec(
        "UPDATE workflows SET data=? WHERE id=?",
        [canonical(&data), text(&w, "id")],
    )
    .unwrap();
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err());
    let mut data = w["data"].clone();
    data["rounds"][0]["snapshot"]["base"] = json!("0".repeat(40));
    s.exec(
        "UPDATE workflows SET data=? WHERE id=?",
        [canonical(&data), text(&w, "id")],
    )
    .unwrap();
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err());
    s.exec(
        "UPDATE workflows SET data=? WHERE id=?",
        [canonical(&w["data"]), text(&w, "id")],
    )
    .unwrap();
    let patch = text(&w["data"]["rounds"][0]["snapshot"], "patch");
    fs::remove_file(&patch).unwrap();
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err());
    fs::write(&patch, "tampered patch").unwrap();
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err());
    assert_eq!(s.workflows("ses_test").unwrap().len(), 1);
}

#[test]
fn stopped_handoff_requires_a_retained_in_scope_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), false);
    plugin::call(
        &s,
        "ses_test",
        "foreman_stop",
        &json!({"workflow_id":w["id"]}),
    )
    .unwrap();
    teem::control::tick(&s, &f).unwrap();
    let args = json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]});
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err());

    let tmp = tempfile::tempdir().unwrap();
    let (s, _, w) = stopped_candidate_fixture(tmp.path());
    let checkout = PathBuf::from(text(&w["data"]["rounds"][0], "checkout"));
    fs::write(checkout.join("outside"), "not this task\n").unwrap();
    let snapshot = checkouts::capture(
        &checkout,
        &text(&w, "base_revision"),
        &["src".into(), "outside".into()],
        &tmp.path().join("outside-snapshot"),
    )
    .unwrap();
    let mut data = w["data"].clone();
    data["rounds"][0]["snapshot"] = snapshot;
    s.exec(
        "UPDATE workflows SET data=? WHERE id=?",
        [canonical(&data), text(&w, "id")],
    )
    .unwrap();
    let args = json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]});
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err());
}

#[test]
fn stopped_handoff_checks_full_lineage_and_rechecks_it_before_seeding() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = stopped_candidate_fixture(tmp.path());
    let child = plugin::call(
        &s,
        "ses_test",
        "foreman_reconcile",
        &json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]}),
    )
    .unwrap();
    workflows::tick(&s, &f, &mut runner()).unwrap();
    let child = s.workflow(&text(&child, "id")).unwrap();
    let task = text(&child["data"]["rounds"][0], "worker_id");
    s.exec(
        "UPDATE workflows SET state='blocked' WHERE id=?",
        [text(&child, "id")],
    )
    .unwrap();
    let args = json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]});
    // Include queued reservations without using the runtime transition API.
    for state in ["queued", "starting", "accepted", "running", "uncertain"] {
        s.exec("UPDATE tasks SET state=? WHERE id=?", [state, &task])
            .unwrap();
        assert_eq!(s.task(&task).unwrap()["state"], state);
        assert!(
            plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err(),
            "{state}"
        );
    }
    s.update(&task, "interrupted", "", "").unwrap();
    s.exec(
        "INSERT INTO task_stops(task_id,state) VALUES (?,'uncertain')",
        [&task],
    )
    .unwrap();
    for state in ["requested", "attempting", "waiting", "uncertain"] {
        s.exec(
            "UPDATE task_stops SET state=? WHERE task_id=?",
            [state, &task],
        )
        .unwrap();
        assert!(
            plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err(),
            "{state}"
        );
    }
    s.exec(
        "UPDATE task_stops SET state='stopped' WHERE task_id=?",
        [&task],
    )
    .unwrap();
    // A retained stopped sibling/descendant fixture. Its source snapshot is the
    // original candidate; the root task must still reserve the whole lineage.
    s.exec(
        "UPDATE workflows SET data=?,base_revision=? WHERE id=?",
        rusqlite::params![
            canonical(&w["data"]),
            text(&w, "base_revision"),
            text(&child, "id")
        ],
    )
    .unwrap();
    s.exec(
        "INSERT INTO workflow_stops VALUES (?,?)",
        rusqlite::params![text(&child, "id"), now()],
    )
    .unwrap();
    let root_task = text(&w["stop"]["targets"][0], "task_id");
    // The managed stop already made these tasks terminal. Inject newly observed
    // uncertainty without asking update to overwrite their terminal outcomes.
    s.exec(
        "UPDATE tasks SET state='uncertain' WHERE id=?",
        [&root_task],
    )
    .unwrap();
    assert_eq!(s.task(&root_task).unwrap()["state"], "uncertain");
    let from_child =
        json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":child["id"]});
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &from_child).is_err());
    s.exec(
        "UPDATE tasks SET state='interrupted' WHERE id=?",
        [&root_task],
    )
    .unwrap();
    let grandchild = plugin::call(&s, "ses_test", "foreman_reconcile", &from_child).unwrap();
    assert_eq!(grandchild["request"]["reconcile_root"], w["id"]);
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err());
    // A newly observed reservation after admission must also prevent seeding.
    s.exec("UPDATE tasks SET state='uncertain' WHERE id=?", [&task])
        .unwrap();
    assert_eq!(s.task(&task).unwrap()["state"], "uncertain");
    workflows::tick(&s, &f, &mut runner()).unwrap();
    let grandchild = s.workflow(&text(&grandchild, "id")).unwrap();
    assert_eq!(grandchild["state"], "blocked");
    assert!(
        !s.tasks(None)
            .unwrap()
            .iter()
            .any(|t| t["request"]["workflow_id"] == grandchild["id"])
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

fn failed_next_preparation_fixture(tmp: &Path) -> (Store, Value) {
    let (s, mut w) = ready_operator_fixture(tmp);
    w["round"] = json!(1);
    s.save_workflow(
        &mut w,
        "blocked",
        "Managed Git timed out; preserve checkout",
    )
    .unwrap();
    let partial = s
        .directory
        .join("managed")
        .join(text(&w, "id"))
        .join("round-1/worker/.git/objects/pack");
    fs::create_dir_all(&partial).unwrap();
    fs::write(
        partial.join("tmp_pack_retained"),
        "incomplete pack evidence",
    )
    .unwrap();
    (s, w)
}

fn inject_next_round_reservation(s: &Store, w: &Value, state: &str) {
    let id = uuid::Uuid::new_v5(
        &uuid::Uuid::parse_str(&text(w, "id")).unwrap(),
        b"round:1:worker",
    )
    .to_string();
    s.exec("INSERT INTO tasks(id,foreman,role,checkout,state,request,session_id,prompt_id,sparse,result,detail,created,runtime_model,trusted_worker_shell) SELECT ?,foreman,role,checkout,?,request,?,?,sparse,result,detail,created,runtime_model,trusted_worker_shell FROM tasks WHERE id=?", rusqlite::params![id,state,format!("ses_reserved_{id}"),format!("msg_reserved_{id}"),text(&w["data"]["rounds"][0],"worker_id")]).unwrap();
}

#[test]
fn failed_next_preparation_continues_without_reusing_partial_checkout_or_qualification() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, w) = failed_next_preparation_fixture(tmp.path());
    let before = s.workflow(&text(&w, "id")).unwrap();
    let args = json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]});
    let next = plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap();
    assert_eq!(
        plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap(),
        next
    );
    assert!(
        plugin::call(
            &s,
            "ses_test",
            "foreman_reconcile",
            &json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]})
        )
        .is_err()
    );
    let f = Fake {
        directory: tmp.path().join("source").to_str().unwrap().into(),
        ..Default::default()
    };
    workflows::tick(&s, &f, &mut runner()).unwrap();
    let next = s.workflow(&text(&next, "id")).unwrap();
    assert_eq!(next["state"], "working");
    assert_eq!(next["request"]["reconcile_from"], w["id"]);
    assert!(next["data"]["rounds"][0].get("check_result").is_none());
    assert!(next["data"]["rounds"][0].get("review_ids").is_none());
    assert_eq!(s.workflow(&text(&w, "id")).unwrap(), before);
    assert_eq!(
        fs::read_to_string(
            s.directory
                .join("managed")
                .join(text(&w, "id"))
                .join("round-1/worker/.git/objects/pack/tmp_pack_retained")
        )
        .unwrap(),
        "incomplete pack evidence"
    );
}

#[test]
fn failed_next_preparation_rejects_any_reserved_worker_and_other_blocked_states() {
    for state in [
        "queued",
        "starting",
        "accepted",
        "running",
        "uncertain",
        "succeeded",
        "failed",
        "interrupted",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let (s, w) = failed_next_preparation_fixture(tmp.path());
        inject_next_round_reservation(&s, &w, state);
        assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]})).is_err(), "{state}");
    }
    for (round, detail) in [
        (0, "Managed Git timed out; preserve checkout"),
        (1, "Native admission uncertain"),
        (2, "Managed Git timed out; preserve checkout"),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let (s, mut w) = failed_next_preparation_fixture(tmp.path());
        w["round"] = json!(round);
        s.save_workflow(&mut w, "blocked", detail).unwrap();
        assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]})).is_err());
    }
}

#[test]
fn failed_next_preparation_rechecks_nonlaunch_before_seeding() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, w) = failed_next_preparation_fixture(tmp.path());
    let next = plugin::call(
        &s,
        "ses_test",
        "foreman_reconcile",
        &json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]}),
    )
    .unwrap();
    inject_next_round_reservation(&s, &w, "succeeded");
    let f = Fake {
        directory: tmp.path().join("source").to_str().unwrap().into(),
        ..Default::default()
    };
    workflows::tick(&s, &f, &mut runner()).unwrap();
    assert_eq!(s.workflow(&text(&next, "id")).unwrap()["state"], "blocked");
    assert!(
        !s.tasks(None)
            .unwrap()
            .iter()
            .any(|t| t["request"]["workflow_id"] == next["id"])
    );
}

#[test]
fn failed_next_preparation_checks_sibling_uncertainty_and_stops_at_admission_and_seed() {
    for after_admission in [false, true] {
        for unresolved_stop in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let (s, w) = failed_next_preparation_fixture(tmp.path());
            let sibling = plugin::call(&s, "ses_test", "foreman_reconcile", &json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]})).unwrap();
            let f = Fake {
                directory: tmp.path().join("source").to_str().unwrap().into(),
                ..Default::default()
            };
            workflows::tick(&s, &f, &mut runner()).unwrap();
            let mut sibling = s.workflow(&text(&sibling, "id")).unwrap();
            let task = text(&sibling["data"]["rounds"][0], "worker_id");
            s.update(&task, "succeeded", "fixture finished", "")
                .unwrap();
            s.save_workflow(&mut sibling, "blocked", "preserved sibling fixture")
                .unwrap();
            let args = json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]});
            if after_admission {
                plugin::call(&s, "ses_test", "foreman_reconcile", &args).unwrap();
            }
            if unresolved_stop {
                s.exec(
                    "INSERT INTO task_stops(task_id,state) VALUES (?,'uncertain')",
                    [&task],
                )
                .unwrap();
            } else {
                s.exec("UPDATE tasks SET state='uncertain' WHERE id=?", [&task])
                    .unwrap();
            }
            if after_admission {
                workflows::tick(&s, &f, &mut runner()).unwrap();
                assert_eq!(
                    s.workflow(&text(&args, "workflow_id")).unwrap()["state"],
                    "blocked",
                    "unresolved stop: {unresolved_stop}"
                );
                assert!(
                    !s.tasks(None)
                        .unwrap()
                        .iter()
                        .any(|t| t["request"]["workflow_id"] == args["workflow_id"])
                );
            } else {
                assert!(
                    plugin::call(&s, "ses_test", "foreman_reconcile", &args).is_err(),
                    "unresolved stop: {unresolved_stop}"
                );
            }
        }
    }
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
    for task in s.tasks(None).unwrap() {
        assert_eq!(task["request"]["issue"], w["request"]["issue"]);
    }
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
fn trusted_worker_shell_survives_enablement_rebuild_without_changing_reviewers() {
    for (trusted, role, expected) in [
        (true, "worker", 1),
        (false, "worker", 0),
        (true, "reviewer", 0),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("repo");
        let base = repo(&path);
        let mut config = json!({"state_dir":tmp.path().join("state"),"allowed_checkouts":[path],"projects":{},"trusted_worker_shell":trusted});
        let s = plugin::open(&config).unwrap();
        s.bind("ses_test", "terminal").unwrap();
        let task = s.submit("ses_test", req(&path, &base, role)).unwrap();
        assert_eq!(task["trusted_worker_shell"], 0);
        let fake = Fake::default();
        runtime::tick(&s, &fake).unwrap();
        let recorded = s.task(&text(&task, "id")).unwrap();
        assert_eq!(recorded["trusted_worker_shell"], expected);
        let rules = array(&runtime::permissions(&recorded));
        let shell = rules.iter().find(|r| r["action"] == "shell");
        if role == "reviewer" {
            assert!(shell.is_none());
        } else {
            assert_eq!(
                shell.unwrap()["effect"],
                if trusted { "allow" } else { "ask" }
            );
        }
        for operation in ["CREATE", "PROMPT"] {
            assert_eq!(
                fake.calls
                    .borrow()
                    .iter()
                    .find(|c| c.0 == operation)
                    .unwrap()
                    .2["trusted_worker_shell"],
                expected
            );
        }
        drop(s);
        config["trusted_worker_shell"] = json!(!trusted);
        let s = plugin::open(&config).unwrap();
        assert_eq!(
            s.task(&text(&task, "id")).unwrap()["trusted_worker_shell"],
            expected
        );
        let mut altered = runtime::session_payload(&recorded, &Value::Null);
        altered["permissions"] = json!([]);
        assert!(runtime::validate_session(&recorded, &altered, &Value::Null).is_err());
    }
}

#[test]
fn trusted_worker_shell_configuration_is_explicit_boolean_only() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    for (setting, valid) in [
        ("", true),
        ("false", true),
        ("true", true),
        ("\"true\"", false),
        ("1", false),
    ] {
        let extra = if setting.is_empty() {
            String::new()
        } else {
            format!("trusted_worker_shell = {setting}\n")
        };
        fs::write(
            &path,
            format!("state_dir = {:?}\n{extra}", tmp.path().join("state")),
        )
        .unwrap();
        let loaded = plugin::load(&path);
        assert_eq!(loaded.is_ok(), valid);
        if valid {
            assert_eq!(loaded.unwrap()["trusted_worker_shell"], setting == "true");
        }
    }
}
#[test]
fn trusted_worker_shell_changes_only_worker_shell_and_requires_a_pinned_flag() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/legacy-wire.json")).unwrap();
    let mut task = fixture["task"].clone();
    let mut expected = runtime::permissions(&task);
    for rule in expected.as_array_mut().unwrap() {
        if rule["action"] == "shell" {
            rule["effect"] = json!("allow");
        }
    }
    task["trusted_worker_shell"] = json!(1);
    assert_eq!(runtime::permissions(&task), expected);
    let old = &fixture["session_payload"];
    assert!(runtime::validate_session(&fixture["task"], old, &Value::Null).is_ok());
    assert!(runtime::validate_session(&task, old, &Value::Null).is_err());
    let session = runtime::session_payload(&task, &Value::Null);
    assert!(runtime::validate_session(&task, &session, &Value::Null).is_ok());
    assert!(runtime::validate_session(&fixture["task"], &session, &Value::Null).is_err());
    task["role"] = json!("reviewer");
    let trusted_reviewer = runtime::permissions(&task);
    task["trusted_worker_shell"] = json!(0);
    assert_eq!(runtime::permissions(&task), trusted_reviewer);
    assert!(
        !array(&trusted_reviewer)
            .iter()
            .any(|r| r["action"] == "shell")
    );
}

#[test]
fn trusted_worker_shell_is_pinned_once_at_dispatch_and_survives_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("repo");
    let base = repo(&path);
    let mut config = json!({"state_dir":tmp.path().join("state"),"allowed_checkouts":[path],"projects":{},"trusted_worker_shell":true});
    let s = plugin::open(&config).unwrap();
    s.bind("ses_test", "terminal").unwrap();
    let task = s.submit("ses_test", req(&path, &base, "worker")).unwrap();
    assert_eq!(task["trusted_worker_shell"], 0);
    s.claim().unwrap();
    let model = json!({"providerID":"test","id":"model"});
    let task = s.record_model(&text(&task, "id"), &model).unwrap();
    assert_eq!(task["trusted_worker_shell"], 1);
    drop(s);
    config["trusted_worker_shell"] = json!(false);
    let s = plugin::open(&config).unwrap();
    assert_eq!(
        s.record_model(&text(&task, "id"), &model).unwrap()["trusted_worker_shell"],
        1
    );
    // A legacy recorded dispatch is not upgraded by a later configuration change.
    s.exec(
        "UPDATE tasks SET trusted_worker_shell=0 WHERE id=?",
        [text(&task, "id")],
    )
    .unwrap();
    drop(s);
    config["trusted_worker_shell"] = json!(true);
    let s = plugin::open(&config).unwrap();
    assert_eq!(
        s.record_model(&text(&task, "id"), &model).unwrap()["trusted_worker_shell"],
        0
    );
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
    assert!(!text(&payload, "text").contains("foreman_"));
    let mut old_payload = payload.clone();
    // Historical receipts predate the autonomous-delivery suffix; do not
    // fabricate a legacy payload by renaming tools inside the new policy.
    old_payload["text"] = json!(
        text(&payload, "text")
            .strip_suffix(&format!(
                "\n\n{}",
                include_str!("../docs/AUTONOMOUS_DELIVERY.md")
            ))
            .unwrap()
            .replace("teem_", "foreman_")
    );
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
    let mut old_message = message.clone();
    old_message["text"] = old_payload["text"].clone();
    r.values.insert(
        format!("{prefix}/message/{}", text(&payload, "id")),
        old_message,
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
fn extended_and_autonomy_completion_receipts_are_accepted_exactly_without_replay() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/legacy-wire.json")).unwrap();
    let e = &fixture["workflow_event"];
    let mut p = fixture["workflow_payload"].clone();
    p["text"] = json!(format!(
        "{} Status retrieval: use this workflow_id or task_id; default status is a bounded summary. Read actual check results, both exact-candidate verdicts and unresolved findings/limitations. Follow marked truncations/missing rows with section pagination or mode=detail and JSON pointer, then next_offset chunks. Never claim success or acknowledge unseen evidence from a returned file path. If output is inaccessible, retry a narrower section/smaller limit; otherwise report the blocker without ack. Do not broaden directory permissions. Execution success is not review approval; ready is candidate qualification. Later integration receipts take precedence over historical candidate details; integration does not verify publication or runtime. Only acknowledge after consuming the relevant readable outcome.",
        text(&p, "text")
    ));
    let mut autonomy = p.clone();
    autonomy["text"] = json!(text(&p,"text").replace(
        "Do not refresh the checklist, dispatch agents, replay work, approve permissions, integrate, merge or publish. Progress is displayed independently; this is the final result notification, not a progress update.",
        "After reading and acknowledging the evidence, continue only an eligible bounded same-task continuation already covered by user authority and remaining budget; otherwise briefly report engineering decisions, verification, limitations and the specific blocker. Do not invent tasks, silently renew budgets, replay uncertain work, approve permissions, integrate, merge or publish. Progress is displayed independently; do not poll or maintain checklists merely for this notification."
    ));
    for p in [p, autonomy] {
        let prefix = format!("/api/session/{}", text(e, "foreman"));
        let mut r = Routes {
            values: BTreeMap::new(),
        };
        r.values.insert(
            format!("{prefix}/inbox"),
            json!([{"id":p["id"],"type":"synthetic","sessionID":e["foreman"],"payload":p}]),
        );
        assert!(r.accepted(e).unwrap());
        let inbox = r.values.get_mut(&format!("{prefix}/inbox")).unwrap();
        inbox[0]["payload"]["text"] = json!(format!("{} ", text(&p, "text")));
        assert!(r.accepted(e).is_err());
        r.values.clear();
        let message = format!("{prefix}/message/{}", text(&p, "id"));
        r.values.insert(
            message.clone(),
            json!({"id":p["id"],"type":"synthetic","text":p["text"],"metadata":p["metadata"]}),
        );
        assert!(r.accepted(e).unwrap());
        r.values.get_mut(&message).unwrap()["metadata"]["teem_workflow"] = json!("wrong");
        assert!(r.accepted(e).is_err());
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

fn block_one_review(s: &Store, id: &str) {
    let w = s.workflow(id).unwrap();
    for (index, task) in array(&w["data"]["rounds"][0]["review_ids"])
        .iter()
        .enumerate()
    {
        let result = if index == 0 {
            json!({"snapshot":w["data"]["rounds"][0]["snapshot"]["sha256"],"verdict":"blocked","findings":[],"reason":"Mandatory policy copy unavailable"})
        } else {
            json!({"snapshot":w["data"]["rounds"][0]["snapshot"]["sha256"],"verdict":"pass","findings":[]})
        };
        s.update(task.as_str().unwrap(), "succeeded", &result.to_string(), "")
            .unwrap();
    }
}

#[test]
fn blocked_review_retains_candidate_without_consuming_a_code_correction() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, id, mut run) = reviewing_fixture(tmp.path());
    block_one_review(&s, &id);
    let snapshot = s.workflow(&id).unwrap()["data"]["rounds"][0]["snapshot"].clone();
    workflows::tick(&s, &f, &mut run).unwrap();
    let w = s.workflow(&id).unwrap();
    assert_eq!(w["state"], "blocked");
    assert_eq!(w["round"], 0);
    assert_eq!(w["data"]["review_blocked"]["snapshot"], snapshot["sha256"]);
    assert!(text(&w, "detail").contains("Mandatory policy copy unavailable"));
    assert!(text(&w, "detail").contains("No code-correction attempt consumed"));
    let outcome = progress::snapshot(&s, "ses_test").unwrap()["latest_result"].clone();
    assert_eq!(outcome["sections"][0]["title"], "Review blocked");
    assert!(text(&outcome["sections"][0], "body").contains("Mandatory policy copy unavailable"));
    assert_eq!(outcome["sections"][1]["title"], "Checks");
    assert_eq!(outcome["sections"][2]["title"], "Reviews");
    let before = s.tasks(None).unwrap();
    workflows::tick(&s, &f, &mut run).unwrap();
    assert_eq!(s.tasks(None).unwrap(), before);
    assert_eq!(before.iter().filter(|t| t["role"] == "worker").count(), 1);
}

#[test]
fn review_prerequisite_recovery_qualifies_the_same_candidate_without_any_worker() {
    for historical in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let (mut s, f, id, mut run) = reviewing_fixture(tmp.path());
        block_one_review(&s, &id);
        workflows::tick(&s, &f, &mut run).unwrap();
        if historical {
            let mut old = s.workflow(&id).unwrap();
            let finding = json!({"snapshot":old["data"]["rounds"][0]["snapshot"]["sha256"],"verdict":"changes_requested","findings":[{"path":"AGENTS.md","line":62,"message":"Mandatory policy read denied; no source defect established"}]});
            let task = old["data"]["rounds"][0]["review_ids"][0]
                .as_str()
                .unwrap()
                .to_owned();
            s.exec(
                "UPDATE tasks SET result=? WHERE id=?",
                rusqlite::params![finding.to_string(), task],
            )
            .unwrap();
            old["data"]["rounds"][0]["review_results"][0] = finding;
            old["data"]
                .as_object_mut()
                .unwrap()
                .remove("review_blocked");
            s.save_workflow(
                &mut old,
                "needs_attention",
                "Revision budget exhausted; preserve candidate and evidence for manual review",
            )
            .unwrap();
        }
        let parent = s.workflow(&id).unwrap();
        let snapshot = parent["data"]["rounds"][0]["snapshot"].clone();
        let guide = tmp.path().join("POLICY.md");
        fs::write(&guide, "Mandatory approved policy").unwrap();
        s.projects["project"]["review_prerequisites"] =
            teem::prerequisites::normalise(&json!([guide]), &tmp.path().join("source")).unwrap();
        let args = json!({"recovery_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":id,"review_snapshot":snapshot["sha256"]});
        let recovery =
            plugin::call(&s, "ses_test", "foreman_recover_qualification", &args).unwrap();
        assert_eq!(recovery["state"], "qualification_queued");
        assert_eq!(
            plugin::call(&s, "ses_test", "foreman_recover_qualification", &args).unwrap(),
            recovery
        );
        let mut duplicate = args.clone();
        duplicate["recovery_id"] = json!(uuid::Uuid::new_v4().to_string());
        assert!(plugin::call(&s, "ses_test", "foreman_recover_qualification", &duplicate).is_err());
        workflows::tick(&s, &f, &mut run).unwrap();
        let checking = s.workflow(&text(&recovery, "id")).unwrap();
        assert_eq!(checking["state"], "checking", "{checking}");
        assert_eq!(
            checking["data"]["rounds"][0]["snapshot"]["sha256"],
            snapshot["sha256"]
        );
        await_receipt(&checking["data"]["rounds"][0]["check_handle"]);
        workflows::tick(&s, &f, &mut run).unwrap();
        let reviewing = s.workflow(&text(&recovery, "id")).unwrap();
        assert_eq!(reviewing["state"], "reviewing", "{reviewing}");
        for task in array(&reviewing["data"]["rounds"][0]["review_ids"]) {
            let t = s.task(task.as_str().unwrap()).unwrap();
            assert_eq!(
                t["request"]["review_prerequisites"],
                s.projects["project"]["review_prerequisites"]
            );
            assert!(
                Path::new(&text(&t, "checkout"))
                    .join(".git/teem-prerequisites/0-POLICY.md")
                    .is_file()
            );
        }
        pass_reviews(&s, &text(&recovery, "id"));
        workflows::tick(&s, &f, &mut run).unwrap();
        assert_eq!(
            s.workflow(&text(&recovery, "id")).unwrap()["state"],
            "ready"
        );
        assert_eq!(s.workflow(&id).unwrap(), parent);
        assert_eq!(
            s.tasks(None)
                .unwrap()
                .iter()
                .filter(|t| t["role"] == "worker")
                .count(),
            1
        );
    }
}

#[test]
fn mixed_review_results_explain_the_blocker_before_source_findings() {
    let tmp = tempfile::tempdir().unwrap();
    let s = store(tmp.path(), &[], json!({}));
    let w = json!({"id":"workflow","state":"blocked","detail":"Review blocked","data":{"rounds":[{"snapshot":{"sha256":"candidate"},"review_results":[{"snapshot":"candidate","verdict":"changes_requested","findings":[{"path":"src/a","line":1,"message":"A demonstrated source finding"}]},{"snapshot":"candidate","verdict":"blocked","findings":[],"reason":"Required policy access denied"}]}]}});
    let sections = progress::result_sections(&s, &w);
    assert_eq!(sections[0]["title"], "Review blocked");
    assert!(text(&sections[0], "body").starts_with("Required policy access denied"));
    assert!(text(&sections[2], "body").contains("A demonstrated source finding"));
}

#[test]
fn maximum_guidance_and_review_feedback_do_not_exhaust_the_task_prompt_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, f, mut w) = workflow_fixture(tmp.path(), false);
    let skill = tmp.path().join("linus-review/SKILL.md");
    fs::create_dir(skill.parent().unwrap()).unwrap();
    fs::write(&skill, "G".repeat(12000)).unwrap();
    s.projects["project"]["review_prerequisites"] =
        teem::prerequisites::normalise(&json!([skill]), &tmp.path().join("source")).unwrap();
    w["policy"] = s.projects["project"].clone();
    w["request"]["prompt"] = json!("A".repeat(8000));
    w["request"]["workers"] = json!([{"scope":["src/a"],"prompt":"B".repeat(8000)},{"scope":["src/b"],"prompt":"C".repeat(8000)}]);
    s.exec(
        "UPDATE workflows SET request=?,policy=? WHERE id=?",
        rusqlite::params![
            w["request"].to_string(),
            w["policy"].to_string(),
            text(&w, "id")
        ],
    )
    .unwrap();
    let id = text(&w, "id");
    let mut run = runner();
    workflows::tick(&s, &f, &mut run).unwrap();
    for task in s.tasks(None).unwrap() {
        assert!(text(&task["request"], "prompt").contains(".git/teem-task-instructions.md"));
        let full = fs::read_to_string(
            Path::new(&text(&task, "checkout")).join(".git/teem-task-instructions.md"),
        )
        .unwrap();
        assert!(full.contains(&"A".repeat(8000)));
        assert!(full.contains(&"G".repeat(12000)));
    }
    finish_workers(&s);
    workflows::tick(&s, &f, &mut run).unwrap();
    await_receipt(&s.workflow(&id).unwrap()["data"]["rounds"][0]["check_handle"]);
    workflows::tick(&s, &f, &mut run).unwrap();
    let review = s.workflow(&id).unwrap();
    for task in array(&review["data"]["rounds"][0]["review_ids"]) {
        s.update(task.as_str().unwrap(),"succeeded",&json!({"snapshot":review["data"]["rounds"][0]["snapshot"]["sha256"],"verdict":"changes_requested","findings":(0..20).map(|_|json!({"path":"src/a","line":1,"message":"F".repeat(500)})).collect::<Vec<_>>()}).to_string(),"").unwrap();
    }
    workflows::tick(&s, &f, &mut run).unwrap();
    let correction = s.workflow(&id).unwrap();
    assert_eq!(correction["state"], "working", "{correction}");
    let task = s
        .task(
            correction["data"]["rounds"][1]["worker_id"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
    assert!(text(&task["request"], "prompt").len() <= 30000);
    let checkout = PathBuf::from(text(&task, "checkout"));
    let feedback = fs::read_to_string(checkout.join(".git/teem-correction-evidence.json")).unwrap();
    assert!(feedback.len() > 12000);
    assert_eq!(feedback.matches(&"F".repeat(500)).count(), 40);
}

#[test]
fn review_recovery_rejects_uncertainty_changed_authority_and_changed_snapshots() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, f, id, mut run) = reviewing_fixture(tmp.path());
    block_one_review(&s, &id);
    workflows::tick(&s, &f, &mut run).unwrap();
    let parent = s.workflow(&id).unwrap();
    let args = json!({"recovery_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":id,"review_snapshot":parent["data"]["rounds"][0]["snapshot"]["sha256"]});
    let before = s.tasks(None).unwrap();
    let mut wrong = args.clone();
    wrong["review_snapshot"] = json!("0".repeat(64));
    assert!(plugin::call(&s, "ses_test", "foreman_recover_qualification", &wrong).is_err());
    wrong = args.clone();
    wrong["expected_snapshot"] = args["review_snapshot"].clone();
    assert!(plugin::call(&s, "ses_test", "foreman_recover_qualification", &wrong).is_err());
    s.bind("ses_other", "other-terminal").unwrap();
    assert!(plugin::call(&s, "ses_other", "foreman_recover_qualification", &args).is_err());
    let policy = s.projects.clone();
    s.projects["project"]["checks"][0]["timeout"] = json!(1);
    assert!(plugin::call(&s, "ses_test", "foreman_recover_qualification", &args).is_err());
    s.projects = policy;
    let review_id = text(
        before.iter().find(|t| t["role"] == "reviewer").unwrap(),
        "id",
    );
    s.exec(
        "UPDATE tasks SET state='uncertain' WHERE id=?",
        [&review_id],
    )
    .unwrap();
    assert!(plugin::call(&s, "ses_test", "foreman_recover_qualification", &args).is_err());
    s.exec(
        "UPDATE tasks SET state='succeeded' WHERE id=?",
        [&review_id],
    )
    .unwrap();
    fs::write(tmp.path().join("source/src/a"), "unfinished source edit").unwrap();
    assert!(plugin::call(&s, "ses_test", "foreman_recover_qualification", &args).is_err());
    assert_eq!(s.tasks(None).unwrap(), before);
    assert_eq!(s.workflow(&id).unwrap(), parent);
}

#[test]
fn review_recovery_revalidates_parent_and_never_retries_another_blocked_review() {
    for tamper in [true, false] {
        let tmp = tempfile::tempdir().unwrap();
        let (s, f, id, mut run) = reviewing_fixture(tmp.path());
        block_one_review(&s, &id);
        workflows::tick(&s, &f, &mut run).unwrap();
        let parent = s.workflow(&id).unwrap();
        let args = json!({"recovery_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":id,"review_snapshot":parent["data"]["rounds"][0]["snapshot"]["sha256"]});
        let recovery =
            plugin::call(&s, "ses_test", "foreman_recover_qualification", &args).unwrap();
        let rid = text(&recovery, "id");
        if tamper {
            let worker = text(&parent["data"]["rounds"][0], "worker_id");
            s.exec("UPDATE tasks SET result='tampered' WHERE id=?", [&worker])
                .unwrap();
            workflows::tick(&s, &f, &mut run).unwrap();
            assert_eq!(s.workflow(&rid).unwrap()["state"], "blocked");
            assert_eq!(s.tasks(None).unwrap().len(), 3);
        } else {
            workflows::tick(&s, &f, &mut run).unwrap();
            await_receipt(&s.workflow(&rid).unwrap()["data"]["rounds"][0]["check_handle"]);
            workflows::tick(&s, &f, &mut run).unwrap();
            block_one_review(&s, &rid);
            workflows::tick(&s, &f, &mut run).unwrap();
            assert_eq!(s.workflow(&rid).unwrap()["state"], "blocked");
            let next = json!({"recovery_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":rid,"review_snapshot":args["review_snapshot"]});
            assert!(plugin::call(&s, "ses_test", "foreman_recover_qualification", &next).is_err());
            assert_eq!(s.workflow(&id).unwrap(), parent);
        }
        assert_eq!(
            s.tasks(None)
                .unwrap()
                .iter()
                .filter(|t| t["role"] == "worker")
                .count(),
            1
        );
    }
}

#[test]
fn explicit_requalification_after_a_new_delivery_repair_preserves_all_prior_candidates() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, f, id, mut run) = reviewing_fixture(tmp.path());
    block_one_review(&s, &id);
    workflows::tick(&s, &f, &mut run).unwrap();
    let parent = s.workflow(&id).unwrap();
    let args = json!({"recovery_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":id,"review_snapshot":parent["data"]["rounds"][0]["snapshot"]["sha256"]});
    let recovery = plugin::call(&s, "ses_test", "foreman_recover_qualification", &args).unwrap();
    let rid = text(&recovery, "id");
    workflows::tick(&s, &f, &mut run).unwrap();
    await_receipt(&s.workflow(&rid).unwrap()["data"]["rounds"][0]["check_handle"]);
    workflows::tick(&s, &f, &mut run).unwrap();
    block_one_review(&s, &rid);
    workflows::tick(&s, &f, &mut run).unwrap();
    let old = s.workflow(&rid).unwrap();
    let file = tmp.path().join("fixed-policy.md");
    fs::write(&file, "New approved prerequisite delivery").unwrap();
    s.projects["project"]["review_prerequisites"] =
        teem::prerequisites::normalise(&json!([file]), &tmp.path().join("source")).unwrap();
    let args = json!({"recovery_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":rid,"review_snapshot":args["review_snapshot"]});
    let next = plugin::call(&s, "ses_test", "foreman_recover_qualification", &args).unwrap();
    workflows::tick(&s, &f, &mut run).unwrap();
    await_receipt(&s.workflow(&text(&next, "id")).unwrap()["data"]["rounds"][0]["check_handle"]);
    workflows::tick(&s, &f, &mut run).unwrap();
    pass_reviews(&s, &text(&next, "id"));
    workflows::tick(&s, &f, &mut run).unwrap();
    assert_eq!(s.workflow(&text(&next, "id")).unwrap()["state"], "ready");
    assert_eq!(s.workflow(&id).unwrap(), parent);
    assert_eq!(s.workflow(&rid).unwrap(), old);
    assert_eq!(
        s.tasks(None)
            .unwrap()
            .iter()
            .filter(|t| t["role"] == "worker")
            .count(),
        1
    );
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
fn stopped_result_names_the_project_source_and_next_action_without_claiming_current_git_state() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, _, mut w) = workflow_fixture(tmp.path(), false);
    s.save_workflow(
        &mut w,
        "blocked",
        "Source has unfinished changes; none discarded",
    )
    .unwrap();
    let result = progress::snapshot(&s, "ses_test").unwrap()["latest_result"].clone();
    assert_eq!(result["blocker"]["kind"], "source_changes");
    assert_eq!(result["blocker"]["project"], "project");
    assert_eq!(result["blocker"]["source"], w["policy"]["path"]);
    assert!(text(&result["blocker"], "action").contains("preserve"));
    assert!(text(&result["blocker"], "action").contains("archive"));
    assert_eq!(result["historical"], true);
    assert!(result["recorded_at"].is_number());
}

#[test]
fn archive_and_restore_notice_preserve_execution_history_and_do_not_resurrect_older_results() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, _, mut w) = workflow_fixture(tmp.path(), false);
    s.save_workflow(
        &mut w,
        "blocked",
        "Source has unfinished changes; none discarded",
    )
    .unwrap();
    let before = s.workflow(&text(&w, "id")).unwrap();
    let events = s.query("SELECT * FROM events", []).unwrap();
    let args = json!({"workflow_id":w["id"],"action":"archive","reason":"Superseded by separately verified manual delivery; retained attempt is history"});
    let notice = plugin::call(&s, "ses_test", "foreman_result_notice", &args).unwrap();
    assert_eq!(notice["action"], "archive");
    assert_eq!(
        plugin::call(&s, "ses_test", "foreman_result_notice", &args).unwrap(),
        notice
    );
    let p = progress::snapshot(&s, "ses_test").unwrap();
    assert_eq!(p["result_notice_cleared"], true);
    assert!(p["latest_result"].is_null());
    assert_eq!(s.workflow(&text(&w, "id")).unwrap(), before);
    assert_eq!(s.query("SELECT * FROM events", []).unwrap(), events);
    s.bind("ses_foreign", "foreign").unwrap();
    assert!(plugin::call(&s, "ses_foreign", "foreman_result_notice", &args).is_err());
    let restore =
        json!({"workflow_id":w["id"],"action":"restore","reason":"Show retained evidence again"});
    plugin::call(&s, "ses_test", "foreman_result_notice", &restore).unwrap();
    assert_eq!(
        progress::snapshot(&s, "ses_test").unwrap()["latest_result"]["workflow_id"],
        w["id"]
    );
}

#[test]
fn active_same_task_successor_clears_the_old_blocker_without_clearing_the_workflow() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, w) = ready_operator_fixture(tmp.path());
    let successor = plugin::call(
        &s,
        "ses_test",
        "foreman_reconcile",
        &json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]}),
    )
    .unwrap();
    let p = progress::snapshot(&s, "ses_test").unwrap();
    assert!(p["latest_result"].is_null());
    assert_eq!(p["result_notice_cleared"], true);
    assert_eq!(p["result_notice"]["successor_workflow_id"], successor["id"]);
    assert_eq!(s.workflow(&text(&w, "id")).unwrap(), w);
    let args = json!({"workflow_id":successor["id"],"action":"archive","reason":"Must not hide active work"});
    assert!(plugin::call(&s, "ses_test", "foreman_result_notice", &args).is_err());
}

#[test]
fn notice_archive_rejects_uncertain_agents_and_check_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), false);
    workflows::tick(&s, &f, &mut runner()).unwrap();
    let mut w = s.workflow(&text(&w, "id")).unwrap();
    let args = json!({"workflow_id":w["id"],"action":"archive","reason":"Manual delivery supersedes this attempt"});
    s.save_workflow(&mut w, "blocked", "Stopped").unwrap();
    // A terminal controller alone is insufficient to hide unresolved execution.
    let id = text(&s.tasks(None).unwrap()[0], "id");
    s.exec(
        "UPDATE tasks SET state='uncertain',request=json_set(request,'$.workflow_id',?) WHERE id=?",
        rusqlite::params![text(&w, "id"), id],
    )
    .unwrap();
    assert!(plugin::call(&s, "ses_test", "foreman_result_notice", &args).is_err());
    s.exec("UPDATE tasks SET state='failed' WHERE id=?", [id])
        .unwrap();
    w["data"]["rounds"] =
        json!([{"check_checkout":"retained checkout without positive non-launch proof"}]);
    // Update fixture evidence without progressing a terminal workflow.
    s.exec(
        "UPDATE workflows SET data=? WHERE id=?",
        rusqlite::params![canonical(&w["data"]), text(&w, "id")],
    )
    .unwrap();
    assert!(plugin::call(&s, "ses_test", "foreman_result_notice", &args).is_err());
    assert!(
        teem::notices::record(&s, &text(&w, "id"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn invalid_workflow_submissions_report_admission_not_execution_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, _, w) = workflow_fixture(tmp.path(), false);
    let mut args = w["request"].clone();
    args["workflow_id"] = json!(uuid::Uuid::new_v4().to_string());
    args["unexpected_argument"] = json!(true);
    let error = plugin::call(&s, "ses_test", "foreman_workflow", &args)
        .unwrap_err()
        .to_string();
    assert!(error.contains("rejected before dispatch"));
    args.as_object_mut().unwrap().remove("unexpected_argument");
    args["scope"] = json!("Describe the task rather than paths");
    let error = format!(
        "{:#}",
        plugin::call(&s, "ses_test", "foreman_workflow", &args).unwrap_err()
    );
    assert!(error.contains("rejected before admission"));
    assert!(error.contains("relative file/directory paths"));
    assert_eq!(s.workflows("ses_test").unwrap().len(), 1);
    assert!(s.tasks(None).unwrap().is_empty());
}

#[test]
fn adding_prerequisites_during_work_does_not_revoke_execution_or_replay_a_worker() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, f, w) = workflow_fixture(tmp.path(), false);
    let mut runner = runner();
    workflows::tick(&s, &f, &mut runner).unwrap();
    let before = s.tasks(None).unwrap();
    let file = tmp.path().join("policy.md");
    fs::write(&file, "Mandatory read-only guidance").unwrap();
    let policy = workflows::normalise_projects(&json!({"project":{"path":w["policy"]["path"],"checks":w["policy"]["checks"],"max_revisions":1,"review_prerequisites":[file]}})).unwrap();
    s.projects = policy;
    workflows::tick(&s, &f, &mut runner).unwrap();
    let current = s.workflow(&text(&w, "id")).unwrap();
    assert_eq!(current["state"], "working");
    assert_eq!(s.tasks(None).unwrap(), before);
    assert_eq!(current["policy"], w["policy"]);
    finish_workers(&s);
    workflows::tick(&s, &f, &mut runner).unwrap();
    let current = s.workflow(&text(&w, "id")).unwrap();
    await_receipt(&current["data"]["rounds"][0]["check_handle"]);
    workflows::tick(&s, &f, &mut runner).unwrap();
    let reviewers = s
        .tasks(None)
        .unwrap()
        .into_iter()
        .filter(|t| t["role"] == "reviewer")
        .collect::<Vec<_>>();
    assert_eq!(reviewers.len(), 2);
    for t in reviewers {
        assert_eq!(
            t["request"]["review_prerequisites"],
            s.projects["project"]["review_prerequisites"]
        );
        teem::prerequisites::validate_task(&t).unwrap();
        assert!(text(&t["request"], "prompt").contains("Mandatory approved prerequisites"));
    }
}

#[test]
fn explicit_prerequisite_repair_resumes_only_existing_work_and_preserves_old_event() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, f, w) = workflow_fixture(tmp.path(), false);
    let mut runner = runner();
    workflows::tick(&s, &f, &mut runner).unwrap();
    let mut blocked = s.workflow(&text(&w, "id")).unwrap();
    s.save_workflow(
        &mut blocked,
        "blocked",
        "Project policy removed or changed; progression stopped",
    )
    .unwrap();
    let events = s.query("SELECT * FROM events", []).unwrap();
    let tasks = s.tasks(None).unwrap();
    let file = tmp.path().join("policy.md");
    fs::write(&file, "Approved policy").unwrap();
    s.projects=workflows::normalise_projects(&json!({"project":{"path":w["policy"]["path"],"checks":w["policy"]["checks"],"max_revisions":1,"review_prerequisites":[file]}})).unwrap();
    let args = json!({"workflow_id":w["id"]});
    let repaired = plugin::call(&s, "ses_test", "foreman_repair_prerequisites", &args).unwrap();
    assert_eq!(repaired["state"], "working");
    assert_eq!(repaired["policy"], w["policy"]);
    assert_eq!(repaired["request"], w["request"]);
    assert_eq!(s.tasks(None).unwrap(), tasks);
    assert_eq!(s.query("SELECT * FROM events", []).unwrap(), events);
    let again = plugin::call(&s, "ses_test", "foreman_repair_prerequisites", &args).unwrap();
    assert_eq!(again, repaired);
    let mut final_w = repaired;
    s.save_workflow(&mut final_w, "ready", "Fresh checks and reviews complete")
        .unwrap();
    let after = s.query("SELECT * FROM events ORDER BY rowid", []).unwrap();
    assert_eq!(after.len(), events.len() + 1);
    assert_eq!(after[0], events[0]);
}

#[test]
fn prerequisite_repair_rejects_authority_drift_uncertainty_and_foreign_ownership() {
    for fault in ["checks", "uncertain", "snapshot", "foreign"] {
        let tmp = tempfile::tempdir().unwrap();
        let (mut s, f, w) = workflow_fixture(tmp.path(), false);
        workflows::tick(&s, &f, &mut runner()).unwrap();
        let mut w = s.workflow(&text(&w, "id")).unwrap();
        s.save_workflow(
            &mut w,
            "blocked",
            "Project policy removed or changed; progression stopped",
        )
        .unwrap();
        let file = tmp.path().join("policy.md");
        fs::write(&file, "Approved policy").unwrap();
        s.projects=workflows::normalise_projects(&json!({"project":{"path":w["policy"]["path"],"checks":w["policy"]["checks"],"max_revisions":1,"review_prerequisites":[file]}})).unwrap();
        if fault == "checks" {
            s.projects["project"]["checks"][0]["argv"] = json!(["git", "version"]);
        }
        if fault == "uncertain" {
            s.update(
                &text(&s.tasks(None).unwrap()[0], "id"),
                "uncertain",
                "",
                "unknown",
            )
            .unwrap();
        }
        if fault == "snapshot" {
            fs::create_dir_all(
                s.directory
                    .join("managed")
                    .join(text(&w, "id"))
                    .join("round-0/snapshot"),
            )
            .unwrap();
        }
        s.bind("ses_foreign", "foreign").unwrap();
        let before = s.workflow(&text(&w, "id")).unwrap();
        let args = json!({"workflow_id":w["id"]});
        assert!(
            plugin::call(
                &s,
                if fault == "foreign" {
                    "ses_foreign"
                } else {
                    "ses_test"
                },
                "foreman_repair_prerequisites",
                &args
            )
            .is_err(),
            "{fault}"
        );
        assert_eq!(s.workflow(&text(&w, "id")).unwrap(), before);
    }
}

#[test]
fn scoped_handoff_allows_selected_evidence_and_continuation_without_rewriting_owners() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, w) = ready_operator_fixture(tmp.path());
    s.bind("ses_target", "target").unwrap();
    let status = json!({"workflow_id":w["id"],"mode":"detail","pointer":"/id"});
    assert!(plugin::call(&s, "ses_target", "foreman_status", &status).is_err());
    let before = s.workflow(&text(&w, "id")).unwrap();
    let mut unrelated = w["request"].clone();
    unrelated["workflow_id"] = json!(uuid::Uuid::new_v4().to_string());
    unrelated["issue"] = json!("tidley/teem#9");
    let unrelated = s.workflow_submit("ses_test", unrelated).unwrap();
    let args = json!({"workflow_id":w["id"],"source_foreman":"ses_test","reason":"Tom approved this exact workflow handoff"});
    let handoff = plugin::call(&s, "ses_target", "foreman_handoff", &args).unwrap();
    assert_eq!(handoff["target_foreman"], "ses_target");
    assert_eq!(
        plugin::call(&s, "ses_target", "foreman_handoff", &args).unwrap(),
        handoff
    );
    plugin::call(&s, "ses_target", "foreman_status", &status).unwrap();
    plugin::call(&s, "ses_test", "foreman_status", &status).unwrap();
    assert!(
        plugin::call(
            &s,
            "ses_target",
            "foreman_status",
            &json!({"workflow_id":unrelated["id"]})
        )
        .is_err()
    );
    assert_eq!(s.workflow(&text(&w, "id")).unwrap(), before);
    let reconcile =
        json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]});
    assert!(plugin::call(&s, "ses_test", "foreman_reconcile", &reconcile).is_err());
    let successor = plugin::call(&s, "ses_target", "foreman_reconcile", &reconcile).unwrap();
    assert_eq!(successor["foreman"], "ses_test");
    let status = json!({"workflow_id":successor["id"],"mode":"detail","pointer":"/id"});
    plugin::call(&s, "ses_target", "foreman_status", &status).unwrap();
    assert!(
        s.workflows("ses_target")
            .unwrap()
            .iter()
            .any(|v| v["id"] == successor["id"])
    );
    s.bind("ses_unrelated", "unrelated").unwrap();
    assert!(plugin::call(&s, "ses_unrelated", "foreman_status", &status).is_err());
    let mut final_w = successor;
    s.save_workflow(&mut final_w, "blocked", "fixture final result")
        .unwrap();
    let event = s
        .query(
            "SELECT id,foreman FROM events WHERE workflow_id=?",
            [text(&final_w, "id")],
        )
        .unwrap();
    assert_eq!(event[0]["foreman"], "ses_target");
    let ack = json!({"event_id":event[0]["id"]});
    assert!(plugin::call(&s, "ses_test", "foreman_ack", &ack).is_err());
    plugin::call(&s, "ses_target", "foreman_ack", &ack).unwrap();
}

#[test]
fn successive_handoffs_preserve_prior_readers_but_only_current_manager_can_ack() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, w) = ready_operator_fixture(tmp.path());
    s.bind("ses_b", "b").unwrap();
    s.bind("ses_c", "c").unwrap();
    let args = json!({"workflow_id":w["id"],"source_foreman":"ses_test","reason":"Approved first handoff"});
    plugin::call(&s, "ses_b", "foreman_handoff", &args).unwrap();
    let event = s
        .query(
            "SELECT id FROM events WHERE workflow_id=?",
            [text(&w, "id")],
        )
        .unwrap();
    assert!(
        plugin::call(
            &s,
            "ses_test",
            "foreman_ack",
            &json!({"event_id":event[0]["id"]})
        )
        .is_err()
    );
    plugin::call(
        &s,
        "ses_c",
        "foreman_handoff",
        &json!({"workflow_id":w["id"],"source_foreman":"ses_b","reason":"Approved second handoff"}),
    )
    .unwrap();
    for f in ["ses_test", "ses_b", "ses_c"] {
        plugin::call(&s, f, "foreman_status", &json!({"workflow_id":w["id"]})).unwrap();
        assert!(s.workflows(f).unwrap().iter().any(|v| v["id"] == w["id"]));
    }
    assert!(
        plugin::call(
            &s,
            "ses_b",
            "foreman_ack",
            &json!({"event_id":event[0]["id"]})
        )
        .is_err()
    );
    plugin::call(
        &s,
        "ses_c",
        "foreman_ack",
        &json!({"event_id":event[0]["id"]}),
    )
    .unwrap();
}

#[test]
fn confirmed_stop_notification_follows_the_scoped_handoff() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, f, w) = workflow_fixture(tmp.path(), false);
    workflows::tick(&s, &f, &mut runner()).unwrap();
    s.bind("ses_target", "target").unwrap();
    plugin::call(
        &s,
        "ses_target",
        "foreman_handoff",
        &json!({"workflow_id":w["id"],"source_foreman":"ses_test","reason":"Approved handoff"}),
    )
    .unwrap();
    plugin::call(
        &s,
        "ses_target",
        "foreman_stop",
        &json!({"workflow_id":w["id"]}),
    )
    .unwrap();
    teem::control::tick(&s, &f).unwrap();
    let events = s
        .query(
            "SELECT foreman FROM events WHERE workflow_id=?",
            [text(&w, "id")],
        )
        .unwrap();
    assert_eq!(events[0]["foreman"], "ses_target");
}

#[test]
fn adding_prerequisites_during_qualification_checks_preserves_progression() {
    let tmp = tempfile::tempdir().unwrap();
    let (mut s, f, id, mut run) = reviewing_fixture(tmp.path());
    block_one_review(&s, &id);
    workflows::tick(&s, &f, &mut run).unwrap();
    let parent = s.workflow(&id).unwrap();
    let args = json!({"recovery_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":id,"review_snapshot":parent["data"]["rounds"][0]["snapshot"]["sha256"]});
    let recovery = plugin::call(&s, "ses_test", "foreman_recover_qualification", &args).unwrap();
    workflows::tick(&s, &f, &mut run).unwrap();
    await_receipt(
        &s.workflow(&text(&recovery, "id")).unwrap()["data"]["rounds"][0]["check_handle"],
    );
    let file = tmp.path().join("new-policy.md");
    fs::write(&file, "Mandatory approved guidance").unwrap();
    s.projects["project"]["review_prerequisites"] =
        teem::prerequisites::normalise(&json!([file]), &tmp.path().join("source")).unwrap();
    workflows::tick(&s, &f, &mut run).unwrap();
    assert_eq!(
        s.workflow(&text(&recovery, "id")).unwrap()["state"],
        "reviewing"
    );
    assert_eq!(s.workflow(&id).unwrap(), parent);
}

#[test]
fn descendant_handoff_preserves_inherited_prior_managers_read_access() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, w) = ready_operator_fixture(tmp.path());
    for f in ["ses_b", "ses_c", "ses_d"] {
        s.bind(f, f).unwrap();
    }
    plugin::call(&s,"ses_b","foreman_handoff",&json!({"workflow_id":w["id"],"source_foreman":"ses_test","reason":"Approved root handoff"})).unwrap();
    let child = plugin::call(
        &s,
        "ses_b",
        "foreman_reconcile",
        &json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]}),
    )
    .unwrap();
    plugin::call(
        &s,
        "ses_c",
        "foreman_handoff",
        &json!({"workflow_id":w["id"],"source_foreman":"ses_b","reason":"Approved root transfer"}),
    )
    .unwrap();
    plugin::call(&s,"ses_d","foreman_handoff",&json!({"workflow_id":child["id"],"source_foreman":"ses_c","reason":"Approved descendant transfer"})).unwrap();
    plugin::call(
        &s,
        "ses_b",
        "foreman_status",
        &json!({"workflow_id":child["id"]}),
    )
    .unwrap();
    assert!(
        s.workflows("ses_b")
            .unwrap()
            .iter()
            .any(|v| v["id"] == child["id"])
    );
    assert!(teem::handoff::authorize(&s, "ses_b", &child, false).is_err());
}

#[test]
fn public_catalog_uses_teem_names_and_legacy_dispatch_stays_compatible() {
    let tmp = tempfile::tempdir().unwrap();
    let s = store(tmp.path(), &[], json!({}));
    let catalog = plugin::tools();
    for tool in catalog.as_array().unwrap() {
        assert!(text(tool, "name").starts_with("teem_"));
        assert!(!text(tool, "description").contains("foreman_"));
    }
    let args = json!({"section":"overview","limit":1});
    assert_eq!(
        plugin::call(&s, "ses_test", "teem_status", &args).unwrap(),
        plugin::call(&s, "ses_test", "foreman_status", &args).unwrap()
    );
    assert!(plugin::call(&s, "ses_test", "teem_not_a_tool", &json!({})).is_err());
}

#[test]
fn result_details_preserve_reported_changes_and_bound_unicode_without_mutating_evidence() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, mut w) = ready_operator_fixture(tmp.path());
    let worker_id = text(&w["data"]["rounds"][0], "worker_id");
    s.exec(
        "UPDATE tasks SET result=? WHERE id=?",
        rusqlite::params![
            format!(
                "Fixed stale firmware qualification.\nRemaining: hardware validation pending.\n{}",
                "🦀".repeat(4000)
            ),
            worker_id
        ],
    )
    .unwrap();
    let before = s.query("SELECT * FROM tasks", []).unwrap();
    let events = s.query("SELECT * FROM events", []).unwrap();
    let details = progress::result_details(&s, &w);
    assert!(details.contains("Fixed stale firmware qualification."));
    assert!(details.contains("hardware validation pending"));
    assert!(details.contains("worker claims, not independent verification"));
    assert!(details.contains("[excerpt; full evidence in teem_status]"));
    assert!(details.contains("tests: passed"));
    assert!(details.contains("Correctness / security: pass"));
    assert!(details.contains("Tests / regressions / scope: pass"));
    assert!(details.contains("Integration: Not recorded in Teem; source was not inspected"));
    assert!(details.len() < 10000);
    assert_eq!(s.query("SELECT * FROM tasks", []).unwrap(), before);
    assert_eq!(s.query("SELECT * FROM events", []).unwrap(), events);

    w["data"]["rounds"][0]["snapshot"]["sha256"] = json!("different candidate");
    let mismatched = progress::result_details(&s, &w);
    assert!(!mismatched.contains("tests: passed"));
    assert!(!mismatched.contains("security: pass"));
    assert_eq!(
        mismatched
            .matches("snapshot mismatch; not qualified")
            .count(),
        3
    );
    w["data"]["rounds"][0]["review_results"][0] = json!({"snapshot":"different candidate","verdict":"changes_requested","findings":[{"path":"src/a","line":12,"message":"Stale firmware can still be accepted."}]});
    w["data"]["rounds"][0]["check_result"]["snapshot"] = json!("different candidate");
    w["data"]["rounds"][0]["check_result"]["results"][0]["timed_out"] = json!(true);
    let failed = progress::result_details(&s, &w);
    assert!(failed.contains("tests: failed (timeout)"));
    assert!(failed.contains("src/a:12 — Stale firmware can still be accepted."));
    w["data"]["rounds"] = json!([]);
    let missing = progress::result_details(&s, &w);
    assert!(missing.contains("No worker summary retained"));
    assert!(missing.contains("tests: not recorded / not run"));
    assert!(missing.contains("no valid verdict recorded"));
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
