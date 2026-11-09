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
    pending: RefCell<Vec<Value>>,
    forms: RefCell<BTreeMap<String, Value>>,
}
impl Runtime for Fake {
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
    assert_eq!(
        runtime::event_payload(&f["event"]).unwrap(),
        f["event_payload"]
    );
    assert_eq!(
        runtime::event_payload(&f["workflow_event"]).unwrap(),
        f["workflow_payload"]
    );
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
    let p = tmp.join("source");
    repo(&p);
    let policy=workflows::normalise_projects(&json!({"project":{"path":p,"max_revisions":1,"checks":[{"name":"tests","kind":"test","argv":["git","status","--porcelain"]}]}})).unwrap();
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
