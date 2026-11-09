BEGIN TRANSACTION;
CREATE TABLE agent_todos (
                task_id TEXT PRIMARY KEY,todos TEXT,message_id TEXT,next_read REAL NOT NULL DEFAULT 0,
                final INTEGER NOT NULL DEFAULT 0,detail TEXT NOT NULL DEFAULT '');
CREATE TABLE approvals (
                task_id TEXT NOT NULL, permission_id TEXT NOT NULL, foreman TEXT NOT NULL,
                form_id TEXT NOT NULL UNIQUE, snapshot TEXT NOT NULL, payload TEXT NOT NULL,
                state TEXT NOT NULL, detail TEXT NOT NULL DEFAULT '', reply_attempted INTEGER NOT NULL DEFAULT 0, next_restore REAL NOT NULL DEFAULT 0,
                PRIMARY KEY (task_id, permission_id));
CREATE TABLE events (
                id TEXT PRIMARY KEY, task_id TEXT NOT NULL UNIQUE, foreman TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT 'pending', request_id TEXT NOT NULL UNIQUE,
                acked INTEGER NOT NULL DEFAULT 0, next_receipt REAL NOT NULL DEFAULT 0,
                transport TEXT NOT NULL DEFAULT 'native', detail TEXT NOT NULL DEFAULT '', refresh_todos INTEGER NOT NULL DEFAULT 0, workflow_id TEXT);
INSERT INTO "events" VALUES('old','old-task','ses_existing','uncertain','00000000-0000-0000-0000-000000000001',0,0.0,'legacy-codr','',0,NULL);
CREATE TABLE foremen (
                id TEXT PRIMARY KEY, terminal_id TEXT NOT NULL);
INSERT INTO "foremen" VALUES('ses_existing','terminal-existing');
CREATE TABLE managed_checkouts (path TEXT PRIMARY KEY,workflow_id TEXT NOT NULL);
CREATE TABLE plans (foreman TEXT PRIMARY KEY, items TEXT NOT NULL);
CREATE TABLE progress_events (
                seq INTEGER PRIMARY KEY AUTOINCREMENT, foreman TEXT NOT NULL,
                fingerprint TEXT NOT NULL, snapshot TEXT NOT NULL, request_id TEXT NOT NULL UNIQUE,
                state TEXT NOT NULL DEFAULT 'pending', sent_at REAL NOT NULL DEFAULT 0,
                next_receipt REAL NOT NULL DEFAULT 0, detail TEXT NOT NULL DEFAULT '');
CREATE TABLE tasks (
                id TEXT PRIMARY KEY, foreman TEXT NOT NULL, role TEXT NOT NULL,
                checkout TEXT NOT NULL, state TEXT NOT NULL, request TEXT NOT NULL,
                session_id TEXT NOT NULL UNIQUE, prompt_id TEXT NOT NULL UNIQUE,
                sparse INTEGER NOT NULL, result TEXT NOT NULL DEFAULT '',
                detail TEXT NOT NULL DEFAULT '', created REAL NOT NULL, runtime_model TEXT);
CREATE TABLE workflows (
                id TEXT PRIMARY KEY,foreman TEXT NOT NULL,project TEXT NOT NULL,
                request TEXT NOT NULL,policy TEXT NOT NULL,base_revision TEXT,
                state TEXT NOT NULL DEFAULT 'queued',round INTEGER NOT NULL DEFAULT 0,
                data TEXT NOT NULL DEFAULT '{"rounds":[]}',detail TEXT NOT NULL DEFAULT '',created REAL NOT NULL);
DELETE FROM "sqlite_sequence";
COMMIT;
