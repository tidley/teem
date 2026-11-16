use crate::{
    common::*,
    runtime::Runtime,
    store::{FINAL, Store, TERMINAL},
};
use anyhow::{Context, Result, ensure};
use rusqlite::params;
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub fn set_plan(s: &Store, f: &str, items: &Value) -> Result<Value> {
    s.binding(f)?;
    let items = items.as_array().context("Plan must be an array")?;
    ensure!(items.len() <= 100, "Plan has too many items");
    let mut ids = BTreeSet::new();
    for item in items {
        let m = item.as_object().context("Invalid plan item")?;
        let id = text(item, "id");
        let content = text(item, "content");
        ensure!(
            !id.is_empty()
                && id.len() <= 64
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                && ids.insert(id)
                && !content.trim().is_empty()
                && content.chars().count() <= 300
                && !content.contains(['\n', '\r', '\0'])
                && m.keys()
                    .all(|k| ["id", "content", "task_id", "status"].contains(&k.as_str())),
            "Invalid plan IDs, labels or fields"
        );
        if let Some(task) = item.get("task_id") {
            uuid(task.as_str().context("Invalid task UUID")?)?;
            ensure!(
                item.get("status").is_none(),
                "Linked plan status is supervisor-owned"
            );
            if let Ok(t) = s.task(task.as_str().unwrap()) {
                ensure!(t["foreman"] == f, "Plan task belongs to another Foreman");
            }
        } else {
            ensure!(
                ["pending", "in_progress", "completed", "cancelled"]
                    .contains(&text(item, "status").as_str()),
                "Unlinked plan requires explicit status"
            );
        }
    }
    s.exec(
        "INSERT INTO plans VALUES (?,?) ON CONFLICT(foreman) DO UPDATE SET items=excluded.items",
        params![f, canonical(&json!(items))],
    )?;
    snapshot(s, f)
}
pub fn title(w: &Value) -> String {
    let title = unlinked_title(w);
    let Ok(issue) = issue_reference(&w["request"]["issue"]) else {
        return title;
    };
    let title = without_issue_prefix(&issue, &title);
    if title.is_empty() {
        "Requested changes".into()
    } else {
        title.into()
    }
}

// A retained observation, not a new qualification or a model-generated summary.
pub fn result_sections(s: &Store, w: &Value) -> Vec<Value> {
    fn excerpt(value: &str, max: usize) -> String {
        let clean: String = value
            .chars()
            .filter(|c| !c.is_control() || *c == '\n')
            .collect();
        if clean.len() <= max {
            return clean;
        }
        let mut end = max;
        while !clean.is_char_boundary(end) {
            end -= 1;
        }
        format!(
            "{}… [excerpt; full evidence in foreman_status]",
            &clean[..end]
        )
    }
    let empty = Value::Null;
    let round = w["data"]["rounds"]
        .as_array()
        .and_then(|r| r.last())
        .unwrap_or(&empty);
    let snapshot = text(&round["snapshot"], "sha256");
    let candidate_detail = w.get("candidate_detail").unwrap_or(&w["detail"]);
    let detail = candidate_detail.as_str().unwrap_or("");
    let detail = if detail.starts_with("Revision budget exhausted") {
        format!(
            "Automatic code-correction limit reached: {} attempts. Candidate and evidence retained.",
            w["policy"]["max_revisions"]
        )
    } else {
        excerpt(detail, 700)
    };
    let unresolved = array(&round["review_results"])
        .into_iter()
        .filter(|v| {
            !snapshot.is_empty()
                && v["snapshot"] == snapshot
                && ["blocked", "changes_requested"].contains(&text(v, "verdict").as_str())
        })
        .min_by_key(|v| if v["verdict"] == "blocked" { 0 } else { 1 });
    let blocker = crate::notices::blocker(w);
    let (heading, summary) = if w["state"] != "ready" {
        if let Some(review) = unresolved {
            let blocked = review["verdict"] == "blocked";
            let reason = if blocked {
                text(&review, "reason")
            } else {
                text(&review["findings"][0], "message")
            };
            (
                if blocked {
                    "Review blocked"
                } else {
                    "Review not approved"
                },
                format!(
                    "{}\n\nNext: the Foreman must resolve this review reason and qualify the retained candidate. A review access/setup failure is not a demonstrated firmware defect. No approval, integration or OTA is implied.\n\nRecorded controller state: {}\n{}",
                    excerpt(&reason, 550),
                    text(w, "state"),
                    excerpt(&detail, 160)
                ),
            )
        } else {
            (
                if blocker["kind"] == "source_changes" {
                    "Source checkout blocked"
                } else {
                    "Stopped attempt"
                },
                format!("{detail}\n\nNext: {}", text(&blocker, "action")),
            )
        }
    } else {
        ("Outcome", format!("Recorded state: ready\n{detail}"))
    };
    let mut sections = vec![format!(
        "{heading}\n{summary}\n\nProject: {}\nSource: {}\nThis is a retained attempt; the current working tree and publication were not rechecked.",
        excerpt(&text(w, "project"), 64),
        excerpt(&text(&w["policy"], "path"), 512)
    )];
    let ids = round
        .get("worker_ids")
        .map(array)
        .unwrap_or_else(|| vec![round["worker_id"].clone()]);
    let mut reports = Vec::new();
    for id in ids.iter().take(2).filter_map(Value::as_str) {
        if let Ok(task) = s.task(id) {
            if task["foreman"] == w["foreman"]
                && task["request"]["workflow_id"] == w["id"]
                && task["role"] == "worker"
            {
                let report = text(&task, "result");
                if !report.trim().is_empty() {
                    reports.push(format!(
                        "Worker {} ({}):\n{}",
                        reports.len() + 1,
                        text(&task, "state"),
                        excerpt(&report, 1800)
                    ));
                }
            }
        }
    }
    let worker_section = if reports.is_empty() {
        "Worker report\nNo worker summary retained for this round.".into()
    } else {
        format!(
            "Worker report\nEarlier worker claims, not independent verification. Recorded checks and reviews above take precedence over this report's earlier pending/status wording.\n\n{}",
            reports.join("\n\n")
        )
    };
    let receipt = &round["check_result"];
    let bound = !snapshot.is_empty() && receipt["snapshot"] == snapshot;
    let checks = array(&receipt["results"]);
    let mut rows = Vec::new();
    for check in array(&w["policy"]["checks"]).iter().take(8) {
        let found = checks.iter().find(|r| r["name"] == check["name"]);
        let state = match found {
            Some(_) if !bound => "snapshot mismatch; not qualified",
            Some(r)
                if r["returncode"] == 0
                    && r["timed_out"] == false
                    && r["output_limited"] == false =>
            {
                "passed"
            }
            Some(r) if r["timed_out"] == true => "failed (timeout)",
            Some(r) if r["output_limited"] == true => "failed (output limit)",
            Some(_) => "failed",
            None => "not recorded / not run",
        };
        rows.push(format!("{}: {state}", excerpt(&text(check, "name"), 64)));
    }
    sections.push(format!(
        "Checks\nRecorded candidate evidence:\n{}",
        if rows.is_empty() {
            "No check results recorded.".into()
        } else {
            rows.join("\n")
        }
    ));
    let reviews = array(&round["review_results"]);
    let review_rows = (0..2)
        .map(|index| {
            let review = reviews.get(index).unwrap_or(&empty);
            let verdict = match text(review, "verdict").as_str() {
                "pass" | "changes_requested" | "blocked"
                    if !snapshot.is_empty() && review["snapshot"] == snapshot =>
                {
                    text(review, "verdict")
                }
                "pass" | "changes_requested" | "blocked" => {
                    "snapshot mismatch; not qualified".into()
                }
                _ => "no valid verdict recorded".into(),
            };
            let findings = array(&review["findings"]);
            let mut row = format!(
                "{}: {verdict} ({} findings)",
                if index == 0 {
                    "Correctness / security"
                } else {
                    "Tests / regressions / scope"
                },
                findings.len()
            );
            if review["verdict"] == "blocked" {
                row.push_str(&format!(
                    "\nReason: {}",
                    excerpt(&text(review, "reason"), 500)
                ));
            }
            for finding in findings.iter().take(2) {
                row.push_str(&format!(
                    "\n{}:{} — {}",
                    excerpt(&text(finding, "path"), 120),
                    finding["line"],
                    excerpt(&text(finding, "message"), 500)
                ));
            }
            if findings.len() > 2 {
                row.push_str("\nMore findings retained; use foreman_status for all findings.");
            }
            row
        })
        .collect::<Vec<_>>();
    sections.push(format!(
        "Reviews\nRecorded exact-candidate verdicts:\n{}",
        review_rows.join("\n")
    ));
    sections.push(format!(
        "Delivery\nIntegration: {}\nPublication: not checked\nDeployment: not checked",
        if w["integration"].is_null() {
            "Not recorded in Teem; source was not inspected".into()
        } else {
            crate::operator::description(&w["integration"])
        }
    ));
    sections.push(worker_section);
    sections.push(format!("Evidence\nWorkflow: {}\nCandidate: {}\nUse foreman_status with this workflow_id for full reports, findings and check logs. These are retained records, not a fresh source or runtime inspection.", text(w, "id"), if snapshot.is_empty() { "not recorded" } else { &snapshot }));
    sections
        .into_iter()
        .map(|section| {
            let (title, body) = section.split_once('\n').unwrap_or((&section, ""));
            json!({"title":title,"body":body})
        })
        .collect()
}
pub fn result_details(s: &Store, w: &Value) -> String {
    result_sections(s, w)
        .iter()
        .map(|section| format!("{}\n{}", text(section, "title"), text(section, "body")))
        .collect::<Vec<_>>()
        .join("\n\n")
}
fn unlinked_title(w: &Value) -> String {
    if let Some(t) = w["request"]["title"].as_str() {
        return t.into();
    }
    let prompt = compact(&text(&w["request"], "prompt"));
    let first = prompt.split_once('(').map(|(a, _)| a).unwrap_or("");
    let mut words = first.split_whitespace();
    if let (Some(action), Some(function), None) = (words.next(), words.next(), words.next()) {
        if ["add", "implement", "improve", "fix"].contains(&action.to_lowercase().as_str())
            && function.chars().all(|c| c.is_alphanumeric() || c == '_')
        {
            let mut a = action.to_lowercase();
            a.get_mut(..1).unwrap().make_ascii_uppercase();
            return format!("{a} {}", function.replace('_', " "));
        }
    }
    let v = short(prompt.split(". ").next().unwrap_or(""), 100)
        .trim_end_matches('.')
        .to_owned();
    if v.is_empty() {
        "Requested changes".into()
    } else {
        v
    }
}
fn row(w: &Value, suffix: &str, label: &str, status: &str, description: &str) -> Value {
    let label = issue_prefix(&w["request"], label);
    let mut row = json!({"id":format!("workflow_{}{}",text(w,"id").replace('-',""),suffix),"source":"supervisor","workflow_id":w["id"],"label":label,"status":status,"content":format!("{label} — {description}")});
    if let Ok(issue) = issue_reference(&w["request"]["issue"]) {
        row["issue"] = json!(issue);
    }
    row
}
fn task_state(s: &Store, id: &Value) -> Result<(String, String, String)> {
    if id.is_null() {
        return Ok(("pending".into(), "pending".into(), "not started".into()));
    }
    let t = s.task(id.as_str().context("Invalid progress task ID")?)?;
    let state = text(&t, "state");
    let status = match state.as_str() {
        "succeeded" => "completed",
        "failed" | "interrupted" => "cancelled",
        "queued" => "pending",
        _ => "in_progress",
    };
    let description = match state.as_str() {
        "queued" => "queued",
        "starting" => "starting",
        "accepted" => "native input accepted",
        "running" if text(&t, "detail").contains("human permission or answer") => {
            "waiting for human approval or answer"
        }
        "running" => "running",
        "uncertain" => "uncertain; no replacement started",
        "succeeded" => "execution finished",
        "failed" => "execution failed",
        _ => "interrupted",
    };
    Ok((state, status.into(), description.into()))
}
fn agent_detail(s: &Store, w: &Value, id: &Value, suffix: &str, label: &str) -> Result<Vec<Value>> {
    if id.is_null() {
        return Ok(vec![]);
    }
    let rows = s.query(
        "SELECT todos FROM agent_todos WHERE task_id=?",
        [id.as_str().unwrap_or("")],
    )?;
    let Some(report) = rows.first() else {
        return Ok(vec![]);
    };
    let Some(todos) = report["todos"].as_str() else {
        return Ok(vec![]);
    };
    let todos: Value = serde_json::from_str(todos)?;
    Ok(array(&todos)
        .iter()
        .take(8)
        .enumerate()
        .map(|(i, t)| {
            let content = text(t, "content");
            let content = issue_reference(&w["request"]["issue"])
                .map(|issue| without_issue_prefix(&issue, &content).to_owned())
                .unwrap_or(content);
            let mut r = row(
                w,
                &format!("{suffix}_reported_{i}"),
                &format!("{label} · reported: {content}"),
                &text(t, "status"),
                "agent-reported progress only",
            );
            r["source"] = json!("agent-reported");
            r["task_id"] = id.clone();
            r["sidebar"] = json!({"kind":"reported","label":content});
            r
        })
        .collect())
}
pub fn workflow_rows(s: &Store, f: &str) -> Result<Vec<Value>> {
    let mut rows = Vec::new();
    for w in s.workflows(f)? {
        let state = text(&w, "state");
        if FINAL.contains(&state.as_str()) {
            continue;
        }
        let label = format!("{} · {}", text(&w, "project"), title(&w));
        let desc = match state.as_str() {
            "queued" => "queued",
            "qualification_queued" => {
                "qualification-only recovery queued; no implementation worker"
            }
            "preparing" => "preparing isolated checkout",
            "working" => "implementation",
            "snapshotting" => "freezing exact candidate",
            "checking" => "running approved checks",
            "review_preparing" => "checks passed; preparing independent reviews",
            "reviewing" => "two independent reviews",
            _ => "uncertain; reserved without replay",
        };
        let mut head = row(
            &w,
            "",
            &label,
            if state == "queued" || state == "qualification_queued" {
                "pending"
            } else {
                "in_progress"
            },
            desc,
        );
        head["workflow_state"] = json!(state);
        head["sidebar"] = json!({"project":w["project"],"title":title(&w),"revision":array(&w["data"]["rounds"]).len().saturating_sub(1)});
        rows.push(head);
        let rounds = array(&w["data"]["rounds"]);
        let number = rounds.len().saturating_sub(1);
        let round = rounds.last().cloned().unwrap_or(json!({}));
        let prefix = if number > 0 {
            format!("{label} · Revision {number}")
        } else {
            label
        };
        let suffix = format!("_round_{number}");
        let ids = round.get("worker_ids").map(array).unwrap_or_else(|| {
            if w["request"].get("qualification_recovery").is_some() {
                vec![]
            } else if round.get("worker_id").is_some() {
                vec![round["worker_id"].clone()]
            } else {
                vec![
                    Value::Null;
                    w["request"]["workers"]
                        .as_array()
                        .map(Vec::len)
                        .unwrap_or(1)
                ]
            }
        });
        for (i, id) in ids.iter().enumerate() {
            let (st, stat, desc) = task_state(s, id)?;
            let label = format!(
                "{prefix} · Worker{}",
                if ids.len() > 1 {
                    format!(" {}", i + 1)
                } else {
                    String::new()
                }
            );
            let suffix = format!("{suffix}_worker_{i}");
            let mut r = row(&w, &suffix, &label, &stat, &desc);
            r["task_id"] = id.clone();
            r["task_state"] = json!(st);
            r["sidebar"] = json!({"kind":"implementation","label":if ids.len()>1 {format!("Implementation {}",i+1)} else {"Implementation".into()}});
            rows.push(r);
            rows.extend(agent_detail(s, &w, id, &suffix, &label)?);
        }
        let results = array(&round["check_result"]["results"]);
        for (i, c) in array(&w["policy"]["checks"]).iter().enumerate() {
            let (status, desc) = if let Some(r) = results.iter().find(|r| r["name"] == c["name"]) {
                if r["returncode"] == 0 && r["timed_out"] == false && r["output_limited"] == false {
                    ("completed", "passed (exit 0)")
                } else {
                    ("cancelled", "failed; see recorded check evidence")
                }
            } else if !round["check_result"].is_null() {
                ("cancelled", "not run; check batch stopped")
            } else if !round["check_handle"].is_null() {
                ("pending", "check batch active; individual result pending")
            } else {
                ("pending", "not run yet")
            };
            let mut r = row(
                &w,
                &format!("{suffix}_check_{i}"),
                &format!("{prefix} · Check: {}", text(c, "name")),
                status,
                desc,
            );
            r["sidebar"] = json!({"kind":"check","label":text(c,"name")});
            rows.push(r);
        }
        for (i, focus) in ["Correctness review", "Tests and scope review"]
            .iter()
            .enumerate()
        {
            let id = round["review_ids"].get(i).cloned().unwrap_or(Value::Null);
            let (st, mut status, mut desc) = task_state(s, &id)?;
            if let Some(v) = round["review_results"].get(i) {
                if v["verdict"] == "pass" {
                    status = "completed".into();
                    desc = "passed; no findings".into();
                } else {
                    status = "cancelled".into();
                    desc = if v["verdict"] == "blocked" {
                        format!("review blocked: {}", text(v, "reason"))
                    } else {
                        "changes requested; see recorded findings".into()
                    };
                }
            } else if st == "succeeded" {
                status = "in_progress".into();
                desc = "execution finished; verdict pending".into();
            }
            let label = format!("{prefix} · {focus}");
            let suffix = format!("{suffix}_review_{i}");
            let mut r = row(&w, &suffix, &label, &status, &desc);
            r["task_id"] = id.clone();
            r["task_state"] = json!(st);
            r["sidebar"] = json!({"kind":"review","label":if i==0 {"Review · correctness"} else {"Review · tests/scope"}});
            rows.push(r);
            rows.extend(agent_detail(s, &w, &id, &suffix, &label)?);
        }
    }
    Ok(rows)
}
pub fn snapshot(s: &Store, f: &str) -> Result<Value> {
    s.binding(f)?;
    let plans = s.query("SELECT items FROM plans WHERE foreman=?", [f])?;
    let Some(plan) = plans.first() else {
        return Ok(Value::Null);
    };
    let mut items = array(&serde_json::from_str::<Value>(
        plan["items"].as_str().unwrap_or("[]"),
    )?);
    for item in &mut items {
        item["source"] = json!("foreman-plan");
        item["label"] = item["content"].clone();
        if let Some(id) = item["task_id"].as_str().map(str::to_owned) {
            let task = s.task(&id).ok().filter(|t| t["foreman"] == f);
            if let Some(w) = task
                .as_ref()
                .and_then(|t| t["request"]["workflow_id"].as_str())
                .and_then(|id| s.workflow(id).ok())
                .filter(|w| w["foreman"] == f)
            {
                item["content"] = json!(issue_prefix(&w["request"], &text(item, "content")));
                item["label"] = item["content"].clone();
            }
            let state = task
                .as_ref()
                .map(|t| text(t, "state"))
                .unwrap_or("unavailable".into());
            let status = match state.as_str() {
                "succeeded" => "completed",
                "failed" | "interrupted" => "cancelled",
                "queued" | "unavailable" => "pending",
                _ => "in_progress",
            };
            let description = match state.as_str() {
                "unavailable" => "not submitted or unavailable",
                "queued" => "queued",
                "starting" => "reserved; starting",
                "accepted" => "native input accepted; execution pending",
                "running"
                    if task.as_ref().is_some_and(|t| {
                        text(t, "detail").contains("human permission or answer")
                    }) =>
                {
                    "waiting for human approval or answer"
                }
                "running" => "running",
                "uncertain" => "uncertain; reservation retained",
                "succeeded" => "execution ended; tests/review not implied",
                "failed" => "execution failed",
                _ => "execution interrupted",
            };
            item["source"] = json!("supervisor");
            item["task_state"] = json!(state);
            item["status"] = json!(status);
            item["content"] = json!(format!("{} — {description}", text(item, "content")));
        }
    }
    items.extend(workflow_rows(s, f)?);
    let mut result = json!({"todos":items.iter().map(|i|json!({"content":i["content"],"status":i["status"]})).collect::<Vec<_>>(),"items":items});
    let works = s.workflows(f)?;
    let (latest, notice) = crate::notices::select(s, &works)?;
    if let Some(notice) = notice {
        result["result_notice_cleared"] = json!(true);
        result["result_notice"] = notice;
    }
    if let Some(w) = latest {
        let blocker = crate::notices::blocker(w);
        let content = if !w["integration"].is_null() {
            crate::operator::description(&w["integration"])
        } else if w["state"] == "ready" {
            "Completed · approved checks and both reviews passed; integration not recorded".into()
        } else {
            format!(
                "Stopped attempt · {}. Next: {}",
                short(&compact(&text(&blocker, "reason")), 400),
                text(&blocker, "action")
            )
        };
        let title = issue_prefix(
            &w["request"],
            &format!("{} · {}", text(w, "project"), title(w)),
        );
        // Preserve the adapter's existing 300 UTF-16-unit outcome-title bound.
        let mut units = 0;
        let title: String = title
            .chars()
            .take_while(|c| {
                units += c.len_utf16();
                units <= 300
            })
            .collect();
        let sections = result_sections(s, w);
        let details = sections
            .iter()
            .map(|section| format!("{}\n{}", text(section, "title"), text(section, "body")))
            .collect::<Vec<_>>()
            .join("\n\n");
        result["latest_result"] = json!({"workflow_id":w["id"],"state":w["state"],"title":title,"content":content,"details":details,"sections":sections,"blocker":blocker,"historical":true,"recorded_at":w["data"]["finished_at"].as_f64().or(w["created"].as_f64()).unwrap_or(0.0)});
    }
    Ok(result)
}
pub fn publish(s: &Store) -> Result<()> {
    let mut sessions = serde_json::Map::new();
    for row in s.query("SELECT foreman FROM plans ORDER BY foreman", [])? {
        let f = text(&row, "foreman");
        sessions.insert(f.clone(), snapshot(s, &f)?);
    }
    s.exec(
        "UPDATE progress_events SET state='superseded' WHERE state='pending'",
        [],
    )?;
    atomic_json(
        &s.directory.join("progress.json"),
        &json!({"version":1,"updated_at":now(),"sessions":sessions}),
    )
}
pub fn read_agents(s: &Store, r: &dyn Runtime) -> Result<()> {
    for t in s.tasks(None)? {
        if t["request"]["native_todos"] != true || t["state"] == "queued" {
            continue;
        }
        let id = text(&t, "id");
        s.exec(
            "INSERT OR IGNORE INTO agent_todos(task_id) VALUES (?)",
            [&id],
        )?;
        let row = s
            .query("SELECT * FROM agent_todos WHERE task_id=?", [&id])?
            .remove(0);
        if row["final"] == 1 || row["next_read"].as_f64().unwrap_or(0.0) > now() {
            continue;
        }
        s.exec(
            "UPDATE agent_todos SET next_read=? WHERE task_id=?",
            params![now() + 5.0, id],
        )?;
        match r.todos(&t) {
            Ok(report) => {
                if let Some(report) = report {
                    s.exec(
                        "UPDATE agent_todos SET todos=?,message_id=?,detail=? WHERE task_id=?",
                        params![
                            canonical(&report["todos"]),
                            text(&report, "message_id"),
                            "Agent-reported progress, not check/review evidence",
                            id
                        ],
                    )?;
                }
                if TERMINAL.contains(&text(&t, "state").as_str()) {
                    s.exec("UPDATE agent_todos SET final=1 WHERE task_id=?", [&id])?;
                }
            }
            Err(_) => {
                s.exec(
                    "UPDATE agent_todos SET detail=? WHERE task_id=?",
                    params![
                        "Native agent todo report unavailable; last observed entries retained",
                        id
                    ],
                )?;
            }
        }
    }
    Ok(())
}
