//! Local dispatch/summary contracts only; no live UI, model, or runtime calls.
use serde_json::{Value, json};
use std::fs;
use teem::{
    common::{git, git_text},
    plugin,
    store::Store,
};

const FOREMAN: &str = "ses_issue3";

struct Fixture {
    store: Store,
    checkout: std::path::PathBuf,
    revision: String,
    _directory: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let checkout = directory.path().join("repo");
        fs::create_dir(&checkout).unwrap();
        git(&checkout, &["init", "-q"], None, None).unwrap();
        git(&checkout, &["config", "user.name", "Test"], None, None).unwrap();
        git(
            &checkout,
            &["config", "user.email", "test@example.invalid"],
            None,
            None,
        )
        .unwrap();
        fs::write(checkout.join("a"), "base\n").unwrap();
        git(&checkout, &["add", "."], None, None).unwrap();
        git(&checkout, &["commit", "-qm", "base"], None, None).unwrap();
        let revision = git_text(&checkout, &["rev-parse", "HEAD"]).unwrap();
        let store = Store::open(
            &directory.path().join("state"),
            &[checkout.to_string_lossy().into_owned()],
            json!({"pilot":{"path":checkout,"max_revisions":1,"checks":[]}}),
        )
        .unwrap();
        store.bind(FOREMAN, "terminal").unwrap();
        Self {
            store,
            checkout,
            revision,
            _directory: directory,
        }
    }

    fn workflow(&self) -> Value {
        json!({
            "workflow_id":uuid::Uuid::new_v4().to_string(),
            "project":"pilot", "title":"Compact dispatch",
            "assignment":{"scope":["a","b"],"prompt":long_prompt()}
        })
    }

    fn submit(&self) -> Value {
        json!({
            "task_id":uuid::Uuid::new_v4().to_string(), "role":"worker",
            "assignment":{"checkout":self.checkout,"revision":self.revision,
                "scope":["a"],"prompt":long_prompt()}
        })
    }

    fn call(&self, name: &str, args: &Value) -> anyhow::Result<Value> {
        plugin::call(&self.store, FOREMAN, name, args)
    }

    fn request_detail(&self, key: &str, id: &Value) -> Value {
        let mut args = json!({"mode":"detail","pointer":"/request","limit":1024});
        args[key] = id.clone();
        let mut text = String::new();
        let mut digest = None;
        loop {
            let page = self.call("foreman_status", &args).unwrap();
            if let Some(digest) = &digest {
                assert_eq!(digest, &page["sha256"]);
            } else {
                digest = Some(page["sha256"].clone());
            }
            text.push_str(page["chunk"].as_str().unwrap());
            if page["next_offset"].is_null() {
                break;
            }
            args["offset"] = page["next_offset"].clone();
        }
        serde_json::from_str(&text).unwrap()
    }
}

fn long_prompt() -> String {
    format!(
        "  EXACT_PRIVATE_INPUT\n{}\n\tend  ",
        "λ🦀\\\"\t".repeat(400)
    )
}

fn legacy(compact: &Value) -> Value {
    let mut result = compact.as_object().unwrap().clone();
    let assignment = result.remove("assignment").unwrap();
    result.extend(assignment.as_object().unwrap().clone());
    Value::Object(result)
}

// Fixture of the supplied host contract: only TOP-LEVEL string/number/bool
// inputs are joined as key=value, without a length bound. Not a live UI test.
fn primitive_summary(input: &Value) -> String {
    input
        .as_object()
        .unwrap()
        .iter()
        .filter_map(|(key, value)| {
            let text = match value {
                Value::String(text) => text.clone(),
                Value::Number(_) | Value::Bool(_) => value.to_string(),
                _ => return None,
            };
            Some(format!("{key}={text}"))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[test]
fn schemas_advertise_exclusive_compact_and_legacy_forms() {
    let tools = plugin::tools();
    for (name, common, fields, required) in [
        (
            "foreman_submit",
            json!(["task_id", "role"]),
            vec!["checkout", "revision", "scope", "prompt"],
            json!(["checkout", "revision", "scope", "prompt"]),
        ),
        (
            "foreman_workflow",
            json!(["workflow_id", "project"]),
            vec!["scope", "prompt", "workers"],
            json!(["scope", "prompt"]),
        ),
    ] {
        let tool = tools
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap();
        let schema = &tool["inputSchema"];
        let nested = &schema["properties"]["assignment"];
        assert_eq!(schema["required"], common);
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(nested["type"], "object");
        assert_eq!(nested["required"], required);
        assert_eq!(nested["additionalProperties"], false);
        assert_eq!(
            nested["properties"].as_object().unwrap().len(),
            fields.len()
        );
        for field in &fields {
            assert_eq!(nested["properties"][*field], schema["properties"][*field]);
        }
        let prohibited = fields
            .iter()
            .map(|k| json!({"required":[k]}))
            .collect::<Vec<_>>();
        assert_eq!(
            schema["oneOf"],
            json!([
                {"required":["assignment"],"not":{"anyOf":prohibited}},
                {"required":required,"not":{"required":["assignment"]}}
            ])
        );
        assert!(
            tool["description"]
                .as_str()
                .unwrap()
                .contains("Prefer assignment")
        );
    }
    for tool in tools.as_array().unwrap() {
        if !["foreman_submit", "foreman_workflow"].contains(&tool["name"].as_str().unwrap()) {
            assert!(
                tool["inputSchema"]["properties"]
                    .get("assignment")
                    .is_none()
            );
        }
    }
}

#[test]
fn compact_single_and_two_worker_requests_keep_exact_inputs_and_retry_identity() {
    let f = Fixture::new();
    for count in [0, 1, 2] {
        let mut args = f.workflow();
        if count > 0 {
            args["assignment"]["workers"] = json!(
                (0..count)
                    .map(|i| json!({"scope":[if i == 0 {"a"} else {"b"}],
                        "prompt":format!("part {i}\n{}", long_prompt())}))
                    .collect::<Vec<_>>()
            );
        }
        let summary = primitive_summary(&args);
        assert!(summary.contains("title=Compact dispatch"));
        assert!(summary.len() < 160);
        assert!(!summary.contains("EXACT_PRIVATE_INPUT"));
        assert!(primitive_summary(&legacy(&args)).contains(&long_prompt()));
        let admitted = f.call("foreman_workflow", &args).unwrap();
        assert_eq!(admitted["request"], legacy(&args));
        assert_eq!(admitted["request"]["prompt"], long_prompt());
        assert_eq!(
            admitted["request"]["workers"],
            args["assignment"]["workers"]
        );
        assert_eq!(f.call("foreman_workflow", &args).unwrap(), admitted);
        assert_eq!(
            f.call("foreman_workflow", &legacy(&args)).unwrap(),
            admitted
        );
        assert_eq!(
            f.request_detail("workflow_id", &args["workflow_id"]),
            admitted["request"]
        );
        let mut changed = args.clone();
        changed["assignment"]["prompt"] = json!(format!("{}!", long_prompt()));
        assert!(
            f.call("foreman_workflow", &changed)
                .unwrap_err()
                .to_string()
                .contains("immutable")
        );
        if count > 0 {
            changed = args.clone();
            changed["assignment"]["workers"][0]["prompt"] = json!("Different part");
            assert!(f.call("foreman_workflow", &changed).is_err());
        }
    }
    // Old top-level admission can also be retried in the recommended form.
    let args = f.workflow();
    let old = f.call("foreman_workflow", &legacy(&args)).unwrap();
    assert_eq!(f.call("foreman_workflow", &args).unwrap(), old);
}

#[test]
fn compact_submit_preserves_revision_prompt_scope_and_legacy_retries() {
    let f = Fixture::new();
    let args = f.submit();
    assert!(!primitive_summary(&args).contains("EXACT_PRIVATE_INPUT"));
    assert!(!primitive_summary(&args).contains(&f.revision));
    let task = f.call("foreman_submit", &args).unwrap();
    let mut expected = legacy(&args);
    expected["foreman"] = json!(FOREMAN);
    assert_eq!(task["request"], expected);
    assert_eq!(f.call("foreman_submit", &legacy(&args)).unwrap(), task);
    assert_eq!(f.call("foreman_submit", &args).unwrap(), task);
    assert_eq!(f.request_detail("task_id", &args["task_id"]), expected);
    for (key, value) in [
        ("prompt", json!("Different task")),
        ("scope", json!(["b"])),
        ("revision", json!("a".repeat(40))),
    ] {
        let mut changed = args.clone();
        changed["assignment"][key] = value;
        assert!(
            f.call("foreman_submit", &changed)
                .unwrap_err()
                .to_string()
                .contains("immutable")
        );
    }
    let args = f.submit();
    let old = f.call("foreman_submit", &legacy(&args)).unwrap();
    assert_eq!(f.call("foreman_submit", &args).unwrap(), old);
}

#[test]
fn malformed_assignments_and_mixed_forms_never_admit_work() {
    let f = Fixture::new();
    for (name, valid) in [
        ("foreman_workflow", f.workflow()),
        ("foreman_submit", f.submit()),
    ] {
        for bad in [Value::Null, json!("text"), json!([]), json!(true)] {
            assert!(f.call(name, &bad).is_err());
        }
        for bad in [
            Value::Null,
            json!("text"),
            json!([]),
            json!(true),
            json!({}),
            json!({"unknown":1}),
        ] {
            let mut args = valid.clone();
            args["assignment"] = bad;
            assert!(f.call(name, &args).is_err());
        }
        for key in valid["assignment"].as_object().unwrap().keys() {
            let mut args = valid.clone();
            args[key] = args["assignment"][key].clone();
            assert!(f.call(name, &args).is_err(), "equal mixed {key}");
            args[key] = json!("conflict");
            assert!(f.call(name, &args).is_err(), "conflicting mixed {key}");
            args = valid.clone();
            args["assignment"].as_object_mut().unwrap().remove(key);
            assert!(f.call(name, &args).is_err(), "missing {key}");
        }
        for (key, value) in [
            ("unknown", json!(1)),
            ("prompt", json!(1)),
            ("prompt", json!(" \n\t")),
            ("prompt", json!("x".repeat(30001))),
            ("scope", json!("a")),
            ("scope", json!([])),
            ("scope", json!([7])),
            ("scope", json!(["../escape"])),
        ] {
            let mut args = valid.clone();
            args["assignment"][key] = value;
            assert!(f.call(name, &args).is_err(), "invalid {key}");
        }
        let mut args = valid.clone();
        args["unknown"] = json!(1);
        assert!(f.call(name, &args).is_err());
        for key in [
            "role",
            "foreman",
            "workflow_id",
            "native_todos",
            "input_tree",
            "reconcile_root",
        ] {
            args = valid.clone();
            args["assignment"][key] = json!("worker");
            assert!(f.call(name, &args).is_err(), "nested control field {key}");
        }
    }
    for workers in [
        json!(null),
        json!({}),
        json!([]),
        json!([null]),
        json!([{"scope":["a"]}]),
        json!([{"scope":["a"],"prompt":1}]),
        json!([{"scope":["a"],"prompt":" \n"}]),
        json!([{"scope":["a"],"prompt":"x".repeat(8001)}]),
        json!([{"scope":"a","prompt":"ok"}]),
        json!([{"scope":["a"],"prompt":"ok","extra":true}]),
        json!([{"scope":["outside"],"prompt":"ok"}]),
        json!([{"scope":["a"],"prompt":"one"},{"scope":["a"],"prompt":"two"}]),
        json!(vec![json!({"scope":["a"],"prompt":"one"}); 3]),
    ] {
        let mut args = f.workflow();
        args["assignment"]["workers"] = workers;
        assert!(f.call("foreman_workflow", &args).is_err());
    }
    // workers is optional, but ANY top-level assignment field conflicts.
    let mut args = f.workflow();
    args["workers"] = json!([]);
    assert!(f.call("foreman_workflow", &args).is_err());
    args = f.workflow();
    args["assignment"]["prompt"] = json!("x".repeat(8001));
    assert!(f.call("foreman_workflow", &args).is_err());
    for (key, value) in [("checkout", json!(7)), ("revision", json!(false))] {
        let mut args = f.submit();
        args["assignment"][key] = value;
        assert!(f.call("foreman_submit", &args).is_err());
    }
    assert!(
        f.store
            .query("SELECT id FROM workflows", [])
            .unwrap()
            .is_empty()
    );
    assert!(
        f.store
            .query("SELECT id FROM tasks", [])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn malformed_compact_call_remains_a_visible_mcp_tool_error() {
    let f = Fixture::new();
    let mut unauthorized = f.submit();
    unauthorized["assignment"]["checkout"] = json!(f.checkout.parent().unwrap());
    for (name, mut args, expected) in [
        (
            "foreman_workflow",
            f.workflow(),
            "Assignment must be an object",
        ),
        ("foreman_submit", f.submit(), "Assignment must be an object"),
        (
            "foreman_submit",
            unauthorized,
            "Checkout is not explicitly authorised",
        ),
    ] {
        if expected == "Assignment must be an object" {
            args["assignment"] = Value::Null;
        }
        let request = json!({"jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":name,"arguments":args}});
        let mut wire = serde_json::to_vec(&request).unwrap();
        wire.push(b'\n');
        let mut output = Vec::new();
        plugin::mcp(&f.store, FOREMAN, &wire[..], &mut output).unwrap();
        let response: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(response["id"], 1);
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(response["result"]["content"][0]["text"], expected);
    }
}
