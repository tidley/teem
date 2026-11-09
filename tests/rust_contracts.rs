use serde_json::{Value, json};
use std::io::Write;
use std::{fs, process::Command};

fn cli(config: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_teem"))
        .arg("--config")
        .arg(config)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn reads_python_database_and_preserves_native_identities_without_runtime() {
    let tmp = tempfile::tempdir().unwrap();
    let config = tmp.path().join("config.toml");
    let state = tmp.path().join("state");
    fs::write(
        &config,
        format!(
            "enabled = false\nstate_dir = {:?}\n",
            state.to_str().unwrap()
        ),
    )
    .unwrap();
    let script = "from foreman_supervisor import Store\nimport sys\nwith Store(sys.argv[1],[]) as s:\n s.bind('ses_existing','terminal-existing')\n s.db.execute(\"INSERT INTO events(id,task_id,foreman,request_id,transport,state) VALUES ('old','old-task','ses_existing','00000000-0000-0000-0000-000000000001','legacy-codr','uncertain')\")\n s.db.commit()\n";
    assert!(
        Command::new("python3")
            .env(
                "PYTHONPATH",
                concat!(env!("CARGO_MANIFEST_DIR"), "/tests/reference")
            )
            .args(["-B", "-c", script])
            .arg(&state)
            .status()
            .unwrap()
            .success()
    );
    let out = cli(&config, &["status", "--foreman", "ses_existing"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let status: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(status["terminal_id"], "terminal-existing");
    assert_eq!(status["events"][0]["transport"], "legacy-codr");
    assert_eq!(status["events"][0]["state"], "uncertain");
    assert_eq!(status["tasks"], json!([]));
}

#[test]
fn generated_unit_has_no_python_runtime_and_does_not_activate_service() {
    let out = cli(
        std::path::Path::new("/nonexistent/teem.toml"),
        &["service-unit"],
    );
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("ExecStart="));
    assert!(!text.contains("python"));
    assert!(text.contains("UMask=0077"));
}

#[test]
fn unconfigured_start_is_a_noop() {
    let out = cli(
        std::path::Path::new("/nonexistent/teem.toml"),
        &["service-start"],
    );
    assert!(out.status.success());
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .contains("No service was started")
    );
}

#[test]
fn installed_binary_and_mcp_round_trip_need_no_python_runtime() {
    let tmp = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_teem"))
        .arg("install-plugin")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let installed = tmp.path().join("bin/teem");
    assert!(
        Command::new(&installed)
            .arg("--version")
            .status()
            .unwrap()
            .success()
    );
    let config = tmp.path().join("config.toml");
    let state = tmp.path().join("state");
    fs::write(
        &config,
        format!("enabled=false\nstate_dir={:?}\n", state.to_str().unwrap()),
    )
    .unwrap();
    let s = teem::store::Store::open(&state, &[], json!({})).unwrap();
    s.bind("ses_mcp", "terminal-mcp").unwrap();
    drop(s);
    let mut child = Command::new(installed)
        .arg("--config")
        .arg(&config)
        .args(["mcp", "--foreman", "ses_mcp"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let requests = [
        json!({"id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}),
        json!({"id":2,"method":"tools/list"}),
        json!({"id":3,"method":"tools/call","params":{"name":"foreman_plan","arguments":{"items":[{"id":"plan","content":"Inspect source","status":"pending"}]}}}),
    ];
    let mut stdin = child.stdin.take().unwrap();
    for request in requests {
        writeln!(stdin, "{request}").unwrap();
    }
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let responses = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(responses.len(), 3);
    assert_eq!(responses[0]["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(responses[1]["result"]["tools"].as_array().unwrap().len(), 5);
    let plan: Value = serde_json::from_str(
        responses[2]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(plan["todos"][0]["content"], "Inspect source");
}
