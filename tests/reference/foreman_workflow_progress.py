"""Frozen workflow-progress reference for Rust regression qualification."""

import json
import re
import uuid


def title(workflow):
    if workflow['request'].get('title'):
        return workflow['request']['title']
    prompt = ' '.join(workflow['request']['prompt'].split())
    function = re.match(r'(Add|Implement|Improve|Fix)\s+([A-Za-z_]\w*)\(', prompt, re.IGNORECASE)
    if function:
        return function[1].capitalize() + ' ' + function[2].replace('_', ' ')
    return prompt.split('. ')[0][:100].rstrip('.') or 'Requested changes'


def row(workflow, suffix, label, status, description, **metadata):
    return {'id': 'workflow_' + uuid.UUID(workflow['id']).hex + suffix, 'source': 'supervisor',
            'workflow_id': workflow['id'], 'label': label, 'status': status,
            'content': label + ' — ' + description, **metadata}


def task_state(store, task_id):
    if not task_id:
        return 'pending', 'pending', 'not started'
    task = store.task(task_id)
    state = task['state']
    status = ('completed' if state == 'succeeded' else 'cancelled' if state in ('failed', 'interrupted')
              else 'pending' if state == 'queued' else 'in_progress')
    description = {'queued': 'queued', 'starting': 'starting', 'accepted': 'native input accepted',
                   'running': 'running', 'uncertain': 'uncertain; no replacement started',
                   'succeeded': 'execution finished', 'failed': 'execution failed', 'interrupted': 'interrupted'}[state]
    if state == 'running' and 'human permission or answer' in task['detail']:
        description = 'waiting for human approval or answer'
    return state, status, description


def latest_result(store, foreman):
    finished = [w for w in store.workflows(foreman) if w['state'] in ('ready', 'blocked', 'needs_attention')]
    if not finished:
        return None
    workflow = max(finished, key=lambda w: (w['data'].get('finished_at', w['created']), w['id']))
    state = workflow['state']
    description = ('Completed · approved checks and both reviews passed; not integrated' if state == 'ready' else
                   ('Blocked' if state == 'blocked' else 'Needs attention') + ' · ' + ' '.join(workflow['detail'].split())[:800])
    return {'workflow_id': workflow['id'], 'state': state,
            'title': workflow['project'] + ' · ' + title(workflow), 'content': description}


def agent_detail(store, workflow, task_id, suffix, label):
    if not task_id:
        return []
    report = store.db.execute('SELECT todos FROM agent_todos WHERE task_id=?', (task_id,)).fetchone()
    if not report or report['todos'] is None:
        return []
    todos = json.loads(report['todos'])
    return [row(workflow, suffix + f'_reported_{i}', label + ' · reported: ' + todo['content'],
                todo['status'], 'agent-reported progress only', source='agent-reported', task_id=task_id)
            for i, todo in enumerate(todos[:8])]


def workflow_rows(store, foreman):
    workflows = store.workflows(foreman)
    items = []
    for workflow in workflows:
        state = workflow['state']
        if state in ('ready', 'needs_attention', 'blocked'):
            continue
        label = workflow['project'] + ' · ' + title(workflow)
        description = {'queued': 'queued', 'preparing': 'preparing isolated checkout', 'working': 'implementation',
            'snapshotting': 'freezing exact candidate', 'checking': 'running approved checks',
            'review_preparing': 'checks passed; preparing independent reviews', 'reviewing': 'two independent reviews',
            'uncertain': 'uncertain; reserved without replay', 'blocked': 'blocked; inspect recorded evidence',
            'needs_attention': 'revision budget exhausted; manual attention required',
            'ready': 'candidate checks and reviews passed; not integrated'}[state]
        status = ('completed' if state == 'ready' else 'cancelled' if state in ('blocked', 'needs_attention')
                  else 'pending' if state == 'queued' else 'in_progress')
        items.append(row(workflow, '', label, status, description, workflow_state=state))
        rounds = workflow['data']['rounds'] or [{}]
        for number, round in enumerate(rounds):
            if number != len(rounds) - 1:
                continue
            prefix = label + (' · Revision ' + str(number) if number else '')
            suffix = f'_round_{number}'
            stopped = state in ('blocked', 'needs_attention') or number < len(rounds) - 1
            ids = round.get('worker_ids', [round.get('worker_id')])
            if not round:
                ids = [None] * len(workflow['request'].get('workers', [None]))
            for index, task_id in enumerate(ids):
                worker_state, worker_status, worker_description = task_state(store, task_id)
                worker_label = prefix + ' · Worker' + (f' {index + 1}' if len(ids) > 1 else '')
                worker_suffix = suffix + f'_worker_{index}'
                items.append(row(workflow, worker_suffix, worker_label, worker_status, worker_description,
                                 task_id=task_id, task_state=worker_state))
                items.extend(agent_detail(store, workflow, task_id, worker_suffix, worker_label))
            result = round.get('check_result')
            results = {c['name']: c for c in result.get('results', [])} if result else {}
            for index, check in enumerate(workflow['policy']['checks']):
                outcome = results.get(check['name'])
                check_status, check_description = 'pending', 'not run yet'
                if outcome:
                    passed = outcome.get('returncode') == 0 and not outcome.get('timed_out') and not outcome.get('output_limited')
                    check_status = 'completed' if passed else 'cancelled'
                    check_description = 'passed (exit 0)' if passed else 'failed; see recorded check evidence'
                elif result:
                    check_status, check_description = 'cancelled', 'not run; check batch stopped'
                elif round.get('check_handle'):
                    check_status, check_description = 'in_progress', 'check batch active; individual result pending'
                elif stopped:
                    check_status, check_description = 'cancelled', 'not run; candidate superseded or workflow stopped'
                items.append(row(workflow, suffix + f'_check_{index}', prefix + ' · Check: ' + check['name'],
                                 check_status, check_description))
            for index, focus in enumerate(('Correctness review', 'Tests and scope review')):
                ids = round.get('review_ids', [])
                task_id = ids[index] if index < len(ids) else None
                review_state, review_status, review_description = task_state(store, task_id)
                verdicts = round.get('review_results', [])
                if index < len(verdicts):
                    passed = verdicts[index]['verdict'] == 'pass'
                    review_status = 'completed' if passed else 'cancelled'
                    review_description = 'passed; no findings' if passed else 'changes requested; see recorded findings'
                elif review_state == 'succeeded':
                    review_status, review_description = ('cancelled', 'execution finished; verdict did not qualify') if stopped else (
                        'in_progress', 'execution finished; verdict pending')
                elif stopped and review_status == 'pending':
                    review_status, review_description = 'cancelled', 'not run; candidate superseded or workflow stopped'
                review_label = prefix + ' · ' + focus
                items.append(row(workflow, suffix + f'_review_{index}', review_label, review_status, review_description,
                                 task_id=task_id, task_state=review_state))
                if number == len(rounds) - 1:
                    items.extend(agent_detail(store, workflow, task_id, suffix + f'_review_{index}', review_label))
    return items
