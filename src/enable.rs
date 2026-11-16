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
    let receipt = json!({"foreman":id,"terminal_id":a["terminal_id"],
        "directory":n["location"]["directory"],"project_id":n["projectID"],
        "project":project,"agent":n["agent"],"model":selected,"native_model":observed});
    s.transaction(|| {
        if let Some(binding) = s.query("SELECT * FROM foremen WHERE id=?", [&id])?.first() {
            ensure!(
                binding["terminal_id"] == receipt["terminal_id"]
                    || binding["terminal_id"] == format!("native-session:{id}"),
                "Existing terminal binding changed; inspect it before enabling"
            );
        }
        if let Some(old) = s
            .query("SELECT * FROM chat_enablements WHERE foreman=?", [&id])?
            .first()
        {
            for k in ["terminal_id", "directory", "project_id", "project", "agent"] {
                ensure!(
                    old[k] == receipt[k] || (k == "project" && old[k] == ""),
                    "Existing enabled chat {k} changed; inspect it before enabling"
                );
            }
        }
        s.exec(
            "INSERT OR IGNORE INTO foremen(id,terminal_id) VALUES (?,?)",
            params![id, text(&receipt, "terminal_id")],
        )?;
        // Replace only our native-session marker with the verified Herdr terminal.
        s.exec("UPDATE foremen SET terminal_id=? WHERE id=? AND terminal_id=?",
            params![text(&receipt, "terminal_id"), id, format!("native-session:{id}")])?;
        s.exec(
            "INSERT INTO chat_enablements(foreman,terminal_id,directory,project_id,project,agent,model,native_model) VALUES (?,?,?,?,?,?,?,?) ON CONFLICT(foreman) DO UPDATE SET project=excluded.project,model=excluded.model,native_model=excluded.native_model",
            params![
                id,
                text(&receipt, "terminal_id"),
                text(&receipt, "directory"),
                text(&receipt, "project_id"),
                project,
                text(&receipt, "agent"),
                canonical(&selected),
                canonical(&json!(observed))
            ],
        )?;
        Ok(())
    })?;
    Ok(
        json!({"enabled":true,"sessionID":id,"project":project,"terminal_id":a["terminal_id"],"started":false}),
    )
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
    json!(
        array(&plugin::tools())
            .into_iter()
            .filter(|t| t["name"] != "teem_submit")
            .collect::<Vec<_>>()
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
