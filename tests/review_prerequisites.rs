use anyhow::Result;
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};
use teem::{
    common::*,
    runtime::{self, Runtime},
    store::Store,
    workflows,
};

struct Local(PathBuf);
impl Runtime for Local {
    fn request(&self, method: &str, path: &str, _: Option<&Value>) -> Result<Value> {
        assert_eq!((method, path), ("GET", "/api/session/ses_test"));
        Ok(json!({"id":"ses_test","location":{"directory":self.0}}))
    }
}
fn fixture(tmp: &std::path::Path) -> (Store, Value, PathBuf) {
    let source = tmp.join("source");
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
    let guide = tmp.join("PAVE.md");
    fs::write(&guide, "Mandatory project policy.\n").unwrap();
    let policy = workflows::normalise_projects(&json!({"project":{"path":source,"review_prerequisites":[guide],"checks":[{"name":"tests","kind":"test","argv":["git","diff","--check","HEAD","--"]}]}})).unwrap();
    let s = Store::open(&tmp.join("state"), &[], policy).unwrap();
    s.bind("ses_test", "terminal").unwrap();
    let w = s.workflow_submit("ses_test", json!({"workflow_id":uuid::Uuid::new_v4().to_string(),"project":"project","issue":"tidley/teem#12","scope":["src"],"prompt":"Implement scoped change"})).unwrap();
    (s, w, guide)
}
fn runner() -> teem::checks::Runner {
    teem::checks::Runner::with_executable(PathBuf::from(env!("CARGO_BIN_EXE_teem")))
}

#[test]
fn prerequisite_files_are_pinned_and_supplied_locally_without_external_permission_grants() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, w, guide) = fixture(tmp.path());
    workflows::tick(&s, &Local(tmp.path().join("source")), &mut runner()).unwrap();
    let task = s.tasks(None).unwrap().remove(0);
    assert_eq!(
        task["request"]["review_prerequisites"],
        w["policy"]["review_prerequisites"]
    );
    let copy = PathBuf::from(text(&task, "checkout")).join(".git/teem-prerequisites/0-PAVE.md");
    assert_eq!(fs::read(&copy).unwrap(), fs::read(&guide).unwrap());
    assert_eq!(fs::metadata(copy).unwrap().permissions().mode() & 0o222, 0);
    assert!(text(&task["request"], "prompt").contains(".git/teem-prerequisites/0-PAVE.md"));
    assert!(
        !array(&runtime::permissions(&task))
            .iter()
            .any(|r| text(r, "resource").contains(guide.to_str().unwrap()))
    );
}

#[test]
fn missing_or_changed_prerequisites_stop_before_any_worker_is_submitted() {
    for remove in [true, false] {
        let tmp = tempfile::tempdir().unwrap();
        let (s, w, guide) = fixture(tmp.path());
        if remove {
            fs::remove_file(guide).unwrap();
        } else {
            fs::write(guide, "changed policy").unwrap();
        }
        workflows::tick(&s, &Local(tmp.path().join("source")), &mut runner()).unwrap();
        let w = s.workflow(&text(&w, "id")).unwrap();
        assert_eq!(w["state"], "blocked");
        assert!(text(&w, "detail").contains("prerequisite"));
        assert!(s.tasks(None).unwrap().is_empty());
        assert_eq!(w["round"], 0);
    }
}

#[test]
fn prerequisite_drift_after_queueing_is_rejected_before_native_session_creation() {
    let tmp = tempfile::tempdir().unwrap();
    let (s, _, guide) = fixture(tmp.path());
    let local = Local(tmp.path().join("source"));
    workflows::tick(&s, &local, &mut runner()).unwrap();
    fs::write(guide, "changed after queueing").unwrap();
    runtime::tick(&s, &local).unwrap();
    let task = s.tasks(None).unwrap().remove(0);
    assert_eq!(task["state"], "failed");
    assert!(text(&task, "detail").contains("prerequisite"));
}

#[test]
fn blocked_verdict_is_not_a_source_finding_or_approval() {
    let snapshot = json!({"sha256":"candidate"});
    let blocked = json!({"snapshot":"candidate","verdict":"blocked","findings":[],"reason":"Mandatory policy read was denied"});
    assert_eq!(
        workflows::verdict(&blocked.to_string(), &snapshot).unwrap(),
        blocked
    );
    for invalid in [
        json!({"snapshot":"candidate","verdict":"blocked","findings":[]}),
        json!({"snapshot":"candidate","verdict":"blocked","findings":[],"reason":""}),
        json!({"snapshot":"candidate","verdict":"pass","findings":[],"reason":"not a blocked verdict"}),
        json!({"snapshot":"other","verdict":"blocked","findings":[],"reason":"Denied"}),
        json!({"snapshot":"candidate","verdict":"blocked","findings":[{"path":"src/a","line":1,"message":"defect"}],"reason":"Denied"}),
    ] {
        assert!(workflows::verdict(&invalid.to_string(), &snapshot).is_err());
    }
}

#[test]
fn prerequisites_are_explicit_bounded_files_and_preloaded_guidance_does_not_grant_tools() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("project");
    fs::create_dir(&project).unwrap();
    for value in [
        json!([tmp.path()]),
        json!([tmp.path().join("missing")]),
        json!(["relative.md"]),
        json!("not an array"),
    ] {
        assert!(teem::prerequisites::normalise(&value, &project).is_err());
    }
    let bad = tmp.path().join("bad.md");
    fs::write(&bad, [255]).unwrap();
    assert!(teem::prerequisites::normalise(&json!([bad]), &project).is_err());
    let skill = tmp.path().join("linus-review/SKILL.md");
    fs::create_dir(skill.parent().unwrap()).unwrap();
    fs::write(&skill, "Required review guidance.").unwrap();
    let declaration = teem::prerequisites::normalise(&json!([skill]), &project).unwrap();
    let checkout = tmp.path().join("checkout");
    fs::create_dir_all(checkout.join(".git")).unwrap();
    let prompt =
        teem::prerequisites::prepare(&checkout, &json!({"review_prerequisites":declaration}))
            .unwrap();
    assert!(prompt.contains("<skill_content name=\"linus-review\">"));
    assert!(prompt.contains("Required review guidance."));
    assert!(prompt.contains("0-SKILL.md"));
    for role in ["worker", "reviewer"] {
        let plain = json!({"role":role,"request":{}});
        let supplied = json!({"role":role,"request":{"review_prerequisites":declaration}});
        assert_eq!(
            runtime::permissions(&plain),
            runtime::permissions(&supplied)
        );
    }
    fs::write(skill, "x".repeat(12001)).unwrap();
    assert!(
        teem::prerequisites::normalise(
            &json!([tmp.path().join("linus-review/SKILL.md")]),
            &project
        )
        .is_err()
    );
}
