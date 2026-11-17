//! Native chat authority; optional model capture and fixed MCP remain independent.
use crate::{common::*, plugin, runtime::Runtime, store::Store};
use anyhow::{Context, Result, ensure};
use rusqlite::params;
use serde_json::{Value, json};
use std::path::Path;

fn native(r: &dyn Runtime, id: &str) -> Result<Value> {
    ensure!(
        id.starts_with("ses_")
            && id.len() > 4
            && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
        "Select an exact native OpenCode session ID"
    );
    let response = r.request("GET", &format!("/api/session/{id}"), None)?;
    let s = response.get("data").unwrap_or(&response);
    ensure!(
        s["id"] == id && s["parentID"].is_null(),
        "Enable only the exact top-level chat, not a worker or child session"
    );
    for key in ["agent", "projectID"] {
        ensure!(
            s[key].as_str().is_some_and(|v| !v.is_empty()),
            "Native chat {key} unavailable; refresh the current chat"
        );
    }
    ensure!(
        s["location"]["directory"]
            .as_str()
            .is_some_and(|p| Path::new(p).is_absolute()),
        "Native chat directory unavailable"
    );
    Ok(s.clone())
}

fn projects_for(s: &Store, n: &Value) -> Result<Vec<String>> {
    let directory = Path::new(&text(&n["location"], "directory")).canonicalize()?;
    Ok(s.projects
        .as_object()
        .context("Invalid project policies")?
        .iter()
        .filter(|(_, p)| {
            Path::new(&text(p, "path")).canonicalize().ok().as_ref() == Some(&directory)
        })
        .map(|(name, _)| name.clone())
        .collect())
}

fn execution_project(s: &Store, receipt: &Value) -> Result<String> {
    let matches = projects_for(s, &receipt["native"])?;
    ensure!(
        matches.len() == 1,
        "Teem is available in this chat; delegation requires exactly one project policy with approved checks for its directory"
    );
    ensure!(
        text(receipt, "project").is_empty() || receipt["project"] == matches[0],
        "Enabled project policy removed or directory changed"
    );
    Ok(matches[0].clone())
}

pub fn enable(s: &Store, r: &dyn Runtime, request: &Value, herdr: &Value) -> Result<Value> {
    enable_observed(s, r, request, herdr, None)
}

fn enable_observed(
    s: &Store,
    r: &dyn Runtime,
    request: &Value,
    herdr: &Value,
    expected: Option<&Value>,
) -> Result<Value> {
    let fields = ["sessionID", "agent", "projectID", "directory", "model"];
    let args = request
        .as_object()
        .context("Enable requires chat identity")?;
    ensure!(
        args.len() == fields.len() && fields.iter().all(|k| args.contains_key(*k)),
        "Enable requires sessionID, agent, projectID, directory and selected model"
    );
    let id = text(request, "sessionID");
    let n = native(r, &id)?;
    if let Some(expected) = expected {
        for field in ["agent", "projectID", "location", "model"] {
            ensure!(
                n[field] == expected[field],
                "Native chat identity or model changed during enablement refresh"
            );
        }
    }
    let a = &herdr["result"]["agent"];
    let h = &a["agent_session"];
    ensure!(
        a["agent"] == "opencode"
            && h["agent"] == "opencode"
            && h["kind"] == "id"
            && h["source"] == "herdr:opencode"
            && h["value"] == id
            && a["terminal_id"].as_str().is_some_and(|v| !v.is_empty()),
        "Herdr pane must identify this exact native OpenCode chat and terminal"
    );
    ensure!(
        request["agent"] == n["agent"]
            && request["projectID"] == n["projectID"]
            && request["directory"] == n["location"]["directory"],
        "Current chat identity changed; run /teem enable again in the intended chat"
    );
    let matches = projects_for(s, &n)?;
    ensure!(
        matches.len() <= 1,
        "Configure at most one managed project alias for this chat directory"
    );
    let project = matches.first().map(String::as_str).unwrap_or("");
    let selected = model(&request["model"])?;
    let observed = n
        .get("model")
        .filter(|m| !m.is_null())
        .map(model)
        .transpose()?;
    let mut receipt = json!({"foreman":id,"terminal_id":a["terminal_id"],
        "directory":n["location"]["directory"],"project_id":n["projectID"],
        "project":project,"agent":n["agent"],"model":selected,"native_model":observed});
    let mut refreshed = false;
    s.transaction(|| {
        let bindings = s.query("SELECT * FROM foremen WHERE id=?", [&id])?;
        let old = s
            .query("SELECT * FROM chat_enablements WHERE foreman=?", [&id])?
            .into_iter().next();
        if let Some(old) = &old {
            // Same-chat enablement is not execution admission. Preserve the old
            // policy association if no policy now matches the unchanged home;
            // execution_project still rejects delegation from that directory.
            if matches.is_empty() {
                receipt["project"] = old["project"].clone();
            }
            for k in ["directory", "project_id", "project"] {
                ensure!(
                    old[k] == receipt[k] || (k == "project" && old[k] == ""),
                    "Existing enabled chat {k} changed; inspect it before enabling"
                );
            }
            ensure!(bindings.first().is_some_and(|b| b["terminal_id"] == old["terminal_id"]),
                "Stored enablement and terminal binding disagree; inspect before refreshing");
            refreshed = old["terminal_id"] != receipt["terminal_id"] || old["agent"] != receipt["agent"];
            if refreshed {
                ensure!(old["agent"] == receipt["agent"] ||
                    (receipt["agent"] == "teem" && matches!(old["agent"].as_str(),Some("build"|"teem_foreman"))),
                    "Unsupported enabled-agent change; inspect the intended chat before refreshing");
                validate_refresh_roster(herdr, &id, &text(old,"terminal_id"))?;
                let current = native(r, &id)?;
                ensure!(current["agent"] == n["agent"] && current["projectID"] == n["projectID"]
                    && current["location"] == n["location"] && current["model"] == n["model"],
                    "Native chat identity or model changed during enablement refresh");
            }
        }
        if let Some(binding) = bindings.first() {
            ensure!(refreshed || binding["terminal_id"] == receipt["terminal_id"]
                || binding["terminal_id"] == format!("native-session:{id}"),
                "Existing terminal binding changed; inspect it before enabling");
        }
        s.exec(
            "INSERT OR IGNORE INTO foremen(id,terminal_id) VALUES (?,?)",
            params![id, text(&receipt, "terminal_id")],
        )?;
        // Replace only our native-session marker with the verified Herdr terminal.
        s.exec("UPDATE foremen SET terminal_id=? WHERE id=? AND terminal_id=?",
            params![text(&receipt, "terminal_id"), id, format!("native-session:{id}")])?;
        if refreshed {
            s.exec("UPDATE foremen SET terminal_id=? WHERE id=? AND terminal_id=?",
                params![text(&receipt,"terminal_id"),id,text(old.as_ref().unwrap(),"terminal_id")])?;
        }
        s.exec(
            "INSERT INTO chat_enablements(foreman,terminal_id,directory,project_id,project,agent,model,native_model) VALUES (?,?,?,?,?,?,?,?) ON CONFLICT(foreman) DO UPDATE SET terminal_id=excluded.terminal_id,agent=excluded.agent,project=excluded.project,model=excluded.model,native_model=excluded.native_model",
            params![
                id,
                text(&receipt, "terminal_id"),
                text(&receipt, "directory"),
                text(&receipt, "project_id"),
                text(&receipt,"project"),
                text(&receipt, "agent"),
                canonical(&selected),
                canonical(&json!(observed))
            ],
        )?;
        if refreshed {
            let current=s.query("SELECT * FROM chat_enablements WHERE foreman=?",[&id])?;
            s.exec("INSERT INTO chat_enablement_refreshes(foreman,previous_receipt,current_receipt,created) VALUES (?,?,?,?)",
                params![id,canonical(old.as_ref().unwrap()),canonical(&current[0]),now()])?;
        }
        Ok(())
    })?;
    Ok(
        json!({"enabled":true,"sessionID":id,"project":receipt["project"],"terminal_id":a["terminal_id"],"refreshed":refreshed,"started":false}),
    )
}

fn validate_refresh_roster(herdr: &Value, id: &str, previous_terminal: &str) -> Result<()> {
    let current = &herdr["result"]["agent"];
    let roster = herdr["result"]["agents"].as_array().context(
        "Enablement refresh needs a fresh Herdr pane roster; run /teem enable in the intended chat",
    )?;
    let mut terminals = std::collections::BTreeSet::new();
    for pane in roster {
        let terminal = pane["terminal_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .context("Invalid Herdr pane roster; refresh refused")?;
        ensure!(
            terminals.insert(terminal),
            "Duplicate terminal in Herdr pane roster; refresh refused"
        );
    }
    let same_chat = roster
        .iter()
        .filter(|p| p["agent_session"]["value"] == id)
        .collect::<Vec<_>>();
    ensure!(
        same_chat.len() == 1
            && same_chat[0]["terminal_id"] == current["terminal_id"]
            && same_chat[0]["agent"] == current["agent"]
            && same_chat[0]["agent_session"] == current["agent_session"],
        "Herdr must show one exact current pane for this native chat; refresh refused"
    );
    ensure!(
        current["terminal_id"] == previous_terminal || !terminals.contains(previous_terminal),
        "Previous terminal is still live; explicit pane handoff is required before refreshing"
    );
    Ok(())
}

/// Explicit operator migration, never an implicit relaxation of native authority.
pub fn migrate_agent(s: &Store, r: &dyn Runtime, request: &Value) -> Result<Value> {
    let fields = ["sessionID", "fromAgent", "toAgent"];
    let args = request
        .as_object()
        .context("Migration requires exact chat identity")?;
    ensure!(
        args.len() == fields.len()
            && fields.iter().all(|key| args.contains_key(*key))
            && request["fromAgent"] == "teem_foreman"
            && request["toAgent"] == "teem",
        "Only an explicit teem_foreman to teem receipt migration is supported"
    );
    let id = text(request, "sessionID");
    let observed = native(r, &id)?;
    ensure!(
        observed["agent"] == "teem",
        "Select teem in the intended chat before migration"
    );
    s.transaction(|| {
        let old = s
            .query("SELECT * FROM chat_enablements WHERE foreman=?", [&id])?
            .into_iter()
            .next()
            .context("This chat has no enablement receipt to migrate")?;
        ensure!(
            (old["agent"] == "teem_foreman" || old["agent"] == "teem")
                && old["project_id"] == observed["projectID"]
                && old["directory"] == observed["location"]["directory"]
                && old["terminal_id"] == s.binding(&id)?["terminal_id"],
            "Enabled chat project, directory, terminal or previous agent changed; migration refused"
        );
        let current = native(r, &id)?;
        ensure!(
            current["agent"] == observed["agent"]
                && current["projectID"] == observed["projectID"]
                && current["location"] == observed["location"]
                && current["model"] == observed["model"],
            "Native chat identity or model changed during migration"
        );
        s.exec(
            "UPDATE chat_enablements SET agent='teem' WHERE foreman=? AND agent='teem_foreman'",
            [&id],
        )?;
        Ok(())
    })?;
    Ok(json!({"sessionID":id,"agent":"teem","migrated":true,"started":false}))
}

pub fn authority(s: &Store, r: &dyn Runtime, id: &str, caller_agent: &str) -> Result<Value> {
    let n = native(r, id)?;
    ensure!(
        n["agent"] == caller_agent,
        "Native caller agent does not match this chat"
    );
    let stored = s
        .query("SELECT * FROM chat_enablements WHERE foreman=?", [id])?
        .into_iter()
        .next();
    let Some(mut receipt) = stored else {
        // Native tools already carry authenticated session identity. No Herdr
        // pane or managed execution policy is needed for session-local access.
        s.exec(
            "INSERT OR IGNORE INTO foremen(id,terminal_id) VALUES (?,?)",
            params![id, format!("native-session:{id}")],
        )?;
        // Native admission can preserve a real Herdr terminal binding. Keep its
        // provenance separately; the terminal value is not an access-mode tag.
        s.exec(
            "INSERT OR IGNORE INTO native_chats(foreman) VALUES (?)",
            [id],
        )?;
        return Ok(json!({"foreman":id,"project":"","native":n}));
    };
    ensure!(
        receipt["agent"] == caller_agent
            && n["agent"] == receipt["agent"]
            && n["projectID"] == receipt["project_id"]
            && n["location"]["directory"] == receipt["directory"]
            && s.binding(id)?["terminal_id"] == receipt["terminal_id"],
        "Enabled chat identity or caller agent changed; inspect and enable the intended chat"
    );
    for k in ["model", "native_model"] {
        receipt[k] = serde_json::from_str(receipt[k].as_str().context("Invalid enable receipt")?)?;
    }
    receipt["native"] = n;
    Ok(receipt)
}

pub fn tools() -> Value {
    let mut tools = array(&plugin::tools())
        .into_iter()
        .filter(|t| t["name"] != "teem_submit")
        .collect::<Vec<_>>();
    tools.push(json!({"name":"teem_refresh_enablement","description":"Repair this authenticated chat's stale enablement after terminal restoration or selection of Teem. Retains model selection, permissions and workflow ownership; verifies native identity and a complete Herdr pane roster. Starts no work; no repeated user approval is needed for same-chat repair.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}}));
    tools.push(json!({"name":"teem_issue_status","description":"Find historical workflows and stop evidence for an exact issue in this chat's native home directory, even without /teem enable. Read-only coordination metadata includes the responsible coordinator, workflow/task IDs and recorded stop states. Does not grant full owner access, confirm live termination, release a hold or authorise management/replay. Use this instead of asking the user to find an old workflow ID.","inputSchema":{"type":"object","properties":{"issue":{"type":"string","description":"Exact owner/repo#positive-number"},"offset":{"type":"integer","minimum":0,"maximum":1000000}},"required":["issue"],"additionalProperties":false}}));
    json!(tools)
}

pub struct RefreshRequest {
    pub request: Value,
    observed: Value,
}

pub fn refresh_request(s: &Store, r: &dyn Runtime, envelope: &Value) -> Result<RefreshRequest> {
    ensure!(
        envelope.as_object().is_some_and(|a| a.len() == 4
            && ["sessionID", "agent", "name", "arguments"]
                .iter()
                .all(|k| a.contains_key(*k)))
            && envelope["name"] == "teem_refresh_enablement"
            && envelope["arguments"]
                .as_object()
                .is_some_and(|a| a.is_empty()),
        "Use the authenticated same-chat refresh with no arguments"
    );
    let id = text(envelope, "sessionID");
    let n = native(r, &id)?;
    ensure!(
        n["agent"] == envelope["agent"],
        "Native caller agent does not match this chat"
    );
    let old = s
        .query("SELECT * FROM chat_enablements WHERE foreman=?", [&id])?
        .into_iter()
        .next()
        .context("No existing enablement receipt to refresh")?;
    let captured: Value =
        serde_json::from_str(old["model"].as_str().context("Invalid enable receipt")?)?;
    let baseline: Value = serde_json::from_str(
        old["native_model"]
            .as_str()
            .context("Invalid enable receipt")?,
    )?;
    // Preserve dispatch_model's effective selection, including a user change
    // made since capture, before enable establishes a new native baseline.
    let selected = model(if n["model"] != baseline {
        &n["model"]
    } else {
        &captured
    })?;
    Ok(RefreshRequest {
        request: json!({"sessionID":id,"agent":n["agent"],"projectID":n["projectID"],"directory":n["location"]["directory"],"model":selected}),
        observed: n,
    })
}

pub fn refresh(
    s: &Store,
    r: &dyn Runtime,
    request: &RefreshRequest,
    herdr: &Value,
) -> Result<Value> {
    enable_observed(s, r, &request.request, herdr, Some(&request.observed))
}

/// Operator CLI repair of an explicitly selected chat, never a native tool target.
pub fn operator_refresh_request(s: &Store, r: &dyn Runtime, id: &str) -> Result<RefreshRequest> {
    let n = native(r, id)?;
    refresh_request(
        s,
        r,
        &json!({"sessionID":id,"agent":n["agent"],"name":"teem_refresh_enablement","arguments":{}}),
    )
}

pub fn call(s: &Store, r: &dyn Runtime, envelope: &Value) -> Result<Value> {
    let args = envelope
        .as_object()
        .context("Native tool context required")?;
    ensure!(
        args.len() == 4
            && ["sessionID", "agent", "name", "arguments"]
                .iter()
                .all(|k| args.contains_key(*k)),
        "Use the trusted native tool context"
    );
    let id = text(envelope, "sessionID");
    let receipt = authority(s, r, &id, &text(envelope, "agent"))?;
    let name = plugin::legacy_tool_name(&text(envelope, "name"));
    ensure!(
        name != "foreman_submit",
        "Native chats use managed teem_workflow, not legacy teem_submit"
    );
    let input = &envelope["arguments"];
    if name == "foreman_issue_status" {
        return issue_status(s, &receipt, input);
    }
    // A retained recovery/setup-handoff retry is inspection, not renewed launch admission.
    // Authentication and immutable-argument/owner validation still apply.
    if name == "foreman_recover_qualification"
        && !s
            .query(
                "SELECT id FROM workflows WHERE id=?",
                [text(input, "recovery_id")],
            )?
            .is_empty()
    {
        return plugin::call(s, &id, &name, input);
    }
    if name == "foreman_reconcile"
        && input.get("setup_snapshot").is_some()
        && !s
            .query(
                "SELECT id FROM workflows WHERE id=?",
                [text(input, "workflow_id")],
            )?
            .is_empty()
    {
        return plugin::call(s, &id, &name, input);
    }
    if name == "foreman_workflow" {
        // Authenticate the chat's home policy without restricting the requested
        // target. Managed admission validates that target's configured policy.
        execution_project(s, &receipt)?;
    }
    if name == "foreman_reconcile" || name == "foreman_recover_qualification" {
        let source = s.workflow(&text(input, "source_workflow_id"))?;
        let owner = crate::handoff::authorize(s, &id, &source, false)?;
        // An explicit handoff authorises only this retained task, even when
        // the receiving chat is elsewhere. New workflows still need a home policy.
        if owner == id {
            execution_project(s, &receipt)?;
        }
    }
    // Inspection remains available even for retained evidence from other projects.
    plugin::call(s, &id, &name, input)
}

fn issue_status(s: &Store, receipt: &Value, input: &Value) -> Result<Value> {
    ensure!(
        input.as_object().is_some_and(|a| a.contains_key("issue")
            && a.keys().all(|k| ["issue", "offset"].contains(&k.as_str()))),
        "Use only issue and optional offset for historical discovery"
    );
    let issue = issue_reference(&input["issue"])?;
    let offset = if input.get("offset").is_some() {
        input["offset"]
            .as_i64()
            .filter(|n| (0..=1000000).contains(n))
            .context("Invalid discovery offset")?
    } else {
        0
    };
    // Discovery is project-local metadata, not an alternate route into another
    // coordinator's evidence or authority. The caller cannot supply a directory.
    let directory = Path::new(&text(&receipt["native"]["location"], "directory")).canonicalize()?;
    let rows = s.query("SELECT id FROM workflows WHERE json_extract(request,'$.issue')=? AND json_extract(policy,'$.path')=? ORDER BY created DESC,id LIMIT 6 OFFSET ?",params![issue,directory.to_str().context("Invalid native directory")?,offset])?;
    let more = rows.len() > 5;
    let mut workflows = Vec::new();
    for row in rows.iter().take(5) {
        let id = text(row, "id");
        let workflow = crate::handoff::metadata(s, &id)?;
        let manager = crate::handoff::manager(s, &workflow)?;
        // Aggregate in SQLite instead of loading private, potentially unbounded
        // stop targets. Match control::snapshot's recorded-state precedence.
        let stops = s.query("SELECT ws.requested,(SELECT CASE WHEN coalesce(sum(st.state='uncertain'),0)>0 THEN 'uncertain' WHEN coalesce(sum(st.state!='stopped'),0)>0 THEN 'requested' ELSE 'stopped' END FROM task_stops st JOIN tasks t ON t.id=st.task_id WHERE json_extract(t.request,'$.workflow_id')=ws.workflow_id) AS state FROM workflow_stops ws WHERE ws.workflow_id=?",[&id])?;
        let stop = stops.first().cloned().unwrap_or(Value::Null);
        // Never expose prompts, results, paths or private review contents through
        // discovery. Full evidence and all mutations retain normal owner checks.
        let tasks = s.query("SELECT t.id,t.role,t.state,st.state AS stop_state FROM tasks t LEFT JOIN task_stops st ON st.task_id=t.id WHERE json_extract(t.request,'$.workflow_id')=? ORDER BY t.created,t.id LIMIT 11",[&id])?;
        workflows.push(json!({"id":id,"project":workflow["project"],"state":workflow["state"],"responsible_coordinator":manager,
            "workflow_paused":!stops.is_empty(),"stop_state":stop["state"],"stop_requested_at":stop["requested"],
            "tasks":tasks.iter().take(10).collect::<Vec<_>>(),"tasks_truncated":tasks.len()>10}));
    }
    Ok(
        json!({"issue":issue,"read_only":true,"workflows":workflows,"has_more":more,"next_offset":if more { json!(offset+5) } else { Value::Null },
        "evidence":"Recorded coordination metadata only; absence of a live session or observed running task is not confirmation of termination.",
        "responsibility":"Use the recorded responsible coordinator for full evidence and stop recovery. Discovery does not transfer ownership, release a hold or authorise replacement. Do not ask the user to locate IDs available here."}),
    )
}

pub fn dispatch_model(s: &Store, r: &dyn Runtime, task: &Value) -> Result<Value> {
    let id = text(task, "foreman");
    let rows = s.query("SELECT agent FROM chat_enablements WHERE foreman=?", [&id])?;
    let Some(row) = rows.first() else {
        if !s
            .query("SELECT foreman FROM native_chats WHERE foreman=?", [&id])?
            .is_empty()
            || s.query("SELECT terminal_id FROM foremen WHERE id=?", [&id])?
                .first()
                .is_some_and(|binding| binding["terminal_id"] == format!("native-session:{id}"))
        {
            let n = native(r, &id)?;
            execution_project(s, &json!({"project":"","native":n}))?;
            return model(&n["model"]);
        }
        return r.dispatch_model(task);
    };
    let receipt = authority(s, r, &id, &text(row, "agent"))?;
    execution_project(s, &receipt)?;
    let current = &receipt["native"]["model"];
    model(if *current != receipt["native_model"] {
        current
    } else {
        &receipt["model"]
    })
}

/// Native chats authenticate against their home policy, not a task's target.
/// Fixed MCP callers retain their original exact-directory contract.
pub fn validate_execution(s: &Store, r: &dyn Runtime, foreman: &str, target: &str) -> Result<()> {
    let enabled = !s
        .query(
            "SELECT foreman FROM chat_enablements WHERE foreman=?",
            [foreman],
        )?
        .is_empty();
    let automatic = s.binding(foreman)?["terminal_id"] == format!("native-session:{foreman}");
    let native = !s
        .query(
            "SELECT foreman FROM native_chats WHERE foreman=?",
            [foreman],
        )?
        .is_empty();
    if enabled || automatic || native {
        dispatch_model(s, r, &json!({"foreman":foreman}))?;
    } else {
        ensure!(
            r.directory(foreman)? == target,
            "Teem coordinator location differs from approved project"
        );
    }
    Ok(())
}
