"""Read-only native agent todo observations. Never supplies execution or gate evidence."""

import json
import time

from foreman_supervisor import TERMINAL


class AgentTodoReader:
    def __init__(self, runtime):
        self.runtime = runtime

    def deliver(self, store, *, now=None):
        now = time.time() if now is None else now
        for task in store.tasks():
            if not task['request'].get('native_todos') or task['state'] == 'queued':
                continue
            with store.db:
                store.db.execute('INSERT OR IGNORE INTO agent_todos(task_id) VALUES (?)', (task['id'],))
            row = store.db.execute('SELECT * FROM agent_todos WHERE task_id=?', (task['id'],)).fetchone()
            if row['final'] or row['next_read'] > now:
                continue
            with store.db:
                store.db.execute('UPDATE agent_todos SET next_read=? WHERE task_id=?', (now + 5, task['id']))
            try:
                report = self.runtime.read_todos(task)
                with store.db:
                    if report is not None:
                        store.db.execute('UPDATE agent_todos SET todos=?,message_id=?,detail=? WHERE task_id=?',
                            (json.dumps(report['todos']), report['message_id'], 'Agent-reported progress, not check/review evidence', task['id']))
                    if task['state'] in TERMINAL:
                        store.db.execute('UPDATE agent_todos SET final=1 WHERE task_id=?', (task['id'],))
            except (OSError, ValueError, KeyError, TypeError):
                with store.db:
                    store.db.execute('UPDATE agent_todos SET detail=? WHERE task_id=?',
                                     ('Native agent todo report unavailable; last observed entries retained', task['id']))
