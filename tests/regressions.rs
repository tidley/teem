//! Native replacements for the prototype's state, notification and approval tests.
use anyhow::Result;
use rusqlite::params;
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    fs,
};
use teem::{
    approvals,
    common::*,
    plugin, progress,
    runtime::{self, Runtime},
    store::Store,
};

struct Fixture {
    store: Store,
    _directory: tempfile::TempDir,
    task: String,
    runtime: Agent,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(&directory.path().join("state"), &[], json!({})).unwrap();
        store.bind("ses_test", "terminal").unwrap();
        store.bind("ses_other", "other-terminal").unwrap();
        let task = uuid::Uuid::new_v4().to_string();
        store.exec("INSERT INTO tasks(id,foreman,role,checkout,state,request,session_id,prompt_id,sparse,created) VALUES (?, 'ses_test','worker','/example/worker','running','{}','ses_worker','msg_worker',0,0)",[&task]).unwrap();
        Self {
            store,
            _directory: directory,
            task,
            runtime: Agent::default(),
        }
    }
    fn reopen(&mut self) {
        self.store = Store::open(&self.store.directory, &[], json!({})).unwrap();
    }
    fn approve(&self) {
        approvals::deliver(&self.store, &self.runtime).unwrap();
    }
    fn answer(&self, decision: &str) {
        for form in self.runtime.forms.borrow_mut().values_mut() {
            form["state"] = json!({"status":"answered","answer":{"decision":decision}});
        }
    }
    fn row(&self) -> Value {
        self.store
            .query("SELECT * FROM approvals ORDER BY rowid", [])
            .unwrap()
            .remove(0)
    }
    fn plan(&self) -> Value {
        progress::set_plan(&self.store,"ses_test",&json!([{"id":"worker","content":"Implementation","task_id":self.task},{"id":"verify","content":"Verify result","status":"pending"}])).unwrap()
    }
    fn state(&self, state: &str) {
        self.store
            .exec(
                "UPDATE tasks SET state=? WHERE id=?",
                params![state, self.task],
            )
            .unwrap();
    }
}
struct Agent {
    calls: RefCell<Vec<(String, String, Value)>>,
    pending: RefCell<Vec<Value>>,
    forms: RefCell<BTreeMap<String, Value>>,
    fail_create: Cell<bool>,
    fail_reply: Cell<bool>,
    lose_reply: Cell<bool>,
    fail_reads: Cell<bool>,
    sends: Cell<usize>,
    receipt_reads: Cell<usize>,
    fail_send: Cell<bool>,
    accepted: Cell<bool>,
    todos: RefCell<Option<Value>>,
    todo_reads: Cell<usize>,
    fail_todos: Cell<bool>,
}
impl Default for Agent {
    fn default() -> Self {
        Self {
            calls: RefCell::new(vec![]),
            pending: RefCell::new(vec![
                json!({"permission":{"id":"per_check","sessionID":"ses_worker","action":"shell","resources":["git status"],"source":{"type":"tool","messageID":"msg_tool","id":"call_tool"}},"input":{"command":"git status","workdir":"/example/worker"}}),
            ]),
            forms: RefCell::new(BTreeMap::new()),
            fail_create: Cell::new(false),
            fail_reply: Cell::new(false),
            lose_reply: Cell::new(false),
            fail_reads: Cell::new(false),
            sends: Cell::new(0),
            receipt_reads: Cell::new(0),
            fail_send: Cell::new(false),
            accepted: Cell::new(false),
            todos: RefCell::new(None),
            todo_reads: Cell::new(0),
            fail_todos: Cell::new(false),
        }
    }
}
impl Agent {
    fn posts(&self, suffix: &str) -> usize {
        self.calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p.ends_with(suffix))
            .count()
    }
    fn replies(&self) -> Vec<Value> {
        self.calls
            .borrow()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p.ends_with("/reply"))
            .map(|(_, _, v)| v.clone())
            .collect()
    }
}
impl Runtime for Agent {
    fn request(&self, m: &str, path: &str, p: Option<&Value>) -> Result<Value> {
        self.calls
            .borrow_mut()
            .push((m.into(), path.into(), p.cloned().unwrap_or(Value::Null)));
        if m == "POST" && path.ends_with("/form") {
            let mut form = p.unwrap().clone();
            form["sessionID"] = json!("ses_test");
            form["state"] = json!({"status":"pending"});
            self.forms
                .borrow_mut()
                .insert(text(&form, "id"), form.clone());
            if self.fail_create.get() {
                anyhow::bail!("Lost create response")
            }
            return Ok(form);
        }
        if m == "GET" {
            if self.fail_reads.get() {
                anyhow::bail!("Unavailable form read")
            }
            return self
                .forms
                .borrow()
                .get(path.rsplit('/').next().unwrap())
                .cloned()
                .ok_or_else(|| runtime::Missing.into());
        }
        if m == "DELETE" {
            if let Some(form) = self
                .forms
                .borrow_mut()
                .get_mut(path.rsplit('/').next().unwrap())
            {
                form["state"] = json!({"status":"cancelled"});
            }
            return Ok(Value::Null);
        }
        if m == "POST" && path.ends_with("/reply") {
            if self.fail_reply.get() {
                anyhow::bail!("Lost reply response")
            }
            let id = path.split('/').nth_back(1).unwrap();
            if p.unwrap()["decision"] == "reject" {
                self.pending.borrow_mut().clear();
            } else {
                self.pending
                    .borrow_mut()
                    .retain(|v| v["permission"]["id"] != id);
            }
            if self.lose_reply.get() {
                anyhow::bail!("Accepted reply, response lost")
            }
            return Ok(Value::Null);
        }
        anyhow::bail!("Unexpected fixture request")
    }
    fn approvals(&self, _: &Value) -> Result<Vec<Value>> {
        if self.fail_reads.get() {
            anyhow::bail!("Native read unavailable")
        }
        Ok(self.pending.borrow().clone())
    }
    fn notify(&self, _: &Value) -> Result<()> {
        self.sends.set(self.sends.get() + 1);
        if self.fail_send.get() {
            anyhow::bail!("Lost notification response")
        }
        Ok(())
    }
    fn accepted(&self, _: &Value) -> Result<bool> {
        self.receipt_reads.set(self.receipt_reads.get() + 1);
        Ok(self.accepted.get())
    }
    fn todos(&self, _: &Value) -> Result<Option<Value>> {
        self.todo_reads.set(self.todo_reads.get() + 1);
        if self.fail_todos.get() {
            anyhow::bail!("Read unavailable")
        }
        Ok(self.todos.borrow().clone())
    }
}

#[test]
fn approval_card_is_unique_exact_owner_and_has_no_permanent_option() {
    let f = Fixture::new();
    f.approve();
    f.approve();
    assert_eq!(f.runtime.posts("/form"), 1);
    let forms = f.runtime.forms.borrow();
    let form = forms.values().next().unwrap();
    assert_eq!(form["sessionID"], "ses_test");
    assert_eq!(
        form["fields"][0]["options"],
        json!([{"value":"once","label":"Approve once"},{"value":"reject","label":"Reject worker batch"}])
    );
    assert!(form["fields"][0].get("default").is_none());
    assert!(text(&form["fields"][0], "description").contains("/example/worker"));
    assert!(f.runtime.replies().is_empty());
}
#[test]
fn approval_once_is_sent_to_original_permission_once() {
    let f = Fixture::new();
    f.approve();
    f.answer("once");
    f.approve();
    f.approve();
    assert_eq!(f.runtime.replies(), vec![json!({"decision":"once"})]);
    assert!(
        f.runtime
            .calls
            .borrow()
            .iter()
            .any(|(_, p, _)| p == "/api/session/ses_worker/permission/per_check/reply")
    );
}
#[test]
fn permanent_approval_is_rejected() {
    let f = Fixture::new();
    f.approve();
    f.answer("always");
    f.approve();
    assert!(f.runtime.replies().is_empty());
    assert_eq!(f.row()["state"], "uncertain");
}
#[test]
fn human_rejection_is_forwarded() {
    let f = Fixture::new();
    f.approve();
    f.answer("reject");
    f.approve();
    assert_eq!(f.runtime.replies(), vec![json!({"decision":"reject"})]);
}
#[test]
fn replacement_permission_does_not_inherit_old_answer() {
    let f = Fixture::new();
    f.approve();
    f.answer("once");
    f.runtime.pending.borrow_mut()[0]["permission"]["id"] = json!("per_replacement");
    f.approve();
    assert!(f.runtime.replies().is_empty());
}
#[test]
fn changed_permission_with_same_id_does_not_inherit_answer() {
    let f = Fixture::new();
    f.approve();
    f.answer("once");
    f.runtime.pending.borrow_mut()[0]["input"]["command"] = json!("git push");
    f.approve();
    assert!(f.runtime.replies().is_empty());
    assert_eq!(f.row()["state"], "stale");
}
#[test]
fn vanished_permission_cancels_native_card() {
    let f = Fixture::new();
    f.approve();
    f.runtime.pending.borrow_mut().clear();
    f.approve();
    assert_eq!(
        f.runtime.forms.borrow().values().next().unwrap()["state"]["status"],
        "cancelled"
    );
}
#[test]
fn lost_card_creation_response_reconciles_without_reposting() {
    let f = Fixture::new();
    f.runtime.fail_create.set(true);
    f.approve();
    f.approve();
    assert_eq!(f.runtime.posts("/form"), 1);
    f.answer("once");
    f.approve();
    assert_eq!(f.runtime.replies().len(), 1);
}
#[test]
fn lost_permission_reply_never_repeats_after_database_reopen() {
    let mut f = Fixture::new();
    f.approve();
    f.answer("once");
    f.runtime.fail_reply.set(true);
    f.approve();
    f.reopen();
    f.approve();
    assert_eq!(f.runtime.replies().len(), 1);
    assert_eq!(f.row()["reply_attempted"], 1);
}
#[test]
fn wrong_form_owner_cannot_authorise() {
    let f = Fixture::new();
    f.approve();
    f.answer("once");
    for form in f.runtime.forms.borrow_mut().values_mut() {
        form["sessionID"] = json!("ses_other");
    }
    f.approve();
    assert!(f.runtime.replies().is_empty());
}
#[test]
fn altered_form_cannot_authorise() {
    let f = Fixture::new();
    f.approve();
    f.answer("once");
    for form in f.runtime.forms.borrow_mut().values_mut() {
        form["fields"] = json!([]);
    }
    f.approve();
    assert!(f.runtime.replies().is_empty());
}
#[test]
fn completed_task_invalidates_its_permission_card() {
    let f = Fixture::new();
    f.approve();
    f.answer("once");
    f.store.update(&f.task, "succeeded", "done", "").unwrap();
    f.approve();
    assert!(f.runtime.replies().is_empty());
}
#[test]
fn evicted_card_restores_same_id_without_reusing_answer() {
    let f = Fixture::new();
    f.approve();
    let id = f.runtime.forms.borrow().keys().next().unwrap().clone();
    f.runtime.forms.borrow_mut().clear();
    f.approve();
    assert_eq!(f.runtime.forms.borrow().keys().next().unwrap(), &id);
    assert_eq!(f.runtime.posts("/form"), 2);
    assert!(f.runtime.replies().is_empty());
    f.answer("once");
    f.approve();
    assert_eq!(f.runtime.replies().len(), 1);
}
#[test]
fn missing_card_after_possible_reply_is_never_restored() {
    let mut f = Fixture::new();
    f.approve();
    f.answer("once");
    f.runtime.fail_reply.set(true);
    f.approve();
    f.reopen();
    f.runtime.forms.borrow_mut().clear();
    f.approve();
    f.approve();
    assert_eq!(f.runtime.posts("/form"), 1);
    assert!(f.runtime.forms.borrow().is_empty());
    assert_eq!(f.runtime.replies().len(), 1);
}
#[test]
fn missing_card_with_changed_request_is_not_restored() {
    let f = Fixture::new();
    f.approve();
    f.runtime.forms.borrow_mut().clear();
    f.runtime.pending.borrow_mut()[0]["permission"]["resources"] = json!(["changed"]);
    f.approve();
    assert_eq!(f.runtime.posts("/form"), 1);
}
#[test]
fn failed_card_restoration_backs_off() {
    let f = Fixture::new();
    f.approve();
    f.runtime.forms.borrow_mut().clear();
    f.runtime.fail_create.set(true);
    f.approve();
    f.runtime.forms.borrow_mut().clear();
    f.approve();
    assert_eq!(f.runtime.posts("/form"), 2);
    assert!(f.runtime.replies().is_empty());
}
#[test]
fn once_preserves_other_pending_permissions() {
    let f = Fixture::new();
    let mut second = f.runtime.pending.borrow()[0].clone();
    second["permission"]["id"] = json!("per_second");
    f.runtime.pending.borrow_mut().push(second);
    f.approve();
    for form in f.runtime.forms.borrow_mut().values_mut() {
        if form["metadata"]["teem_permission"] == "per_check" {
            form["state"] = json!({"status":"answered","answer":{"decision":"once"}});
        }
    }
    f.approve();
    assert_eq!(f.runtime.pending.borrow().len(), 1);
    assert_eq!(
        f.runtime.pending.borrow()[0]["permission"]["id"],
        "per_second"
    );
}
#[test]
fn reject_warns_of_batch_scope_and_cancels_other_cards() {
    let f = Fixture::new();
    let mut second = f.runtime.pending.borrow()[0].clone();
    second["permission"]["id"] = json!("per_second");
    f.runtime.pending.borrow_mut().push(second);
    f.approve();
    for form in f.runtime.forms.borrow_mut().values_mut() {
        if form["metadata"]["teem_permission"] == "per_check" {
            assert!(text(&form["fields"][0], "description").contains("all pending permissions"));
            form["state"] = json!({"status":"answered","answer":{"decision":"reject"}});
        }
    }
    f.approve();
    f.approve();
    assert!(f.runtime.pending.borrow().is_empty());
    assert_eq!(f.runtime.replies().len(), 1);
    let forms = f.runtime.forms.borrow();
    let other = forms
        .values()
        .find(|v| v["metadata"]["teem_permission"] == "per_second")
        .unwrap();
    assert_eq!(other["state"]["status"], "cancelled");
}
#[test]
fn accepted_reply_with_lost_response_is_not_repeated() {
    let mut f = Fixture::new();
    f.approve();
    f.answer("once");
    f.runtime.lose_reply.set(true);
    f.approve();
    f.reopen();
    f.approve();
    assert!(f.runtime.pending.borrow().is_empty());
    assert_eq!(f.runtime.replies().len(), 1);
}
#[test]
fn pending_card_survives_reopened_store() {
    let mut f = Fixture::new();
    f.approve();
    f.reopen();
    f.approve();
    assert_eq!(f.runtime.posts("/form"), 1);
    f.answer("once");
    f.approve();
    assert_eq!(f.runtime.replies().len(), 1);
}
#[test]
fn failed_native_read_cannot_cancel_or_approve_a_card() {
    let f = Fixture::new();
    f.approve();
    f.runtime.fail_reads.set(true);
    f.approve();
    assert_eq!(f.row()["state"], "pending");
    assert!(f.runtime.replies().is_empty());
}
#[test]
fn dismissed_card_is_not_permission_rejection() {
    let f = Fixture::new();
    f.approve();
    for form in f.runtime.forms.borrow_mut().values_mut() {
        form["state"] = json!({"status":"cancelled"});
    }
    f.approve();
    assert_eq!(f.row()["state"], "cancelled");
    assert_eq!(f.runtime.pending.borrow().len(), 1);
    assert!(f.runtime.replies().is_empty());
}

#[test]
fn completion_is_one_durable_event_with_one_bounded_result() {
    let f = Fixture::new();
    f.store
        .update(&f.task, "succeeded", "finished", "")
        .unwrap();
    f.store.update(&f.task, "failed", "changed", "").unwrap();
    let events = f.store.query("SELECT * FROM events", []).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(f.store.task(&f.task).unwrap()["result"], "finished");
    assert_eq!(f.store.task(&f.task).unwrap()["state"], "succeeded");
}
#[test]
fn native_queue_admission_is_not_foreman_consumption() {
    let f = Fixture::new();
    f.store.update(&f.task, "succeeded", "done", "").unwrap();
    runtime::notify(&f.store, &f.runtime).unwrap();
    let row = f.store.query("SELECT * FROM events", []).unwrap().remove(0);
    assert_eq!(row["state"], "queued");
    assert_eq!(row["acked"], 0);
    runtime::notify(&f.store, &f.runtime).unwrap();
    assert_eq!(f.runtime.sends.get(), 1);
}
#[test]
fn unknown_completion_send_uses_receipt_reads_after_reopen() {
    let mut f = Fixture::new();
    f.store.update(&f.task, "succeeded", "done", "").unwrap();
    f.runtime.fail_send.set(true);
    runtime::notify(&f.store, &f.runtime).unwrap();
    f.reopen();
    f.store
        .exec("UPDATE events SET next_receipt=0", [])
        .unwrap();
    f.runtime.accepted.set(true);
    runtime::notify(&f.store, &f.runtime).unwrap();
    assert_eq!(f.runtime.sends.get(), 1);
    assert_eq!(f.runtime.receipt_reads.get(), 1);
    assert_eq!(
        f.store.query("SELECT state FROM events", []).unwrap()[0]["state"],
        "queued"
    );
}
#[test]
fn attempting_notification_is_reconciled_by_reads_only() {
    let f = Fixture::new();
    f.store.update(&f.task, "succeeded", "done", "").unwrap();
    f.store
        .exec("UPDATE events SET state='attempting'", [])
        .unwrap();
    runtime::notify(&f.store, &f.runtime).unwrap();
    assert_eq!(f.runtime.sends.get(), 0);
    assert_eq!(f.runtime.receipt_reads.get(), 1);
}
#[test]
fn uncertain_notification_reads_back_off_across_reopen() {
    let mut f = Fixture::new();
    f.store.update(&f.task, "succeeded", "done", "").unwrap();
    f.store
        .exec("UPDATE events SET state='uncertain'", [])
        .unwrap();
    runtime::notify(&f.store, &f.runtime).unwrap();
    f.reopen();
    runtime::notify(&f.store, &f.runtime).unwrap();
    assert_eq!(f.runtime.receipt_reads.get(), 1);
}
#[test]
fn acknowledged_and_legacy_notifications_are_never_resent() {
    for legacy in [false, true] {
        let f = Fixture::new();
        f.store.update(&f.task, "succeeded", "done", "").unwrap();
        if legacy {
            f.store
                .exec("UPDATE events SET transport='legacy-codr'", [])
                .unwrap();
        } else {
            let e = f.store.query("SELECT * FROM events", []).unwrap().remove(0);
            f.store.ack("ses_test", &text(&e, "id")).unwrap();
        }
        runtime::notify(&f.store, &f.runtime).unwrap();
        assert_eq!(f.runtime.sends.get(), 0);
        assert_eq!(f.runtime.receipt_reads.get(), 0);
    }
}
#[test]
fn unknown_old_receipts_do_not_starve_new_completion_events() {
    let f = Fixture::new();
    for _ in 0..12 {
        let id = uuid::Uuid::new_v4().to_string();
        f.store.exec("INSERT INTO events(id,task_id,foreman,request_id,state) VALUES (?,?,'ses_test',?,'uncertain')",params![id,format!("old:{id}"),uuid::Uuid::new_v4().to_string()]).unwrap();
    }
    f.store.update(&f.task, "succeeded", "done", "").unwrap();
    runtime::notify(&f.store, &f.runtime).unwrap();
    assert_eq!(f.runtime.sends.get(), 1);
}
#[test]
fn foreign_event_ack_is_rejected() {
    let f = Fixture::new();
    f.store.update(&f.task, "succeeded", "done", "").unwrap();
    let e = f.store.query("SELECT * FROM events", []).unwrap().remove(0);
    assert!(f.store.ack("ses_other", &text(&e, "id")).is_err());
}

#[test]
fn plan_status_is_supervisor_owned_and_verification_remains_manual() {
    let f = Fixture::new();
    f.plan();
    f.store
        .update(&f.task, "succeeded", "worker claims tests pass", "")
        .unwrap();
    let p = progress::snapshot(&f.store, "ses_test").unwrap();
    assert_eq!(p["todos"][0]["status"], "completed");
    assert!(text(&p["todos"][0], "content").contains("tests/review not implied"));
    assert_eq!(p["todos"][1]["status"], "pending");
}
#[test]
fn uncertain_failed_and_interrupted_execution_never_become_success() {
    let f = Fixture::new();
    f.plan();
    for (state, status) in [
        ("uncertain", "in_progress"),
        ("failed", "cancelled"),
        ("interrupted", "cancelled"),
    ] {
        f.state(state);
        assert_eq!(
            progress::snapshot(&f.store, "ses_test").unwrap()["todos"][0]["status"],
            status
        );
    }
}
#[test]
fn plans_reject_duplicate_ids_linked_status_and_foreign_tasks() {
    let f = Fixture::new();
    for items in [
        json!([{"id":"x","content":"x","task_id":f.task,"status":"completed"}]),
        json!([{"id":"x","content":"a","status":"pending"},{"id":"x","content":"b","status":"pending"}]),
    ] {
        assert!(progress::set_plan(&f.store, "ses_test", &items).is_err());
    }
    f.store
        .exec("UPDATE tasks SET foreman='ses_other'", [])
        .unwrap();
    assert!(
        progress::set_plan(
            &f.store,
            "ses_test",
            &json!([{"id":"x","content":"x","task_id":f.task}])
        )
        .is_err()
    );
}
#[test]
fn future_plan_link_does_not_leak_foreign_task_details() {
    let f = Fixture::new();
    let future = uuid::Uuid::new_v4().to_string();
    progress::set_plan(
        &f.store,
        "ses_other",
        &json!([{"id":"future","content":"future","task_id":future}]),
    )
    .unwrap();
    f.store
        .exec("UPDATE tasks SET id=?,detail='private result'", [future])
        .unwrap();
    let p = progress::snapshot(&f.store, "ses_other").unwrap();
    assert_eq!(p["todos"][0]["status"], "pending");
    assert!(!p.to_string().contains("private result"));
}
#[test]
fn unsubmitted_task_stays_pending_after_reopen() {
    let mut f = Fixture::new();
    let future = uuid::Uuid::new_v4().to_string();
    progress::set_plan(
        &f.store,
        "ses_test",
        &json!([{"id":"future","content":"future","task_id":future}]),
    )
    .unwrap();
    f.reopen();
    assert_eq!(
        progress::snapshot(&f.store, "ses_test").unwrap()["todos"][0]["status"],
        "pending"
    );
}
#[test]
fn each_foreman_has_independent_progress_and_clear_is_explicit() {
    let f = Fixture::new();
    f.plan();
    progress::set_plan(
        &f.store,
        "ses_other",
        &json!([{"id":"other","content":"Other work","status":"pending"}]),
    )
    .unwrap();
    progress::publish(&f.store).unwrap();
    let projection = read_json(&f.store.directory.join("progress.json")).unwrap();
    assert_eq!(projection["sessions"].as_object().unwrap().len(), 2);
    progress::set_plan(&f.store, "ses_test", &json!([])).unwrap();
    assert_eq!(
        progress::snapshot(&f.store, "ses_test").unwrap()["todos"],
        json!([])
    );
    assert_eq!(
        progress::snapshot(&f.store, "ses_other").unwrap()["todos"][0]["content"],
        "Other work"
    );
}
#[test]
fn publishing_progress_is_private_silent_and_preserves_old_admission_history() {
    use std::os::unix::fs::PermissionsExt;
    let mut f = Fixture::new();
    f.plan();
    for state in ["pending", "queued", "uncertain"] {
        f.store.exec("INSERT INTO progress_events(foreman,fingerprint,snapshot,request_id,state) VALUES ('ses_test',?,'{}',?,?)",params![state,uuid::Uuid::new_v4().to_string(),state]).unwrap();
    }
    progress::publish(&f.store).unwrap();
    f.reopen();
    progress::publish(&f.store).unwrap();
    let states = f
        .store
        .query("SELECT state FROM progress_events ORDER BY seq", [])
        .unwrap();
    assert_eq!(
        states,
        json!([{"state":"superseded"},{"state":"queued"},{"state":"uncertain"}])
            .as_array()
            .unwrap()
            .clone()
    );
    assert_eq!(
        fs::metadata(f.store.directory.join("progress.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(f.runtime.calls.borrow().is_empty());
    assert_eq!(f.runtime.sends.get(), 0);
}
#[test]
fn no_plan_does_not_override_native_todos() {
    let f = Fixture::new();
    assert!(progress::snapshot(&f.store, "ses_test").unwrap().is_null());
    progress::publish(&f.store).unwrap();
    assert_eq!(
        read_json(&f.store.directory.join("progress.json")).unwrap()["sessions"],
        json!({})
    );
}
#[test]
fn human_permission_wait_is_visible_in_progress() {
    let f = Fixture::new();
    f.plan();
    f.store
        .exec(
            "UPDATE tasks SET detail='Requires human permission or answer'",
            [],
        )
        .unwrap();
    assert!(
        text(
            &progress::snapshot(&f.store, "ses_test").unwrap()["todos"][0],
            "content"
        )
        .contains("human")
    );
}

fn report_task(f: &Fixture) {
    f.store
        .exec(
            "UPDATE tasks SET request=?",
            [json!({"native_todos":true}).to_string()],
        )
        .unwrap();
    *f.runtime.todos.borrow_mut() = Some(
        json!({"message_id":"msg_report","todos":[{"content":"Reported work","status":"in_progress"}]}),
    );
}
#[test]
fn native_agent_reports_are_persisted_and_throttled() {
    let f = Fixture::new();
    report_task(&f);
    progress::read_agents(&f.store, &f.runtime).unwrap();
    progress::read_agents(&f.store, &f.runtime).unwrap();
    assert_eq!(f.runtime.todo_reads.get(), 1);
    let row = f
        .store
        .query("SELECT * FROM agent_todos", [])
        .unwrap()
        .remove(0);
    assert_eq!(row["message_id"], "msg_report");
}
#[test]
fn failed_report_read_preserves_last_observation() {
    let f = Fixture::new();
    report_task(&f);
    progress::read_agents(&f.store, &f.runtime).unwrap();
    f.runtime.fail_todos.set(true);
    f.store
        .exec("UPDATE agent_todos SET next_read=0", [])
        .unwrap();
    progress::read_agents(&f.store, &f.runtime).unwrap();
    let row = f
        .store
        .query("SELECT * FROM agent_todos", [])
        .unwrap()
        .remove(0);
    assert!(text(&row, "todos").contains("Reported work"));
    assert!(text(&row, "detail").contains("unavailable"));
}
#[test]
fn terminal_report_is_read_once_and_explicit_empty_list_clears_it() {
    let f = Fixture::new();
    report_task(&f);
    progress::read_agents(&f.store, &f.runtime).unwrap();
    f.state("succeeded");
    *f.runtime.todos.borrow_mut() = Some(json!({"message_id":"msg_final_report","todos":[]}));
    f.store
        .exec("UPDATE agent_todos SET next_read=0", [])
        .unwrap();
    progress::read_agents(&f.store, &f.runtime).unwrap();
    progress::read_agents(&f.store, &f.runtime).unwrap();
    let row = f
        .store
        .query("SELECT * FROM agent_todos", [])
        .unwrap()
        .remove(0);
    assert_eq!(row["todos"], "[]");
    assert_eq!(row["final"], 1);
    assert_eq!(f.runtime.todo_reads.get(), 2);
}
#[test]
fn legacy_task_without_todo_capability_is_not_read() {
    let f = Fixture::new();
    progress::read_agents(&f.store, &f.runtime).unwrap();
    assert_eq!(f.runtime.todo_reads.get(), 0);
    assert!(
        f.store
            .query("SELECT * FROM agent_todos", [])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn mcp_rejects_foreign_bindings_and_unrecognised_arguments() {
    let f = Fixture::new();
    assert!(
        plugin::mcp(
            &f.store,
            "ses_unknown",
            std::io::Cursor::new(b""),
            Vec::new()
        )
        .is_err()
    );
    for (name, args) in [
        ("foreman_status", json!({"foreman":"ses_other"})),
        ("foreman_submit", json!({})),
        ("missing", json!({})),
        ("foreman_ack", json!({"event_id":"missing"})),
    ] {
        assert!(plugin::call(&f.store, "ses_test", name, &args).is_err());
    }
}
#[test]
fn mcp_invalid_requests_do_not_break_following_requests() {
    let f = Fixture::new();
    let input=b"not-json\n[]\n{\"id\":1,\"method\":\"ping\",\"params\":[]}\n{\"id\":2,\"method\":\"missing\"}\n{\"id\":3,\"method\":\"ping\"}\n";
    let mut out = Vec::new();
    plugin::mcp(&f.store, "ses_test", std::io::Cursor::new(input), &mut out).unwrap();
    let lines = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str::<Value>(s).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 5);
    assert_eq!(lines[0]["error"]["code"], -32700);
    assert_eq!(lines[2]["error"]["code"], -32602);
    assert_eq!(lines[3]["error"]["code"], -32601);
    assert_eq!(lines[4]["result"], json!({}));
}
#[test]
fn task_and_workflow_status_never_expose_other_foreman_data() {
    let f = Fixture::new();
    assert!(
        f.store
            .status("ses_other", &json!({"task_id":f.task}))
            .is_err()
    );
    assert!(
        f.store
            .status("ses_test", &json!({"task_id":f.task,"workflow_id":"both"}))
            .is_err()
    );
}
#[test]
fn configuration_disabled_mode_needs_no_service_credentials() {
    let f = Fixture::new();
    let path = f._directory.path().join("config.toml");
    fs::write(
        &path,
        format!(
            "enabled=false\nstate_dir={:?}\n",
            f.store.directory.to_str().unwrap()
        ),
    )
    .unwrap();
    let c = plugin::load(&path).unwrap();
    assert_eq!(c["enabled"], false);
    for extra in [
        "enabled=\"yes\"\nstate_dir=\"/x\"\n",
        "enabled=false\nstate_dir=\"relative\"\n",
        "enabled=true\nstate_dir=\"/x\"\n",
    ] {
        fs::write(&path, extra).unwrap();
        assert!(plugin::load(&path).is_err());
    }
}
#[test]
fn coordinator_profile_and_manifest_keep_native_entrypoints_and_no_worker_panes() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest: toml::Value =
        toml::from_str(&fs::read_to_string(root.join("herdr-plugin.toml")).unwrap()).unwrap();
    assert_eq!(manifest["id"].as_str(), Some("local.teem"));
    for kind in ["startup", "actions", "panes"] {
        for entry in manifest[kind].as_array().unwrap() {
            assert_eq!(entry["command"][0].as_str(), Some("bin/teem"));
        }
    }
    assert_eq!(manifest["panes"].as_array().unwrap().len(), 1);
    assert_eq!(manifest["panes"][0]["placement"].as_str(), Some("popup"));
    let profile = read_json(&root.join("examples/pilot-foreman.json")).unwrap();
    assert_eq!(profile["default_agent"], "teem_foreman");
    assert_eq!(profile["warming"], false);
    let permissions = array(&profile["agents"]["teem_foreman"]["permissions"]);
    assert_eq!(
        permissions[0],
        json!({"action":"*","resource":"*","effect":"deny"})
    );
    assert!(!permissions.iter().any(|p| {
        ["shell", "subagent", "execute"].contains(&text(p, "action").as_str())
            && p["effect"] == "allow"
    }));
    for (action, effect) in [
        ("shell", "ask"),
        ("edit", "allow"),
        ("teem_foreman_integrate", "ask"),
        ("teem_foreman_color", "allow"),
    ] {
        assert!(
            permissions
                .iter()
                .any(|p| p["action"] == action && p["effect"] == effect)
        );
    }
    assert!(permissions.iter().any(|p| p["action"] == "edit"
        && p["resource"] == "**/opencode.json"
        && p["effect"] == "deny"));
    // OpenCode uses project-relative resources and '*' wildcards, not globstar:
    // '**/opencode.json' does not match the root-level 'opencode.json'.
    for resource in [
        "opencode.json",
        "opencode.jsonc",
        ".git",
        ".git/*",
        ".opencode",
        ".opencode/*",
    ] {
        assert!(
            permissions.iter().any(|p| p["action"] == "edit"
                && p["resource"] == resource
                && p["effect"] == "deny"),
            "Missing root protection for {resource}"
        );
    }
}
