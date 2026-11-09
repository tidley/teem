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
fn frozen_python_wire_and_digest_contracts_match() {
    let f: Value = serde_json::from_str(include_str!("fixtures/python-contracts.json")).unwrap();
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
fn historical_python_digest_encoding_handles_exponents_and_del() {
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
fn historical_float_encoding_matches_python_over_a_deterministic_sample() {
    let script = "import json,random,struct,math\nr=random.Random(1)\nvalues=[]\nfor _ in range(2000):\n f=struct.unpack('d',r.randbytes(8))[0]\n if math.isfinite(f):values.append([f,json.dumps(f)])\nprint(json.dumps(values))";
    let output = std::process::Command::new("python3")
        .args(["-B", "-c", script])
        .output()
        .unwrap();
    assert!(output.status.success());
    let cases: Value = serde_json::from_slice(&output.stdout).unwrap();
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
    for (name, code, flag) in [
        ("timeout", "import time;time.sleep(2)", "timed_out"),
        ("limit", "print('x'*10000)", "output_limited"),
        (
            "closed",
            "import os,time;os.close(1);os.close(2);time.sleep(2)",
            "timed_out",
        ),
    ] {
        let mut policy = check_policy(json!(["python3", "-B", "-c", code]));
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
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/python-contracts.json")).unwrap();
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
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/python-contracts.json")).unwrap();
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
    let code = "import subprocess; p=subprocess.Popen(['python3','-B','-c','import time;time.sleep(60)'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);print(p.pid,flush=True)";
    let policy = check_policy(json!(["python3", "-B", "-c", code]));
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
