//! Read-only, bounded tool views. The database remains the full evidence record.
use crate::{
    common::*,
    progress,
    store::{FINAL, LIVE, Store},
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub const MAX_BYTES: usize = 24_000;
const SECTIONS: &[&str] = &[
    "overview",
    "workflows",
    "tasks",
    "events",
    "approvals",
    "projects",
    "progress",
    "agent_progress",
    "progress_delivery",
    "check_output",
    "stopped_changes",
    "changes",
];

// Bound bytes, not characters, including a visible indication of lost detail.
fn clip(v: &Value, key: &str, max: usize) -> Value {
    let Some(s) = v[key].as_str() else {
        return v[key].clone();
    };
    if s.len() <= max {
        return json!(s);
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    json!(format!(
        "{}… [truncated; {} bytes total; use detail]",
        &s[..end],
        s.len()
    ))
}

fn fields(v: &Value, keys: &[&str]) -> Value {
    let mut out = json!({});
    for key in keys {
        out[*key] = clip(v, key, 160);
    }
    out
}

fn counts(tasks: &[Value], works: &[Value]) -> Value {
    let mut result = json!({"tasks":{},"workflows":{"total":works.len(),"active":0,"ready":0,"blocked":0,"needs_attention":0,"uncertain":0,"other_active":0}});
    for role in ["worker", "reviewer", "other"] {
        let mut c = json!({"total":0,"active":0,"queued":0,"starting":0,"accepted":0,"running":0,"uncertain":0,"succeeded":0,"failed":0,"interrupted":0,"other":0});
        for t in tasks.iter().filter(|t| {
            if role == "other" {
                t["role"] != "worker" && t["role"] != "reviewer"
            } else {
                t["role"] == role
            }
        }) {
            let state = text(t, "state");
            let key = if c.get(&state).is_some() && state != "total" && state != "active" {
                state.as_str()
            } else {
                "other"
            };
            c[key] = json!(c[key].as_u64().unwrap() + 1);
            c["total"] = json!(c["total"].as_u64().unwrap() + 1);
            if LIVE.contains(&state.as_str()) {
                c["active"] = json!(c["active"].as_u64().unwrap() + 1);
            }
        }
        result["tasks"][role] = c;
    }
    for w in works {
        let state = text(w, "state");
        if !FINAL.contains(&state.as_str()) {
            result["workflows"]["active"] =
                json!(result["workflows"]["active"].as_u64().unwrap() + 1);
        }
        let key = if FINAL.contains(&state.as_str()) || state == "uncertain" {
            state.as_str()
        } else {
            "other_active"
        };
        result["workflows"][key] = json!(result["workflows"][key].as_u64().unwrap() + 1);
    }
    result
}

fn task_summary(t: &Value) -> Value {
    let mut out = fields(
        t,
        &[
            "id",
            "role",
            "state",
            "session_id",
            "prompt_id",
            "detail",
            "result",
            "sparse",
        ],
    );
    out["workflow_id"] = t["request"]["workflow_id"].clone();
    out["result_bytes"] = json!(text(t, "result").len());
    out["meaning"] = json!("Execution state only; succeeded is not check/review approval");
    out
}

fn workflow_summary(w: &Value, tasks: &[Value]) -> Value {
    let mut out = fields(
        w,
        &[
            "id",
            "project",
            "state",
            "round",
            "base_revision",
            "detail",
            "candidate_detail",
        ],
    );
    out["title"] = clip(&json!({"title":progress::title(w)}), "title", 120);
    out["reconcile_from"] = w["request"]["reconcile_from"].clone();
    if !w["stop"].is_null() {
        let mut stop = fields(&w["stop"], &["state", "requested_at", "workflow_paused"]);
        let targets = array(&w["stop"]["targets"]);
        stop["targets_total"] = json!(targets.len());
        stop["targets"] = json!(
            targets
                .iter()
                .take(4)
                .map(|t| fields(t, &["task_id", "state", "detail"]))
                .collect::<Vec<_>>()
        );
        stop["targets_truncated"] = json!(targets.len() > 4);
        out["stop"] = stop;
    }
    let related = tasks
        .iter()
        .filter(|t| t["request"]["workflow_id"] == w["id"])
        .cloned()
        .collect::<Vec<_>>();
    out["counts"] = counts(&related, &[])["tasks"].clone();
    let rounds = w["data"]["rounds"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let empty = Value::Null;
    let round = rounds.last().unwrap_or(&empty);
    out["rounds_total"] = json!(rounds.len());
    let worker_ids = round.get("worker_ids").map(array).unwrap_or_else(|| {
        if round["worker_id"].is_null() {
            vec![]
        } else {
            vec![round["worker_id"].clone()]
        }
    });
    out["workers_total"] = json!(worker_ids.len());
    out["workers"] = json!(
        worker_ids
            .iter()
            .take(2)
            .map(|id| {
                tasks
                    .iter()
                    .find(|t| t["id"] == *id)
                    .map(task_summary)
                    .unwrap_or_else(|| json!({"id":id,"state":"unavailable"}))
            })
            .collect::<Vec<_>>()
    );
    out["snapshot"] = fields(&round["snapshot"], &["sha256", "tree"]);
    let receipt = &round["check_result"];
    let results = array(&receipt["results"]);
    let configured = array(&w["policy"]["checks"]);
    let mut checks = Vec::new();
    for c in configured.iter().take(8) {
        let r = results.iter().find(|r| r["name"] == c["name"]);
        let mut entry = fields(c, &["name", "kind"]);
        let state = if let Some(r) = r {
            if r["returncode"] == 0 && r["timed_out"] == false && r["output_limited"] == false {
                "passed"
            } else {
                "failed"
            }
        } else if !receipt.is_null() {
            "not run; batch stopped"
        } else if !round["check_handle"].is_null() {
            "batch active; result pending"
        } else {
            "not run"
        };
        entry["state"] = json!(state);
        entry["result"] = r
            .map(|r| {
                fields(
                    r,
                    &[
                        "returncode",
                        "timed_out",
                        "output_limited",
                        "error",
                        "output_sha256",
                    ],
                )
            })
            .unwrap_or(Value::Null);
        checks.push(entry);
    }
    out["checks"] = json!({"snapshot":receipt["snapshot"],"exact_candidate":!receipt.is_null() && receipt["snapshot"] == round["snapshot"]["sha256"],"passed":receipt["passed"],"error":clip(receipt,"error",160),"configured_total":configured.len(),"results_total":results.len(),"items":checks,"truncated":configured.len()>8});
    let ids = array(&round["review_ids"]);
    let verdicts = array(&round["review_results"]);
    let mut reviews = Vec::new();
    for (i, focus) in ["correctness", "tests and scope"].iter().enumerate() {
        let id = ids.get(i).cloned().unwrap_or(Value::Null);
        let task = tasks.iter().find(|t| t["id"] == id);
        let v = verdicts.get(i).cloned().unwrap_or(Value::Null);
        let findings = array(&v["findings"]);
        reviews.push(json!({"focus":focus,"task_id":id,"execution_state":task.map(|t| &t["state"]),"verdict":v["verdict"],"snapshot":v["snapshot"],"exact_candidate":!v.is_null() && v["snapshot"] == round["snapshot"]["sha256"],"findings_total":findings.len(),"findings":findings.iter().take(2).map(|f|fields(f,&["path","line","message"])).collect::<Vec<_>>(),"findings_truncated":findings.len()>2}));
    }
    out["reviews"] = json!(reviews);
    out["review_results_total"] = json!(verdicts.len());
    out["integration"] = if w["integration"].is_null() {
        Value::Null
    } else {
        fields(
            &w["integration"],
            &[
                "state",
                "commit",
                "snapshot",
                "tree",
                "recorded_at",
                "applied_at",
            ],
        )
    };
    out["integration_state"] = if w["integration"].is_null() {
        json!("not recorded; ready is not integration")
    } else {
        clip(&w["integration"], "state", 160)
    };
    out["publication"] = json!("not checked");
    out["runtime"] = json!("not checked");
    out["qualification"] = json!(
        "ready is candidate qualification only; later integration receipt takes precedence; clipped evidence is not consumed"
    );
    out
}

fn number(args: &Value, key: &str, default: usize) -> Result<usize> {
    match args.get(key) {
        Some(v) => usize::try_from(
            v.as_u64()
                .context(format!("{key} must be a nonnegative integer"))?,
        )
        .context("Pagination number too large"),
        None => Ok(default),
    }
}

fn option<'a>(args: &'a Value, key: &str, default: &'a str) -> Result<&'a str> {
    match args.get(key) {
        Some(v) => v.as_str().context(format!("{key} must be a string")),
        None => Ok(default),
    }
}

/// Summary pages are newest first; detail pages are lossless UTF-8 JSON byte chunks.
pub fn read(s: &Store, f: &str, args: &Value) -> Result<Value> {
    let binding = s.binding(f)?;
    ensure!(args.is_object(), "Arguments must be an object");
    let mode = option(args, "mode", "summary")?;
    ensure!(
        ["summary", "detail"].contains(&mode),
        "Use summary or detail mode"
    );
    let section = option(args, "section", "overview")?;
    ensure!(SECTIONS.contains(&section), "Unknown status section");
    ensure!(
        !["check_output", "stopped_changes"].contains(&section) || mode == "detail",
        "check_output and stopped_changes require detail mode"
    );
    let task_id = option(args, "task_id", "")?;
    let workflow_id = option(args, "workflow_id", "")?;
    ensure!(
        task_id.is_empty() || workflow_id.is_empty(),
        "Select task or workflow, not both"
    );
    let pointer = option(args, "pointer", "")?;
    let file = option(args, "file", "")?;
    ensure!(
        file.is_empty() || (["changes", "stopped_changes"].contains(&section) && mode == "detail"),
        "file requires changes detail mode"
    );
    ensure!(
        section != "changes" || !task_id.is_empty(),
        "changes requires one exact worker task_id"
    );
    ensure!(
        mode == "detail" || pointer.is_empty(),
        "pointer requires detail mode"
    );
    let offset = number(args, "offset", 0)?;
    let limit = number(args, "limit", if mode == "detail" { 2048 } else { 2 })?;
    ensure!(
        limit > 0 && limit <= if mode == "detail" { 4096 } else { 5 },
        "Use limit 1..5 for summary or 1..4096 bytes for detail"
    );

    // One SQLite read snapshot prevents counts and evidence disagreeing mid-response.
    s.db.execute_batch("BEGIN")?;
    let result = (|| {
        let all_tasks = s.tasks(Some(f))?;
        let all_works = s.workflows(f)?;
        let mut tasks = all_tasks.clone();
        let mut works = all_works.clone();
        if !workflow_id.is_empty() {
            let w = s.workflow(workflow_id)?;
            ensure!(
                w["foreman"] == f,
                "Workflow does not belong to this Foreman"
            );
            works = vec![w];
            tasks.retain(|t| t["request"]["workflow_id"] == workflow_id);
        }
        if !task_id.is_empty() {
            let t = s.task(task_id)?;
            ensure!(t["foreman"] == f, "Task does not belong to this Foreman");
            works.retain(|w| t["request"]["workflow_id"] == w["id"]);
            tasks = vec![t];
        }
        let scoped = !task_id.is_empty() || !workflow_id.is_empty();
        let ids = tasks.iter().map(|t| text(t, "id")).collect::<BTreeSet<_>>();
        let wids = works.iter().map(|w| text(w, "id")).collect::<BTreeSet<_>>();
        let mut events = s.query(
            if mode == "detail" {
                "SELECT * FROM events WHERE foreman=? ORDER BY rowid"
            } else {
                "SELECT * FROM events WHERE foreman=? AND acked=0 ORDER BY rowid"
            },
            [f],
        )?;
        events.retain(|e| {
            !scoped
                || ids.contains(&text(e, "task_id"))
                || (task_id.is_empty() && wids.contains(&text(e, "workflow_id")))
        });
        let mut approvals = s.query(
            "SELECT * FROM approvals WHERE foreman=? ORDER BY rowid",
            [f],
        )?;
        approvals.retain(|a| !scoped || ids.contains(&text(a, "task_id")));
        let mut agents = s.query("SELECT agent_todos.* FROM agent_todos JOIN tasks ON tasks.id=agent_todos.task_id WHERE tasks.foreman=? ORDER BY tasks.created,tasks.id", [f])?;
        agents.retain(|a| !scoped || ids.contains(&text(a, "task_id")));
        let mut p = progress::snapshot(s, f)?;
        if scoped {
            let items = array(&p["items"])
                .into_iter()
                .filter(|i| {
                    ids.contains(&text(i, "task_id"))
                        || (task_id.is_empty() && wids.contains(&text(i, "workflow_id")))
                })
                .collect::<Vec<_>>();
            p = json!({"items":items});
        }
        let delivery = if scoped {
            vec![]
        } else {
            s.query(
                if mode == "detail" {
                    "SELECT * FROM progress_events WHERE foreman=? ORDER BY seq DESC"
                } else {
                    "SELECT seq,state,detail FROM progress_events WHERE foreman=? ORDER BY seq DESC LIMIT 1"
                },
                [f],
            )?
        };
        let projects = s
            .projects
            .as_object()
            .map(|m| {
                m.iter()
                    .filter(|(name, _)| {
                        !scoped || works.iter().any(|w| w["project"] == name.as_str())
                    })
                    .map(|(name, policy)| json!({"name":name,"policy":policy}))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if mode == "detail" {
            let raw = match section {
                "overview" => {
                    if !task_id.is_empty() {
                        tasks[0].clone()
                    } else if !workflow_id.is_empty() {
                        works[0].clone()
                    } else {
                        anyhow::bail!(
                            "Detail requires task_id, workflow_id, or a non-overview section"
                        )
                    }
                }
                "check_output" => {
                    ensure!(
                        !workflow_id.is_empty(),
                        "check_output requires workflow_id and a pointer to a recorded check result"
                    );
                    works[0].clone()
                }
                "stopped_changes" => {
                    ensure!(!task_id.is_empty(), "stopped_changes requires one task_id");
                    let task = &tasks[0];
                    let rows =
                        s.query("SELECT state FROM task_stops WHERE task_id=?", [task_id])?;
                    ensure!(
                        rows.first().is_some_and(|r| r["state"] == "stopped"),
                        "Confirm the selected task stopped before reading its handoff"
                    );
                    crate::inspection::changes(s, task, file)?
                }
                "tasks" => json!(tasks),
                "changes" => crate::inspection::changes(s, &tasks[0], file)?,
                "workflows" => json!(works),
                "events" => json!(events),
                "approvals" => json!(approvals),
                "projects" => json!(projects),
                "progress" => p,
                "agent_progress" => json!(agents),
                "progress_delivery" => json!(delivery),
                _ => unreachable!(),
            };
            let selected = raw.pointer(pointer).context("JSON pointer not found")?;
            let encoded = if section == "check_output" {
                let parts = pointer.split('/').collect::<Vec<_>>();
                ensure!(
                    parts.len() == 7
                        && parts[1] == "data"
                        && parts[2] == "rounds"
                        && parts[3].parse::<usize>().is_ok()
                        && parts[4] == "check_result"
                        && parts[5] == "results"
                        && parts[6].parse::<usize>().is_ok(),
                    "Point to /data/rounds/N/check_result/results/M"
                );
                let path = std::path::PathBuf::from(text(selected, "log"));
                let root = s.directory.join("managed").join(workflow_id);
                ensure!(
                    path.is_absolute() && path.starts_with(&root) && path.canonicalize()? == path,
                    "Check log escaped workflow storage or follows a symlink"
                );
                let bytes = regular(&path, 1048576)?;
                ensure!(
                    json!(sha(&bytes)) == selected["output_sha256"],
                    "Check log digest changed; do not consume unverified evidence"
                );
                match String::from_utf8(bytes) {
                    Ok(text) => canonical(&json!(text)),
                    Err(error) => {
                        canonical(&json!({"encoding":"bytes","bytes":error.into_bytes()}))
                    }
                }
            } else {
                canonical(selected)
            };
            ensure!(
                offset <= encoded.len() && encoded.is_char_boundary(offset),
                "Detail offset must be a UTF-8 byte boundary; use next_offset"
            );
            let mut end = offset.saturating_add(limit).min(encoded.len());
            while !encoded.is_char_boundary(end) {
                end -= 1;
            }
            ensure!(
                end > offset || offset == encoded.len(),
                "Limit too small for next UTF-8 character"
            );
            return Ok(
                json!({"mode":"detail","section":section,"task_id":task_id,"workflow_id":workflow_id,"pointer":pointer,"offset":offset,"total_bytes":encoded.len(),"sha256":sha(encoded.as_bytes()),"chunk":&encoded[offset..end],"next_offset":if end<encoded.len(){Some(end)}else{None},"instructions":"Concatenate chunks only while sha256 is unchanged. Read all relevant evidence before ack; a file path is not consumed evidence."}),
            );
        }

        let scoped_counts = counts(&tasks, &works);
        let global_counts = counts(&all_tasks, &all_works);
        let latest = works
            .iter()
            .filter(|w| FINAL.contains(&text(w, "state").as_str()))
            .max_by(|a, b| {
                a["data"]["finished_at"]
                    .as_f64()
                    .or(a["created"].as_f64())
                    .unwrap_or(0.0)
                    .total_cmp(
                        &b["data"]["finished_at"]
                            .as_f64()
                            .or(b["created"].as_f64())
                            .unwrap_or(0.0),
                    )
                    .then_with(|| text(a, "id").cmp(&text(b, "id")))
            });
        // Rich entries have independent bounds; if several exceed the response budget,
        // shorten pages, never remove a failure from a displayed candidate's verdicts.
        for page_limit in (1..=limit).rev() {
            let mut out = json!({"mode":"summary","foreman":clip(&json!({"f":f}),"f",160),"terminal_id":clip(&binding,"terminal_id",160),"counts":scoped_counts,"foreman_counts":global_counts,"count_semantics":"active = starting + accepted + running + uncertain (reserved slots); queued is separate; running is observed state, not dispatch count","limits":{"worker":2,"reviewer":2},"limit_scope":"per managed task; legacy tasks share two slots per role","completion_transport":"native-runtime","pages":{},"retrieval":"Use section plus offset/limit for summary pages. Use mode=detail with task_id/workflow_id and optional JSON pointer for lossless evidence chunks. All clipped strings are marked; collection totals include hidden entries. Do not ack unseen evidence.","max_response_bytes":MAX_BYTES});
            let sections = if section == "overview" {
                vec![
                    "workflows",
                    "tasks",
                    "events",
                    "approvals",
                    "projects",
                    "progress",
                ]
            } else {
                vec![section]
            };
            for name in sections {
                let raw: Vec<Value> = match name {
                    "workflows" => works
                        .iter()
                        .rev()
                        .map(|w| workflow_summary(w, &all_tasks))
                        .collect(),
                    "tasks" => tasks.iter().rev().map(task_summary).collect(),
                    "changes" => array(&crate::inspection::changes(s,&tasks[0],"")?["files"]),
                    "events" => events
                        .iter()
                        .rev()
                        .map(|e| {
                            fields(e, &["id", "task_id", "workflow_id", "state", "transport", "detail"])
                        })
                        .collect(),
                    "approvals" => approvals
                        .iter()
                        .rev()
                        .map(|a| fields(a, &["task_id", "permission_id", "form_id", "state", "detail"]))
                        .collect(),
                    "projects" => projects.iter().map(|p|json!({"name":p["name"],"checks_total":array(&p["policy"]["checks"]).len(),"max_revisions":p["policy"]["max_revisions"]})).collect(),
                    "progress" => array(&p["items"])
                        .iter()
                        .map(|i| {
                            fields(
                                i,
                                &["id", "task_id", "workflow_id", "source", "content", "status", "task_state"],
                            )
                        })
                        .collect(),
                    "agent_progress" => agents
                        .iter()
                        .rev()
                        .map(|a| fields(a, &["task_id", "message_id", "final", "detail"]))
                        .collect(),
                    "progress_delivery" => delivery
                        .iter()
                        .map(|d| fields(d, &["seq", "state", "detail"]))
                        .collect(),
                    _ => unreachable!(),
                };
                let rows = raw
                    .iter()
                    .skip(offset)
                    .take(page_limit)
                    .cloned()
                    .collect::<Vec<_>>();
                let end = offset.saturating_add(rows.len()).min(raw.len());
                out["pages"][name] = json!({"offset":offset,"returned":rows.len(),"total":raw.len(),"next_offset":if end<raw.len(){Some(end)}else{None},"order":if ["projects","progress","progress_delivery"].contains(&name){"stored order"}else{"newest first"}});
                out[name] = if name == "progress" {
                    json!({"items":rows,"latest_result":latest.map(|w|fields(&json!({"workflow_id":w["id"],"title":progress::title(w),"state":w["state"],"content":w["detail"]}),&["workflow_id","title","state","content"]))})
                } else {
                    json!(rows)
                };
            }
            if serde_json::to_string_pretty(&out)?.len() <= MAX_BYTES {
                return Ok(out);
            }
        }
        anyhow::bail!(
            "Summary cannot fit the 24000-byte budget. Select one workflow/task or section; use detail chunks for full evidence. No failure was discarded."
        )
    })();
    s.db.execute_batch("ROLLBACK")?;
    let result = result?;
    ensure!(
        serde_json::to_string_pretty(&result)?.len() <= MAX_BYTES,
        "Status exceeds byte budget; select a smaller detail limit"
    );
    Ok(result)
}
