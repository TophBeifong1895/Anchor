"""Fixture Graphs and expected answers for P0. Inputs are synthetic and fixed."""

import random

SEED = 20261020


def t1_numbers():
    rng = random.Random(SEED)
    return [rng.randint(100, 999) for _ in range(30)]


def t1_expected():
    numbers = t1_numbers()
    top = sorted(numbers, reverse=True)[:5]
    return f"SUM={sum(numbers)}; TOP5={','.join(map(str, top))}"


def t2_files():
    rng = random.Random(SEED + 1)
    words = ['bolt', 'gear', 'pipe', 'lamp', 'wire', 'cell', 'disk', 'fuse', 'knob', 'plug']
    files = {}
    for number in range(1, 11):
        lines = []
        for _ in range(6):
            lines.append(f"item={rng.choice(words)} qty={rng.randint(1, 40)} "
                         f"color={rng.choice(['red', 'blue', 'green'])}")
        files[f'f{number:02d}.txt'] = lines
    return files


def t2_expected():
    red = 0
    blue_files = 0
    for lines in t2_files().values():
        fields = [dict(part.split('=') for part in line.split()) for line in lines]
        red += sum(int(f['qty']) for f in fields if f['color'] == 'red')
        blue_files += sum(f['color'] == 'blue' for f in fields) >= 2
    return {'red_qty': red, 'blue_files': blue_files}


def _prepare_command():
    parts = ['set -eu', 'mkdir -p data']
    for name, lines in t2_files().items():
        quoted = ' '.join(f"'{line}'" for line in lines)
        parts.append(f"printf '%s\\n' {quoted} > data/{name}")
    return '; '.join(parts)


def graphs():
    numbers = ', '.join(map(str, t1_numbers()))
    expected = t2_expected()
    t1 = {
        'objective': 'P0 fixture T1: deterministic arithmetic without tools',
        'entry': 'answer',
        'agents': {'solver': {
            'model': 'models.default.flash',
            'network': False,
            'reads': [],
            'writes': [],
            'instructions': (
                f'Here are 30 integers: {numbers}. Compute their sum and the five largest '
                'values in descending order. Do not call any tool other than final_result. '
                'Call final_result once, with summary exactly in the form '
                '`SUM=<sum>; TOP5=<a>,<b>,<c>,<d>,<e>` and nothing else.'),
        }},
        'nodes': [{'id': 'answer', 'agent': 'solver'}],
        'edges': [],
    }
    t2 = {
        'objective': 'P0 fixture T2: file tools, then a deterministic check',
        'entry': 'prepare',
        'ops': {
            'prepare': {'run': _prepare_command(), 'writes': ['data']},
            'verify': {
                'run': (f"set -eu; grep -qx 'red_qty={expected['red_qty']}' /in/worker/result.txt; "
                        f"grep -qx 'blue_files={expected['blue_files']}' /in/worker/result.txt; "
                        'cat /in/worker/result.txt'),
                'reads': ['result.txt'],
            },
        },
        'agents': {'worker': {
            'model': 'models.default.flash',
            'network': False,
            'reads': ['data'],
            'writes': ['result.txt'],
            'instructions': (
                'The directory /in/prepare/data contains ten text files. Every line has the form '
                '`item=<name> qty=<n> color=<color>`. Using authorized tools, read every file. '
                'Compute red_qty, the total qty over all lines whose color is red, and blue_files, '
                'the number of files with at least two blue lines. Write exactly two lines to '
                '/workspace/result.txt: `red_qty=<N>` and `blue_files=<M>`. Read the file back to '
                'check it. Then call final_result separately with route verify, a one-line summary, '
                'and an actual observed receipt as required by its schema.'),
        }},
        'nodes': [{'id': 'prepare', 'op': 'prepare'}, {'id': 'worker', 'agent': 'worker'},
                  {'id': 'verify', 'op': 'verify'}],
        'edges': [{'from': 'prepare', 'to': 'worker'}, {'from': 'worker', 'to': 'verify'}],
    }
    return {'T1': t1, 'T2': t2}
