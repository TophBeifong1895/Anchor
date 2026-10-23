"""The evaluation task set: synthetic, seeded inputs, precise instructions,
expected values and a reference solution per task. Each task becomes one
Graph: a fixed `prepare` Op writes the inputs, the AgentNode under test
works on them, and a fixed `grade` Op checks its outputs with graders.py.
The grader is never part of what a variant may change.
"""

import base64
import hashlib
import json
import math
import random
import zlib

from graded import graded_tasks
import generators
from hard import hard_tasks
from harder import harder_tasks
from spec import AGENT, COMMANDS, COMPLETION, HERE, PYTHON, SEED, Task  # noqa: F401


# ---------------------------------------------------------------------------
# ledger: reconcile orders and payments
# ---------------------------------------------------------------------------

def ledger():
    rng = random.Random(SEED)
    customers = ['acme', 'globex', 'initech', 'umbrella', 'hooli', 'stark', 'wayne', 'tyrell']
    orders = [(f'O{1001 + i}', rng.choice(customers), rng.randint(500, 50000),
               f'2026-09-{rng.randint(1, 30):02d}') for i in range(60)]
    ids = [o[0] for o in orders]
    amount = {o[0]: o[2] for o in orders}
    rng.shuffle(ids)
    kinds = (['unpaid'] * 6 + ['short'] * 4 + ['over'] * 3 + ['split'] * 4 + ['split_short'] * 2
             + ['duplicate'] * 3)
    kinds += ['exact'] * (len(ids) - len(kinds))
    payments = []
    for order, kind in zip(ids, kinds):
        a = amount[order]
        if kind == 'exact':
            payments.append((order, a))
        elif kind == 'short':
            payments.append((order, a - rng.randint(1, a // 2)))
        elif kind == 'over':
            payments.append((order, a + rng.randint(1, 2000)))
        elif kind == 'split':
            part = rng.randint(1, a - 1)
            payments += [(order, part), (order, a - part)]
        elif kind == 'split_short':
            part = rng.randint(1, a // 2)
            payments += [(order, part), (order, a - part - rng.randint(1, 300))]
        elif kind == 'duplicate':
            payments += [(order, a), (order, a)]
    payments += [(f'O{9001 + i}', rng.randint(500, 9000)) for i in range(3)]
    rng.shuffle(payments)
    payments = [(f'P{5001 + i}', order, cents) for i, (order, cents) in enumerate(payments)]

    paid = {}
    for _, order, cents in payments:
        paid[order] = paid.get(order, 0) + cents
    known = set(amount)
    expected = {
        'unpaid': sorted(o for o in known if o not in paid),
        'short': sorted(o for o in known if 0 < paid.get(o, 0) < amount[o]),
        'over': sorted(o for o in known if paid.get(o, 0) > amount[o]),
        'unknown_payments': sorted(p for p, o, _ in payments if o not in known),
        'total_received_cents': sum(c for _, o, c in payments if o in known),
    }
    files = {
        'orders.csv': 'order_id,customer,amount_cents,date\n' + ''.join(
            f'{o},{c},{a},{d}\n' for o, c, a, d in orders),
        'payments.csv': 'payment_id,order_id,amount_cents\n' + ''.join(
            f'{p},{o},{c}\n' for p, o, c in payments),
    }
    instructions = (
        'Reconcile the orders in /in/prepare/data/orders.csv with the payments in '
        '/in/prepare/data/payments.csv (both CSV with a header row). An order\'s paid total is the '
        'sum of amount_cents over all payments whose order_id is that order; an order may have '
        'several payments. Write /workspace/result.json, a JSON object with exactly these keys: '
        '`unpaid`: order ids with no payment; `short`: order ids whose paid total is above 0 but '
        'below amount_cents; `over`: order ids whose paid total is above amount_cents; '
        '`unknown_payments`: payment ids whose order_id is not in orders.csv; '
        '`total_received_cents`: the sum of amount_cents of all payments whose order_id is in '
        'orders.csv, as an integer. Every list is sorted ascending and holds strings. ' + COMPLETION)
    return Task('ledger', 1, files, instructions, ['result.json'], expected,
                {'result.json': json.dumps(expected)})


# ---------------------------------------------------------------------------
# logs: triage service logs
# ---------------------------------------------------------------------------

def _logs(seed):
    rng = random.Random(seed)
    services = ['api', 'auth', 'billing', 'search']
    codes = [f'E{n}' for n in range(10, 20)]
    weights = [rng.randint(1, 9) for _ in codes]
    files, lines = {}, []
    for host in range(1, 13):
        stamps = sorted(rng.randint(0, 7199) for _ in range(60))
        rows = []
        for second in stamps:
            service = rng.choice(services)
            roll = rng.random()
            level = 'ERROR' if roll < 0.08 else 'WARN' if roll < 0.2 else 'INFO'
            latency = rng.randint(5, 900 if service == 'search' else 300)
            code = rng.choices(codes, weights)[0] if level == 'ERROR' else '-'
            ts = f'2026-10-01T{8 + second // 3600:02d}:{second // 60 % 60:02d}:{second % 60:02d}Z'
            rows.append(f'{ts} {level} {service} {latency} {code}')
            lines.append((ts, level, service, latency, code))
        files[f'logs/app-{host:02d}.log'] = '\n'.join(rows) + '\n'
    deploys = sorted((rng.randint(0, 7199), rng.choice(services), f'v{n}') for n in range(1, 7))
    deploys.append((rng.randint(1800, 5400), 'billing', 'v9'))
    deploys.sort()
    files['deploys.txt'] = ''.join(
        f'2026-10-01T{8 + s // 3600:02d}:{s // 60 % 60:02d}:{s % 60:02d}Z {svc} {ver}\n'
        for s, svc, ver in deploys)
    last_billing = max(f'2026-10-01T{8 + s // 3600:02d}:{s // 60 % 60:02d}:{s % 60:02d}Z'
                       for s, svc, _ in deploys if svc == 'billing')
    errors = [l for l in lines if l[1] == 'ERROR']
    by_code = {}
    for l in errors:
        by_code[l[4]] = by_code.get(l[4], 0) + 1
    by_minute = {}
    for l in lines:
        by_minute[l[0][:16]] = by_minute.get(l[0][:16], 0) + 1
    search = sorted(l[3] for l in lines if l[2] == 'search')
    expected = {
        'errors_by_service': {s: sum(l[2] == s for l in errors) for s in services},
        'top_error_code': max(by_code, key=by_code.get),
        'p95_latency_ms_search': search[math.ceil(0.95 * len(search)) - 1],
        'billing_errors_after_last_deploy': sum(
            1 for l in errors if l[2] == 'billing' and l[0] > last_billing),
        'busiest_minute': max(by_minute, key=by_minute.get),
    }
    unique = (sorted(by_code.values())[-1] != sorted(by_code.values())[-2]
              and sorted(by_minute.values())[-1] != sorted(by_minute.values())[-2])
    return files, expected, unique


def logs():
    seed = SEED
    files, expected, unique = _logs(seed)
    while not unique:
        seed += 1
        files, expected, unique = _logs(seed)
    instructions = (
        'The directory /in/prepare/data/logs holds twelve log files. Every line has the form '
        '`<timestamp> <level> <service> <latency_ms> <code>`, where code is `-` unless the level is '
        'ERROR. /in/prepare/data/deploys.txt has lines `<timestamp> <service> <version>`. '
        'Timestamps are UTC in the form 2026-10-01T08:15:03Z. Write /workspace/result.json, a JSON '
        'object with exactly these keys: `errors_by_service`: an object mapping each of api, auth, '
        'billing and search to its number of ERROR lines; `top_error_code`: the code with the most '
        'ERROR lines; `p95_latency_ms_search`: the 95th percentile of latency_ms over all search '
        'lines by nearest rank (sort ascending, take the value at 1-based position '
        'ceil(0.95 * n)), as an integer; `billing_errors_after_last_deploy`: the number of billing '
        'ERROR lines strictly after the latest billing deploy; `busiest_minute`: the minute with '
        'the most lines over all files, as `YYYY-MM-DDTHH:MM`. ' + COMPLETION)
    return Task('logs', 1, files, instructions, ['result.json'], expected,
                {'result.json': json.dumps(expected)})


# ---------------------------------------------------------------------------
# contacts: normalize by exact rules
# ---------------------------------------------------------------------------

def _phone(raw):
    digits = ''.join(ch for ch in raw if ch.isdigit())
    if len(digits) == 13 and digits.startswith('86'):
        digits = digits[2:]
    if len(digits) == 11 and digits.startswith('1'):
        return f'+86 {digits[:3]} {digits[3:7]} {digits[7:]}'
    return ''


def contacts():
    rng = random.Random(SEED + 2)
    first = ['alice', 'bruno', 'chen', 'dara', 'emil', 'fatima', 'goran', 'hana', 'ivan', 'jun',
             'kofi', 'lena', 'mateo', 'nadia', 'omar', 'priya', 'quinn', 'rosa', 'sven', 'tara']
    last = ['smith', 'wang', 'garcia', 'novak', 'okafor', 'silva', 'tanaka', 'muller', 'rossi',
            'kumar', 'li', 'jensen', 'haddad', 'costa', 'berg']
    domains = ['example.com', 'mail.test', 'corp.example']

    def mess(text):
        return ''.join(ch.upper() if rng.random() < 0.3 else ch for ch in text)

    def phone_text(digits):
        style = rng.randrange(6)
        if style == 0:
            return digits
        if style == 1:
            return f'{digits[:3]} {digits[3:7]} {digits[7:]}'
        if style == 2:
            return f'+86 {digits[:3]}-{digits[3:7]}-{digits[7:]}'
        if style == 3:
            return f'86 {digits}'
        if style == 4:
            return f'(010) {rng.randint(1000, 9999)} {rng.randint(1000, 9999)}'
        return rng.choice(['', '12345', '+1 415 555 0100'])

    people, seen = [], set()
    while len(people) < 40:
        f, l = rng.choice(first), rng.choice(last)
        email = f'{f}.{l}{rng.randint(1, 99)}@{rng.choice(domains)}'
        if email in seen:
            continue
        seen.add(email)
        digits = '1' + ''.join(str(rng.randint(0, 9)) for _ in range(10))
        people.append((f'{f} {l}', email, phone_text(digits)))
    raw = []
    for name, email, phone in people:
        raw.append((name, email, phone))
    for name, email, phone in rng.sample(people, 8):
        raw.insert(rng.randint(0, len(raw)),
                   (name.upper(), email.upper(), phone_text('1' + '0' * 10)))
    # The first occurrence of each email wins, so duplicates placed before
    # their original change what is kept.
    lines, kept = [], {}
    for name, email, phone in raw:
        spaced_name = '  '.join(mess(part) for part in name.split()) if rng.random() < 0.5 else mess(name)
        lines.append(f'  {spaced_name} |  {mess(email)} | {phone}  ')
        normalized = email.strip().lower()
        if normalized not in kept:
            title = ' '.join(w[0].upper() + w[1:].lower() for w in name.split())
            kept[normalized] = (title, normalized, _phone(phone))
    want = sorted(kept.values(), key=lambda row: row[1])
    reference = 'name,email,phone\n' + ''.join(f'{n},{e},{p}\n' for n, e, p in want)
    instructions = (
        'Normalize the contact list /in/prepare/data/contacts.txt. Each line holds three fields, '
        'name, email and phone, separated by `|`. Apply these rules exactly: trim every field; '
        'in the name, collapse runs of spaces to one and write every word with its first letter '
        'upper case and the rest lower case; write the email in lower case; keep only the first '
        'line of each email (compared after lower-casing) and drop later ones; for the phone, keep '
        'only its digits, drop a leading 86 if 13 digits remain, and if exactly 11 digits starting '
        'with 1 remain write it as `+86 XXX XXXX XXXX`, otherwise leave the phone empty. Write '
        '/workspace/contacts.csv with the header line `name,email,phone` and one line per kept '
        'contact, sorted by email ascending, fields separated by commas without quotes. '
        + COMPLETION)
    return Task('contacts', 1, {'contacts.txt': '\n'.join(lines) + '\n'}, instructions,
                ['contacts.csv'], {'rows': [list(r) for r in want]}, {'contacts.csv': reference})


# ---------------------------------------------------------------------------
# repair: fix a module against its documented behavior
# ---------------------------------------------------------------------------

BUGGY = '''"""Inventory helpers. Each function's docstring is its contract."""


def parse_line(line):
    """Parse `sku,qty,price` into (sku, qty, price_cents): sku upper-cased,
    qty an int, price a decimal string such as 12.34 turned into exact
    cents (1234). Spaces around fields are ignored."""
    sku, qty, price = [part.strip() for part in line.split(',')]
    return sku.upper(), int(qty), int(float(price) * 100)


def merge(lines):
    """Total qty per sku over all lines. Skus compare case-insensitively.
    Blank lines and lines whose first non-space character is # are skipped."""
    totals = {}
    for line in lines:
        if not line.strip() or line.startswith('#'):
            continue
        sku, qty, _ = parse_line(line)
        totals[sku] = qty
    return totals


def page(items, number, size):
    """The items of 1-based page `number`, `size` items per page. A page
    past the end is empty."""
    start = number * size
    return items[start:start + size]


def value(lines):
    """Total stock value in cents: the sum over skus of total qty times the
    price of the sku's last line. Lines are skipped as in merge."""
    qty = merge(lines)
    price = {}
    for line in lines:
        if not line.strip() or line.lstrip().startswith('#'):
            continue
        sku, _, cents = parse_line(line)
        price[sku] = cents
    return sum(qty[sku] * price[sku] for sku in qty)
'''

VISIBLE = '''from inventory import merge, page

assert page([1, 2, 3, 4, 5], 1, 2) == [1, 2]
assert page([1, 2, 3, 4, 5], 3, 2) == [5]
assert merge(['a,1,1.00', 'A,2,1.00']) == {'A': 3}
print('OK')
'''

HIDDEN = {
    'visible': VISIBLE.replace('from inventory import merge, page\n', ''),
    'parse': '''
assert parse_line(' ab , 3 , 0.29 ') == ('AB', 3, 29)
assert parse_line('x,1,19.99') == ('X', 1, 1999)
assert parse_line('x,1,4.35') == ('X', 1, 435)
print('OK')
''',
    'merge': '''
assert merge(['a,1,1.00', '', '  # note', 'b,2,1.00', 'A,4,2.00', '#x,9,1.00']) == {'A': 5, 'B': 2}
assert merge([]) == {}
print('OK')
''',
    'page': '''
items = list(range(10))
assert page(items, 1, 3) == [0, 1, 2]
assert page(items, 4, 3) == [9]
assert page(items, 5, 3) == []
print('OK')
''',
    'value': '''
assert value(['a,2,0.29', 'b,1,1.10', ' # c,5,1.00', 'A,1,0.30']) == 3 * 30 + 110
print('OK')
''',
}

FIXED = (BUGGY
         .replace('int(float(price) * 100)', 'int(round(float(price) * 100))')
         .replace("line.startswith('#')", "line.lstrip().startswith('#')", 1)
         .replace('totals[sku] = qty', 'totals[sku] = totals.get(sku, 0) + qty')
         .replace('start = number * size', 'start = (number - 1) * size'))


def repair():
    instructions = (
        'The Python module /in/prepare/data/inventory.py does not do what its docstrings say. '
        'Its tests are in /in/prepare/data/test_inventory.py (run them with python3.12 from a '
        'copy of both files). Fix the module so that every function meets its docstring; the tests '
        'given are not complete. Do not change any docstring or function signature. Write the '
        'fixed module to /workspace/inventory.py. ' + COMPLETION)
    return Task('repair', 1, {'inventory.py': BUGGY, 'test_inventory.py': VISIBLE}, instructions,
                ['inventory.py'], {'tests': HIDDEN}, {'inventory.py': FIXED})


# ---------------------------------------------------------------------------
# schedule: satisfy constraints
# ---------------------------------------------------------------------------

def _solve(conference):
    talks, rooms, slots = conference['talks'], conference['rooms'], conference['slots']
    before, unavailable = conference['before'], conference['unavailable']
    names = sorted(talks)
    cells = [(r, s) for s in slots for r in rooms]

    def ok(assign, talk, cell):
        room, slot = cell
        if rooms[room] < talks[talk]['audience'] or cell in assign.values():
            return False
        if slot in unavailable.get(talks[talk]['speaker'], []):
            return False
        for other, (_, other_slot) in assign.items():
            if other_slot == slot and talks[other]['speaker'] == talks[talk]['speaker']:
                return False
        for a, b in before:
            if a == talk and b in assign and slots.index(slot) >= slots.index(assign[b][1]):
                return False
            if b == talk and a in assign and slots.index(assign[a][1]) >= slots.index(slot):
                return False
        return True

    def search(assign):
        if len(assign) == len(names):
            return dict(assign)
        talk = names[len(assign)]
        for cell in cells:
            if ok(assign, talk, cell):
                assign[talk] = cell
                found = search(assign)
                if found:
                    return found
                del assign[talk]
        return None

    return search({})


def schedule():
    seed = SEED + 3
    while True:
        rng = random.Random(seed)
        rooms = {'A': 120, 'B': 60, 'C': 30}
        slots = ['09:00', '10:00', '11:00', '14:00']
        speakers = ['ana', 'ben', 'cleo', 'dev', 'eli', 'fay', 'gus']
        talks = {f'T{n}': {'speaker': rng.choice(speakers),
                           'audience': rng.choice([20, 25, 40, 50, 80, 100])}
                 for n in range(1, 11)}
        names = sorted(talks)
        before = [tuple(sorted(rng.sample(names, 2))) for _ in range(4)]
        unavailable = {s: rng.sample(slots, rng.randint(1, 2)) for s in rng.sample(speakers, 3)}
        conference = {'rooms': rooms, 'slots': slots, 'talks': talks,
                      'before': [list(p) for p in before], 'unavailable': unavailable}
        solution = _solve(conference)
        if solution:
            break
        seed += 1
    instructions = (
        'Schedule a conference. /in/prepare/data/conference.json gives `rooms` (room to seat '
        'capacity), `slots` (in time order), `talks` (talk to speaker and expected audience), '
        '`before` (pairs [a, b]: talk a must be in an earlier slot than talk b) and `unavailable` '
        '(speaker to the slots they cannot speak in). Put every talk in one room and one slot so '
        'that no room holds two talks in one slot, no speaker gives two talks in one slot, every '
        'room seats its talk\'s audience, and every `before` and `unavailable` rule holds. Write '
        '/workspace/schedule.json, a JSON object mapping every talk to [room, slot]. '
        + COMPLETION)
    expected = {'talks': talks, 'rooms': rooms, 'slots': slots,
                'before': [list(p) for p in before], 'unavailable': unavailable}
    return Task('schedule', 1, {'conference.json': json.dumps(conference, indent=2)},
                instructions, ['schedule.json'], expected,
                {'schedule.json': json.dumps({t: list(c) for t, c in solution.items()})})


def all_tasks():
    return ([ledger(), logs(), contacts(), repair(), schedule()] + hard_tasks() + harder_tasks()
            + graded_tasks())


TASKS = {task.id: task for task in all_tasks()}

# ---------------------------------------------------------------------------
# Graphs
# ---------------------------------------------------------------------------


# Tasks whose graders live in graders_harder.py; the others' grader source,
# and so their grader digests, is graders.py alone.
HARDER = {'shards', 'payroll', 'contracts', 'bugtrail'}


def grader_source(task=None):
    """The grader code `task`'s grade Op runs, all of it without a task."""
    source = (HERE / 'graders.py').read_text()
    if task is None or task.id in HARDER:
        source += '\n\n' + (HERE / 'graders_harder.py').read_text()
    return source


def grading():
    """Every task's grader by id, loaded the way a grade Op loads them."""
    namespace = {'__name__': 'graders'}
    exec(compile(grader_source(), 'graders', 'exec'), namespace)
    return namespace['GRADERS'], namespace


def grader_digest(task):
    """The grader's configuration: task definition, expected values and the
    grader code. Stamped on every grade it records."""
    material = json.dumps({'task': task.id, 'version': task.version, 'expected': task.expected,
                           'grader': grader_source(task)}, sort_keys=True)
    return hashlib.sha256(material.encode()).hexdigest()


# An Op runs as `sh -c <run>`, one argv string, which Linux caps at 128 KiB
# (MAX_ARG_STRLEN). A script near that size is carried compressed.
MAX_RUN = 131072
COMPRESS_ABOVE = 64 * 1024


def _python(script):
    if len(script.encode()) > COMPRESS_ABOVE:
        packed = base64.b64encode(zlib.compress(script.encode(), 9)).decode()
        script = f'import base64, zlib\nexec(zlib.decompress(base64.b64decode({packed!r})).decode())'
    return f"{PYTHON} - <<'ANCHOR_EVAL_SCRIPT'\n{script}\nANCHOR_EVAL_SCRIPT"


def _writer(directory, files):
    return _python(
        'import json, os\n'
        f'files = json.loads({json.dumps(json.dumps(files))})\n'
        'for name, text in files.items():\n'
        f'    path = os.path.join({directory!r}, name)\n'
        '    os.makedirs(os.path.dirname(path), exist_ok=True)\n'
        '    open(path, "w").write(text)\n')


def _generate(task):
    """Run the task's generator in the sandbox, refuse inputs whose digest
    differs from the task definition's, and write them under data/."""
    function, seed = task.generator
    return _python(
        (HERE / 'generators.py').read_text()
        + '\nimport os, sys\n'
        + f'files, _ = {function}({seed})\n'
        + f'if digest(files) != {generators.digest(task.files)!r}:\n'
        + '    sys.exit("generated inputs differ from the task definition")\n'
        + 'for name, data in files.items():\n'
        + '    path = os.path.join("data", name)\n'
        + '    os.makedirs(os.path.dirname(path), exist_ok=True)\n'
        + '    open(path, "wb").write(data)\n')


def grade_command(task, directory='/in/' + AGENT):
    return _python(
        grader_source(task)
        + '\n'
        + f'checks = GRADERS[{task.id!r}]({directory!r}, json.loads({json.dumps(json.dumps(task.expected))}))\n'
        + f'print(json.dumps({{"anchor_grade": 1, "task": {task.id!r}, "checks": '
          '[{"name": n, "passed": bool(p)} for n, p in checks]}))\n')


def graph(task, agent=None):
    """The Graph of `task`. `agent` replaces the AgentNode under test: a
    variant, or for the reference run an Op that writes the reference."""
    node = agent or {'agent': 'solver'}
    graph = {
        'objective': f'Evaluation task {task.id} v{task.version}',
        'entry': 'prepare',
        'ops': {
            'prepare': {'run': 'set -eu; mkdir -p data\n' + (
                _generate(task) if task.generator else _writer('data', task.files)),
                        'writes': ['data']},
            'grade': {'run': grade_command(task), 'reads': list(task.outputs)},
        },
        'agents': {'solver': {
            'model': 'models.default.flash',
            'network': False,
            'reads': ['data'],
            'writes': list(task.outputs),
            'instructions': task.instructions,
        }},
        'nodes': [{'id': 'prepare', 'op': 'prepare'}, dict(node, id=AGENT),
                  {'id': 'grade', 'op': 'grade'}],
        'edges': [{'from': 'prepare', 'to': AGENT}, {'from': AGENT, 'to': 'grade'}],
    }
    for name, op in graph['ops'].items():
        assert len(op['run'].encode()) < MAX_RUN, (task.id, name)
    return graph


def reference_graph(task):
    """The task solved by an Op that writes the reference outputs: grades 1.0
    if graders, Graph plumbing and sandbox agree."""
    g = graph(task, {'op': 'reference'})
    g['ops']['reference'] = {'run': 'set -eu\n' + _writer('.', task.reference),
                             'reads': ['data'], 'writes': list(task.outputs)}
    del g['agents']
    assert len(g['ops']['reference']['run'].encode()) < MAX_RUN, task.id
    return g
