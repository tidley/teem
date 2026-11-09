import json
from pathlib import Path
import tempfile
import unittest

from foreman_supervisor import Store


class AgentProgressTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='teem-agent-todos-')
        self.addCleanup(self.temp.cleanup)
        self.store = Store(Path(self.temp.name) / 'state', [])
        self.addCleanup(self.store.close)
        self.store.bind('ses_foreman', 'term_foreman')
        request = {'native_todos': True}
        with self.store.db:
            self.store.db.execute("""INSERT INTO tasks
                (id,foreman,role,checkout,state,request,session_id,prompt_id,sparse,created)
                VALUES ('task','ses_foreman','worker','/example','running',?,'ses_exact','msg_exact',0,0)""", (json.dumps(request),))
        self.calls = []
        self.response = {'message_id': 'msg_todos', 'todos': [{'content': 'Implement labels', 'status': 'in_progress'}]}
        self.fail = False
        case = self

        class Runtime:
            def read_todos(self, task):
                case.assertEqual(task['session_id'], 'ses_exact')
                case.calls.append(task['id'])
                if case.fail:
                    raise OSError('unavailable')
                return case.response

        self.runtime = Runtime()

    def poll(self, now):
        from foreman_agent_progress import AgentTodoReader
        AgentTodoReader(self.runtime).deliver(self.store, now=now)

    def test_real_agent_report_is_persisted_and_polling_is_throttled(self):
        self.poll(100)
        self.poll(101)
        self.assertEqual(self.calls, ['task'])
        row = self.store.db.execute('SELECT * FROM agent_todos').fetchone()
        self.assertEqual(json.loads(row['todos']), self.response['todos'])
        self.assertEqual(row['message_id'], 'msg_todos')

    def test_failed_reads_preserve_last_report_without_faking_completion(self):
        self.poll(100)
        self.fail = True
        self.poll(106)
        row = self.store.db.execute('SELECT * FROM agent_todos').fetchone()
        self.assertEqual(json.loads(row['todos'])[0]['status'], 'in_progress')
        self.assertIn('unavailable', row['detail'])

    def test_explicit_empty_list_clears_report_and_terminal_read_happens_once(self):
        self.poll(100)
        self.store.update('task', 'succeeded', result='done')
        self.response = {'message_id': 'msg_clear', 'todos': []}
        self.poll(106)
        self.poll(120)
        self.assertEqual(len(self.calls), 2)
        row = self.store.db.execute('SELECT * FROM agent_todos').fetchone()
        self.assertEqual(json.loads(row['todos']), [])
        self.assertTrue(row['final'])

    def test_legacy_task_without_capability_is_untouched(self):
        with self.store.db:
            self.store.db.execute("UPDATE tasks SET request='{}'")
        self.poll(100)
        self.assertEqual(self.calls, [])


if __name__ == '__main__':
    unittest.main()
