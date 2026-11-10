//! Structured issue contracts only; no network, credentials, or model calls.
use rusqlite::params;
use serde_json::{Value, json};
use teem::{common::canonical, plugin, progress, store::Store};

const FOREMAN: &str = "ses_issue8";
const ISSUE: &str = "tidley/teem#8";

struct Fixture {
    store: Store,
    _directory: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(
            &directory.path().join("state"),
            &[],
            json!({"pilot":{"path":directory.path(),"checks":[{"name":"tests"}],"max_revisions":1}}),
        )
        .unwrap();
        store.bind(FOREMAN, "terminal").unwrap();
        Self {
            store,
            _directory: directory,
        }
    }

    fn request(&self) -> Value {
        json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"project":"pilot",
            "issue":ISSUE,"title":"Track todos","assignment":{"scope":["src"],"prompt":"Implement scoped work"}})
    }

    fn call(&self, args: &Value) -> anyhow::Result<Value> {
        plugin::call(&self.store, FOREMAN, "foreman_workflow", args)
    }
}

#[test]
fn new_managed_admission_requires_explicit_valid_reference_not_prose() {
    let f = Fixture::new();
    let mut missing = f.request();
    missing.as_object_mut().unwrap().remove("issue");
    missing["title"] = json!(ISSUE);
    missing["assignment"]["prompt"] = json!(format!("Implement {ISSUE}"));
    assert!(
        f.call(&missing)
            .unwrap_err()
            .to_string()
            .contains("owner/repo#positive-number")
    );
    for bad in [
        Value::Null,
        json!({"repo":"tidley/teem","number":8}),
        json!(8),
        json!("#8"),
        json!("teem#8"),
        json!("https://github.com/tidley/teem/issues/8"),
        json!("tidley/teem#0"),
        json!("tidley/teem#-1"),
        json!("tidley/teem#1.5"),
        json!("tidley/teem#18446744073709551616"),
        json!("tidley/teem#8\n"),
        json!(" tidley/teem#8"),
        json!("tidley/teem/other#8"),
        json!("tidley/teem#8#9"),
        json!("-tidley/teem#8"),
        json!("tidley/..#8"),
        json!("tidley/#8"),
        json!("ti--dley/teem#8"),
    ] {
        let mut request = f.request();
        request["issue"] = bad;
        assert!(
            f.call(&request)
                .unwrap_err()
                .to_string()
                .contains("owner/repo#positive-number")
        );
    }
    assert!(f.store.workflows(FOREMAN).unwrap().is_empty());
    let request = f.request();
    assert_eq!(f.call(&request).unwrap()["request"]["issue"], ISSUE);
}

#[test]
fn canonical_compact_and_legacy_retries_keep_issue_scope_and_uuid_immutable() {
    let f = Fixture::new();
    let mut request = f.request();
    request["issue"] = json!("Tidley/Teem#0008");
    let admitted = f.call(&request).unwrap();
    assert_eq!(admitted["request"]["issue"], ISSUE);
    assert_eq!(f.call(&request).unwrap(), admitted);
    let mut flat = request.clone();
    let assignment = flat.as_object_mut().unwrap().remove("assignment").unwrap();
    flat.as_object_mut()
        .unwrap()
        .extend(assignment.as_object().unwrap().clone());
    flat["issue"] = json!(ISSUE);
    assert_eq!(f.call(&flat).unwrap(), admitted);
    for (key, value) in [
        ("issue", json!("tidley/teem#9")),
        ("scope", json!(["tests"])),
    ] {
        let mut changed = flat.clone();
        changed[key] = value;
        assert!(
            f.call(&changed)
                .unwrap_err()
                .to_string()
                .contains("immutable")
        );
    }
    flat.as_object_mut().unwrap().remove("issue");
    assert!(f.call(&flat).unwrap_err().to_string().contains("immutable"));
    assert_eq!(
        f.store.workflow(admitted["id"].as_str().unwrap()).unwrap(),
        admitted
    );
}

#[test]
fn historical_retry_and_progress_do_not_fabricate_associations() {
    let f = Fixture::new();
    let mut request = f.request();
    let assignment = request
        .as_object_mut()
        .unwrap()
        .remove("assignment")
        .unwrap();
    request
        .as_object_mut()
        .unwrap()
        .extend(assignment.as_object().unwrap().clone());
    request.as_object_mut().unwrap().remove("issue");
    request["title"] = json!("Historical tidley/teem#8 mention");
    let id = request["workflow_id"].as_str().unwrap();
    f.store
        .exec(
            "INSERT INTO workflows(id,foreman,project,request,policy,created) VALUES (?,?,'pilot',?,?,0)",
            params![
                id,
                FOREMAN,
                canonical(&request),
                canonical(&f.store.projects["pilot"])
            ],
        )
        .unwrap();
    f.store
        .exec("INSERT INTO plans VALUES (?, '[]')", [FOREMAN])
        .unwrap();
    let before = f.store.workflow(id).unwrap();
    assert_eq!(f.call(&request).unwrap(), before);
    let progress = progress::snapshot(&f.store, FOREMAN).unwrap();
    assert!(
        progress["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r.get("issue").is_none())
    );
    assert_eq!(f.store.workflow(id).unwrap(), before);
    request["workflow_id"] = json!(uuid::Uuid::new_v4().to_string());
    assert!(f.call(&request).is_err());
}

#[test]
fn longest_reference_remains_intact_within_existing_outcome_title_bound() {
    let f = Fixture::new();
    let mut request = f.request();
    let issue = format!(
        "{}/{}#18446744073709551615",
        "a".repeat(39),
        "b".repeat(100)
    );
    request["issue"] = json!(issue);
    request["title"] = json!("🦀".repeat(100));
    let w = f.call(&request).unwrap();
    f.store
        .exec(
            "UPDATE workflows SET state='ready' WHERE id=?",
            [w["id"].as_str().unwrap()],
        )
        .unwrap();
    let p = progress::snapshot(&f.store, FOREMAN).unwrap();
    let title = p["latest_result"]["title"].as_str().unwrap();
    assert!(title.starts_with(&format!("{issue} ")));
    assert!(title.encode_utf16().count() <= 300);
}

#[test]
fn every_generated_todo_and_linked_plan_has_one_prefix_without_rewriting_reports() {
    let f = Fixture::new();
    let mut request = f.request();
    request["title"] = json!(format!("Tidley/Teem#0008 {ISSUE} Track todos"));
    let w = f.call(&request).unwrap();
    let worker = uuid::Uuid::new_v4().to_string();
    let reviewer = uuid::Uuid::new_v4().to_string();
    for (id, role) in [(&worker, "worker"), (&reviewer, "reviewer")] {
        let req = json!({"workflow_id":w["id"]});
        f.store
            .exec(
                "INSERT INTO tasks(id,foreman,role,checkout,state,request,session_id,prompt_id,sparse,created) VALUES (?,?,?,'/fixture','running',?,?,?,0,0)",
                params![
                    id,
                    FOREMAN,
                    role,
                    canonical(&req),
                    format!("ses_{id}"),
                    format!("msg_{id}")
                ],
            )
            .unwrap();
        let todos =
            json!([{"content":format!("{ISSUE} {ISSUE} Inspect changes"),"status":"in_progress"}]);
        f.store
            .exec(
                "INSERT INTO agent_todos(task_id,todos) VALUES (?,?)",
                params![id, canonical(&todos)],
            )
            .unwrap();
    }
    let round = json!({"worker_ids":[worker],"review_ids":[reviewer]});
    f.store
        .exec(
            "INSERT INTO approvals(task_id,permission_id,foreman,form_id,snapshot,payload,state) VALUES (?,'permission_exact',?,'form_exact','{}','{}','pending')",
            params![worker, FOREMAN],
        )
        .unwrap();
    let approvals_before = f.store.query("SELECT * FROM approvals", []).unwrap();
    f.store
        .exec(
            "UPDATE workflows SET state='reviewing',data=? WHERE id=?",
            params![
                canonical(&json!({"rounds":[{},round]})),
                w["id"].as_str().unwrap()
            ],
        )
        .unwrap();
    let reports_before = f
        .store
        .query("SELECT * FROM agent_todos ORDER BY task_id", [])
        .unwrap();
    let p = progress::set_plan(
        &f.store,
        FOREMAN,
        &json!([
            {"id":"linked","content":format!("{ISSUE} Check implementation"),"task_id":worker},
            {"id":"manual","content":"Manual unknown","status":"pending"},
            {"id":"missing","content":"Unknown linked task","task_id":uuid::Uuid::new_v4().to_string()}
        ]),
    )
    .unwrap();
    let items = p["items"].as_array().unwrap();
    assert_eq!(items[1]["content"], "Manual unknown");
    assert!(
        items[2]["content"]
            .as_str()
            .unwrap()
            .starts_with("Unknown linked task")
    );
    for item in items
        .iter()
        .filter(|item| item["id"] != "manual" && item["id"] != "missing")
    {
        for key in ["label", "content"] {
            let text = item[key].as_str().unwrap();
            assert!(text.starts_with(&format!("{ISSUE} ")), "{item}");
            assert_eq!(text.matches(ISSUE).count(), 1, "{item}");
        }
    }
    assert!(items.iter().any(|r| r["sidebar"]["kind"] == "check"));
    assert_eq!(
        items
            .iter()
            .filter(|r| r["sidebar"]["kind"] == "review")
            .count(),
        2
    );
    assert_eq!(
        items
            .iter()
            .filter(|r| r["source"] == "agent-reported")
            .count(),
        2
    );
    assert_eq!(
        f.store
            .query("SELECT * FROM agent_todos ORDER BY task_id", [])
            .unwrap(),
        reports_before
    );
    let status = plugin::call(&f.store, FOREMAN, "foreman_status", &json!({})).unwrap();
    assert!(serde_json::to_string_pretty(&status).unwrap().len() <= 24000);
    assert_eq!(f.store.task(&worker).unwrap()["id"], worker);
    assert_eq!(status["approvals"][0]["task_id"], worker);
    assert_eq!(status["approvals"][0]["permission_id"], "permission_exact");
    assert_eq!(status["approvals"][0]["form_id"], "form_exact");
    assert_eq!(
        f.store.query("SELECT * FROM approvals", []).unwrap(),
        approvals_before
    );
}
