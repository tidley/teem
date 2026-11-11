use crate::{
    approvals, checks,
    common::*,
    progress,
    runtime::{self, OpenCode},
    store::Store,
    workflows,
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, Read, Write},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

pub fn load(path: &Path) -> Result<Value> {
    let raw = fs::read_to_string(path)?;
    let toml: toml::Value = toml::from_str(&raw)?;
    let mut c = serde_json::to_value(toml)?;
    if c.get("enabled").is_none() {
        c["enabled"] = json!(false);
    }
    ensure!(
        c["enabled"].is_boolean(),
        "Configuration enabled must be true or false"
    );
    if c.get("trusted_worker_shell").is_none() {
        c["trusted_worker_shell"] = json!(false);
    }
    ensure!(
        c["trusted_worker_shell"].is_boolean(),
        "Configuration trusted_worker_shell must be true or false"
    );
    let state = expand(
        c["state_dir"]
            .as_str()
            .context("Select a private absolute state_dir")?,
    );
    ensure!(state.is_absolute(), "state_dir must be absolute");
    c["state_dir"] = json!(state);
    if c.get("allowed_checkouts").is_none() {
        c["allowed_checkouts"] = json!([]);
    }
    for p in c["allowed_checkouts"]
        .as_array()
        .context("List authorised checkout roots")?
    {
        ensure!(
            p.as_str().is_some_and(|s| expand(s).is_absolute()),
            "Authorised checkout roots must be absolute"
        );
    }
    if c["enabled"] == true {
        ensure!(
            c["registry"].is_string() && c["model"].as_str().is_some_and(|s| !s.is_empty()),
            "Enabled supervision requires explicit registry and model"
        );
    }
    c["projects"] = workflows::normalise_projects(c.get("projects").unwrap_or(&json!({})))?;
    Ok(c)
}
pub fn open(c: &Value) -> Result<Store> {
    let mut store = Store::open(
        Path::new(&text(c, "state_dir")),
        &array(&c["allowed_checkouts"])
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>(),
        c["projects"].clone(),
    )?;
    store.trusted_worker_shell = c["trusted_worker_shell"] == true;
    Ok(store)
}
pub fn tools() -> Value {
    serde_json::from_str(include_str!("tools.json")).unwrap()
}
fn dispatch_args(name: &str, args: &Value) -> Result<Value> {
    let fields: &[&str] = match name {
        "foreman_submit" => &["checkout", "revision", "scope", "prompt"],
        "foreman_workflow" => &["scope", "prompt", "workers"],
        _ => return Ok(args.clone()),
    };
    let mut result = args
        .as_object()
        .context("Arguments must be an object")?
        .clone();
    if let Some(assignment) = result.remove("assignment") {
        let assignment = assignment
            .as_object()
            .context("Assignment must be an object")?;
        ensure!(
            !assignment.is_empty() && assignment.keys().all(|k| fields.contains(&k.as_str())),
            "Use exactly documented assignment fields"
        );
        ensure!(
            fields.iter().all(|k| !result.contains_key(*k)),
            "Do not mix assignment with top-level assignment fields"
        );
        result.extend(assignment.clone());
    }
    ensure!(
        fields
            .iter()
            .filter(|k| **k != "workers")
            .all(|k| result.contains_key(*k)),
        "Missing required assignment fields"
    );
    Ok(Value::Object(result))
}
pub fn call(s: &Store, f: &str, name: &str, args: &Value) -> Result<Value> {
    let tools = tools();
    let def = tools
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == name)
        .context("Unknown tool")?;
    let schema = &def["inputSchema"];
    let arguments = args.as_object().context("Arguments must be an object")?;
    ensure!(
        arguments
            .keys()
            .all(|k| schema["properties"].get(k).is_some())
            && array(&schema["required"])
                .iter()
                .all(|k| arguments.contains_key(k.as_str().unwrap_or(""))),
        "Use exactly documented tool arguments"
    );
    match name {
        "foreman_submit" => s.submit(f, dispatch_args(name, args)?),
        "foreman_status" => s.status(f, args),
        "foreman_plan" => progress::set_plan(s, f, &args["items"]),
        "foreman_workflow" => s.workflow_submit(f, dispatch_args(name, args)?),
        "foreman_integrate" => crate::operator::integrate(s, f, args),
        "foreman_color" => crate::operator::color(s, f, args),
        "foreman_stop" => crate::control::request(s, f, args),
        "foreman_reconcile" => crate::reconcile::submit(s, f, args),
        "foreman_ack" => {
            s.ack(f, args["event_id"].as_str().context("Invalid event ID")?)?;
            Ok(json!({"acknowledged":true}))
        }
        _ => bail!("Unknown tool"),
    }
}
pub fn mcp(s: &Store, f: &str, input: impl BufRead, output: impl Write) -> Result<()> {
    mcp_configured(s, f, input, output, None)
}
fn mcp_configured(
    s: &Store,
    f: &str,
    mut input: impl BufRead,
    mut output: impl Write,
    config: Option<&Path>,
) -> Result<()> {
    s.binding(f)?;
    loop {
        let mut line = Vec::new();
        let n = std::io::Read::take(&mut input, 65537).read_until(b'\n', &mut line)?;
        if n == 0 {
            break;
        }
        if !line.ends_with(b"\n") && line.len() > 65536 {
            loop {
                let buf = input.fill_buf()?;
                if buf.is_empty() {
                    break;
                }
                let n = buf
                    .iter()
                    .position(|b| *b == b'\n')
                    .map(|i| i + 1)
                    .unwrap_or(buf.len());
                let done = buf[n - 1] == b'\n';
                input.consume(n);
                if done {
                    break;
                }
            }
        }
        let value = serde_json::from_slice::<Value>(&line)
            .ok()
            .filter(|v| v.is_object() && line.len() <= 65536);
        let response = if let Some(req) = value {
            if req.get("id").is_none() {
                continue;
            }
            let id = &req["id"];
            let params = req.get("params").cloned().unwrap_or(json!({}));
            if !params.is_object() {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":"Parameters must be an object"}})
            } else {
                let result = match text(&req, "method").as_str() {
                    "initialize" => {
                        let version = params["protocolVersion"].as_str().unwrap_or("");
                        json!({"protocolVersion":if ["2024-11-05","2025-03-26","2025-06-18","2025-11-25"].contains(&version){version}else{"2024-11-05"},"capabilities":{"tools":{}},"serverInfo":{"name":"Teem","version":env!("CARGO_PKG_VERSION")}})
                    }
                    "ping" => json!({}),
                    "tools/list" => json!({"tools":tools()}),
                    "tools/call" => {
                        let result = (|| {
                            let fresh = config
                                .map(|path| load(path).and_then(|c| open(&c)))
                                .transpose()?;
                            call(
                                fresh.as_ref().unwrap_or(s),
                                f,
                                &text(&params, "name"),
                                params.get("arguments").unwrap_or(&json!({})),
                            )
                        })();
                        match result {
                            Ok(data) => {
                                let rendered = if text(&params, "name") == "foreman_status" {
                                    serde_json::to_string_pretty(&data)?
                                } else {
                                    serde_json::to_string(&data)?
                                };
                                json!({"content":[{"type":"text","text":rendered}]})
                            }
                            Err(e) => {
                                json!({"isError":true,"content":[{"type":"text","text":e.to_string()}]})
                            }
                        }
                    }
                    _ => {
                        writeln!(
                            output,
                            "{}",
                            json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found"}})
                        )?;
                        output.flush()?;
                        continue;
                    }
                };
                json!({"jsonrpc":"2.0","id":id,"result":result})
            }
        } else {
            json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Invalid JSON request"}})
        };
        writeln!(output, "{response}")?;
        output.flush()?;
    }
    Ok(())
}
fn quote(s: &str) -> Result<String> {
    ensure!(
        !s.contains(['\n', '\r', '\0']),
        "Service paths cannot contain line breaks"
    );
    Ok(format!(
        "\"{}\"",
        s.replace('%', "%%")
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
    ))
}
pub fn service_unit(config: &Path) -> Result<String> {
    let exe = quote(&std::env::current_exe()?.to_string_lossy())?;
    let cfg = quote(&absolute(config)?.to_string_lossy())?;
    Ok(format!(
        "[Unit]\nDescription=Teem headless task supervisor\nAfter=network-online.target\n\n[Service]\nType=simple\nExecStart={exe} --config {cfg} serve\nRestart=on-failure\nRestartSec=5\nUMask=0077\nTimeoutStopSec=20\n\n[Install]\nWantedBy=default.target\n"
    ))
}
fn focused_agent() -> Result<Value> {
    let pane = std::env::var("HERDR_PANE_ID").context("Bind inside focused Herdr pane")?;
    ensure!(
        std::env::var("HERDR_ENV").ok().as_deref() == Some("1") && !pane.is_empty(),
        "Bind inside focused Herdr pane"
    );
    let out = Command::new(std::env::var_os("HERDR_BIN_PATH").unwrap_or("herdr".into()))
        .args(["agent", "get", &pane])
        .output()?;
    ensure!(
        out.status.success() && out.stdout.len() <= 1048576,
        "Cannot inspect focused Herdr agent"
    );
    Ok(serde_json::from_slice(&out.stdout)?)
}
pub fn bind(s: &Store) -> Result<Value> {
    let response = focused_agent()?;
    let a = &response["result"]["agent"];
    let native = &a["agent_session"];
    ensure!(
        a["agent"] == "opencode"
            && native["agent"] == "opencode"
            && native["kind"] == "id"
            && native["source"] == "herdr:opencode",
        "Cannot bind exact native OpenCode session"
    );
    s.bind(&text(native, "value"), &text(a, "terminal_id"))?;
    Ok(
        json!({"foreman":native["value"],"terminal_id":a["terminal_id"],"completion_transport":"native-runtime"}),
    )
}
pub fn serve(path: &Path, initial: &Value) -> Result<()> {
    if initial["enabled"] != true {
        println!("Foreman supervision is disabled");
        return Ok(());
    }
    let mut s = open(initial)?;
    let lock = open_lock(&s.directory.join("supervisor.lock"))?;
    // SAFETY: the file descriptor is valid and stays owned for the complete serve loop.
    ensure!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "Another supervisor owns this state directory"
    );
    let stop = Arc::new(AtomicBool::new(false));
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        signal_hook::flag::register(sig, stop.clone())?;
    }
    let mut runner = checks::Runner::default();
    while !stop.load(Ordering::Relaxed) {
        runner.reap();
        let cycle = (|| -> Result<()> {
            let c = load(path)?;
            ensure!(
                c["state_dir"] == initial["state_dir"],
                "State directory changed while service running"
            );
            if c["enabled"] != true {
                return Ok(());
            }
            s.projects = c["projects"].clone();
            s.trusted_worker_shell = c["trusted_worker_shell"] == true;
            s.allowed = array(&c["allowed_checkouts"])
                .iter()
                .map(|v| {
                    absolute(&expand(v.as_str().unwrap_or("")))
                        .map(|p| p.to_string_lossy().into_owned())
                })
                .collect::<Result<_>>()?;
            let runtime = OpenCode::new(&expand(&text(&c, "registry")), &text(&c, "model"))?;
            crate::control::tick(&s, &runtime)?;
            workflows::tick(&s, &runtime, &mut runner)?;
            runtime::tick(&s, &runtime)?;
            approvals::deliver(&s, &runtime)?;
            progress::read_agents(&s, &runtime)?;
            progress::publish(&s)?;
            Ok(())
        })();
        if cycle.is_err() {
            eprintln!("Foreman runtime unavailable. Reservations retained");
        }
        for _ in 0..20 {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
    Ok(())
}
pub fn status_options(args: &[String]) -> Result<Value> {
    ensure!(
        args.chunks_exact(2).remainder().is_empty(),
        "Status options require values"
    );
    let mut result = json!({});
    for pair in args.chunks_exact(2) {
        let key = match pair[0].as_str() {
            "--task-id" => "task_id",
            "--workflow-id" => "workflow_id",
            "--mode" => "mode",
            "--section" => "section",
            "--offset" => "offset",
            "--limit" => "limit",
            "--pointer" => "pointer",
            "--file" => "file",
            _ => bail!("Unknown status option"),
        };
        ensure!(result.get(key).is_none(), "Duplicate status option");
        result[key] = if ["offset", "limit"].contains(&key) {
            json!(
                pair[1]
                    .parse::<u64>()
                    .context("Use a nonnegative integer")?
            )
        } else {
            json!(pair[1])
        };
    }
    Ok(result)
}

pub fn main(args: Vec<String>) -> Result<()> {
    let mut config = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home().join(".config"))
                .join("teem")
        })
        .join("config.toml");
    let mut args = args.into_iter();
    let mut command = args.next().unwrap_or_else(|| "help".into());
    if command == "--config" {
        config = expand(&args.next().context("--config needs a path")?);
        command = args.next().context("Command required")?;
    }
    let rest = args.collect::<Vec<_>>();
    match command.as_str() {
        "--version" | "version" => {
            println!("teem {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        "help" | "--help" | "-h" => {
            println!(
                "teem [--config PATH] bind|status --foreman ID|mcp --foreman ID|board|open-status|serve|service-start|service-unit\nteem install-plugin\nstatus options: [--task-id ID | --workflow-id ID] [--mode summary|detail] [--section overview|workflows|tasks|events|approvals|projects|progress|agent_progress|progress_delivery|check_output] [--offset N] [--limit N] [--pointer JSON_POINTER]\nSummary limit 1..5 rows (default 2); detail limit 1..4096 UTF-8 JSON bytes (default 2048). Use returned next_offset. Status is bounded to 24000 pretty-JSON bytes. check_output requires detail, workflow-id, and pointer /data/rounds/N/check_result/results/M.\nNo command starts model work without enabled configuration."
            );
            return Ok(());
        }
        "install-plugin" => {
            let destination = std::env::current_dir()?.join("bin/teem");
            private_dir(destination.parent().unwrap())?;
            let data = regular(&std::env::current_exe()?, 512 * 1024 * 1024)?;
            let mut f = tempfile::NamedTempFile::new_in(destination.parent().unwrap())?;
            f.write_all(&data)?;
            use std::os::unix::fs::PermissionsExt;
            f.as_file()
                .set_permissions(fs::Permissions::from_mode(0o755))?;
            f.as_file().sync_all()?;
            f.persist(&destination).map_err(|e| e.error)?;
            println!(
                "Installed native plugin executable: {}",
                destination.display()
            );
            return Ok(());
        }
        "check-helper" => {
            ensure!(
                rest.len() == 1,
                "check-helper requires one private spec path"
            );
            checks::run(&read_json(Path::new(&rest[0]))?)?;
            return Ok(());
        }
        "service-unit" => {
            print!("{}", service_unit(&config)?);
            return Ok(());
        }
        "open-status" => {
            ensure!(
                std::env::var("HERDR_ENV").ok().as_deref() == Some("1"),
                "Open task status inside Herdr"
            );
            ensure!(
                Command::new(std::env::var_os("HERDR_BIN_PATH").unwrap_or("herdr".into()))
                    .args([
                        "plugin",
                        "pane",
                        "open",
                        "--plugin",
                        "local.teem",
                        "--entrypoint",
                        "status"
                    ])
                    .status()?
                    .success(),
                "Cannot open Teem status"
            );
            return Ok(());
        }
        _ => {}
    }
    if command == "service-start" && !config.exists() {
        println!("Foreman is not configured. No service was started");
        return Ok(());
    }
    ensure!(
        [
            "bind",
            "status",
            "mcp",
            "board",
            "serve",
            "service-start",
            "enable-chat",
            "native-tools",
            "native-call",
        ]
        .contains(&command.as_str()),
        "Unknown command"
    );
    let c = load(&config)?;
    if command == "native-tools" {
        ensure!(rest.is_empty(), "native-tools accepts no arguments");
        println!("{}", crate::enable::tools());
        return Ok(());
    }
    if command == "serve" {
        return serve(&config, &c);
    }
    if command == "service-start" {
        if c["enabled"] != true {
            println!("Foreman supervision is disabled");
        } else {
            ensure!(
                Command::new("systemctl")
                    .args(["--user", "start", "teem.service"])
                    .status()?
                    .success(),
                "Cannot start configured Teem service"
            );
        }
        return Ok(());
    }
    let s = open(&c)?;
    match command.as_str() {
        "enable-chat" | "native-call" => {
            ensure!(rest.is_empty(), "Native bridge accepts JSON on stdin only");
            let mut bytes = Vec::new();
            std::io::Read::take(std::io::stdin(), 65537).read_to_end(&mut bytes)?;
            ensure!(bytes.len() <= 65536, "Native bridge input too large");
            let request: Value = serde_json::from_slice(&bytes)?;
            let runtime = OpenCode::new(&expand(&text(&c, "registry")), &text(&c, "model"))
                .context("Configure an available native runtime registry and fallback model")?;
            let result = if command == "enable-chat" {
                crate::enable::enable(&s, &runtime, &request, &focused_agent()?)?
            } else {
                crate::enable::call(&s, &runtime, &request)?
            };
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        "bind" => println!("{}", bind(&s)?),
        "status" | "mcp" => {
            ensure!(
                rest.len() >= 2 && rest[0] == "--foreman",
                "Select --foreman ID first"
            );
            let f = &rest[1];
            if command == "mcp" {
                ensure!(rest.len() == 2, "MCP accepts only --foreman ID");
                mcp_configured(
                    &s,
                    f,
                    std::io::stdin().lock(),
                    std::io::stdout().lock(),
                    Some(&config),
                )?;
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&s.status(f, &status_options(&rest[2..])?)?)?
                );
            }
        }
        "board" => {
            let statuses = s
                .query("SELECT id FROM foremen ORDER BY id", [])?
                .iter()
                .map(|r| s.status(&text(r, "id"), &json!({})))
                .collect::<Result<Vec<_>>>()?;
            println!("{}", serde_json::to_string_pretty(&statuses)?);
            use std::io::IsTerminal;
            if std::io::stdin().is_terminal() {
                println!("\nPress Enter to close. Worker sessions remain headless.");
                let _ = std::io::stdin().read_line(&mut String::new());
            }
        }
        _ => unreachable!(),
    }
    Ok(())
}
