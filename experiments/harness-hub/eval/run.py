"""Run evaluation tasks in isolated Hosts, grade them, and record the grades.

Each Run uses the P0 isolation: a fresh state root under
.local/harness-hub-p4/runs, the live service's Host environment and a
private snapshot of its saved model settings; the live data is never
touched. After the Host exits, the grade Op's checks are read back from the
Run's own history and recorded through `anchor-eval` into the Run root's
`kernel-grades`, where the data hub reads them.

Usage: python3 run.py TASK|exact|graded|all COUNT [--reference] [--start N]
       python3 run.py --regrade RUNS_DIR

--reference replaces the AgentNode with an Op that writes the reference
solution: a full pass checks graders, Graph plumbing and sandbox together.
Environment: ANCHOR_P0_HOST (Host binary), ANCHOR_EVAL (anchor-eval binary).
"""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from urllib.parse import quote

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
LOCAL = Path(os.environ.get('ANCHOR_EVAL_LOCAL', REPO / '.local/harness-hub-p4'))
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent / 'p0'))
import graders  # noqa: E402
import run_p0  # noqa: E402
import tasks  # noqa: E402

EVAL = Path(os.environ.get('ANCHOR_EVAL', REPO / 'rust/target/debug/anchor-eval'))


def check_names(task):
    with tempfile.TemporaryDirectory() as empty:
        return [name for name, _ in graders.GRADERS[task.id](empty, task.expected)]


def graded_checks(root, run, task):
    """The grade Op's checks from the Run's history, or every check failed
    when the Run never reached a grade."""
    history = root / 'state/kernel-runs' / quote(f'run/{run}', safe='') / 'history.jsonl'
    if history.exists():
        for line in history.read_text().splitlines():
            record = json.loads(line)
            if record['submission'].get('slot') != 'grade#1' or \
                    record['verdict']['effect'] != 'accepted':
                continue
            stdout = (record['submission']['content'].get('output') or {}).get('stdout') or ''
            for text in reversed(stdout.strip().splitlines()):
                try:
                    printed = json.loads(text)
                except ValueError:
                    continue
                if printed.get('anchor_grade') == 1 and printed.get('task') == task.id:
                    return printed['checks'], True
    return [{'name': name, 'passed': False} for name in check_names(task)], False


def record(root, run, task, checks):
    grade = {'version': 1, 'run_id': run, 'task': task.id, 'task_version': task.version,
             'checks': checks}
    done = subprocess.run([str(EVAL), 'record', '--state', str(root / 'state'),
                           '--config', tasks.grader_digest(task)],
                          input=json.dumps(grade), capture_output=True, text=True, check=False)
    if done.returncode != 0:
        raise SystemExit(f'recording the grade of {run} failed: {done.stdout}{done.stderr}')
    return grade


def agent_outputs(root, run, task):
    """The files the AgentNode committed, as the Run recorded them."""
    history = root / 'state/kernel-runs' / quote(f'run/{run}', safe='') / 'history.jsonl'
    for line in history.read_text().splitlines() if history.exists() else []:
        record = json.loads(line)
        if record['submission'].get('slot') == f'{tasks.AGENT}#1' and \
                record['verdict']['effect'] == 'accepted':
            commit = record['submission']['content']['commit']['id']
            files = root / 'state/artifacts' / commit / 'files'
            return {name: (files / name).read_text() for name in task.outputs
                    if (files / name).is_file()}
    return {}


def regrade(directory):
    """Grade every Run under `directory` again with the current task
    definitions, record the grades, and report whether each equals the grade
    printed by the grade Op inside the Run."""
    for root in sorted(Path(directory).glob('*/')):
        meta = json.loads((root / 'meta.json').read_text())
        task = tasks.TASKS[meta['task']]
        if meta.get('guard'):
            continue
        in_run, _ = graded_checks(root, meta['run'], task)
        version_at_run = json.loads((root / 'grade.json').read_text())['task_version'] \
            if (root / 'grade.json').exists() else meta.get('task_version')
        with tempfile.TemporaryDirectory() as scratch:
            for name, text in agent_outputs(root, meta['run'], task).items():
                Path(scratch, name).write_text(text)
            checks = [{'name': n, 'passed': bool(p)}
                      for n, p in graders.GRADERS[task.id](scratch, task.expected)]
        grade = record(root, meta['run'], task, checks)
        run_p0.save(root / 'grade.json', grade)
        meta.update({'task_version': task.version,
                     'score': sum(c['passed'] for c in checks) / len(checks)})
        run_p0.save(root / 'meta.json', meta)
        # Same task version: the local grade must equal the one the grade Op
        # printed in the sandbox.
        print(json.dumps({'run': meta['run'], 'version': [version_at_run, task.version],
                          'same_as_in_run': in_run == checks, 'score': meta['score']}),
              flush=True)


def main():
    args = sys.argv[1:]
    if args and args[0] == '--regrade':
        regrade(args[1])
        return
    reference = '--reference' in args
    start = int(args[args.index('--start') + 1]) if '--start' in args else 1
    positional = [a for i, a in enumerate(args)
                  if not a.startswith('--') and (i == 0 or args[i - 1] != '--start')]
    which, count = positional[0], int(positional[1])
    chosen = [t for t in tasks.TASKS.values()
              if which in ('all', t.tier, t.id)]
    pid, host, live_env = run_p0.live_host()
    if os.environ.get('ANCHOR_P0_HOST'):
        host = Path(os.environ['ANCHOR_P0_HOST'])
    run_p0.save(LOCAL / 'identity.json', {
        'live_pid': pid, 'host': str(host), 'host_sha256': run_p0.sha(host),
        'goose_sha256': run_p0.sha(live_env['ANCHOR_GOOSE_BINARY'])
        if 'ANCHOR_GOOSE_BINARY' in live_env else None,
        'graders': {t.id: tasks.grader_digest(t) for t in tasks.TASKS.values()}})
    for index in range(start, start + count):
        for task in chosen:
            graph = tasks.reference_graph(task) if reference else tasks.graph(task)
            meta = run_p0.run_once(task.id, index, host, live_env, graph=graph,
                                   commands=task.commands,
                                   local=LOCAL / ('reference' if reference else 'baseline'),
                                   prefix='ref' if reference else 'eval')
            root, run = Path(meta['root']), meta['run']
            meta.update({'variant': 'reference' if reference else 'baseline',
                         'task_version': task.version})
            if meta['guard']:
                # Stopped by this runner's watchdog, not ended by the agent:
                # there is no result to grade (P0 rule: a guard is no failure).
                meta.update({'graded': False, 'score': None})
            else:
                checks, reached = graded_checks(root, run, task)
                run_p0.save(root / 'grade.json', record(root, run, task, checks))
                meta.update({'graded': reached,
                             'score': sum(c['passed'] for c in checks) / len(checks)})
            run_p0.save(root / 'meta.json', meta)
            print(json.dumps({k: meta[k] for k in ('run', 'wall_seconds', 'returncode', 'guard',
                                                   'graded', 'score')}), flush=True)


if __name__ == '__main__':
    main()
