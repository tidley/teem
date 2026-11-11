//! Explicit per-chat native authority; the fixed MCP binding remains independent.
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
        s["id"] == id && s.get("parentID") == Some(&Value::Null),
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
    let directory = Path::new(&text(&n["location"], "directory")).canonicalize()?;
    let projects = s
        .projects
        .as_object()
        .context("Configure a managed project policy")?;
    let matches = projects
        .iter()
        .filter(|(_, p)| {
            Path::new(&text(p, "path")).canonicalize().ok().as_ref() == Some(&directory)
        })
        .collect::<Vec<_>>();
    ensure!(
        matches.len() == 1,
        "Configure exactly one managed project alias for this chat directory"
    );
    let project = matches[0].0;
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
                binding["terminal_id"] == receipt["terminal_id"],
                "Existing terminal binding changed; inspect it before enabling"
            );
        }
        if let Some(old) = s
            .query("SELECT * FROM chat_enablements WHERE foreman=?", [&id])?
            .first()
        {
            for k in ["terminal_id", "directory", "project_id", "project", "agent"] {
                ensure!(
                    old[k] == receipt[k],
                    "Existing enabled chat {k} changed; inspect it before enabling"
                );
            }
        }
        s.exec(
            "INSERT OR IGNORE INTO foremen(id,terminal_id) VALUES (?,?)",
            params![id, text(&receipt, "terminal_id")],
        )?;
        s.exec(
            "INSERT INTO chat_enablements(foreman,terminal_id,directory,project_id,project,agent,model,native_model) VALUES (?,?,?,?,?,?,?,?) ON CONFLICT(foreman) DO UPDATE SET model=excluded.model,native_model=excluded.native_model",
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

pub fn authority(s: &Store, r: &dyn Runtime, id: &str, caller_agent: &str) -> Result<Value> {
    let mut receipt = s
        .query("SELECT * FROM chat_enablements WHERE foreman=?", [id])?
        .into_iter()
        .next()
        .context("Run /teem enable in this chat first")?;
    let n = native(r, id)?;
    ensure!(
        receipt["agent"] == caller_agent
            && n["agent"] == receipt["agent"]
            && n["projectID"] == receipt["project_id"]
            && n["location"]["directory"] == receipt["directory"]
            && s.binding(id)?["terminal_id"] == receipt["terminal_id"],
        "Enabled chat identity or caller agent changed; inspect and enable the intended chat"
    );
    let policy = &s.projects[text(&receipt, "project")];
    ensure!(
        policy["path"].as_str().is_some_and(|p| {
            Path::new(p)
                .canonicalize()
                .ok()
                .zip(Path::new(&text(&receipt, "directory")).canonicalize().ok())
                .is_some_and(|(a, b)| a == b)
        }),
        "Enabled project policy removed or directory changed"
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
            .filter(|t| t["name"] != "foreman_submit")
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
    let name = text(envelope, "name");
    ensure!(
        name != "foreman_submit",
        "Native chats use managed foreman_workflow, not legacy foreman_submit"
    );
    let input = &envelope["arguments"];
    if name == "foreman_workflow" {
        ensure!(
            input["project"] == receipt["project"],
            "Workflow must use this enabled project alias"
        );
    }
    if name == "foreman_reconcile" {
        let source = s.workflow(&text(input, "source_workflow_id"))?;
        ensure!(
            source["foreman"] == id,
            "Candidate belongs to another Foreman"
        );
        ensure!(
            source["project"] == receipt["project"],
            "Reconcile source must belong to this enabled project alias"
        );
    }
    // Inspection remains available even for retained evidence from other projects.
    plugin::call(s, &id, &name, input)
}

pub fn dispatch_model(s: &Store, r: &dyn Runtime, task: &Value) -> Result<Value> {
    let id = text(task, "foreman");
    let rows = s.query("SELECT agent FROM chat_enablements WHERE foreman=?", [&id])?;
    let Some(row) = rows.first() else {
        return r.dispatch_model(task);
    };
    let receipt = authority(s, r, &id, &text(row, "agent"))?;
    let current = &receipt["native"]["model"];
    model(if *current != receipt["native_model"] {
        current
    } else {
        &receipt["model"]
    })
}
