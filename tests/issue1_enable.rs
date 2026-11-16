//! Local deterministic enablement contracts, never live services or paid models.
use anyhow::Result;
use rusqlite::params;
use serde_json::{Value, json};
use std::{cell::RefCell, fs, path::Path};
use teem::{
    checkouts,
    common::{canonical, git, git_text, now, text},
    enable, plugin,
    runtime::{self, Runtime},
    store::Store,
};

const CHAT: &str = "ses_issue1";

#[test]
fn explicit_legacy_agent_migration_repairs_dispatch_without_rebinding_or_replay() {
    let f = Fixture::new();
    f.native.session.borrow_mut()["agent"] = json!("teem_foreman");
    let mut request = f.request();
    request["agent"] = json!("teem_foreman");
    enable::enable(&f.store, &f.native, &request, &f.herdr()).unwrap();
    let before = f.store.query("SELECT * FROM chat_enablements", []).unwrap()[0].clone();
    f.native.session.borrow_mut()["agent"] = json!("teem");
    let migration = json!({"sessionID":CHAT,"fromAgent":"teem_foreman","toAgent":"teem"});
    assert!(enable::dispatch_model(&f.store, &f.native, &json!({"foreman":CHAT})).is_err());
    let result = enable::migrate_agent(&f.store, &f.native, &migration).unwrap();
    assert_eq!(result["started"], false);
    let mut expected = before;
    expected["agent"] = json!("teem");
    assert_eq!(
        f.store.query("SELECT * FROM chat_enablements", []).unwrap()[0],
        expected
    );
    assert_eq!(
        enable::dispatch_model(&f.store, &f.native, &json!({"foreman":CHAT})).unwrap(),
        request["model"]
    );
    assert_eq!(
        enable::migrate_agent(&f.store, &f.native, &migration).unwrap(),
        result
    );
    for table in ["tasks", "workflows", "events", "approvals"] {
        assert!(
            f.store
                .query(&format!("SELECT * FROM {table}"), [])
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn explicit_agent_migration_rejects_other_identity_changes() {
    for field in ["agent", "projectID", "directory", "terminal_id", "parentID"] {
        let f = Fixture::new();
        f.native.session.borrow_mut()["agent"] = json!("teem_foreman");
        let mut request = f.request();
        request["agent"] = json!("teem_foreman");
        enable::enable(&f.store, &f.native, &request, &f.herdr()).unwrap();
        let before = f.store.query("SELECT * FROM chat_enablements", []).unwrap();
        f.native.session.borrow_mut()["agent"] = json!("teem");
        match field {
            "directory" => {
                f.native.session.borrow_mut()["location"]["directory"] = json!("/different")
            }
            "terminal_id" => {
                f.store
                    .exec(
                        "UPDATE foremen SET terminal_id='different' WHERE id=?",
                        [CHAT],
                    )
                    .unwrap();
            }
            other => f.native.session.borrow_mut()[other] = json!("different"),
        }
        assert!(
            enable::migrate_agent(
                &f.store,
                &f.native,
                &json!({"sessionID":CHAT,"fromAgent":"teem_foreman","toAgent":"teem"})
            )
            .is_err(),
            "accepted {field}"
        );
        assert_eq!(
            f.store.query("SELECT * FROM chat_enablements", []).unwrap(),
            before
        );
    }
}

struct Native {
    session: RefCell<Value>,
    calls: RefCell<Vec<String>>,
}
impl Runtime for Native {
    fn request(&self, method: &str, path: &str, payload: Option<&Value>) -> Result<Value> {
        assert_eq!(method, "GET");
        assert_eq!(path, format!("/api/session/{CHAT}"));
        assert!(payload.is_none());
        self.calls.borrow_mut().push(path.into());
        Ok(self.session.borrow().clone())
    }
    fn dispatch_model(&self, _: &Value) -> Result<Value> {
        Ok(json!({"providerID":"legacy","id":"fallback"}))
    }
}

struct Fixture {
    directory: tempfile::TempDir,
    store: Store,
    native: Native,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        for name in ["a", "b"] {
            fs::create_dir(directory.path().join(name)).unwrap();
        }
        let store = Store::open(
            &directory.path().join("state"),
            &[],
            json!({"a":{"path":directory.path().join("a"),"checks":[{"name":"tests"}]},
                "b":{"path":directory.path().join("b"),"checks":[{"name":"tests"}]}}),
        )
        .unwrap();
        let native = Native {
            session: RefCell::new(json!({"id":CHAT,"parentID":null,"agent":"build",
                "projectID":"project_a","location":{"directory":directory.path().join("a")},
                "model":{"providerID":"native","id":"previous"}})),
            calls: RefCell::new(Vec::new()),
        };
        Self {
            directory,
            store,
            native,
        }
    }
    fn request(&self) -> Value {
        json!({"sessionID":CHAT,"agent":"build","projectID":"project_a",
            "directory":self.directory.path().join("a"),
            "model":{"providerID":"selected","id":"current","variant":"high"}})
    }
    fn herdr(&self) -> Value {
        json!({"result":{"agent":{"agent":"opencode","terminal_id":"terminal_a",
            "agent_session":{"agent":"opencode","kind":"id","source":"herdr:opencode","value":CHAT}}}})
    }
    fn enable(&self) -> Value {
        enable::enable(&self.store, &self.native, &self.request(), &self.herdr()).unwrap()
    }
    fn call(&self, name: &str, args: &Value) -> Result<Value> {
        enable::call(
            &self.store,
            &self.native,
            &json!({
                "sessionID":CHAT,"agent":"build","name":name,"arguments":args,
            }),
        )
    }
    fn workflow(&self, project: &str) -> Value {
        json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"project":project,
            "issue":"tidley/teem#1","assignment":{"scope":["src"],"prompt":"Scoped issue 1 work"}})
    }
}

#[test]
fn exact_enable_is_idempotent_captures_models_and_starts_nothing() {
    let f = Fixture::new();
    f.store.bind("ses_unrelated", "terminal_other").unwrap();
    let before = f.store.binding("ses_unrelated").unwrap();
    let result = f.enable();
    assert_eq!(result, f.enable());
    assert_eq!(result["started"], false);
    assert_eq!(result["project"], "a");
    let mut request = f.request();
    request["model"]["variant"] = json!("low");
    enable::enable(&f.store, &f.native, &request, &f.herdr()).unwrap();
    let receipt = enable::authority(&f.store, &f.native, CHAT, "build").unwrap();
    assert_eq!(receipt["model"], request["model"]);
    assert_eq!(receipt["native_model"], f.native.session.borrow()["model"]);
    assert_eq!(f.store.binding("ses_unrelated").unwrap(), before);
    assert_eq!(
        f.store
            .query("SELECT * FROM chat_enablements", [])
            .unwrap()
            .len(),
        1
    );
    for table in ["tasks", "workflows", "events", "approvals"] {
        assert!(
            f.store
                .query(&format!("SELECT * FROM {table}"), [])
                .unwrap()
                .is_empty()
        );
    }
    // The fixture accepts only exact-session GETs: writes or model starts panic.
    assert!(!f.native.calls.borrow().is_empty());
}

#[test]
fn loaded_tools_work_without_enablement_or_project_registration() {
    let mut f = Fixture::new();
    f.store.projects = json!({});
    assert!(f.call("foreman_status", &json!({})).is_ok());
    assert!(f.call("foreman_plan", &json!({"items":[{"id":"inspect","content":"Inspect current project","status":"pending"}]})).is_ok());
    assert!(
        f.call("foreman_status", &json!({"section":"progress"}))
            .is_ok()
    );
    assert!(f.store.tasks(None).unwrap().is_empty());
    assert!(f.store.workflows(CHAT).unwrap().is_empty());
    assert!(
        f.store
            .query("SELECT * FROM chat_enablements", [])
            .unwrap()
            .is_empty()
    );
    let error = f.call("foreman_workflow", &f.workflow("a")).unwrap_err();
    assert!(error.to_string().contains("approved checks"));
}

#[test]
fn optional_enablement_does_not_require_project_registration() {
    let mut f = Fixture::new();
    f.store.projects = json!({});
    let result = f.enable();
    assert_eq!(result["started"], false);
    assert!(f.call("foreman_status", &json!({})).is_ok());
    assert!(f.call("foreman_workflow", &f.workflow("a")).is_err());
    assert!(f.store.tasks(None).unwrap().is_empty());
}

#[test]
fn automatic_access_keeps_sessions_isolated_and_rejects_children_and_wrong_agents() {
    let f = Fixture::new();
    f.store.bind("ses_other", "terminal_other").unwrap();
    plugin::call(
        &f.store,
        "ses_other",
        "foreman_plan",
        &json!({"items":[{"id":"private","content":"Other chat plan","status":"pending"}]}),
    )
    .unwrap();
    let original = f.store.binding("ses_other").unwrap();
    assert!(f.call("foreman_status", &json!({})).is_ok());
    let status = f
        .call("foreman_status", &json!({"section":"progress"}))
        .unwrap();
    assert!(!canonical(&status).contains("Other chat plan"));
    assert_eq!(f.store.binding("ses_other").unwrap(), original);
    f.native.session.borrow_mut()["agent"] = json!("plan");
    assert!(f.call("foreman_status", &json!({})).is_err());
    assert!(
        enable::call(
            &f.store,
            &f.native,
            &json!({"sessionID":CHAT,"agent":"plan","name":"foreman_status","arguments":{}})
        )
        .is_ok()
    );
    f.native.session.borrow_mut()["parentID"] = json!("ses_parent");
    assert!(
        enable::call(
            &f.store,
            &f.native,
            &json!({"sessionID":CHAT,"agent":"plan","name":"foreman_status","arguments":{}})
        )
        .is_err()
    );
}

#[test]
fn configured_native_chat_can_delegate_without_enablement() {
    let mut f = Fixture::new();
    repository(&f.directory.path().join("a"));
    let w = f.call("foreman_workflow", &f.workflow("a")).unwrap();
    assert_eq!(w["project"], "a");
    assert!(
        f.store
            .query("SELECT * FROM chat_enablements", [])
            .unwrap()
            .is_empty()
    );
    let b = f.call("foreman_workflow", &f.workflow("b")).unwrap();
    assert_eq!(b["project"], "b");
    assert_eq!(b["foreman"], CHAT);
    assert_eq!(
        enable::dispatch_model(&f.store, &f.native, &json!({"foreman":CHAT})).unwrap(),
        f.native.session.borrow()["model"]
    );
    f.native.session.borrow_mut()["model"] =
        json!({"providerID":"native","id":"changed","variant":"low"});
    assert_eq!(
        enable::dispatch_model(&f.store, &f.native, &json!({"foreman":CHAT})).unwrap(),
        f.native.session.borrow()["model"]
    );
    f.store.projects = json!({});
    assert!(enable::dispatch_model(&f.store, &f.native, &json!({"foreman":CHAT})).is_err());
}

#[test]
fn cross_project_work_progresses_under_the_original_native_home_identity() {
    for (enabled, bound) in [(false, false), (false, true), (true, true)] {
        let f = Fixture::new();
        repository(&f.directory.path().join("b"));
        if bound {
            f.store.bind(CHAT, "terminal_a").unwrap();
        }
        if enabled {
            f.enable();
        }
        let w = f.call("foreman_workflow", &f.workflow("b")).unwrap();
        let mut runner = teem::checks::Runner::with_executable(std::path::PathBuf::from(env!(
            "CARGO_BIN_EXE_teem"
        )));
        teem::workflows::tick(&f.store, &f.native, &mut runner).unwrap();
        let progressed = f.store.workflow(&text(&w, "id")).unwrap();
        assert_eq!(progressed["state"], "working", "{}", progressed["detail"]);
        let tasks = f.store.tasks(None).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["foreman"], CHAT);
        assert_eq!(tasks[0]["request"]["workflow_id"], w["id"]);
        assert_eq!(f.native.session.borrow()["projectID"], "project_a");
    }
}

#[test]
fn automatic_access_preserves_existing_terminal_bindings_and_optional_model_capture() {
    let f = Fixture::new();
    f.store.bind(CHAT, "terminal_a").unwrap();
    f.call("foreman_status", &json!({})).unwrap();
    assert_eq!(f.store.binding(CHAT).unwrap()["terminal_id"], "terminal_a");
    f.enable();
    assert_eq!(
        enable::dispatch_model(&f.store, &f.native, &json!({"foreman":CHAT})).unwrap(),
        f.request()["model"]
    );
}

#[test]
fn optional_enablement_upgrades_only_the_automatic_session_marker() {
    let f = Fixture::new();
    f.call("foreman_status", &json!({})).unwrap();
    assert_eq!(
        f.store.binding(CHAT).unwrap()["terminal_id"],
        format!("native-session:{CHAT}")
    );
    f.enable();
    assert_eq!(f.store.binding(CHAT).unwrap()["terminal_id"], "terminal_a");
    let mut herdr = f.herdr();
    herdr["result"]["agent"]["terminal_id"] = json!("different");
    assert!(enable::enable(&f.store, &f.native, &f.request(), &herdr).is_err());
}

#[test]
fn existing_herdr_bind_action_can_upgrade_automatic_access_without_rebinding() {
    let f = Fixture::new();
    f.call("foreman_status", &json!({})).unwrap();
    f.store.bind(CHAT, "terminal_a").unwrap();
    assert_eq!(f.store.binding(CHAT).unwrap()["terminal_id"], "terminal_a");
    assert!(f.store.bind(CHAT, "terminal_b").is_err());
    assert_eq!(f.store.binding(CHAT).unwrap()["terminal_id"], "terminal_a");
}

#[test]
fn automatic_cross_project_reconciliation_still_checks_candidate_eligibility() {
    let f = Fixture::new();
    f.call("foreman_status", &json!({})).unwrap();
    repository(&f.directory.path().join("b"));
    let source = plugin::call(&f.store, CHAT, "foreman_workflow", &f.workflow("b")).unwrap();
    let workflows = f.store.workflows(CHAT).unwrap();
    let tasks = f.store.tasks(None).unwrap();
    let error = f.call("foreman_reconcile", &json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":source["id"]})).unwrap_err();
    assert!(!error.to_string().contains("enabled project"));
    assert_eq!(f.store.workflows(CHAT).unwrap(), workflows);
    assert_eq!(f.store.tasks(None).unwrap(), tasks);
}

#[test]
fn unconfigured_optional_chat_can_later_enable_its_approved_policy() {
    let mut f = Fixture::new();
    let policies = f.store.projects.clone();
    f.store.projects = json!({});
    f.enable();
    f.store.projects = policies;
    assert_eq!(f.enable()["project"], "a");
    assert_eq!(
        f.store
            .query("SELECT project FROM chat_enablements", [])
            .unwrap()[0]["project"],
        "a"
    );
}

#[test]
fn cross_project_admission_rejects_missing_target_checks_and_foreign_ownership() {
    let mut f = Fixture::new();
    f.enable();
    let source = f.call("foreman_workflow", &f.workflow("b")).unwrap();
    f.store
        .exec(
            "UPDATE workflows SET foreman='ses_other' WHERE id=?",
            [text(&source, "id")],
        )
        .unwrap();
    for tool in ["foreman_reconcile", "foreman_recover_qualification"] {
        let args = if tool == "foreman_reconcile" {
            json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":source["id"]})
        } else {
            json!({"recovery_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":source["id"],"expected_snapshot":"a".repeat(64)})
        };
        assert!(
            f.call(tool, &args)
                .unwrap_err()
                .to_string()
                .contains("another Foreman")
        );
    }
    let before = f.store.query("SELECT * FROM workflows", []).unwrap();
    f.store.projects["b"]["checks"] = json!([]);
    assert!(teem::workflows::normalise_projects(&json!({"b":f.store.projects["b"]})).is_err());
    f.store.projects.as_object_mut().unwrap().remove("b");
    assert!(f.call("foreman_workflow", &f.workflow("b")).is_err());
    assert_eq!(
        f.store.query("SELECT * FROM workflows", []).unwrap(),
        before
    );
    assert!(f.store.tasks(None).unwrap().is_empty());
}

#[test]
fn missing_or_changed_native_identity_herdr_policy_and_model_fail_without_binding() {
    let f = Fixture::new();
    let original = f.native.session.borrow().clone();
    for (pointer, value) in [
        ("/id", json!("ses_other")),
        ("/parentID", json!("ses_parent")),
        ("/parentID", json!("")),
        ("/parentID", json!(false)),
        ("/agent", Value::Null),
        ("/projectID", Value::Null),
        ("/location/directory", json!("relative")),
        ("/model", json!({"id":"missing_provider"})),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        *f.native.session.borrow_mut() = changed;
        assert!(enable::enable(&f.store, &f.native, &f.request(), &f.herdr()).is_err());
    }
    *f.native.session.borrow_mut() = original;
    for (pointer, value) in [
        ("/result/agent/agent_session/value", json!("ses_other")),
        ("/result/agent/agent_session/kind", json!("display_name")),
        ("/result/agent/agent_session/source", json!("guessed")),
        ("/result/agent/terminal_id", json!("")),
    ] {
        let mut herdr = f.herdr();
        *herdr.pointer_mut(pointer).unwrap() = value;
        assert!(enable::enable(&f.store, &f.native, &f.request(), &herdr).is_err());
    }
    for key in ["sessionID", "agent", "projectID", "directory", "model"] {
        let mut request = f.request();
        request[key] = json!("invalid/identity");
        assert!(enable::enable(&f.store, &f.native, &request, &f.herdr()).is_err());
    }
    assert!(
        f.store
            .query("SELECT * FROM foremen", [])
            .unwrap()
            .is_empty()
    );
    assert!(
        f.store
            .query("SELECT * FROM chat_enablements", [])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn top_level_native_chat_can_omit_optional_parent_id() {
    for enveloped in [false, true] {
        let f = Fixture::new();
        f.native
            .session
            .borrow_mut()
            .as_object_mut()
            .unwrap()
            .remove("parentID");
        if enveloped {
            let session = f.native.session.borrow().clone();
            *f.native.session.borrow_mut() = json!({"data":session});
        }
        assert_eq!(f.enable()["started"], false);
        assert!(
            f.call("foreman_status", &json!({"section":"projects","limit":2}))
                .is_ok()
        );
        assert!(f.store.tasks(None).unwrap().is_empty());
    }
}

#[test]
fn policies_and_existing_receipts_cannot_silently_rebind_a_chat() {
    let mut f = Fixture::new();
    let a = f.store.projects["a"].clone();
    f.store.projects["duplicate"] = a.clone();
    assert!(enable::enable(&f.store, &f.native, &f.request(), &f.herdr()).is_err());
    f.store
        .projects
        .as_object_mut()
        .unwrap()
        .remove("duplicate");
    f.store.projects["a"] = a;
    f.enable();
    let before = f.store.query("SELECT * FROM chat_enablements", []).unwrap();
    for key in ["agent", "projectID"] {
        let old = f.native.session.borrow()[key].clone();
        f.native.session.borrow_mut()[key] = json!("changed");
        let mut request = f.request();
        request[key] = json!("changed");
        assert!(enable::enable(&f.store, &f.native, &request, &f.herdr()).is_err());
        f.native.session.borrow_mut()[key] = old;
    }
    let mut herdr = f.herdr();
    herdr["result"]["agent"]["terminal_id"] = json!("different");
    assert!(enable::enable(&f.store, &f.native, &f.request(), &herdr).is_err());
    f.store.projects["renamed"] = f.store.projects["a"].clone();
    f.store.projects.as_object_mut().unwrap().remove("a");
    assert!(enable::enable(&f.store, &f.native, &f.request(), &f.herdr()).is_err());
    assert_eq!(
        f.store.query("SELECT * FROM chat_enablements", []).unwrap(),
        before
    );
}

#[test]
fn dispatch_inherits_selected_model_then_native_changes_without_copying_permissions() {
    let mut f = Fixture::new();
    let task = json!({"foreman":CHAT,"role":"reviewer","request":{"scope":["src"]}});
    assert_eq!(
        enable::dispatch_model(&f.store, &f.native, &task).unwrap()["providerID"],
        "legacy"
    );
    f.enable();
    assert_eq!(
        enable::dispatch_model(&f.store, &f.native, &task).unwrap(),
        f.request()["model"]
    );
    let changed = json!({"providerID":"native","id":"new","variant":"low"});
    f.native.session.borrow_mut()["model"] = changed.clone();
    assert_eq!(
        enable::dispatch_model(&f.store, &f.native, &task).unwrap(),
        changed
    );
    f.native.session.borrow_mut()["permissions"] = json!([{"action":"*","effect":"allow"}]);
    assert!(
        runtime::permissions(&task)
            .as_array()
            .unwrap()
            .iter()
            .all(|p| p["effect"] != "ask" && !(p["action"] == "*" && p["effect"] == "allow"))
    );
    assert!(enable::authority(&f.store, &f.native, CHAT, "other_agent").is_err());
    f.native.session.borrow_mut()["agent"] = json!("other_agent");
    assert!(enable::dispatch_model(&f.store, &f.native, &task).is_err());
    f.native.session.borrow_mut()["agent"] = json!("build");
    f.store.projects["a"]["path"] = f.store.projects["b"]["path"].clone();
    assert!(enable::authority(&f.store, &f.native, CHAT, "build").is_ok());
    assert!(enable::dispatch_model(&f.store, &f.native, &task).is_err());
}

#[test]
fn data_envelopes_and_missing_native_model_use_captured_ui_selection() {
    let f = Fixture::new();
    let mut native = f.native.session.borrow().clone();
    native.as_object_mut().unwrap().remove("model");
    *f.native.session.borrow_mut() = json!({"data":native});
    f.enable();
    assert_eq!(
        enable::dispatch_model(&f.store, &f.native, &json!({"foreman":CHAT})).unwrap(),
        f.request()["model"]
    );
}

#[test]
fn native_tools_allow_configured_cross_project_work_but_reject_legacy_submission() {
    let mut f = Fixture::new();
    assert!(f.call("foreman_status", &json!({})).is_ok());
    f.enable();
    assert!(
        enable::tools()
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["name"] != "teem_submit")
    );
    assert!(
        plugin::tools()
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "teem_submit")
    );
    let a = f.directory.path().join("a");
    let base = repository(&a);
    f.store.allowed.insert(a.to_str().unwrap().into());
    let legacy = json!({"task_id":uuid::Uuid::new_v4().to_string(),"role":"worker",
        "checkout":a,"revision":base,"scope":["src"],"prompt":"Legacy compatibility fixture"});
    assert!(
        f.call("foreman_submit", &legacy)
            .unwrap_err()
            .to_string()
            .contains("legacy")
    );
    assert!(f.store.workflows(CHAT).unwrap().is_empty());
    assert!(f.store.tasks(None).unwrap().is_empty());
    assert!(
        f.call("teem_submit", &legacy)
            .unwrap_err()
            .to_string()
            .contains("legacy")
    );
    let request_b = f.workflow("b");
    let b = f.call("foreman_workflow", &request_b).unwrap();
    assert_eq!(b["project"], "b");
    assert_eq!(b["foreman"], CHAT);
    assert_eq!(f.call("foreman_workflow", &request_b).unwrap(), b);
    assert_eq!(f.call("teem_workflow", &request_b).unwrap(), b);
    assert_eq!(
        enable::authority(&f.store, &f.native, CHAT, "build").unwrap()["project"],
        "a"
    );
    assert_eq!(
        enable::dispatch_model(&f.store, &f.native, &json!({"foreman":CHAT})).unwrap(),
        f.request()["model"]
    );
    let request = f.workflow("a");
    let w = f.call("foreman_workflow", &request).unwrap();
    assert_eq!(w["request"]["issue"], "tidley/teem#1");
    assert_eq!(f.call("foreman_workflow", &request).unwrap(), w);
    assert!(plugin::call(&f.store, CHAT, "foreman_submit", &legacy).is_ok());
}

fn repository(path: &Path) -> String {
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

#[test]
fn native_reconcile_retains_configured_source_project_and_owner() {
    let f = Fixture::new();
    f.enable();
    // A genuine eligible project-B candidate owned by this project-A chat.
    let b = f.directory.path().join("b");
    let base = repository(&b);
    let w = plugin::call(&f.store, CHAT, "foreman_workflow", &f.workflow("b")).unwrap();
    let snapshot = checkouts::capture(
        &b,
        &base,
        &["src".into()],
        &f.directory.path().join("snapshot"),
    )
    .unwrap();
    f.store
        .exec(
            "UPDATE workflows SET state='ready',base_revision=?,data=? WHERE id=?",
            params![
                base,
                canonical(&json!({"rounds":[{"checkout":b,"snapshot":snapshot}]})),
                text(&w, "id")
            ],
        )
        .unwrap();
    let args = json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":w["id"]});
    assert!(
        f.call("foreman_status", &json!({"workflow_id":w["id"]}))
            .is_ok()
    );
    let successor = f.call("foreman_reconcile", &args).unwrap();
    assert_eq!(successor["project"], "b");
    assert_eq!(successor["foreman"], CHAT);
    assert_eq!(successor["request"]["issue"], "tidley/teem#1");
    let mut runner =
        teem::checks::Runner::with_executable(std::path::PathBuf::from(env!("CARGO_BIN_EXE_teem")));
    teem::workflows::tick(&f.store, &f.native, &mut runner).unwrap();
    let progressed = f.store.workflow(&text(&successor, "id")).unwrap();
    assert_eq!(progressed["state"], "working", "{}", progressed["detail"]);
}

#[test]
fn native_recovery_retry_is_read_only_after_policy_removal() {
    let mut f = Fixture::new();
    f.enable();
    // Seed only the durable reservation here: recovery admission/provenance is
    // exercised separately by issue20_recovery's setup-failure fixtures.
    let source = f.call("foreman_workflow", &f.workflow("a")).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let args = json!({"recovery_id":id,"source_workflow_id":source["id"],"expected_snapshot":"a".repeat(64)});
    let mut request = source["request"].clone();
    request["workflow_id"] = json!(id);
    request["qualification_recovery"] = json!({"arguments":args});
    f.store
        .exec(
            "INSERT INTO workflows(id,foreman,project,request,policy,state,created) VALUES (?,?,?,?,?,'qualification_queued',?)",
            params![
                id,
                CHAT,
                "a",
                canonical(&request),
                canonical(&source["policy"]),
                now()
            ],
        )
        .unwrap();
    let retained = f.store.workflow(&id).unwrap();
    f.store.projects = json!({});
    let rows = f.store.query("SELECT * FROM workflows", []).unwrap();
    assert_eq!(
        f.call("foreman_recover_qualification", &args).unwrap(),
        retained
    );
    let mut changed = args.clone();
    changed["expected_snapshot"] = json!("b".repeat(64));
    assert!(f.call("foreman_recover_qualification", &changed).is_err());
    assert!(
        f.call(
            "foreman_recover_qualification",
            &json!({"recovery_id":uuid::Uuid::new_v4().to_string(),"source_workflow_id":source["id"],"expected_snapshot":"a".repeat(64)})
        )
        .is_err()
    );
    assert_eq!(f.store.query("SELECT * FROM workflows", []).unwrap(), rows);
    assert!(f.store.tasks(None).unwrap().is_empty());
}

#[test]
fn cli_enable_uses_native_herdr_subprocess_and_exact_local_http_without_activation() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };
    let f = Fixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let registry = f.directory.path().join("registry.json");
    fs::write(
        &registry,
        canonical(
            &json!({"url":format!("http://{}",listener.local_addr().unwrap()),"password":"test-only"}),
        ),
    )
    .unwrap();
    let config = f.directory.path().join("config.toml");
    let settings = json!({"enabled":false,"state_dir":f.store.directory,"registry":registry,"model":"test/fallback",
        "projects":{"a":{"path":f.directory.path().join("a"),"checks":[{"name":"tests","kind":"test","argv":["cargo","test","--locked"]}]}}});
    fs::write(&config, toml::to_string(&settings).unwrap()).unwrap();
    let session = f.native.session.borrow().clone();
    let server = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "Exact native GET never arrived");
                    thread::sleep(Duration::from_millis(5));
                }
                Err(e) => panic!("{e}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).unwrap();
            request.push(byte[0]);
            assert!(request.len() < 65536);
        }
        assert!(
            String::from_utf8(request)
                .unwrap()
                .starts_with(&format!("GET /api/session/{CHAT} HTTP/1.1\r\n"))
        );
        let body = canonical(&json!({"data":session}));
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_teem"))
        .args(["--config", config.to_str().unwrap(), "enable-chat"])
        .env("HERDR_ENV", "1")
        .env("HERDR_PANE_ID", "issue1-pane")
        .env(
            "HERDR_BIN_PATH",
            env!("CARGO_BIN_EXE_teem-subprocess-fixture"),
        )
        .env("TEEM_FIXTURE_HERDR_RESULT", canonical(&f.herdr()))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(canonical(&f.request()).as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    server.join().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["enabled"], true);
    assert_eq!(result["started"], false);
    assert_eq!(result["sessionID"], CHAT);
    assert!(f.store.tasks(None).unwrap().is_empty());
    assert!(f.store.workflows(CHAT).unwrap().is_empty());
    let output = Command::new(env!("CARGO_BIN_EXE_teem"))
        .args(["--config", config.to_str().unwrap(), "enable-chat"])
        .env_remove("HERDR_ENV")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(canonical(&f.request()).as_bytes())?;
            child.wait_with_output()
        })
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Herdr pane"));
}
