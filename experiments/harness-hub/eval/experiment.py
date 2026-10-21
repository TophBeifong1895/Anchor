"""Run a controlled comparison of harness arms on the evaluation task set.

Each arm is a Host build. Runs are interleaved and counterbalanced: in
every round each task runs once per arm, and the order of the arms flips
with the round and the task, so that drift in the provider or the machine
falls on both arms alike. Every Run is graded and recorded as in run.py;
which arm a Run belongs to is part of its recorded configuration (the Host
executable digest), so the data hub tells the arms apart without anything
from this script.

Usage: python3 experiment.py NAME ROUNDS TASK|exact|graded|all... --arm NAME=HOST ...
"""

import json
import os
from pathlib import Path
import sys

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent / 'p0'))
import run  # noqa: E402
import run_p0  # noqa: E402
import tasks  # noqa: E402


def main():
    args = sys.argv[1:]
    arms, rest = {}, []
    while args:
        arg = args.pop(0)
        if arg == '--arm':
            name, host = args.pop(0).split('=', 1)
            arms[name] = Path(host)
        else:
            rest.append(arg)
    name, rounds, which = rest[0], int(rest[1]), rest[2:]
    chosen = [t for t in tasks.TASKS.values() if any(w in ('all', t.tier, t.id) for w in which)]
    local = Path(os.environ.get('ANCHOR_EXPERIMENT_LOCAL', REPO / '.local/harness-hub-p6')) / name
    _, _, live_env = run_p0.live_host()
    run_p0.save(local / 'design.json', {
        'arms': {arm: {'host': str(host), 'host_sha256': run_p0.sha(host)}
                 for arm, host in arms.items()},
        'rounds': rounds,
        'tasks': {t.id: {'version': t.version, 'grader': tasks.grader_digest(t)} for t in chosen},
        'order': 'per round and task, arms in listed order when (round + task index) is even, '
                 'reversed otherwise',
    })
    names = list(arms)
    for round_ in range(1, rounds + 1):
        for index, task in enumerate(chosen):
            order = names if (round_ + index) % 2 == 0 else list(reversed(names))
            for arm in order:
                meta = run_p0.run_once(task.id, round_, arms[arm], live_env,
                                       graph=tasks.graph(task), commands=task.commands,
                                       local=local / arm, prefix=f'{name}-{arm}')
                root, run_id = Path(meta['root']), meta['run']
                meta.update({'experiment': name, 'arm': arm, 'task_version': task.version})
                if meta['guard']:
                    meta.update({'graded': False, 'score': None})
                else:
                    checks, reached = run.graded_checks(root, run_id, task)
                    run_p0.save(root / 'grade.json', run.record(root, run_id, task, checks))
                    meta.update({'graded': reached,
                                 'score': sum(c['passed'] for c in checks) / len(checks)})
                run_p0.save(root / 'meta.json', meta)
                print(json.dumps({k: meta[k] for k in ('run', 'arm', 'wall_seconds', 'guard',
                                                       'score')}), flush=True)


if __name__ == '__main__':
    main()
