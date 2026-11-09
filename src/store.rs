use crate::{checkouts, common::*, progress, workflows};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, Params, params};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::Duration,
};

pub const LIVE: &[&str] = &["starting", "accepted", "running", "uncertain"];
pub const TERMINAL: &[&str] = &["succeeded", "failed", "interrupted"];
pub const FINAL: &[&str] = &["ready", "needs_attention", "blocked"];
pub struct Store {
    pub db: Connection,
    pub directory: PathBuf,
    pub allowed: BTreeSet<String>,
    pub projects: Value,
}
impl Store {
    pub fn open(directory: &Path, allowed: &[String], projects: Value) -> Result<Self> {
        private_dir(directory)?;
        let directory = directory.canonicalize()?;
        let path = directory.join("tasks.sqlite3");
        let _ = open_lock(&path)?;
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_secs(15))?;
        db.execute_batch(include_str!("schema.sql"))?;
        let s = Self {
            db,
            directory,
            allowed: allowed
                .iter()
                .map(|p| absolute(&expand(p)).map(|p| p.to_string_lossy().into_owned()))
                .collect::<Result<_>>()?,
            projects,
        };
        s.transaction(||{
            s.column("tasks","runtime_model","TEXT")?;
            s.column("events","transport","TEXT NOT NULL DEFAULT 'legacy-codr'")?;
            s.column("events","detail","TEXT NOT NULL DEFAULT ''")?;
            s.column("events","refresh_todos","INTEGER NOT NULL DEFAULT 0")?;
            s.column("events","workflow_id","TEXT")?;
            if s.column("approvals","reply_attempted","INTEGER NOT NULL DEFAULT 0")? {s.exec("UPDATE approvals SET reply_attempted=1 WHERE state IN ('replying','uncertain','done')",[])?;}
            s.column("approvals","next_restore","REAL NOT NULL DEFAULT 0")?;Ok(())
        })?;
        Ok(s)
    }
    fn column(&self, table: &str, column: &str, definition: &str) -> Result<bool> {
        let rows = self.query(&format!("PRAGMA table_info({table})"), [])?;
        if rows.iter().any(|r| r["name"] == column) {
            return Ok(false);
        }
        self.db.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {definition}"
        ))?;
        Ok(true)
    }
    pub fn exec(&self, sql: &str, p: impl Params) -> Result<usize> {
        Ok(self.db.execute(sql, p)?)
    }
    pub fn query(&self, sql: &str, p: impl Params) -> Result<Vec<Value>> {
        let mut stmt = self.db.prepare(sql)?;
        let names = stmt
            .column_names()
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let rows = stmt.query_map(p, |r| {
            let mut m = serde_json::Map::new();
            for (i, n) in names.iter().enumerate() {
                let v = match r.get_ref(i)? {
                    rusqlite::types::ValueRef::Null => Value::Null,
                    rusqlite::types::ValueRef::Integer(i) => json!(i),
                    rusqlite::types::ValueRef::Real(f) => json!(f),
                    rusqlite::types::ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                    rusqlite::types::ValueRef::Blob(_) => Value::Null,
                };
                m.insert(n.clone(), v);
            }
            Ok(Value::Object(m))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
    pub fn transaction<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.db.execute_batch("BEGIN IMMEDIATE")?;
        match f() {
            Ok(v) => {
                self.db.execute_batch("COMMIT")?;
                Ok(v)
            }
            Err(e) => {
                let _ = self.db.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }
    pub fn bind(&self, foreman: &str, terminal: &str) -> Result<()> {
        ensure!(
            foreman.starts_with("ses_")
                && foreman.len() > 4
                && foreman
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                && !terminal.is_empty(),
            "Use an exact native session and terminal ID"
        );
        self.transaction(|| {
            let old = self.query("SELECT * FROM foremen WHERE id=?", [foreman])?;
            ensure!(
                old.first().is_none_or(|r| r["terminal_id"] == terminal),
                "Foreman terminal changed; inspect binding before replacing it"
            );
            self.exec(
                "INSERT OR IGNORE INTO foremen VALUES (?,?)",
                params![foreman, terminal],
            )?;
            Ok(())
        })
    }
    pub fn binding(&self, foreman: &str) -> Result<Value> {
        self.query("SELECT * FROM foremen WHERE id=?", [foreman])?
            .into_iter()
            .next()
            .context("Register this Foreman explicitly before delegation")
    }
    fn decode_task(mut v: Value) -> Result<Value> {
        v["request"] = serde_json::from_str(
            v["request"]
                .as_str()
                .context("Invalid stored task request")?,
        )?;
        v["sparse"] = json!(v["sparse"] == 1);
        v["runtime_model"] = match v["runtime_model"].as_str() {
            Some(s) => serde_json::from_str(s)?,
            None => Value::Null,
        };
        Ok(v)
    }
    pub fn task(&self, id: &str) -> Result<Value> {
        Self::decode_task(
            self.query("SELECT * FROM tasks WHERE id=?", [id])?
                .into_iter()
                .next()
                .context("Task not found")?,
        )
    }
    pub fn tasks(&self, foreman: Option<&str>) -> Result<Vec<Value>> {
        let rows = if let Some(f) = foreman {
            self.query(
                "SELECT * FROM tasks WHERE foreman=? ORDER BY created,id",
                [f],
            )?
        } else {
            self.query("SELECT * FROM tasks ORDER BY created,id", [])?
        };
        rows.into_iter().map(Self::decode_task).collect()
    }
    pub fn workflow(&self, id: &str) -> Result<Value> {
        let mut v = self
            .query("SELECT * FROM workflows WHERE id=?", [id])?
            .into_iter()
            .next()
            .context("Workflow not found")?;
        for k in ["request", "policy", "data"] {
            v[k] = serde_json::from_str(v[k].as_str().context("Invalid stored workflow")?)?;
        }
        Ok(v)
    }
    pub fn workflows(&self, f: &str) -> Result<Vec<Value>> {
        self.query(
            "SELECT id FROM workflows WHERE foreman=? ORDER BY created,id",
            [f],
        )?
        .iter()
        .map(|r| self.workflow(&text(r, "id")))
        .collect()
    }
    pub fn allowed(&self, path: &str, foreman: &str) -> Result<bool> {
        if self.allowed.contains(path) {
            return Ok(true);
        }
        let rows=self.query("SELECT workflows.* FROM managed_checkouts JOIN workflows ON workflows.id=managed_checkouts.workflow_id WHERE path=?",[path])?;
        let Some(w) = rows.first() else {
            return Ok(false);
        };
        Ok([
            "preparing",
            "working",
            "snapshotting",
            "checking",
            "review_preparing",
            "reviewing",
        ]
        .contains(&text(w, "state").as_str())
            && (foreman.is_empty() || w["foreman"] == foreman)
            && self.projects[text(w, "project")]
                == serde_json::from_str::<Value>(w["policy"].as_str().unwrap_or("null"))?)
    }
    pub fn submit(&self, f: &str, mut req: Value) -> Result<Value> {
        self.binding(f)?;
        let id = text(&req, "task_id");
        let uid = uuid(&id)?;
        let role = text(&req, "role");
        ensure!(
            ["worker", "reviewer"].contains(&role.as_str()),
            "Task role must be worker or reviewer"
        );
        let prompt = text(&req, "prompt");
        ensure!(
            !prompt.trim().is_empty() && prompt.len() <= 30000,
            "Task prompt must contain 1 to 30000 UTF-8 bytes"
        );
        scope(&req["scope"])?;
        let raw = text(&req, "checkout");
        ensure!(
            Path::new(&raw).is_absolute(),
            "Use an absolute authorised checkout path"
        );
        let path = absolute(Path::new(&raw))?;
        let path = path.to_string_lossy().into_owned();
        ensure!(
            self.allowed(&path, f)?,
            "Checkout is not explicitly authorised"
        );
        let managed = req.get("workflow_id").is_some();
        ensure!(
            self.allowed.contains(&path) || managed,
            "Managed checkouts require controller-owned tasks"
        );
        if managed {
            let owner = self.query(
                "SELECT workflow_id FROM managed_checkouts WHERE path=?",
                [&path],
            )?;
            ensure!(
                owner
                    .first()
                    .is_some_and(|r| r["workflow_id"] == req["workflow_id"]),
                "Managed checkout ownership mismatch"
            );
        }
        if req.get("native_todos").is_some() {
            ensure!(
                managed && req["native_todos"] == true,
                "Native todos require a managed task"
            );
        }
        if let Some(tree) = req.get("input_tree") {
            ensure!(
                managed && tree.as_str().is_some_and(oid),
                "Snapshot input requires exact managed workflow tree"
            );
        }
        req["foreman"] = json!(f);
        req["checkout"] = json!(path);
        let revision = text(&req, "revision");
        ensure!(oid(&revision), "Use full immutable Git commit ID");
        self.transaction(||{if let Some(old)=self.query("SELECT * FROM tasks WHERE id=?",[&id])?.into_iter().next(){let task=Self::decode_task(old)?;ensure!(task["request"]==req,"Task UUID belongs to a different immutable request");return Ok(task)}
            let sparse=checkouts::task_checkout(&json!({"checkout":path,"role":role,"request":req}))?;
            self.exec("INSERT INTO tasks(id,foreman,role,checkout,state,request,session_id,prompt_id,sparse,created) VALUES (?,?,?,?,'queued',?,?,?,?,?)",params![id,f,role,path,canonical(&req),format!("ses_foreman_{}",uid.simple()),format!("msg_foreman_{}",uid.simple()),sparse,now()])?;self.task(&id)})
    }
    pub fn claim(&self) -> Result<Option<Value>> {
        self.transaction(|| {
            let tasks = self.tasks(None)?;
            let live = tasks
                .iter()
                .filter(|t| LIVE.contains(&text(t, "state").as_str()))
                .collect::<Vec<_>>();
            for mut t in tasks.clone() {
                if t["state"] != "queued"
                    || live
                        .iter()
                        .filter(|l| {
                            l["role"] == t["role"]
                                && l["request"]["workflow_id"] == t["request"]["workflow_id"]
                        })
                        .count()
                        >= 2
                    || live.iter().any(|l| l["checkout"] == t["checkout"])
                {
                    continue;
                }
                self.exec(
                    "UPDATE tasks SET state='starting' WHERE id=?",
                    [text(&t, "id")],
                )?;
                t["state"] = json!("starting");
                return Ok(Some(t));
            }
            Ok(None)
        })
    }
    pub fn record_model(&self, id: &str, m: &Value) -> Result<Value> {
        model(m)?;
        self.transaction(|| {
            let t = self.task(id)?;
            ensure!(
                t["state"] == "starting"
                    && (t["runtime_model"].is_null() || t["runtime_model"] == *m),
                "A recorded dispatch model cannot change"
            );
            self.exec(
                "UPDATE tasks SET runtime_model=? WHERE id=?",
                params![canonical(m), id],
            )?;
            self.task(id)
        })
    }
    pub fn update(&self, id: &str, state: &str, result: &str, detail: &str) -> Result<()> {
        ensure!(
            LIVE.contains(&state) || TERMINAL.contains(&state),
            "Unknown task state"
        );
        self.transaction(||{let t=self.task(id)?;if TERMINAL.contains(&text(&t,"state").as_str()){return Ok(())}self.exec("UPDATE tasks SET state=?,result=?,detail=? WHERE id=?",params![state,short(result,100000),short(detail,2000),id])?;if TERMINAL.contains(&state){self.exec("INSERT OR IGNORE INTO events(id,task_id,foreman,request_id,transport,refresh_todos,state) VALUES (?,?,?,?,'native',0,?)",params![uuid::Uuid::new_v4().to_string(),id,text(&t,"foreman"),uuid::Uuid::new_v4().to_string(),if t["request"].get("workflow_id").is_some(){"passive"}else{"pending"}])?;}Ok(())})
    }
    pub fn ack(&self, f: &str, id: &str) -> Result<()> {
        ensure!(
            !self
                .query(
                    "SELECT id FROM events WHERE id=? AND foreman=?",
                    params![id, f]
                )?
                .is_empty(),
            "Event does not belong to this Foreman"
        );
        self.exec("UPDATE events SET acked=1 WHERE id=?", [id])?;
        Ok(())
    }
    pub fn status(&self, f: &str, filter: &Value) -> Result<Value> {
        let binding = self.binding(f)?;
        let mut tasks = self.tasks(Some(f))?;
        let mut works = self.workflows(f)?;
        ensure!(
            filter["task_id"].is_null() || filter["workflow_id"].is_null(),
            "Select task or workflow, not both"
        );
        if let Some(id) = filter["workflow_id"].as_str() {
            let w = self.workflow(id)?;
            ensure!(
                w["foreman"] == f,
                "Workflow does not belong to this Foreman"
            );
            works = vec![w];
            tasks.retain(|t| t["request"]["workflow_id"] == id);
        }
        if let Some(id) = filter["task_id"].as_str() {
            let t = self.task(id)?;
            ensure!(t["foreman"] == f, "Task does not belong to this Foreman");
            tasks = vec![t];
        }
        let mut approvals=self.query("SELECT task_id,permission_id,form_id,state,detail FROM approvals WHERE foreman=? ORDER BY rowid",[f])?;
        if let Some(id) = filter["task_id"].as_str() {
            approvals.retain(|a| a["task_id"] == id);
        }
        Ok(
            json!({"foreman":f,"terminal_id":binding["terminal_id"],"limits":{"worker":2,"reviewer":2},"limit_scope":"per managed task; legacy tasks share two slots per role","completion_transport":"native-runtime","tasks":tasks.into_iter().rev().take(100).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>(),"events":self.query("SELECT * FROM events WHERE foreman=? AND acked=0 ORDER BY rowid",[f])?,"workflows":works.into_iter().rev().take(100).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>(),"projects":self.projects,"agent_progress":self.query("SELECT agent_todos.* FROM agent_todos JOIN tasks ON tasks.id=agent_todos.task_id WHERE tasks.foreman=?",[f])?,"approvals":approvals,"progress":progress::snapshot(self,f)?,"progress_delivery":self.query("SELECT seq,state,detail FROM progress_events WHERE foreman=? ORDER BY seq DESC LIMIT 1",[f])?}),
        )
    }
    pub fn save_workflow(&self, w: &mut Value, state: &str, detail: &str) -> Result<()> {
        w["state"] = json!(state);
        w["detail"] = json!(detail);
        if FINAL.contains(&state) && w["data"]["finished_at"].is_null() {
            w["data"]["finished_at"] = json!(now());
        }
        self.transaction(||{self.exec("UPDATE workflows SET state=?,detail=?,round=?,base_revision=?,data=? WHERE id=?",params![state,detail,w["round"].as_i64().unwrap_or(0),w["base_revision"].as_str(),canonical(&w["data"]),text(w,"id")])?;if FINAL.contains(&state){self.exec("INSERT OR IGNORE INTO events(id,task_id,foreman,request_id,transport,refresh_todos,workflow_id) VALUES (?,?,?,?,'native',0,?)",params![uuid::Uuid::new_v4().to_string(),format!("workflow:{}",text(w,"id")),text(w,"foreman"),uuid::Uuid::new_v4().to_string(),text(w,"id")])?;}Ok(())})
    }
    pub fn workflow_submit(&self, f: &str, args: Value) -> Result<Value> {
        workflows::submit(self, f, args)
    }
}
