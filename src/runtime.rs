use crate::{
    checkouts,
    common::*,
    store::{LIVE, Store, TERMINAL},
};
use anyhow::{Context, Result, ensure};
use rusqlite::params;
use serde_json::{Value, json};
use std::{fmt, io::Read, path::Path, time::Duration};

#[derive(Debug)]
pub struct Missing;
impl fmt::Display for Missing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Exact native resource not found")
    }
}
impl std::error::Error for Missing {}
pub struct OpenCode {
    client: reqwest::blocking::Client,
    url: String,
    password: String,
    pub model: Value,
}
impl OpenCode {
    pub fn new(registry: &Path, selected: &str) -> Result<Self> {
        let record = read_json(registry)
            .map_err(|_| anyhow::anyhow!("Local service registry unavailable"))?;
        let url = reqwest::Url::parse(&text(&record, "url"))
            .map_err(|_| anyhow::anyhow!("Invalid local registry URL"))?;
        ensure!(
            url.scheme() == "http"
                && matches!(url.host_str(), Some("127.0.0.1" | "::1" | "[::1]"))
                && url.port_or_known_default().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && ["", "/"].contains(&url.path())
                && url.query().is_none()
                && url.fragment().is_none(),
            "Use a loopback-only OpenCode service registry"
        );
        let password = text(&record, "password");
        ensure!(!password.is_empty(), "Registry credentials missing");
        let (provider, name) = selected
            .split_once('/')
            .context("Use explicit provider/model")?;
        let (id, variant) = name
            .split_once('#')
            .map(|(a, b)| (a, Some(b)))
            .unwrap_or((name, None));
        let mut m = json!({"providerID":provider,"id":id});
        if let Some(v) = variant {
            m["variant"] = json!(v);
        }
        model(&m)?;
        Ok(Self {
            client: reqwest::blocking::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(10))
                .build()?,
            url: url.as_str().trim_end_matches('/').into(),
            password,
            model: m,
        })
    }
}
pub fn permissions(t: &Value) -> Value {
    let mut rules = vec![json!({"action":"*","resource":"*","effect":"deny"})];
    for action in ["read", "glob", "grep"] {
        rules.push(json!({"action":action,"resource":"*","effect":"allow"}));
    }
    if t["request"]["native_todos"] == true {
        rules.push(json!({"action":"todowrite","resource":"*","effect":"allow"}));
    }
    if t["role"] == "worker" {
        rules.push(
            json!({"action":"external_directory","resource":"/mnt/fastbuild/**","effect":"allow"}),
        );
        for p in array(&t["request"]["scope"]) {
            for resource in [
                p.as_str().unwrap_or("").to_owned(),
                format!("{}/**", p.as_str().unwrap_or("")),
            ] {
                rules.push(json!({"action":"edit","resource":resource,"effect":"allow"}));
            }
        }
        rules.push(json!({"action":"shell","resource":"*","effect":"ask"}));
    }
    for resource in [".git", ".git/**", "**/.git", "**/.git/**"] {
        rules.push(json!({"action":"edit","resource":resource,"effect":"deny"}));
    }
    json!(rules)
}
pub fn session_payload(t: &Value, default: &Value) -> Value {
    let mut metadata = json!({"task_id":t["id"],"role":t["role"],"revision":t["request"]["revision"],"scope":t["request"]["scope"]});
    if let Some(id) = t["request"].get("workflow_id") {
        metadata["workflow_id"] = id.clone();
        metadata["input_tree"] = t["request"]["input_tree"].clone();
        if t["request"]["native_todos"] == true {
            metadata["native_todos"] = json!(true);
        }
    }
    json!({"id":t["session_id"],"title":format!("Foreman {} {}",text(t,"role"),text(t,"id")),"agent":"build","model":if t["runtime_model"].is_null(){default}else{&t["runtime_model"]},"location":{"directory":t["checkout"]},"permissions":permissions(t),"metadata":{"herdr_foreman":metadata}})
}
pub fn validate_session(t: &Value, s: &Value, default: &Value) -> Result<()> {
    let expected = session_payload(t, default);
    for k in ["id", "location", "permissions", "agent"] {
        ensure!(s[k] == expected[k], "Native session {k} mismatch");
    }
    let mut model = s["model"].clone();
    let mut required = expected["model"].clone();
    ensure!(model.is_object(), "Native session model missing");
    if model.get("variant").is_none() {
        model["variant"] = json!("default");
    }
    if required.get("variant").is_none() {
        required["variant"] = json!("default");
    }
    ensure!(
        model == required
            && s["metadata"]["herdr_foreman"] == expected["metadata"]["herdr_foreman"],
        "Native session model or metadata mismatch"
    );
    Ok(())
}
pub fn prompt_text(t: &Value) -> String {
    format!(
        "Assigned {} task {}. Base commit: {}.\nFile scope: {}. Sparse checkout: {}.\nDo not delegate, merge, publish, deploy, or change runtime permissions. Report results, checks, and unresolved limitations. Execution success is not review approval.\n\n{}",
        text(t, "role"),
        text(t, "id"),
        text(&t["request"], "revision"),
        array(&t["request"]["scope"])
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", "),
        if t["sparse"] == true { "True" } else { "False" },
        text(&t["request"], "prompt")
    )
}
pub fn event_payload(e: &Value) -> Result<Value> {
    let id = uuid(&text(e, "request_id"))?.simple().to_string();
    let event = text(e, "id");
    if let Some(workflow) = e["workflow_id"].as_str() {
        return Ok(
            json!({"id":format!("msg_teem_workflow_{id}"),"text":format!("Teem workflow outcome event {event} for workflow {workflow}. Read foreman_status with this workflow_id and give the human one concise result summary: task title, completed or blocked/needs attention, recorded check/review outcomes, and any limitations. Ready means the exact candidate passed its configured checks and both reviews, not integrated. Then foreman_ack this event after consuming it. Do not refresh the checklist, dispatch agents, replay work, approve permissions, integrate, merge or publish. Progress is displayed independently; this is the final result notification, not a progress update."),"description":"Teem workflow finished","metadata":{"teem_event":event,"teem_workflow":workflow},"delivery":"queue","resume":true}),
        );
    }
    let task = text(e, "task_id");
    let refresh = if e["refresh_todos"] == true || e["refresh_todos"] == 1 {
        "If status has progress.todos, refresh the native todowrite checklist with that current snapshot. "
    } else {
        ""
    };
    Ok(
        json!({"id":format!("msg_teem_event_{id}"),"text":format!("Teem completion event {event} for task {task}. Use foreman_status to read the recorded result, then foreman_ack after consuming this event. {refresh}This event is not engineering approval or permission to merge."),"description":"Teem task completed","metadata":{"teem_event":event,"teem_task":task},"delivery":"queue","resume":true}),
    )
}
fn validate_event(e: &Value, m: &Value, queued: bool) -> Result<()> {
    let expected = event_payload(e)?;
    let body = if queued { &m["payload"] } else { m };
    ensure!(
        m["id"] == expected["id"]
            && m["type"] == "synthetic"
            && (!queued || m["sessionID"] == e["foreman"])
            && body["text"] == expected["text"]
            && body["metadata"] == expected["metadata"],
        "Native completion input mismatch"
    );
    Ok(())
}
pub trait Runtime {
    fn request(&self, method: &str, path: &str, payload: Option<&Value>) -> Result<Value>;
    fn default_model(&self) -> Value {
        Value::Null
    }
    fn directory(&self, f: &str) -> Result<String> {
        let s = self.request("GET", &format!("/api/session/{f}"), None)?;
        ensure!(
            s["id"] == f
                && s["location"]["directory"]
                    .as_str()
                    .is_some_and(|p| Path::new(p).is_absolute()),
            "Exact native Foreman location unavailable"
        );
        Ok(absolute(Path::new(&text(&s["location"], "directory")))?
            .to_string_lossy()
            .into_owned())
    }
    fn dispatch_model(&self, t: &Value) -> Result<Value> {
        let s = self.request("GET", &format!("/api/session/{}", text(t, "foreman")), None)?;
        ensure!(s["id"] == t["foreman"], "Foreman session mismatch");
        model(&s["model"])
    }
    fn create(&self, t: &Value) -> Result<()> {
        let p = session_payload(t, &self.default_model());
        let s = self.request("POST", "/api/session", Some(&p))?;
        validate_session(t, &s, &self.default_model())
    }
    fn prompt(&self, t: &Value) -> Result<()> {
        let p = json!({"id":t["prompt_id"],"text":prompt_text(t),"delivery":"queue","resume":true});
        let s = self.request(
            "POST",
            &format!("/api/session/{}/prompt", text(t, "session_id")),
            Some(&p),
        )?;
        ensure!(
            s["id"] == t["prompt_id"],
            "Native prompt admission ID mismatch"
        );
        Ok(())
    }
    fn inspect(&self, t: &Value) -> Result<Value> {
        let prefix = format!("/api/session/{}", text(t, "session_id"));
        let session = self.request("GET", &prefix, None)?;
        validate_session(t, &session, &self.default_model())?;
        let inbox = self.request("GET", &format!("{prefix}/inbox"), None)?;
        for item in inbox.as_array().context("Invalid native inbox")? {
            if item["id"] == t["prompt_id"] {
                ensure!(
                    item["type"] == "user" && item["payload"]["text"] == prompt_text(t),
                    "Native inbox payload mismatch"
                );
                return Ok(
                    json!({"state":"accepted","detail":"Native input queued. Execution not yet confirmed"}),
                );
            }
        }
        let prompt = match self.request(
            "GET",
            &format!("{prefix}/message/{}", text(t, "prompt_id")),
            None,
        ) {
            Err(e) if e.is::<Missing>() => {
                return Ok(
                    json!({"state":"uncertain","detail":"No matching native input. Do not replay automatically"}),
                );
            }
            v => v?,
        };
        ensure!(
            prompt["id"] == t["prompt_id"]
                && prompt["type"] == "user"
                && prompt["text"] == prompt_text(t),
            "Native task message mismatch"
        );
        let active = self.request("GET", "/api/session/active", None)?;
        ensure!(active.is_object(), "Invalid activity response");
        if active.get(text(t, "session_id")).is_some() {
            let permissions = self.request("GET", &format!("{prefix}/permission"), None)?;
            let forms = self.request("GET", &format!("{prefix}/form"), None)?;
            return Ok(
                json!({"state":"running","detail":if !array(&permissions).is_empty()||!array(&forms).is_empty(){"Requires human permission or answer. Check Foreman approval cards and approval status; inspect the native worker session for other questions"}else{"Native execution active"}}),
            );
        }
        let idle = session["time"]["idle"].as_f64().unwrap_or(0.0);
        let created = prompt["time"]["created"]
            .as_f64()
            .context("Native prompt time missing")?;
        let outcome = text(&session, "outcome");
        if idle <= created || !TERMINAL.contains(&outcome.as_str()) {
            return Ok(
                json!({"state":"uncertain","detail":"No terminal outcome after this input. Reservation retained"}),
            );
        }
        if outcome != "succeeded" {
            return Ok(json!({"state":outcome,"detail":"Native execution did not succeed"}));
        }
        let messages = self.request(
            "GET",
            &format!("{prefix}/message?limit=20&order=desc"),
            None,
        )?;
        for m in messages.as_array().context("Invalid messages")? {
            let time = m["time"]["created"].as_f64().unwrap_or(0.0);
            if m["type"] != "assistant"
                || !m["error"].is_null()
                || m["finish"] == "tool-call"
                || time < created
                || time > idle
                || m["time"]["completed"].as_f64().unwrap_or(0.0) == 0.0
            {
                continue;
            }
            let result = array(&m["content"])
                .iter()
                .filter(|p| p["type"] == "text")
                .map(|p| text(p, "text"))
                .collect::<Vec<_>>()
                .join("\n");
            if !result.trim().is_empty() {
                return Ok(json!({"state":"succeeded","result":short(&result,100000)}));
            }
        }
        Ok(
            json!({"state":"uncertain","detail":"Native success has no final text result in latest 20 messages"}),
        )
    }
    fn notify(&self, e: &Value) -> Result<()> {
        let payload = event_payload(e)?;
        let response = self.request(
            "POST",
            &format!("/api/session/{}/synthetic", text(e, "foreman")),
            Some(&payload),
        )?;
        validate_event(e, &response, true)
    }
    fn accepted(&self, e: &Value) -> Result<bool> {
        let expected = event_payload(e)?;
        let prefix = format!("/api/session/{}", text(e, "foreman"));
        match self.request(
            "GET",
            &format!("{prefix}/message/{}", text(&expected, "id")),
            None,
        ) {
            Ok(m) => {
                validate_event(e, &m, false)?;
                Ok(true)
            }
            Err(e2) if e2.is::<Missing>() => {
                let inbox = self.request("GET", &format!("{prefix}/inbox"), None)?;
                for item in inbox.as_array().context("Invalid native inbox")? {
                    ensure!(item.is_object(), "Invalid native inbox entry");
                    if item["id"] == expected["id"] {
                        validate_event(e, item, true)?;
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            Err(e) => Err(e),
        }
    }
    fn approvals(&self, t: &Value) -> Result<Vec<Value>> {
        if self.inspect(t)?["state"] != "running" {
            return Ok(vec![]);
        }
        let prefix = format!("/api/session/{}", text(t, "session_id"));
        let permissions = self.request("GET", &format!("{prefix}/permission"), None)?;
        let mut result = Vec::new();
        for p in permissions.as_array().context("Invalid permission list")? {
            let id = text(p, "id");
            let source = &p["source"];
            let message = text(source, "messageID");
            ensure!(
                p["sessionID"] == t["session_id"]
                    && safe_id(&id, "per_")
                    && p["action"].is_string()
                    && p["resources"].is_array()
                    && source["type"] == "tool"
                    && safe_id(&message, "msg_"),
                "Invalid native permission identity"
            );
            let m = self.request("GET", &format!("{prefix}/message/{message}"), None)?;
            ensure!(
                m["id"] == message && m["type"] == "assistant" && m["content"].is_array(),
                "Invalid native permission tool message"
            );
            let parts = array(&m["content"])
                .into_iter()
                .filter(|p| p["type"] == "tool" && p["id"] == source["id"])
                .collect::<Vec<_>>();
            ensure!(
                parts.len() == 1 && parts[0]["state"]["input"].is_object(),
                "Native tool input unavailable"
            );
            result.push(json!({"permission":p,"input":parts[0]["state"]["input"]}));
        }
        Ok(result)
    }
    fn todos(&self, t: &Value) -> Result<Option<Value>> {
        let prefix = format!("/api/session/{}", text(t, "session_id"));
        validate_session(
            t,
            &self.request("GET", &prefix, None)?,
            &self.default_model(),
        )?;
        let messages = self.request(
            "GET",
            &format!("{prefix}/message?limit=50&order=desc&type=assistant"),
            None,
        )?;
        for m in messages.as_array().context("Invalid progress messages")? {
            if m["type"] != "assistant" || !m["error"].is_null() {
                continue;
            }
            for part in m["content"]
                .as_array()
                .context("Invalid progress content")?
                .iter()
                .rev()
            {
                if part["type"] != "tool"
                    || part["name"] != "todowrite"
                    || part["state"]["status"] != "completed"
                {
                    continue;
                }
                ensure!(m["id"].is_string(), "Invalid todo report");
                let mut todos = Vec::new();
                for todo in part["state"]["input"]["todos"]
                    .as_array()
                    .context("Invalid todo report")?
                    .iter()
                    .take(100)
                {
                    ensure!(
                        todo["content"].is_string()
                            && ["pending", "in_progress", "completed", "cancelled"]
                                .contains(&text(todo, "status").as_str()),
                        "Invalid todo entry"
                    );
                    todos.push(json!({"content":short(&compact(&text(todo,"content")),300),"status":todo["status"]}));
                }
                return Ok(Some(json!({"message_id":m["id"],"todos":todos})));
            }
        }
        Ok(None)
    }
}
fn safe_id(id: &str, prefix: &str) -> bool {
    id.starts_with(prefix) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
impl Runtime for OpenCode {
    fn default_model(&self) -> Value {
        self.model.clone()
    }
    fn request(&self, method: &str, path: &str, payload: Option<&Value>) -> Result<Value> {
        ensure!(
            path.starts_with("/api/") && !path.contains(['\n', '\r']),
            "Invalid native API path"
        );
        let mut req = self
            .client
            .request(
                reqwest::Method::from_bytes(method.as_bytes())?,
                format!("{}{path}", self.url),
            )
            .basic_auth("opencode", Some(&self.password));
        if let Some(p) = payload {
            req = req.json(p);
        }
        let response = req.send().map_err(|_| {
            anyhow::anyhow!("OpenCode response unavailable; acceptance unconfirmed")
        })?;
        if response.status() == 404 {
            return Err(Missing.into());
        }
        ensure!(
            response.status().is_success(),
            "OpenCode rejected request; inspect native state before retrying"
        );
        if response.status() == 204 {
            return Ok(Value::Null);
        }
        let mut bytes = Vec::new();
        response
            .take(1048577)
            .read_to_end(&mut bytes)
            .map_err(|_| anyhow::anyhow!("OpenCode response unavailable"))?;
        ensure!(bytes.len() <= 1048576, "OpenCode response too large");
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| anyhow::anyhow!("Invalid OpenCode response"))?;
        Ok(value
            .get("data")
            .context("Invalid OpenCode response envelope")?
            .clone())
    }
}
pub fn tick(store: &Store, runtime: &dyn Runtime) -> Result<()> {
    for task in store.tasks(None)? {
        if !LIVE.contains(&text(&task, "state").as_str()) {
            continue;
        }
        let outcome = (|| -> Result<Value> {
            let outcome = runtime.inspect(&task)?;
            if TERMINAL.contains(&text(&outcome, "state").as_str()) && task["role"] == "reviewer" {
                checkouts::task_checkout(&task)?;
            }
            Ok(outcome)
        })();
        match outcome {
            Ok(o) => store.update(
                &text(&task, "id"),
                &text(&o, "state"),
                &text(&o, "result"),
                &text(&o, "detail"),
            )?,
            Err(_) => store.update(
                &text(&task, "id"),
                "uncertain",
                "",
                "Runtime inspection failed. Reservation retained",
            )?,
        }
    }
    while let Some(t) = store.claim()? {
        let validation = (|| -> Result<()> {
            ensure!(
                store.allowed(&text(&t, "checkout"), &text(&t, "foreman"))?,
                "Checkout authorisation removed"
            );
            checkouts::task_checkout(&t)?;
            Ok(())
        })();
        if let Err(e) = validation {
            store.update(&text(&t, "id"), "failed", "", &e.to_string())?;
            continue;
        }
        let result = (|| -> Result<()> {
            let task = store.record_model(&text(&t, "id"), &runtime.dispatch_model(&t)?)?;
            runtime.create(&task)?;
            runtime.prompt(&task)?;
            Ok(())
        })();
        store.update(
            &text(&t, "id"),
            if result.is_ok() {
                "accepted"
            } else {
                "uncertain"
            },
            "",
            if result.is_ok() {
                ""
            } else {
                "Worker setup/admission uncertain. No automatic replay"
            },
        )?;
    }
    notify(store, runtime)
}
pub fn notify(s: &Store, r: &dyn Runtime) -> Result<()> {
    let events=s.query("SELECT events.* FROM events JOIN foremen ON foremen.id=events.foreman WHERE events.acked=0 AND events.transport='native' AND (events.state='pending' OR (events.state IN ('attempting','uncertain') AND events.next_receipt<=?)) ORDER BY CASE WHEN events.state='pending' THEN 0 ELSE 1 END,events.next_receipt,events.rowid LIMIT 8",[now()])?;
    for e in events {
        if e["state"] == "pending" {
            if let Ok(t) = s.task(&text(&e, "task_id")) {
                if !t["request"]["workflow_id"].is_null() {
                    s.exec(
                        "UPDATE events SET state='passive' WHERE id=? AND state='pending'",
                        [text(&e, "id")],
                    )?;
                    continue;
                }
            }
            if s.exec("UPDATE events SET state='attempting' WHERE id=? AND acked=0 AND state='pending' AND transport='native'",[text(&e,"id")])?==0{continue}
        }
        let admitted = if e["state"] == "pending" {
            r.notify(&e).map(|_| true)
        } else {
            r.accepted(&e)
        };
        let queued = admitted.unwrap_or(false);
        s.exec(
            "UPDATE events SET state=?,detail=?,next_receipt=? WHERE id=? AND acked=0",
            params![
                if queued { "queued" } else { "uncertain" },
                if queued {
                    ""
                } else {
                    "Native completion delivery uncertain. No automatic resend"
                },
                now() + 60.0,
                text(&e, "id")
            ],
        )?;
    }
    Ok(())
}
