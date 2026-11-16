//! Isolated native fixtures; no live models, real config or retained runtime state.
use anyhow::Result;
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::Command,
    thread,
    time::{Duration, Instant},
};
use teem::{
    checks::Runner, common::*, plugin, recovery, runtime::Runtime, store::Store, workflows,
};

const FOREMAN: &str = "ses_recovery";

struct Fixture {
    tmp: tempfile::TempDir,
    source: PathBuf,
    store: Store,
    parent: String,
}

#[test]
fn setup_recovery_blocked_review_can_requalify_after_explicit_delivery_repair() {
    let mut f = Fixture::new(false);
    let args = f.args();
    let recovery = f.admit(&args).unwrap();
    let rid = text(&recovery, "id");
    let reviewing = f.tick_until(&rid, &["reviewing"], &mut runner());
    for id in array(&reviewing["data"]["rounds"][0]["review_ids"]) {
        f.store.update(id.as_str().unwrap(),"succeeded",&json!({"snapshot":reviewing["data"]["rounds"][0]["snapshot"]["sha256"],"verdict":"blocked","findings":[],"reason":"Required policy unavailable"}).to_string(),"").unwrap();
    }
    let blocked = f.tick_until(&rid, &["blocked"], &mut runner());
    let file = f.tmp.path().join("new-policy.md");
    fs::write(&file, "Approved policy delivery").unwrap();
    f.store.projects["pilot"]["review_prerequisites"] =
        teem::prerequisites::normalise(&json!([file]), &f.source).unwrap();
    let args = json!({"recovery_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":rid,"review_snapshot":blocked["data"]["rounds"][0]["snapshot"]["sha256"]});
    let next = f.admit(&args).unwrap();
    let reviewing = f.tick_until(&text(&next, "id"), &["reviewing"], &mut runner());
    f.finish_reviews(&reviewing, ["pass", "pass"]);
    assert_eq!(
        f.tick_until(&text(&next, "id"), &["ready"], &mut runner())["state"],
        "ready"
    );
    f.no_recovery_workers(&text(&next, "id"));
    assert_eq!(f.store.workflow(&rid).unwrap(), blocked);
}
impl Runtime for Fixture {
    fn request(&self, method: &str, path: &str, _: Option<&Value>) -> Result<Value> {
        assert_eq!((method, path), ("GET", "/api/session/ses_recovery"));
        Ok(json!({"id":FOREMAN,"location":{"directory":self.source}}))
    }
}
fn runner() -> Runner {
    Runner::with_executable(PathBuf::from(env!("CARGO_BIN_EXE_teem")))
}
impl Fixture {
    fn new(fail_checks: bool) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        fs::create_dir_all(source.join("src")).unwrap();
        fs::write(source.join("src/a"), "base\n").unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.name", "Test"],
            vec!["config", "user.email", "test@example.invalid"],
            vec!["add", "."],
            vec!["commit", "-qm", "base"],
        ] {
            git(&source, &args, None, None).unwrap();
        }
        let checks = json!((0..4).map(|i| json!({"name":format!("check_{i}"),"kind":"test","argv":if fail_checks && i == 0 {vec!["git","diff","--exit-code","HEAD","--"]} else {vec!["git","diff","--check","HEAD","--"]},"cwd":"."})).collect::<Vec<_>>());
        let projects =
            workflows::normalise_projects(&json!({"pilot":{"path":source,"checks":checks}}))
                .unwrap();
        let store = Store::open(&tmp.path().join("state"), &[], projects).unwrap();
        store.bind(FOREMAN, "terminal").unwrap();
        let parent = uuid::Uuid::new_v4().to_string();
        store.workflow_submit(FOREMAN, json!({"workflow_id":parent,"project":"pilot","issue":"tidley/teem#20","scope":["src"],"prompt":"Implement only the scoped task"})).unwrap();
        let f = Self {
            tmp,
            source,
            store,
            parent,
        };
        workflows::tick(&f.store, &f, &mut runner()).unwrap();
        let worker = f.store.tasks(None).unwrap().remove(0);
        fs::write(
            PathBuf::from(text(&worker, "checkout")).join("src/a"),
            "candidate\n",
        )
        .unwrap();
        f.store
            .update(
                &text(&worker, "id"),
                "succeeded",
                "execution claim, not qualification",
                "",
            )
            .unwrap();
        let mut missing = Runner::with_executable(f.tmp.path().join("absent-teem"));
        workflows::tick(&f.store, &f, &mut missing).unwrap();
        let w = f.store.workflow(&f.parent).unwrap();
        assert_eq!(w["state"], "blocked", "{w}");
        assert!(w["data"]["rounds"][0].get("check_handle").is_none());
        assert_eq!(
            w["data"]["rounds"][0]["check_setup_failure"]["not_started"],
            true
        );
        f
    }
    fn args(&self) -> Value {
        json!({"recovery_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":self.parent,"expected_snapshot":self.store.workflow(&self.parent).unwrap()["data"]["rounds"][0]["snapshot"]["sha256"]})
    }
    fn evidence_dir(&self) -> PathBuf {
        self.store
            .directory
            .join("managed")
            .join(&self.parent)
            .join("round-0/check-evidence")
    }
    fn historical(&self) {
        // Model the old synchronous Runner failure, which persisted the same
        // blocked round/event but had neither of the newly introduced receipts.
        fs::remove_file(self.evidence_dir().join("launch-failure.json")).unwrap();
        let mut w = self.store.workflow(&self.parent).unwrap();
        w["data"]["rounds"][0]
            .as_object_mut()
            .unwrap()
            .remove("check_setup_failure");
        assert_eq!(w["detail"], "No such file or directory (os error 2)");
        self.store
            .exec(
                "UPDATE workflows SET data=? WHERE id=?",
                rusqlite::params![canonical(&w["data"]), self.parent],
            )
            .unwrap();
    }
    fn admit(&self, args: &Value) -> Result<Value> {
        plugin::call(&self.store, FOREMAN, "foreman_recover_qualification", args)
    }
    fn handoff_args(&self, recovery: &Value) -> Value {
        json!({"workflow_id":recovery["recovery_id"],"source_workflow_id":recovery["source_workflow_id"],"setup_snapshot":recovery["expected_snapshot"]})
    }
    fn handoff(&self, args: &Value) -> Result<Value> {
        plugin::call(&self.store, FOREMAN, "foreman_reconcile", args)
    }
    fn tick_until(&self, id: &str, states: &[&str], runner: &mut Runner) -> Value {
        let end = Instant::now() + Duration::from_secs(15);
        loop {
            workflows::tick(&self.store, self, runner).unwrap();
            let w = self.store.workflow(id).unwrap();
            if states.contains(&text(&w, "state").as_str()) {
                return w;
            }
            assert!(Instant::now() < end, "Timed out: {w}");
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn no_recovery_workers(&self, id: &str) {
        let tasks = self.store.tasks(None).unwrap();
        assert_eq!(tasks.iter().filter(|t| t["role"] == "worker").count(), 1);
        assert!(
            tasks
                .iter()
                .filter(|t| t["request"]["workflow_id"] == id)
                .all(|t| t["role"] == "reviewer")
        );
    }
    fn finish_reviews(&self, w: &Value, verdicts: [&str; 2]) {
        let round = &w["data"]["rounds"][0];
        let ids = array(&round["review_ids"]);
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
        let tasks = ids
            .iter()
            .map(|id| self.store.task(id.as_str().unwrap()).unwrap())
            .collect::<Vec<_>>();
        assert_ne!(tasks[0]["session_id"], tasks[1]["session_id"]);
        assert_ne!(tasks[0]["checkout"], tasks[1]["checkout"]);
        assert!(text(&tasks[0]["request"], "prompt").contains("correctness, contracts"));
        assert!(text(&tasks[1]["request"], "prompt").contains("tests, regressions"));
        for (task, verdict) in tasks.iter().zip(verdicts) {
            assert_eq!(task["request"]["input_tree"], round["snapshot"]["tree"]);
            assert_eq!(task["request"]["issue"], "tidley/teem#20");
            let permissions = teem::runtime::permissions(task);
            assert!(!array(&permissions).iter().any(|p| {
                ["shell", "edit", "patch"].contains(&text(p, "action").as_str())
                    && p["effect"] == "allow"
            }));
            let findings = if verdict == "pass" {
                json!([])
            } else {
                json!([{"path":"src/a","line":1,"message":"Demonstrated fixture finding"}])
            };
            self.store.update(&text(task,"id"), "succeeded", &canonical(&json!({"snapshot":round["snapshot"]["sha256"],"verdict":verdict,"findings":findings})), "").unwrap();
        }
    }
}

fn passing_and_findings() {
    for (historical, verdicts) in [
        (false, ["pass", "pass"]),
        (false, ["changes_requested", "pass"]),
        (false, ["pass", "changes_requested"]),
        (true, ["pass", "pass"]),
        (true, ["changes_requested", "pass"]),
        (true, ["pass", "changes_requested"]),
    ] {
        let f = Fixture::new(false);
        if historical {
            f.historical();
        }
        let parent = f.store.workflow(&f.parent).unwrap();
        let tasks = f.store.tasks(None).unwrap();
        let old_spec = fs::read(f.evidence_dir().join("spec.json")).unwrap();
        let args = f.args();
        let id = text(&args, "recovery_id");
        let admitted = f.admit(&args).unwrap();
        assert_eq!(admitted["state"], "qualification_queued");
        assert_eq!(
            admitted["request"]["qualification_recovery"]["non_launch_proof"]["kind"],
            if historical {
                "historical-controller-enoent-v1"
            } else {
                "synchronous-spawn-rejection"
            }
        );
        assert_eq!(f.admit(&args).unwrap(), admitted);
        if historical {
            // Delivery/acknowledgement is mutable, unlike launch provenance.
            f.store
                .exec(
                    "UPDATE events SET acked=1,state='done',detail='delivered' WHERE workflow_id=?",
                    [&f.parent],
                )
                .unwrap();
        }
        let progress = teem::progress::workflow_rows(&f.store, FOREMAN).unwrap();
        assert!(!progress.iter().any(|r| text(r, "label").contains("Worker")));
        let w = f.tick_until(&id, &["reviewing", "blocked"], &mut runner());
        assert_eq!(w["state"], "reviewing", "{w}");
        let round = &w["data"]["rounds"][0];
        let old = &parent["data"]["rounds"][0];
        for key in ["sha256", "tree", "base"] {
            assert_eq!(round["snapshot"][key], old["snapshot"][key]);
        }
        assert_ne!(round["snapshot"]["patch"], old["snapshot"]["patch"]);
        assert_eq!(
            fs::read(text(&round["snapshot"], "patch")).unwrap(),
            fs::read(text(&old["snapshot"], "patch")).unwrap()
        );
        assert_ne!(round["check_checkout"], old["check_checkout"]);
        assert_eq!(
            round["check_result"]["results"].as_array().unwrap().len(),
            4
        );
        let output = f.store.status(FOREMAN, &json!({"mode":"detail","section":"check_output","workflow_id":id,"pointer":"/data/rounds/0/check_result/results/0"})).unwrap();
        assert_eq!(output["chunk"], canonical(&json!("")));
        let old_dir = f
            .store
            .directory
            .join("managed")
            .join(&f.parent)
            .join("round-0/check-evidence");
        assert_eq!(
            fs::read_dir(&old_dir).unwrap().count(),
            if historical { 2 } else { 3 }
        );
        assert_eq!(fs::metadata(old_dir.join("helper.log")).unwrap().len(), 0);
        assert_eq!(fs::read(old_dir.join("spec.json")).unwrap(), old_spec);
        f.finish_reviews(&w, verdicts);
        let done = f.tick_until(&id, &["ready", "needs_attention", "blocked"], &mut runner());
        assert_eq!(
            done["state"],
            if verdicts == ["pass", "pass"] {
                "ready"
            } else {
                "needs_attention"
            },
            "{done}"
        );
        assert_eq!(done["round"], 0);
        for key in ["issue", "scope", "prompt"] {
            assert_eq!(done["request"][key], parent["request"][key]);
        }
        assert!(done["integration"].is_null());
        f.no_recovery_workers(&id);
        assert_eq!(f.store.workflow(&f.parent).unwrap(), parent);
        assert_eq!(f.store.task(&text(&tasks[0], "id")).unwrap(), tasks[0]);
        assert_eq!(
            fs::read_to_string(f.source.join("src/a")).unwrap(),
            "base\n"
        );
        assert_eq!(f.admit(&args).unwrap(), done);
        let summary = f.store.status(FOREMAN, &json!({"workflow_id":id})).unwrap();
        assert_eq!(summary["workflows"][0]["workers_total"], 0);
        assert_eq!(
            summary["workflows"][0]["qualification_recovery"]["arguments"],
            args
        );
        let detail = f.store.status(FOREMAN, &json!({"mode":"detail","workflow_id":id,"pointer":"/request/qualification_recovery/arguments"})).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(detail["chunk"].as_str().unwrap()).unwrap(),
            args
        );
        assert!(f.admit(&f.args()).is_err());
    }
}
fn failing_checks_stop() {
    failing_checks_case(false);
    failing_checks_case(true);
}
fn failing_checks_case(historical: bool) {
    let f = Fixture::new(true);
    if historical {
        f.historical();
    }
    let args = f.args();
    let id = text(&args, "recovery_id");
    f.admit(&args).unwrap();
    let w = f.tick_until(&id, &["needs_attention", "blocked"], &mut runner());
    assert_eq!(w["state"], "needs_attention", "{w}");
    assert_ne!(
        w["data"]["rounds"][0]["check_result"]["results"][0]["returncode"],
        0
    );
    assert!(w["data"]["rounds"][0].get("review_ids").is_none());
    f.no_recovery_workers(&id);
}
fn rejections() {
    rejection_cases(false);
    rejection_cases(true);
}
fn rejection_cases(historical: bool) {
    for case in [
        "digest",
        "source",
        "advanced-source",
        "policy",
        "project-path",
        "paused",
        "stopped",
        "worker",
        "queued",
        "missing-worker",
        "extra-reviewer",
        "missing-proof-field",
        "changed-proof",
        "receipt",
        "arbitrary-state",
        "budget",
        "patch",
        "tree",
        "base",
        "scope",
        "issue",
        "native",
        "session",
        "prompt",
        "review-launch",
        "resume-phase",
        "worker-scope",
    ] {
        let mut f = Fixture::new(false);
        if historical {
            f.historical();
        }
        let mut args = f.args();
        let mut parent = f.store.workflow(&f.parent).unwrap();
        let dir = f
            .store
            .directory
            .join("managed")
            .join(&f.parent)
            .join("round-0/check-evidence");
        match case {
            "digest" => args["expected_snapshot"] = json!("0".repeat(64)),
            "source" => {
                fs::write(f.source.join("src/a"), "drift\n").unwrap();
            }
            "advanced-source" => {
                fs::write(f.source.join("src/a"), "advanced\n").unwrap();
                git(&f.source, &["add", "."], None, None).unwrap();
                git(&f.source, &["commit", "-qm", "advanced"], None, None).unwrap();
            }
            "policy" => f.store.projects["pilot"]["checks"][0]["argv"] = json!(["git", "status"]),
            "project-path" => f.store.projects["pilot"]["path"] = json!(f.tmp.path()),
            "paused" => {
                f.store
                    .exec("INSERT INTO workflow_stops VALUES (?,0)", [&f.parent])
                    .unwrap();
            }
            "stopped" => {
                let t = f.store.tasks(None).unwrap().remove(0);
                f.store
                    .exec(
                        "INSERT INTO task_stops VALUES (?,'uncertain','')",
                        [text(&t, "id")],
                    )
                    .unwrap();
            }
            "worker" => {
                f.store
                    .exec("UPDATE tasks SET state='uncertain'", [])
                    .unwrap();
            }
            "queued" => {
                f.store.exec("UPDATE tasks SET state='queued'", []).unwrap();
            }
            "missing-worker" => {
                f.store.exec("DELETE FROM tasks", []).unwrap();
            }
            "extra-reviewer" => {
                f.store.exec("INSERT INTO tasks(id,foreman,role,checkout,state,request,session_id,prompt_id,sparse,created) SELECT 'extra',foreman,'reviewer',checkout,'succeeded',request,'ses_extra','msg_extra',sparse,created FROM tasks", []).unwrap();
            }
            "missing-proof-field" => {
                if historical {
                    // Partial modern evidence cannot use the historical path.
                    fs::write(dir.join("launch-failure.json"), "{}").unwrap();
                } else {
                    parent["data"]["rounds"][0]
                        .as_object_mut()
                        .unwrap()
                        .remove("check_setup_failure");
                }
            }
            "changed-proof" => {
                fs::write(dir.join("launch-failure.json"), "{}").unwrap();
            }
            "receipt" => {
                fs::write(dir.join("receipt.json"), "{}").unwrap();
            }
            "arbitrary-state" => parent["state"] = json!("needs_attention"),
            "budget" => parent["round"] = json!(3),
            "patch" => {
                let path = text(&parent["data"]["rounds"][0]["snapshot"], "patch");
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
                fs::write(&path, "changed").unwrap();
            }
            "tree" => parent["data"]["rounds"][0]["snapshot"]["tree"] = json!("0".repeat(40)),
            "base" => parent["data"]["rounds"][0]["snapshot"]["base"] = json!("0".repeat(40)),
            "scope" => parent["request"]["scope"] = json!(["other"]),
            "issue" => {
                parent["request"].as_object_mut().unwrap().remove("issue");
            }
            "native" => {
                f.store
                    .exec("UPDATE tasks SET role='reviewer'", [])
                    .unwrap();
            }
            "review-launch" => parent["data"]["rounds"][0]["review_ids"] = json!(["unknown"]),
            "resume-phase" => parent["data"]["resume_phase"] = json!("checking"),
            "worker-scope" => {
                f.store
                    .exec(
                        "UPDATE tasks SET request=json_set(request,'$.scope',json('[\"other\"]'))",
                        [],
                    )
                    .unwrap();
            }
            "session" => {
                f.store.exec("UPDATE tasks SET session_id=''", []).unwrap();
            }
            "prompt" => {
                f.store.exec("UPDATE tasks SET prompt_id=''", []).unwrap();
            }
            _ => unreachable!(),
        }
        f.store
            .exec(
                "UPDATE workflows SET request=?,data=?,state=?,round=?,detail=? WHERE id=?",
                rusqlite::params![
                    canonical(&parent["request"]),
                    canonical(&parent["data"]),
                    text(&parent, "state"),
                    parent["round"].as_i64().unwrap(),
                    text(&parent, "detail"),
                    f.parent
                ],
            )
            .unwrap();
        assert!(
            f.admit(&args).is_err(),
            "admitted {case}, historical={historical}"
        );
        // Reuse the same isolated mutation: setup handoff shares recovery's
        // proof/provenance guards, but deliberately permits clean source/policy drift.
        if !["advanced-source", "policy"].contains(&case) {
            assert!(
                f.handoff(&f.handoff_args(&args)).is_err(),
                "handoff admitted {case}, historical={historical}"
            );
        }
        assert_eq!(f.store.workflows(FOREMAN).unwrap().len(), 1, "{case}");
    }
    let f = Fixture::new(false);
    f.store.bind("ses_foreign", "other").unwrap();
    assert!(recovery::submit(&f.store, "ses_foreign", &f.args()).is_err());
    assert!(teem::reconcile::submit(&f.store, "ses_foreign", &f.handoff_args(&f.args())).is_err());
}
fn concurrency_and_restart() {
    concurrency_case(false);
    concurrency_case(true);
}
fn concurrency_case(historical: bool) {
    let f = Fixture::new(false);
    if historical {
        f.historical();
    }
    let args = f.args();
    let handles = (0..2)
        .map(|_| {
            let path = f.store.directory.clone();
            let projects = f.store.projects.clone();
            let args = args.clone();
            thread::spawn(move || {
                recovery::submit(&Store::open(&path, &[], projects).unwrap(), FOREMAN, &args)
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    let results = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results[0], results[1]);
    let mut changed = args.clone();
    changed["expected_snapshot"] = json!("0".repeat(64));
    assert!(f.admit(&changed).is_err());
    changed = args.clone();
    changed["source_workflow_id"] = json!(uuid::Uuid::new_v4().to_string());
    assert!(f.admit(&changed).is_err());
    for key in ["scope", "issue", "prompt", "workers"] {
        let mut changed = args.clone();
        changed[key] = json!("not inherited");
        assert!(f.admit(&changed).is_err());
    }
    assert!(f.admit(&f.args()).is_err());
    let id = text(&args, "recovery_id");
    workflows::tick(&f.store, &f, &mut runner()).unwrap();
    let before = f.store.workflow(&id).unwrap();
    let reopened = Store::open(&f.store.directory, &[], f.store.projects.clone()).unwrap();
    assert_eq!(recovery::submit(&reopened, FOREMAN, &args).unwrap(), before);
    let end = Instant::now() + Duration::from_secs(15);
    loop {
        workflows::tick(&reopened, &f, &mut runner()).unwrap();
        let w = reopened.workflow(&id).unwrap();
        assert_eq!(
            w["data"]["rounds"][0]["check_handle"],
            before["data"]["rounds"][0]["check_handle"]
        );
        if w["state"] == "reviewing" {
            break;
        }
        assert!(Instant::now() < end, "{w}");
        thread::sleep(Duration::from_millis(10));
    }
}
fn uncertain_never_replays() {
    for (historical, after_launch) in [(false, false), (false, true), (true, false), (true, true)] {
        let f = Fixture::new(false);
        if historical {
            f.historical();
        }
        let args = f.args();
        let id = text(&args, "recovery_id");
        let mut w = f.admit(&args).unwrap();
        if after_launch {
            let mut child = Runner::with_executable(PathBuf::from(env!(
                "CARGO_BIN_EXE_teem-subprocess-fixture"
            )));
            workflows::tick(&f.store, &f, &mut child).unwrap();
            let mut admitted = f.store.workflow(&id).unwrap();
            admitted["data"]["rounds"][0]["check_handle"]["deadline"] = json!(0);
            f.store
                .save_workflow(
                    &mut admitted,
                    "checking",
                    "fixture expired uncertain subprocess",
                )
                .unwrap();
            w = f.tick_until(&id, &["uncertain", "blocked"], &mut runner());
            thread::sleep(Duration::from_millis(50));
            child.reap();
        } else {
            f.store
                .save_workflow(&mut w, "snapshotting", "fixture interrupted preparation")
                .unwrap();
            workflows::tick(&f.store, &f, &mut runner()).unwrap();
            w = f.store.workflow(&id).unwrap();
        }
        assert_eq!(w["state"], "uncertain", "{w}");
        for _ in 0..3 {
            workflows::tick(&f.store, &f, &mut runner()).unwrap();
        }
        assert_eq!(f.store.workflow(&id).unwrap(), w);
        assert_eq!(f.admit(&args).unwrap(), w);
        assert!(f.admit(&f.args()).is_err());
        f.no_recovery_workers(&id);
    }
}
fn changes_after_reservation_fail_closed() {
    for case in [
        "source",
        "policy",
        "parent",
        "native",
        "receipt",
        "historical-spec",
        "historical-terminal",
        "paused",
        "task-stopped",
    ] {
        let mut f = Fixture::new(false);
        f.historical();
        let args = f.args();
        let id = text(&args, "recovery_id");
        f.admit(&args).unwrap();
        let w = f.tick_until(&id, &["reviewing", "blocked"], &mut runner());
        assert_eq!(w["state"], "reviewing", "{w}");
        match case {
            "source" => {
                fs::write(f.source.join("src/a"), "drift\n").unwrap();
            }
            // Scheduling budgets and read-only prerequisite delivery do not
            // revoke existing qualification; approved check changes still do.
            "policy" => f.store.projects["pilot"]["checks"][0]["timeout"] = json!(1),
            "parent" => {
                f.store
                    .exec(
                        "UPDATE workflows SET detail='changed' WHERE id=?",
                        [&f.parent],
                    )
                    .unwrap();
            }
            "native" => {
                f.store
                    .exec(
                        "UPDATE tasks SET session_id='ses_changed' WHERE role='worker'",
                        [],
                    )
                    .unwrap();
            }
            "receipt" => {
                fs::write(
                    text(&w["data"]["rounds"][0]["check_handle"], "receipt"),
                    "{}",
                )
                .unwrap();
                f.finish_reviews(&w, ["pass", "pass"]);
            }
            "historical-spec" => {
                let path = f.evidence_dir().join("spec.json");
                let mut spec = read_json(&path).unwrap();
                spec["token"] = json!(uuid::Uuid::new_v4().to_string());
                fs::write(path, canonical(&spec)).unwrap();
            }
            "historical-terminal" => {
                f.store
                    .exec(
                        "UPDATE events SET request_id=? WHERE workflow_id=?",
                        rusqlite::params![uuid::Uuid::new_v4().to_string(), f.parent],
                    )
                    .unwrap();
            }
            "paused" => {
                f.store
                    .exec("INSERT INTO workflow_stops VALUES (?,0)", [&id])
                    .unwrap();
            }
            "task-stopped" => {
                let task = &w["data"]["rounds"][0]["review_ids"][0];
                f.store
                    .exec(
                        "INSERT INTO task_stops VALUES (?,'uncertain','')",
                        [task.as_str().unwrap()],
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }
        if case == "paused" {
            // The #2 store guard refuses even a blocked-state write after a
            // user pause. Recovery must preserve that lineage, not override it.
            let retained = f.store.workflow(&id).unwrap();
            let tasks = f.store.tasks(None).unwrap();
            let events = f
                .store
                .query("SELECT * FROM events ORDER BY id", [])
                .unwrap();
            for _ in 0..2 {
                let error = workflows::tick(&f.store, &f, &mut runner()).unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("Workflow is paused by a user stop")
                );
                assert_eq!(f.store.workflow(&id).unwrap(), retained);
                assert_eq!(f.store.tasks(None).unwrap(), tasks);
                assert_eq!(
                    f.store
                        .query("SELECT * FROM events ORDER BY id", [])
                        .unwrap(),
                    events
                );
                assert!(teem::control::paused(&f.store, &id).unwrap());
                assert_eq!(f.admit(&args).unwrap(), retained);
            }
        } else {
            workflows::tick(&f.store, &f, &mut runner()).unwrap();
            assert_eq!(f.store.workflow(&id).unwrap()["state"], "blocked", "{case}");
        }
        f.no_recovery_workers(&id);
    }
}

fn historical_requires_controller_provenance() {
    for case in [
        "no-event",
        "foreign-event",
        "no-finish",
        "resume-phase",
        "snapshotting",
        "uncertain",
        "policy-error-after-crash",
        "other-error",
        "no-spec",
        "log",
        "receipt",
        "handle",
        "spec-snapshot",
        "spec-checks",
        "spec-checkout",
        "spec-directory",
        "spec-token",
    ] {
        let f = Fixture::new(false);
        f.historical();
        let mut w = f.store.workflow(&f.parent).unwrap();
        match case {
            "no-event" => {
                f.store
                    .exec("DELETE FROM events WHERE workflow_id=?", [&f.parent])
                    .unwrap();
            }
            "foreign-event" => {
                f.store
                    .exec(
                        "UPDATE events SET foreman='ses_foreign' WHERE workflow_id=?",
                        [&f.parent],
                    )
                    .unwrap();
            }
            "no-finish" => {
                w["data"].as_object_mut().unwrap().remove("finished_at");
            }
            "resume-phase" => w["data"]["resume_phase"] = json!("checking"),
            "snapshotting" => w["state"] = json!("snapshotting"),
            "uncertain" => w["state"] = json!("uncertain"),
            "policy-error-after-crash" => {
                w["detail"] = json!("Project policy removed or changed; progression stopped");
            }
            "other-error" => w["detail"] = json!("Permission denied (os error 13)"),
            "no-spec" => fs::remove_file(f.evidence_dir().join("spec.json")).unwrap(),
            "log" => fs::write(f.evidence_dir().join("helper.log"), "possible launch").unwrap(),
            "receipt" => fs::write(f.evidence_dir().join("receipt.json"), "{}").unwrap(),
            "handle" => w["data"]["rounds"][0]["check_handle"] = json!({}),
            _ => {
                let path = f.evidence_dir().join("spec.json");
                let mut spec = read_json(&path).unwrap();
                match case {
                    "spec-snapshot" => spec["snapshot"] = json!("0".repeat(64)),
                    "spec-checks" => spec["checks"] = json!([]),
                    "spec-checkout" => spec["checkout"] = json!(f.source),
                    "spec-directory" => spec["directory"] = json!(f.tmp.path()),
                    "spec-token" => spec["token"] = json!("not-a-uuid"),
                    _ => unreachable!(),
                }
                fs::write(path, canonical(&spec)).unwrap();
            }
        }
        f.store
            .exec(
                "UPDATE workflows SET data=?,state=?,detail=? WHERE id=?",
                rusqlite::params![
                    canonical(&w["data"]),
                    text(&w, "state"),
                    text(&w, "detail"),
                    f.parent
                ],
            )
            .unwrap();
        assert!(f.admit(&f.args()).is_err(), "admitted historical {case}");
        assert!(
            f.handoff(&f.handoff_args(&f.args())).is_err(),
            "handoff admitted historical {case}"
        );
        assert_eq!(f.store.workflows(FOREMAN).unwrap().len(), 1);
    }
}

fn explicit_reconciliation_drops_recovery_only_contract() {
    for verdicts in [["pass", "pass"], ["changes_requested", "pass"]] {
        for advance in [false, true] {
            let f = Fixture::new(false);
            let args = f.args();
            let id = text(&args, "recovery_id");
            f.admit(&args).unwrap();
            let reviewing = f.tick_until(&id, &["reviewing", "blocked"], &mut runner());
            assert_eq!(reviewing["state"], "reviewing");
            f.finish_reviews(&reviewing, verdicts);
            let done = f.tick_until(&id, &["ready", "needs_attention", "blocked"], &mut runner());
            assert_ne!(done["state"], "blocked");
            if advance {
                fs::write(f.source.join("src/b"), "independent source update\n").unwrap();
                git(&f.source, &["add", "."], None, None).unwrap();
                git(&f.source, &["commit", "-qm", "advance"], None, None).unwrap();
            }
            let next_id = uuid::Uuid::new_v4().to_string();
            let next = teem::reconcile::submit(
                &f.store,
                FOREMAN,
                &json!({"workflow_id":next_id,"source_workflow_id":id}),
            )
            .unwrap();
            assert!(next["request"].get("qualification_recovery").is_none());
            assert_eq!(next["request"]["reconcile_from"], id);
            workflows::tick(&f.store, &f, &mut runner()).unwrap();
            let working = f.store.workflow(&next_id).unwrap();
            assert_eq!(working["state"], "working", "{working}");
            assert_eq!(
                working["base_revision"],
                git_text(&f.source, &["rev-parse", "HEAD"]).unwrap()
            );
            let worker = f
                .store
                .task(working["data"]["rounds"][0]["worker_id"].as_str().unwrap())
                .unwrap();
            assert_eq!(worker["role"], "worker");
            assert_eq!(worker["request"]["issue"], "tidley/teem#20");
            assert_eq!(worker["request"]["scope"], done["request"]["scope"]);
            assert_eq!(
                fs::read_to_string(PathBuf::from(text(&worker, "checkout")).join("src/a")).unwrap(),
                "candidate\n"
            );
            assert_eq!(f.store.workflow(&id).unwrap(), done);
        }
    }
}

fn setup_handoff_on_advanced_source() {
    for (historical, recovery_setup) in [(false, false), (true, false), (false, true)] {
        let mut f = Fixture::new(false);
        if recovery_setup {
            let recovery = f.admit(&f.args()).unwrap();
            workflows::tick(
                &f.store,
                &f,
                &mut Runner::with_executable(f.tmp.path().join("missing")),
            )
            .unwrap();
            f.parent = text(&recovery, "id");
            assert_eq!(f.store.workflow(&f.parent).unwrap()["state"], "blocked");
        }
        if historical {
            // A real multi-workflow lineage with older verified checks/reviews,
            // followed by a setup-blocked implementation continuation.
            let recovery = f.admit(&f.args()).unwrap();
            let qualified = f.tick_until(&text(&recovery, "id"), &["reviewing"], &mut runner());
            f.finish_reviews(&qualified, ["pass", "pass"]);
            let ready = f.tick_until(&text(&recovery, "id"), &["ready"], &mut runner());
            let id = uuid::Uuid::new_v4().to_string();
            f.handoff(&json!({"workflow_id":id,"source_workflow_id":ready["id"]}))
                .unwrap();
            workflows::tick(&f.store, &f, &mut runner()).unwrap();
            let w = f.store.workflow(&id).unwrap();
            let worker = w["data"]["rounds"][0]["worker_id"].as_str().unwrap();
            f.store
                .update(worker, "succeeded", "retained implementation", "")
                .unwrap();
            workflows::tick(
                &f.store,
                &f,
                &mut Runner::with_executable(f.tmp.path().join("missing")),
            )
            .unwrap();
            f.parent = id;
            f.historical();
        }
        let parent = f.store.workflow(&f.parent).unwrap();
        let tasks = f.store.tasks(None).unwrap();
        let recovery_args = f.args();
        let args = f.handoff_args(&recovery_args);
        let id = text(&args, "workflow_id");
        let mut implicit = args.clone();
        implicit.as_object_mut().unwrap().remove("setup_snapshot");
        assert!(f.handoff(&implicit).is_err());
        if historical {
            let ancestor_id = text(&parent["request"], "reconcile_from");
            let ancestor = f.store.workflow(&ancestor_id).unwrap();
            assert_eq!(ancestor["state"], "ready");
            f.store
                .exec(
                    "UPDATE workflows SET state='uncertain' WHERE id=?",
                    [&ancestor_id],
                )
                .unwrap();
            assert!(f.handoff(&args).is_err());
            f.store
                .exec(
                    "UPDATE workflows SET state='ready' WHERE id=?",
                    [&ancestor_id],
                )
                .unwrap();
            f.store
                .exec("INSERT INTO workflow_stops VALUES (?,0)", [&ancestor_id])
                .unwrap();
            assert!(f.handoff(&args).is_err());
            f.store
                .exec(
                    "DELETE FROM workflow_stops WHERE workflow_id=?",
                    [&ancestor_id],
                )
                .unwrap();
            let receipt = text(&ancestor["data"]["rounds"][0]["check_handle"], "receipt");
            let bytes = fs::read(&receipt).unwrap();
            fs::write(&receipt, "{}").unwrap();
            assert!(f.handoff(&args).is_err());
            fs::write(&receipt, bytes).unwrap();
            assert_eq!(f.store.workflow(&ancestor_id).unwrap(), ancestor);
        }
        fs::write(f.source.join("src/b"), "new main content\n").unwrap();
        if historical {
            fs::write(f.source.join("src/a"), "newer conflicting main content\n").unwrap();
        }
        git(&f.source, &["add", "."], None, None).unwrap();
        git(&f.source, &["commit", "-qm", "advanced"], None, None).unwrap();
        for check in f.store.projects["pilot"]["checks"].as_array_mut().unwrap() {
            check["name"] = json!(format!("current_{}", text(check, "name")));
        }
        assert!(f.admit(&recovery_args).is_err()); // unchanged qualification guard
        let handles = (0..2)
            .map(|_| {
                let path = f.store.directory.clone();
                let projects = f.store.projects.clone();
                let args = args.clone();
                thread::spawn(move || {
                    teem::reconcile::submit(
                        &Store::open(&path, &[], projects).unwrap(),
                        FOREMAN,
                        &args,
                    )
                    .unwrap()
                })
            })
            .collect::<Vec<_>>();
        let admitted = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(admitted[0], admitted[1]);
        assert_eq!(admitted[0]["policy"], f.store.projects["pilot"]);
        assert!(
            admitted[0]["request"]
                .get("qualification_recovery")
                .is_none()
        );
        assert!(
            admitted[0]["request"]
                .get("reconcile_blocked_snapshot")
                .is_none()
        );
        let reopened = Store::open(&f.store.directory, &[], f.store.projects.clone()).unwrap();
        assert_eq!(
            teem::reconcile::submit(&reopened, FOREMAN, &args).unwrap(),
            admitted[0]
        );
        let mut changed = args.clone();
        changed["setup_snapshot"] = json!("0".repeat(64));
        assert!(f.handoff(&changed).is_err());
        assert!(f.handoff(&f.handoff_args(&f.args())).is_err());
        workflows::tick(&f.store, &f, &mut runner()).unwrap();
        let working = f.store.workflow(&id).unwrap();
        assert_eq!(working["state"], "working", "{working}"); // own preparing reservation must not deadlock
        assert_eq!(
            working["base_revision"],
            git_text(&f.source, &["rev-parse", "HEAD"]).unwrap()
        );
        let worker = f
            .store
            .task(working["data"]["rounds"][0]["worker_id"].as_str().unwrap())
            .unwrap();
        assert_eq!(worker["request"]["scope"], parent["request"]["scope"]);
        assert_eq!(worker["request"]["issue"], parent["request"]["issue"]);
        let checkout = PathBuf::from(text(&worker, "checkout"));
        let notes = read_json(&checkout.join(".git/teem-reconciliation.json")).unwrap();
        assert_eq!(notes["seeded"], !historical);
        assert_eq!(
            fs::read_to_string(checkout.join("src/b")).unwrap(),
            "new main content\n"
        );
        assert_eq!(
            fs::read_to_string(checkout.join("src/a")).unwrap(),
            if historical {
                "newer conflicting main content\n"
            } else {
                "candidate\n"
            }
        );
        if historical {
            fs::write(
                checkout.join("src/a"),
                "scoped correction retaining newer main content\n",
            )
            .unwrap();
        }
        f.store
            .update(&text(&worker, "id"), "succeeded", "not qualification", "")
            .unwrap();
        let reviewing = f.tick_until(&id, &["reviewing", "blocked"], &mut runner());
        assert_eq!(reviewing["state"], "reviewing", "{reviewing}");
        let round = &reviewing["data"]["rounds"][0];
        assert_ne!(
            round["snapshot"]["tree"],
            parent["data"]["rounds"][0]["snapshot"]["tree"]
        );
        assert_eq!(round["snapshot"]["base"], working["base_revision"]);
        assert_ne!(round["snapshot"]["base"], parent["base_revision"]);
        assert_eq!(
            round["check_result"]["results"].as_array().unwrap().len(),
            4
        );
        assert!(text(&round["check_result"]["results"][0], "name").starts_with("current_"));
        for old in &tasks {
            assert!(!array(&round["review_ids"]).contains(&old["id"]));
        }
        f.finish_reviews(&reviewing, ["pass", "pass"]);
        assert_eq!(
            f.tick_until(&id, &["ready", "blocked"], &mut runner())["state"],
            "ready"
        );
        assert_eq!(f.handoff(&args).unwrap()["state"], "ready");
        assert!(f.handoff(&f.handoff_args(&f.args())).is_err());
        assert_eq!(f.store.workflow(&f.parent).unwrap(), parent);
        for task in tasks {
            assert_eq!(f.store.task(&text(&task, "id")).unwrap(), task);
        }
        assert_eq!(
            fs::read_to_string(f.source.join("src/b")).unwrap(),
            "new main content\n"
        );
        assert_eq!(
            fs::read_to_string(f.source.join("src/a")).unwrap(),
            if historical {
                "newer conflicting main content\n"
            } else {
                "base\n"
            }
        );
    }
}

fn setup_handoff_seed_drift_and_uncertainty() {
    let f = Fixture::new(false);
    f.historical();
    let args = f.handoff_args(&f.args());
    let mut w = f.handoff(&args).unwrap();
    w["base_revision"] = json!(git_text(&f.source, &["rev-parse", "HEAD"]).unwrap());
    f.store
        .save_workflow(&mut w, "preparing", "fixture reserved seed")
        .unwrap();
    let checkout = teem::checkouts::prepare(
        &f.source,
        &f.tmp.path().join("seed"),
        &text(&f.store.workflow(&f.parent).unwrap(), "base_revision"),
        None,
    )
    .unwrap();
    let spec_path = f.evidence_dir().join("spec.json");
    let spec = fs::read(&spec_path).unwrap();
    let worker = f.store.tasks(None).unwrap().remove(0);
    for case in ["parent", "native", "spec", "paused", "queued", "review"] {
        match case {
            "parent" => {
                f.store
                    .exec(
                        "UPDATE workflows SET detail='changed' WHERE id=?",
                        [&f.parent],
                    )
                    .unwrap();
            }
            "native" => {
                f.store
                    .exec(
                        "UPDATE tasks SET result='changed' WHERE id=?",
                        [text(&worker, "id")],
                    )
                    .unwrap();
            }
            "spec" => {
                let mut changed: Value = serde_json::from_slice(&spec).unwrap();
                changed["token"] = json!(uuid::Uuid::new_v4().to_string());
                fs::write(&spec_path, canonical(&changed)).unwrap();
            }
            "paused" => {
                f.store
                    .exec("INSERT INTO workflow_stops VALUES (?,0)", [&f.parent])
                    .unwrap();
            }
            "queued" => {
                f.store
                    .exec(
                        "UPDATE tasks SET state='queued' WHERE id=?",
                        [text(&worker, "id")],
                    )
                    .unwrap();
            }
            "review" => {
                f.store.exec("UPDATE workflows SET data=json_set(data,'$.rounds[0].review_ids',json('[]')) WHERE id=?", [&f.parent]).unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            teem::reconcile::seed(&f.store, &w, &checkout).is_err(),
            "seed admitted {case}"
        );
        // Restore only this fixture's isolated mutations; never real runtime rows.
        match case {
            "parent" => {
                f.store.exec("UPDATE workflows SET detail='No such file or directory (os error 2)' WHERE id=?", [&f.parent]).unwrap();
            }
            "native" => {
                f.store
                    .exec(
                        "UPDATE tasks SET result=? WHERE id=?",
                        rusqlite::params![text(&worker, "result"), text(&worker, "id")],
                    )
                    .unwrap();
            }
            "spec" => fs::write(&spec_path, &spec).unwrap(),
            "paused" => {
                f.store
                    .exec(
                        "DELETE FROM workflow_stops WHERE workflow_id=?",
                        [&f.parent],
                    )
                    .unwrap();
            }
            "queued" => {
                f.store
                    .exec(
                        "UPDATE tasks SET state='succeeded' WHERE id=?",
                        [text(&worker, "id")],
                    )
                    .unwrap();
            }
            "review" => {
                f.store.exec("UPDATE workflows SET data=json_remove(data,'$.rounds[0].review_ids') WHERE id=?", [&f.parent]).unwrap();
            }
            _ => unreachable!(),
        }
    }
    assert!(
        teem::reconcile::seed(&f.store, &w, &checkout)
            .unwrap()
            .is_some()
    );
    f.store
        .save_workflow(&mut w, "preparing", "fixture interrupted seed")
        .unwrap();
    let reopened = Store::open(&f.store.directory, &[], f.store.projects.clone()).unwrap();
    workflows::tick(&reopened, &f, &mut runner()).unwrap();
    let retained = reopened.workflow(&text(&args, "workflow_id")).unwrap();
    assert_eq!(retained["state"], "uncertain");
    for _ in 0..2 {
        workflows::tick(&reopened, &f, &mut runner()).unwrap();
        assert_eq!(
            teem::reconcile::submit(&reopened, FOREMAN, &args).unwrap(),
            retained
        );
    }
    assert_eq!(f.store.tasks(None).unwrap(), vec![worker]);
    assert!(f.handoff(&f.handoff_args(&f.args())).is_err());
}

#[test]
fn recovery_setup_handoff() {
    if !isolated("recovery_setup_handoff") {
        return;
    }
    setup_handoff_on_advanced_source();
    setup_handoff_seed_drift_and_uncertainty();
}

fn isolated(name: &str) -> bool {
    // Change process-global HOME/XDG only in a dedicated fixture subprocess.
    if std::env::var_os("TEEM_RECOVERY_FIXTURE").is_none() {
        let home = tempfile::tempdir().unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture"])
            .env("TEEM_RECOVERY_FIXTURE", "1")
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path().join("config"))
            .env("XDG_DATA_HOME", home.path().join("data"))
            .env("XDG_STATE_HOME", home.path().join("state"))
            .env("XDG_CACHE_HOME", home.path().join("cache"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return false;
    }
    true
}

#[test]
fn recovery_pass_and_review_findings() {
    if !isolated("recovery_pass_and_review_findings") {
        return;
    }
    passing_and_findings();
}

#[test]
fn recovery_failed_checks() {
    if !isolated("recovery_failed_checks") {
        return;
    }
    failing_checks_stop();
}

#[test]
fn recovery_admission_rejections() {
    if !isolated("recovery_admission_rejections") {
        return;
    }
    rejections();
}

#[test]
fn recovery_historical_proof() {
    if !isolated("recovery_historical_proof") {
        return;
    }
    historical_requires_controller_provenance();
}

#[test]
fn recovery_concurrency_and_restart() {
    if !isolated("recovery_concurrency_and_restart") {
        return;
    }
    concurrency_and_restart();
}

#[test]
fn recovery_uncertain_launches() {
    if !isolated("recovery_uncertain_launches") {
        return;
    }
    uncertain_never_replays();
}

#[test]
fn recovery_changed_evidence() {
    if !isolated("recovery_changed_evidence") {
        return;
    }
    changes_after_reservation_fail_closed();
}

#[test]
fn recovery_explicit_correction() {
    if !isolated("recovery_explicit_correction") {
        return;
    }
    explicit_reconciliation_drops_recovery_only_contract();
}
