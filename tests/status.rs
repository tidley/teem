//! Status fixtures never launch agents or checks. The supervisor verifies this snapshot.
use rusqlite::params;
use serde_json::{Value, json};
use teem::{common::canonical, plugin, store::Store};

struct Fixture {
    store: Store,
    _directory: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path(), &[], json!({})).unwrap();
        store.bind("ses_test", "terminal").unwrap();
        store.bind("ses_other", "other").unwrap();
        Self {
            store,
            _directory: directory,
        }
    }

    fn workflow(&self, id: &str, state: &str, data: Value) {
        let request = json!({"title":format!("Fix {id}"),"prompt":"PRIVATE_ACCEPTANCE ".repeat(400),"scope":["src"]});
        let policy = json!({"checks":[{"name":"tests"},{"name":"lint"}]});
        self.store.exec("INSERT INTO workflows(id,foreman,project,request,policy,state,data,detail,created) VALUES (?,'ses_test','pilot',?,?,?,?,?,0)", params![id,canonical(&request),canonical(&policy),state,canonical(&data),"Unresolved limitation: platform not checked"]).unwrap();
    }

    fn task(&self, id: &str, workflow: &str, role: &str, state: &str) {
        let request = json!({"workflow_id":workflow,"prompt":"PRIVATE_WORKER ".repeat(4000)});
        self.store.exec("INSERT INTO tasks(id,foreman,role,checkout,state,request,session_id,prompt_id,sparse,result,created) VALUES (?,'ses_test',?,'/fixture',?,?,?,?,0,?,0)",params![id,role,state,canonical(&request),format!("ses_{id}"),format!("msg_{id}"),"EXECUTION_ONLY ".repeat(6000)]).unwrap();
        self.store.exec("INSERT INTO events(id,task_id,foreman,request_id,workflow_id) VALUES (?,?,'ses_test',?,?)",params![format!("event_{id}"),id,format!("request_{id}"),workflow]).unwrap();
        self.store.exec("INSERT INTO approvals(task_id,permission_id,foreman,form_id,snapshot,payload,state) VALUES (?,'permission','ses_test',?,'{}',?,'pending')", params![id,format!("form_{id}"),"PRIVATE_APPROVAL ".repeat(4000)]).unwrap();
        let todos = json!([{
            "content": "PRIVATE_TODOS ".repeat(4000),
            "status": "in_progress"
        }]);
        self.store
            .exec(
                "INSERT INTO agent_todos(task_id,todos) VALUES (?,?)",
                params![id, canonical(&todos)],
            )
            .unwrap();
    }

    fn status(&self, args: Value) -> Value {
        let result = plugin::call(&self.store, "ses_test", "foreman_status", &args).unwrap();
        assert!(serde_json::to_string_pretty(&result).unwrap().len() <= 24000);
        result
    }
}

fn evidence() -> Value {
    let findings = (0..20)
        .map(
            |n| json!({"path":"src/store.rs","line":n+1,"message":"Important failure ".repeat(25)}),
        )
        .collect::<Vec<_>>();
    let round = json!({"snapshot":{"sha256":"candidate","tree":"tree"},"worker_id":"done","review_ids":["r1","r2"],"check_result":{"snapshot":"candidate","passed":false,"error":"CHECK_ERROR ".repeat(100),"results":[{"name":"tests","returncode":1,"timed_out":false,"output_limited":true,"error":"CHECK_FAILURE ".repeat(100)}]},"review_results":[{"snapshot":"candidate","verdict":"changes_requested","findings":findings},{"snapshot":"candidate","verdict":"pass","findings":[]}]});
    json!({"rounds":vec![round; 40]})
}

#[test]
fn large_history_defaults_are_bounded_and_counts_are_not_dispatch_counts() {
    let f = Fixture::new();
    for w in 0..12 {
        let id = format!("workflow_{w:02}");
        f.workflow(&id, "reviewing", evidence());
        for (i, state) in [
            "queued",
            "starting",
            "accepted",
            "running",
            "uncertain",
            "succeeded",
        ]
        .iter()
        .enumerate()
        {
            f.task(
                &format!("{w}_{i}"),
                &id,
                if i % 2 == 0 { "worker" } else { "reviewer" },
                state,
            );
        }
    }
    let result = f.status(json!({}));
    let wire = canonical(&result);
    for private in [
        "PRIVATE_ACCEPTANCE",
        "PRIVATE_WORKER",
        "PRIVATE_APPROVAL",
        "PRIVATE_TODOS",
    ] {
        assert!(!wire.contains(private));
    }
    assert_eq!(result["counts"]["tasks"]["worker"]["queued"], 12);
    assert_eq!(result["counts"]["tasks"]["worker"]["uncertain"], 12);
    assert_eq!(result["counts"]["tasks"]["reviewer"]["running"], 12);
    assert_eq!(result["counts"]["tasks"]["reviewer"]["succeeded"], 12);
    assert_eq!(result["counts"]["tasks"]["worker"]["active"], 24);
    assert_eq!(result["counts"]["tasks"]["worker"]["running"], 0);
    assert_eq!(result["pages"]["workflows"]["total"], 12);
    assert!(result["pages"]["tasks"]["next_offset"].is_number());
    assert_eq!(result["workflows"][0]["checks"]["passed"], false);
    assert_eq!(
        result["workflows"][0]["reviews"][0]["verdict"],
        "changes_requested"
    );
    assert_eq!(result["workflows"][0]["reviews"][0]["findings_total"], 20);
}

#[test]
fn filters_apply_to_every_related_collection_and_task_filter_selects_its_workflow() {
    let f = Fixture::new();
    f.workflow("a", "blocked", evidence());
    // A plan also renders active workflow progress: every referenced task must
    // exist, unlike the historical IDs in the terminal workflow's evidence.
    f.workflow(
        "b",
        "reviewing",
        json!({"rounds":[{"worker_ids":[],"review_ids":["b_task"]}]}),
    );
    f.task("a_task", "a", "worker", "uncertain");
    f.task("b_task", "b", "reviewer", "running");
    f.store
        .exec(
            "INSERT INTO plans VALUES ('ses_test',?)",
            [canonical(&json!([
                {"id":"a","content":"Own task","task_id":"a_task"},
                {"id":"b","content":"Unrelated task","task_id":"b_task"}
            ]))],
        )
        .unwrap();
    for args in [json!({"workflow_id":"a"}), json!({"task_id":"a_task"})] {
        let result = f.status(args);
        assert_eq!(result["workflows"].as_array().unwrap().len(), 1);
        assert_eq!(result["workflows"][0]["id"], "a");
        assert_eq!(result["tasks"][0]["id"], "a_task");
        assert_eq!(result["events"][0]["task_id"], "a_task");
        assert_eq!(result["approvals"][0]["task_id"], "a_task");
        assert_eq!(result["counts"]["tasks"]["reviewer"]["running"], 0);
        assert_eq!(result["foreman_counts"]["tasks"]["reviewer"]["running"], 1);
        assert!(!canonical(&result).contains("b_task"));
        assert!(!canonical(&result).contains("PRIVATE_TODOS"));
    }
    let scoped = f.status(json!({"workflow_id":"a","section":"agent_progress"}));
    assert_eq!(scoped["agent_progress"].as_array().unwrap().len(), 1);
    assert_eq!(scoped["agent_progress"][0]["task_id"], "a_task");
}

#[test]
fn detail_chunks_reconstruct_full_history_and_strings_without_state_changes() {
    let f = Fixture::new();
    f.workflow("a", "blocked", evidence());
    f.task("a_task", "a", "worker", "failed");
    let expected = canonical(&f.store.workflow("a").unwrap());
    let before = f.store.query("SELECT * FROM events", []).unwrap();
    let mut offset = 0;
    let mut full = String::new();
    loop {
        let page =
            f.status(json!({"mode":"detail","workflow_id":"a","offset":offset,"limit":4096}));
        full.push_str(page["chunk"].as_str().unwrap());
        assert_eq!(page["sha256"], teem::common::sha(expected.as_bytes()));
        if let Some(next) = page["next_offset"].as_u64() {
            offset = next;
        } else {
            break;
        }
    }
    assert_eq!(full, expected);
    let part = f.status(json!({"mode":"detail","task_id":"a_task","pointer":"/request/prompt"}));
    assert!(part["chunk"].as_str().unwrap().contains("PRIVATE_WORKER"));
    assert_eq!(before, f.store.query("SELECT * FROM events", []).unwrap());
    assert_eq!(
        f.store.workflow("a").unwrap()["data"]["rounds"]
            .as_array()
            .unwrap()
            .len(),
        40
    );
}

#[test]
fn committed_receipt_takes_precedence_without_claiming_publication_or_runtime() {
    let f = Fixture::new();
    f.workflow(
        "a",
        "ready",
        json!({"rounds":[{
            "snapshot":{"sha256":"candidate","tree":"tree"},
            "check_result":{"snapshot":"candidate","passed":true,"results":[
                {"name":"tests","returncode":0,"timed_out":false,"output_limited":false},
                {"name":"lint","returncode":0,"timed_out":false,"output_limited":false}
            ]},
            "review_results":[
                {"snapshot":"candidate","verdict":"pass","findings":[]},
                {"snapshot":"candidate","verdict":"pass","findings":[]}
            ]
        }]}),
    );
    let receipt =
        json!({"state":"committed","commit":"1234567890","snapshot":"candidate","tree":"tree"});
    f.store
        .exec(
            "INSERT INTO integrations VALUES ('a',?)",
            [canonical(&receipt)],
        )
        .unwrap();
    let result = f.status(json!({"workflow_id":"a"}));
    let w = &result["workflows"][0];
    assert_eq!(w["integration"]["state"], "committed");
    assert!(
        w["detail"]
            .as_str()
            .unwrap()
            .contains("Recorded integration commit")
    );
    assert!(
        w["candidate_detail"]
            .as_str()
            .unwrap()
            .contains("platform not checked")
    );
    assert_eq!(w["checks"]["passed"], true);
    assert_eq!(w["reviews"][0]["verdict"], "pass");
    assert_eq!(w["reviews"][1]["verdict"], "pass");
    assert_eq!(w["publication"], "not checked");
    assert_eq!(w["runtime"], "not checked");
}

#[test]
fn pagination_and_invalid_arguments_are_explicit() {
    let f = Fixture::new();
    for n in 0..7 {
        f.workflow(&format!("w{n}"), "queued", json!({"rounds":[]}));
    }
    let first = f.status(json!({"section":"workflows","limit":2}));
    let next = f.status(json!({"section":"workflows","limit":2,"offset":2}));
    assert_ne!(first["workflows"][0]["id"], next["workflows"][0]["id"]);
    assert_eq!(first["pages"]["workflows"]["next_offset"], 2);
    assert_eq!(next["pages"]["workflows"]["total"], 7);
    for args in [
        json!({"mode":"wrong"}),
        json!({"limit":0}),
        json!({"limit":6}),
        json!({"offset":-1}),
        json!({"workflow_id":7}),
        json!({"section":"wrong"}),
        json!({"pointer":"/data"}),
        json!({"task_id":"missing"}),
    ] {
        assert!(plugin::call(&f.store, "ses_test", "foreman_status", &args).is_err());
    }
}

#[test]
fn mcp_status_is_readable_multiline_and_schema_matches_cli_options() {
    let f = Fixture::new();
    f.workflow("a", "blocked", evidence());
    let input = b"{\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"foreman_status\",\"arguments\":{\"workflow_id\":\"a\"}}}\n";
    let mut output = Vec::new();
    plugin::mcp(
        &f.store,
        "ses_test",
        std::io::Cursor::new(input),
        &mut output,
    )
    .unwrap();
    let envelope: Value = serde_json::from_slice(&output).unwrap();
    let text = envelope["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.lines().count() > 10);
    assert!(text.len() <= 24000);
    let parsed: Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed["workflows"][0]["state"], "blocked");
    let options = [
        "--workflow-id",
        "a",
        "--mode",
        "detail",
        "--pointer",
        "/data/rounds/0",
        "--offset",
        "0",
        "--limit",
        "100",
    ]
    .map(str::to_owned);
    let args = plugin::status_options(&options).unwrap();
    assert_eq!(
        args,
        json!({"workflow_id":"a","mode":"detail","pointer":"/data/rounds/0","offset":0,"limit":100})
    );
    let tools = plugin::tools();
    let schema = &tools
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "foreman_status")
        .unwrap()["inputSchema"]["properties"];
    assert!(
        args.as_object()
            .unwrap()
            .keys()
            .all(|key| schema.get(key).is_some())
    );
    assert!(
        plugin::status_options(&["--limit".into(), "1".into(), "--limit".into(), "2".into()])
            .is_err()
    );
}

#[test]
fn check_output_is_bounded_verified_and_retrievable_without_external_file_reads() {
    let f = Fixture::new();
    let directory = f.store.directory.join("managed/a/round-0/checks");
    std::fs::create_dir_all(&directory).unwrap();
    let log = directory.join("tests.log");
    let output = "An important check error\n".repeat(10000);
    std::fs::write(&log, &output).unwrap();
    f.workflow("a","blocked",json!({"rounds":[{"check_result":{"results":[{"log":log,"output_sha256":teem::common::sha(output.as_bytes())}]}}]}));
    let page = f.status(json!({"mode":"detail","section":"check_output","workflow_id":"a","pointer":"/data/rounds/0/check_result/results/0","limit":1000}));
    assert!(
        page["chunk"]
            .as_str()
            .unwrap()
            .contains("important check error")
    );
    assert!(page["next_offset"].is_number());
    std::fs::write(&log, "changed evidence").unwrap();
    assert!(f.store.status("ses_test",&json!({"mode":"detail","section":"check_output","workflow_id":"a","pointer":"/data/rounds/0/check_result/results/0"})).is_err());
}

#[test]
fn escaped_error_bounds_keep_all_eight_check_outcomes_and_both_reviews() {
    let f = Fixture::new();
    let results = (0..8)
        .map(|i| json!({"name":format!("check{i}"),"returncode":1,"timed_out":true,"output_limited":true,"error":"\u{0}💥".repeat(5000)}))
        .collect::<Vec<_>>();
    let checks = (0..8)
        .map(|i| json!({"name":format!("check{i}")}))
        .collect::<Vec<_>>();
    f.workflow("a", "blocked", evidence());
    let mut data = evidence();
    let round = data["rounds"].as_array_mut().unwrap().last_mut().unwrap();
    round["check_result"]["results"] = json!(results);
    f.store
        .exec(
            "UPDATE workflows SET policy=?,data=? WHERE id='a'",
            params![canonical(&json!({"checks":checks})), canonical(&data)],
        )
        .unwrap();
    let result = f.status(json!({"workflow_id":"a","limit":5}));
    let workflow = &result["workflows"][0];
    assert_eq!(workflow["checks"]["items"].as_array().unwrap().len(), 8);
    assert!(
        workflow["checks"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["state"] == "failed")
    );
    assert_eq!(workflow["reviews"].as_array().unwrap().len(), 2);
    assert!(canonical(&workflow["checks"]).contains("truncated"));
}

#[test]
fn acknowledged_events_and_foreign_records_have_explicit_detail_boundaries() {
    let f = Fixture::new();
    f.workflow("a", "working", json!({"rounds":[]}));
    f.task("a_task", "a", "worker", "succeeded");
    f.store.ack("ses_test", "event_a_task").unwrap();
    assert_eq!(f.status(json!({"workflow_id":"a"}))["events"], json!([]));
    let history = f
        .status(json!({"mode":"detail","workflow_id":"a","section":"events","pointer":"/0/acked"}));
    assert_eq!(history["chunk"], "1");
    assert!(
        f.store
            .status("ses_other", &json!({"mode":"detail","task_id":"a_task"}))
            .is_err()
    );
    assert!(
        f.store
            .status("ses_other", &json!({"workflow_id":"a"}))
            .is_err()
    );
    let execution = f.status(json!({"task_id":"a_task"}));
    assert_eq!(execution["tasks"][0]["state"], "succeeded");
    assert_eq!(
        execution["workflows"][0]["reviews"][0]["verdict"],
        Value::Null
    );
    assert!(
        execution["tasks"][0]["meaning"]
            .as_str()
            .unwrap()
            .contains("not check/review approval")
    );
}
