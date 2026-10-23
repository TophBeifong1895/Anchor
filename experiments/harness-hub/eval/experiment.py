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
           [--variants NAME=V1,V2] [--by FIELD] [--alpha A] [--delta D] [--no-stop]

`--variants` gives an arm the instruction sentences of variants.py. Arms
that share a Host differ only there; `--by instructions_sha256` then tells
them apart, which holds within one task, so it needs exactly one.
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
import variants as variants_mod  # noqa: E402

HUB = Path(os.environ.get('ANCHOR_HUB', REPO / 'rust/target/debug/anchor-hub'))


def gate(local, reference, alpha, delta, by='host_sha256'):
    """The gate's report on the Runs recorded under `local` so far."""
    command = [str(HUB), 'gate', '--json', '--by', by, '--reference', reference,
               '--alpha', str(alpha), '--delta', str(delta), str(local)]
    return json.loads(subprocess.run(command, check=True, capture_output=True, text=True).stdout)


def hub_tool(local, name, args):
    command = [str(HUB), 'tool', name, '--args', json.dumps(args), str(local)]
    return json.loads(subprocess.run(command, check=True, capture_output=True, text=True).stdout)


def recorded(local, run_id, field):
    """`field` of the configuration manifest the AgentNode of `run_id` ran
    under, as the data hub recorded it."""
    items = hub_tool(local, 'invocations', {'run': run_id, 'node': tasks.AGENT})['items']
    config = next(i['config'] for i in items if i.get('config'))
    return hub_tool(local, 'configurations', {'digest': config})['manifest'][field]


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
    arms, variants, rest = {}, {}, []
    alpha, delta, stop, by = 0.05, 0.05, True, 'host_sha256'
    while args:
        arg = args.pop(0)
        if arg == '--arm':
            name, host = args.pop(0).split('=', 1)
            arms[name] = Path(host)
        elif arg == '--variants':
            name, names = args.pop(0).split('=', 1)
            variants[name] = [n for n in names.split(',') if n]
            unknown = set(variants[name]) - set(variants_mod.VARIANTS)
            if unknown:
                raise SystemExit(f'unknown variants: {sorted(unknown)}')
        elif arg == '--by':
            by = args.pop(0)
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
    reference = digests[names[0]]
    others = {a: digests[a] for a in names[1:]}
    if by != 'host_sha256' and len(chosen) != 1:
        # Another field, such as instructions_sha256, differs from task to
        # task, so it names an arm consistently only within one task.
        raise SystemExit('--by other than host_sha256 needs exactly one task')
    run_p0.save(local / 'design.json', {
        'arms': {arm: {'host': str(host), 'host_sha256': digests[arm],
                       'variants': {v: variants_mod.VARIANTS[v] for v in variants.get(arm, [])}}
                 for arm, host in arms.items()},
        'by': by,
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
                graph = tasks.graph(task)
                graph['agents']['solver']['instructions'] = variants_mod.instructions(
                    task, variants.get(arm, []))
                meta = run_p0.run_once(task.id, round_, arms[arm], live_env,
                                       graph=graph, commands=task.commands,
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
                if by != 'host_sha256':
                    value = recorded(local, run_id, by)
                    if arm == names[0]:
                        reference = value
                    else:
                        others[arm] = value
        report = gate(local, reference, alpha, delta, by)
        decided = decisions(report, others)
        run_p0.save(local / 'gate.json', {'round': round_, 'decisions': decided, 'report': report})
        print(json.dumps({'round': round_, 'gate': decided}), flush=True)
        if stop and settled(decided):
            break


if __name__ == '__main__':
    main()
