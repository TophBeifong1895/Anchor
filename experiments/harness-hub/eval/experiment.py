"""Run a controlled comparison of harness arms on the evaluation task set.

Each arm is a Host build. Runs are interleaved and counterbalanced: in
every round each task runs once per arm, and the order of the arms flips
with the round and the task, so that drift in the provider or the machine
falls on both arms alike. Every Run is graded and recorded as in run.py;
which arm a Run belongs to is part of its recorded configuration (the Host
executable digest), so the data hub tells the arms apart without anything
from this script.

ROUNDS is a budget, not a sample size: after every round the data hub's gate
(`anchor-hub gate`) is read, and the experiment stops as soon as it has
decided every arm against the first one, the reference. Its bets are valid at
any stopping time, so reading it after every round costs no error rate. An
arm still open when the budget ends is not adopted. `--no-stop` runs every
round whatever the gate says.

Usage: python3 experiment.py NAME ROUNDS TASK|exact|graded|all... --arm NAME=HOST ...
           [--alpha A] [--delta D] [--no-stop]
"""

import json
import os
from pathlib import Path
import subprocess
import sys

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent / 'p0'))
import run  # noqa: E402
import run_p0  # noqa: E402
import tasks  # noqa: E402

HUB = Path(os.environ.get('ANCHOR_HUB', REPO / 'rust/target/debug/anchor-hub'))


def gate(local, reference, alpha, delta):
    """The gate's report on the Runs recorded under `local` so far."""
    command = [str(HUB), 'gate', '--json', '--reference', reference,
               '--alpha', str(alpha), '--delta', str(delta), str(local)]
    return json.loads(subprocess.run(command, check=True, capture_output=True, text=True).stdout)


def decisions(report, digests):
    """Each arm's decision by name; `digests` maps names to Host digests.
    An arm the gate has not seen yet is open."""
    by_digest = report.get('summary', {}).get('arms', {})
    return {arm: by_digest.get(digest, {}).get('decision', 'open')
            for arm, digest in digests.items()}


def settled(decided):
    return all(d in ('admitted', 'rejected') for d in decided.values())


def main():
    args = sys.argv[1:]
    arms, rest = {}, []
    alpha, delta, stop = 0.05, 0.05, True
    while args:
        arg = args.pop(0)
        if arg == '--arm':
            name, host = args.pop(0).split('=', 1)
            arms[name] = Path(host)
        elif arg == '--alpha':
            alpha = float(args.pop(0))
        elif arg == '--delta':
            delta = float(args.pop(0))
        elif arg == '--no-stop':
            stop = False
        else:
            rest.append(arg)
    name, rounds, which = rest[0], int(rest[1]), rest[2:]
    chosen = [t for t in tasks.TASKS.values() if any(w in ('all', t.tier, t.id) for w in which)]
    local = Path(os.environ.get('ANCHOR_EXPERIMENT_LOCAL', REPO / '.local/harness-hub-p6')) / name
    _, _, live_env = run_p0.live_host()
    digests = {arm: run_p0.sha(host) for arm, host in arms.items()}
    names = list(arms)
    reference, others = digests[names[0]], {a: digests[a] for a in names[1:]}
    run_p0.save(local / 'design.json', {
        'arms': {arm: {'host': str(host), 'host_sha256': digests[arm]}
                 for arm, host in arms.items()},
        'rounds': rounds,
        'stopping': {'rule': 'anchor-hub gate after every round, against the first arm',
                     'alpha': alpha, 'delta': delta} if stop else None,
        'tasks': {t.id: {'version': t.version, 'grader': tasks.grader_digest(t)} for t in chosen},
        'order': 'per round and task, arms in listed order when (round + task index) is even, '
                 'reversed otherwise',
    })
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
        report = gate(local, reference, alpha, delta)
        decided = decisions(report, others)
        run_p0.save(local / 'gate.json', {'round': round_, 'decisions': decided, 'report': report})
        print(json.dumps({'round': round_, 'gate': decided}), flush=True)
        if stop and settled(decided):
            break


if __name__ == '__main__':
    main()
