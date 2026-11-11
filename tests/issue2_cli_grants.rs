//! Deterministic Git regression and narrow native-contract fixture, not a copy
//! of OpenCode's project hash, wildcard matcher, or permission implementation.
use anyhow::Result;
use serde_json::{Value, json};
use std::{cell::RefCell, fs, path::Path};
use teem::{
    checkouts,
    common::{array, git, git_text},
    runtime::{self, Runtime},
};

const REMOTE: &str = "https://example.invalid/team/project.git";
const COMMAND: &str = "cargo test --locked";

fn repo(path: &Path) -> String {
    fs::create_dir(path).unwrap();
    git(path, &["init", "-q"], None, None).unwrap();
    git(path, &["config", "user.name", "Test"], None, None).unwrap();
    git(
        path,
        &["config", "user.email", "test@example.invalid"],
        None,
        None,
    )
    .unwrap();
    fs::write(path.join("file"), "base\n").unwrap();
    git(path, &["add", "file"], None, None).unwrap();
    git(path, &["commit", "-qm", "base"], None, None).unwrap();
    git_text(path, &["rev-parse", "HEAD"]).unwrap()
}

fn origin(path: &Path) -> String {
    git_text(path, &["config", "--get", "remote.origin.url"]).unwrap()
}

fn add_origin(path: &Path, remote: &str) {
    git(path, &["remote", "add", "origin", remote], None, None).unwrap();
}

#[test]
fn local_clone_reproduces_identity_loss_and_prepare_preserves_remote_exactly() {
    for remote in [
        REMOTE,
        "git@example.invalid:team/project.git",
        "ssh://git@example.invalid/team/project.git",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let base = repo(&source);
        add_origin(&source, remote);
        let before = fs::read(source.join(".git/config")).unwrap();
        let raw = tmp.path().join("raw");
        git(
            &source,
            &[
                "clone",
                "--no-local",
                "--no-checkout",
                "--",
                source.to_str().unwrap(),
                raw.to_str().unwrap(),
            ],
            None,
            None,
        )
        .unwrap();
        assert_eq!(origin(&raw), source.to_str().unwrap());
        assert_ne!(origin(&raw), remote);

        let worker = checkouts::prepare(&source, &tmp.path().join("worker"), &base, None).unwrap();
        let review = checkouts::prepare(&worker, &tmp.path().join("review"), &base, None).unwrap();
        for clone in [&worker, &review] {
            assert_eq!(origin(clone), remote);
            assert_eq!(git_text(clone, &["rev-parse", "HEAD"]).unwrap(), base);
            assert!(!clone.join(".git/opencode").exists());
            assert!(
                git_text(clone, &["status", "--porcelain"])
                    .unwrap()
                    .is_empty()
            );
        }
        assert_eq!(fs::read(source.join(".git/config")).unwrap(), before);
    }
}

#[test]
fn linked_source_retains_common_remote_without_copying_cached_identity() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    let base = repo(&source);
    add_origin(&source, REMOTE);
    fs::write(source.join(".git/opencode"), "fixture-cached-project").unwrap();
    let before = fs::read(source.join(".git/config")).unwrap();
    let linked = tmp.path().join("linked");
    git(
        &source,
        &[
            "worktree",
            "add",
            "--detach",
            linked.to_str().unwrap(),
            &base,
        ],
        None,
        None,
    )
    .unwrap();
    assert!(linked.join(".git").is_file());
    assert_eq!(origin(&linked), origin(&source));

    let clone = checkouts::prepare(&linked, &tmp.path().join("worker"), &base, None).unwrap();
    assert_eq!(origin(&clone), REMOTE);
    assert!(!clone.join(".git/opencode").exists());
    assert_eq!(fs::read(source.join(".git/config")).unwrap(), before);
    assert_eq!(
        fs::read_to_string(source.join(".git/opencode")).unwrap(),
        "fixture-cached-project"
    );
}

#[test]
fn unsupported_or_ambiguous_remotes_do_not_get_promoted() {
    for remote in [
        "",
        "/local/repository",
        "file:///local/repository",
        "../repository",
        "https://user:secret@example.invalid/team/project.git",
        "https://user@example.invalid/team/project.git",
        "https://example.invalid/team/project.git?token=secret",
        "https://example.invalid/team/project.git#fragment",
        "ext::untrusted command",
        "ssh://user@example.invalid/team/project.git",
        " https://example.invalid/team/project.git",
        "https://example.invalid/team/project.git ",
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let base = repo(&source);
        if !remote.is_empty() {
            add_origin(&source, remote);
        }
        let clone = checkouts::prepare(&source, &tmp.path().join("worker"), &base, None).unwrap();
        assert_eq!(origin(&clone), source.to_str().unwrap());
    }
    for multiple_urls in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let base = repo(&source);
        add_origin(&source, REMOTE);
        if multiple_urls {
            git(
                &source,
                &[
                    "config",
                    "--add",
                    "remote.origin.url",
                    "https://example.invalid/other.git",
                ],
                None,
                None,
            )
            .unwrap();
        } else {
            git(
                &source,
                &[
                    "remote",
                    "add",
                    "upstream",
                    "https://example.invalid/other.git",
                ],
                None,
                None,
            )
            .unwrap();
        }
        let clone = checkouts::prepare(&source, &tmp.path().join("worker"), &base, None).unwrap();
        assert_eq!(origin(&clone), source.to_str().unwrap());
    }
}

#[test]
fn local_config_discovery_overrides_and_cached_ids_are_not_copied() {
    for (key, value) in [
        ("url.https://example.invalid/other.git.insteadOf", REMOTE),
        (
            "url.https://example.invalid/other.git.pushInsteadOf",
            REMOTE,
        ),
        ("include.path", "/nonexistent/issue2-config"),
        (
            "includeIf.gitdir:/nonexistent/.path",
            "/nonexistent/issue2-config",
        ),
        ("extensions.worktreeConfig", "false"),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("source");
        let base = repo(&source);
        add_origin(&source, REMOTE);
        git(&source, &["config", key, value], None, None).unwrap();
        fs::write(source.join(".git/opencode"), "fixture-cached-project").unwrap();
        let clone = checkouts::prepare(&source, &tmp.path().join("worker"), &base, None).unwrap();
        assert_eq!(origin(&clone), source.to_str().unwrap());
        assert!(!clone.join(".git/opencode").exists());
        assert_eq!(
            fs::read_to_string(source.join(".git/opencode")).unwrap(),
            "fixture-cached-project"
        );
    }
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    let base = repo(&source);
    git(
        &source,
        &["config", "remote.origin.pushurl", REMOTE],
        None,
        None,
    )
    .unwrap();
    let clone = checkouts::prepare(&source, &tmp.path().join("worker"), &base, None).unwrap();
    assert_eq!(origin(&clone), source.to_str().unwrap());
}

// The supplied host contract says saved rules are read fresh per project and
// configured denial wins. This fixture accepts only exact command resources;
// native wildcard normalization/evaluation must still be qualified by the host.
struct NativeContract {
    saved: RefCell<Vec<Value>>,
    sessions: RefCell<Vec<Value>>,
}

impl Runtime for NativeContract {
    fn default_model(&self) -> Value {
        json!({"providerID":"fixture","id":"local"})
    }

    fn request(&self, method: &str, path: &str, payload: Option<&Value>) -> Result<Value> {
        assert_eq!((method, path), ("POST", "/api/session"));
        let session = payload.unwrap().clone();
        self.sessions.borrow_mut().push(session.clone());
        Ok(session)
    }
}

impl NativeContract {
    fn decision(&self, project: &str, session: &Value, command: &str) -> &'static str {
        let rules = array(&session["permissions"]);
        let configured = rules
            .iter()
            .rev()
            .find(|rule| {
                (rule["action"] == "shell" || rule["action"] == "*")
                    && (rule["resource"] == command || rule["resource"] == "*")
            })
            .unwrap();
        if configured["effect"] == "deny" {
            return "deny";
        }
        if self.saved.borrow().iter().any(|grant| {
            grant["projectID"] == project
                && grant["action"] == "shell"
                && grant["resource"] == command
        }) {
            "allow"
        } else {
            "ask"
        }
    }
}

#[test]
fn existing_project_grants_are_native_fresh_and_never_session_allows() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    let base = repo(&source);
    add_origin(&source, REMOTE);
    let unrelated = tmp.path().join("unrelated");
    // Equal root commits must not override different canonical remotes.
    git(
        &source,
        &[
            "clone",
            "--no-local",
            "--",
            source.to_str().unwrap(),
            unrelated.to_str().unwrap(),
        ],
        None,
        None,
    )
    .unwrap();
    git(
        &unrelated,
        &[
            "remote",
            "set-url",
            "origin",
            "https://example.invalid/unrelated.git",
        ],
        None,
        None,
    )
    .unwrap();
    assert_eq!(git_text(&unrelated, &["rev-parse", "HEAD"]).unwrap(), base);
    let unrelated_clone = checkouts::prepare(
        &unrelated,
        &tmp.path().join("unrelated-worker"),
        &base,
        None,
    )
    .unwrap();
    assert_ne!(origin(&unrelated_clone), origin(&source));
    let grant = json!({"projectID":REMOTE,"action":"shell","resource":COMMAND});
    let native = NativeContract {
        saved: RefCell::new(vec![grant.clone()]),
        sessions: RefCell::new(vec![]),
    };
    for index in 0..2 {
        let clone = checkouts::prepare(
            &source,
            &tmp.path().join(format!("worker{index}")),
            &base,
            None,
        )
        .unwrap();
        // Exact remote equality is sufficient for this fixture. Real project IDs
        // are OpenCode-owned hashes of its normalized canonical remote.
        let project = origin(&clone);
        assert_eq!(project, origin(&source));
        let task = json!({"id":format!("task{index}"),"session_id":format!("ses_{index}"),"role":"worker","checkout":clone,"request":{"revision":base,"scope":["file"]}});
        native.create(&task).unwrap();
        let session = native.sessions.borrow().last().unwrap().clone();
        assert_eq!(session["permissions"], runtime::permissions(&task));
        assert_eq!(native.decision(&project, &session, COMMAND), "allow");
        assert_eq!(native.decision(&project, &session, "cargo test"), "ask");
        assert_eq!(
            native.decision(&origin(&unrelated_clone), &session, COMMAND),
            "ask"
        );
        let mut denied = session.clone();
        denied["permissions"]
            .as_array_mut()
            .unwrap()
            .push(json!({"action":"shell","resource":COMMAND,"effect":"deny"}));
        assert_eq!(native.decision(&project, &denied, COMMAND), "deny");
        let mut reviewer = task.clone();
        reviewer["role"] = json!("reviewer");
        assert_eq!(
            native.decision(
                &project,
                &runtime::session_payload(&reviewer, &Value::Null),
                COMMAND
            ),
            "deny"
        );
        native.saved.borrow_mut().clear();
        assert_eq!(native.decision(&project, &session, COMMAND), "ask");
        // Once/pending/rejected/expired requests are not saved rows. Repeated
        // session creation cannot transform them into a durable shell allow.
        native.create(&task).unwrap();
        assert_eq!(
            native.decision(&project, native.sessions.borrow().last().unwrap(), COMMAND),
            "ask"
        );
        native.saved.borrow_mut().push(grant.clone());
    }
}
