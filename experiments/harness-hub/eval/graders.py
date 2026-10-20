"""Graders of the evaluation tasks. Each grader is a pure function of the
directory holding the agent's outputs and the task's expected values, and
returns named checks. Standard library only: this file runs verbatim inside
the grade Op's sandbox, and locally in the grader tests.

A check never raises: unreadable or missing output fails it.
"""

import csv
import io
import json
import math
import os
import shutil
import subprocess
import sys
import tempfile


def _read(directory, name):
    try:
        with open(os.path.join(directory, name), encoding='utf-8') as stream:
            return stream.read()
    except (OSError, UnicodeDecodeError):
        return None


def _json(directory, name):
    text = _read(directory, name)
    if text is None:
        return None
    try:
        value = json.loads(text)
    except ValueError:
        return None
    return value if isinstance(value, dict) else None


def _fields(result, expected, keys):
    return [(key, result is not None and result.get(key) == expected[key]) for key in keys]


def grade_ledger(directory, expected):
    result = _json(directory, 'result.json')
    keys = ['unpaid', 'short', 'over', 'unknown_payments', 'total_received_cents']
    shape = result is not None and sorted(result) == sorted(keys)
    return [('shape', shape)] + _fields(result, expected, keys)


def grade_logs(directory, expected):
    result = _json(directory, 'result.json')
    keys = ['errors_by_service', 'top_error_code', 'p95_latency_ms_search',
            'billing_errors_after_last_deploy', 'busiest_minute']
    shape = result is not None and sorted(result) == sorted(keys)
    return [('shape', shape)] + _fields(result, expected, keys)


def grade_contacts(directory, expected):
    text = _read(directory, 'contacts.csv')
    rows = None
    if text is not None:
        try:
            rows = list(csv.reader(io.StringIO(text)))
        except csv.Error:
            rows = None
    if not rows:
        return [(name, False) for name in ('header', 'emails', 'names', 'phones', 'order')]
    header, body = rows[0], [row for row in rows[1:] if row]
    want = expected['rows']
    well_formed = all(len(row) == 3 for row in body)
    by_email = {row[1]: row for row in body} if well_formed else {}
    names = well_formed and len(by_email) == len(want) and all(
        by_email.get(email, [None, None, None])[0] == name for name, email, _ in want)
    phones = well_formed and len(by_email) == len(want) and all(
        by_email.get(email, [None, None, None])[2] == phone for _, email, phone in want)
    return [
        ('header', header == ['name', 'email', 'phone']),
        ('emails', well_formed and sorted(r[1] for r in body) == sorted(r[1] for r in want)
         and len(body) == len(want)),
        ('names', bool(names)),
        ('phones', bool(phones)),
        ('order', well_formed and [r[1] for r in body] == [r[1] for r in want]),
    ]


def grade_repair(directory, expected):
    """Run the hidden tests against the repaired module, one check per group,
    each in a fresh interpreter so one broken group cannot hide another."""
    checks = []
    source = _read(directory, 'inventory.py')
    for group, test in expected['tests'].items():
        passed = False
        if source is not None:
            program = source + '\n\n' + test + '\n'
            try:
                done = subprocess.run([sys.executable, '-c', program], capture_output=True,
                                      timeout=20, check=False)
                passed = done.returncode == 0 and done.stdout.strip().endswith(b'OK')
            except (OSError, subprocess.TimeoutExpired):
                passed = False
        checks.append((group, passed))
    return checks


def grade_schedule(directory, expected):
    result = _json(directory, 'schedule.json')
    talks, rooms, slots = expected['talks'], expected['rooms'], expected['slots']
    names = ['complete', 'rooms_free', 'speakers_free', 'capacity', 'order', 'availability']
    complete = (
        result is not None and sorted(result) == sorted(talks)
        and all(isinstance(v, list) and len(v) == 2 and v[0] in rooms and v[1] in slots
                for v in result.values()))
    if not complete:
        return [('complete', False)] + [(name, False) for name in names[1:]]
    cells = [tuple(v) for v in result.values()]
    by_slot = {}
    for talk, (room, slot) in result.items():
        by_slot.setdefault(slot, []).append(talks[talk]['speaker'])
    return [
        ('complete', True),
        ('rooms_free', len(set(cells)) == len(cells)),
        ('speakers_free', all(len(s) == len(set(s)) for s in by_slot.values())),
        ('capacity', all(rooms[result[t][0]] >= talks[t]['audience'] for t in talks)),
        ('order', all(slots.index(result[a][1]) < slots.index(result[b][1])
                      for a, b in expected['before'])),
        ('availability', all(result[t][1] not in expected['unavailable'].get(talks[t]['speaker'], [])
                             for t in talks)),
    ]


def grade_audit(directory, expected):
    result = _json(directory, 'result.json')
    keys = ['unpaid', 'short', 'over', 'refund_due', 'unknown_payments', 'total_net_cny_cents',
            'top_outstanding']
    shape = result is not None and sorted(result) == sorted(keys)
    return [('shape', shape)] + _fields(result, expected, keys)


def grade_incident(directory, expected):
    result = _json(directory, 'result.json')
    keys = ['requests_per_service', 'incomplete', 'server_errors_by_service', 'slowest_request',
            'p90_duration_ms', 'top_cause', 'first_error_utc']
    shape = result is not None and sorted(result) == sorted(keys)
    return [('shape', shape)] + _fields(result, expected, keys)


def grade_refactor(directory, expected):
    """Run each hidden test group against the repaired modules, copied
    together into a scratch directory, in a fresh interpreter."""
    sources = {name: _read(directory, name) for name in expected['modules']}
    checks = []
    for group, test in expected['tests'].items():
        passed = False
        if all(text is not None for text in sources.values()):
            scratch = tempfile.mkdtemp()
            try:
                for name, text in sources.items():
                    with open(os.path.join(scratch, name), 'w', encoding='utf-8') as stream:
                        stream.write(text)
                done = subprocess.run([sys.executable, '-c', test], cwd=scratch,
                                      capture_output=True, timeout=20, check=False)
                passed = done.returncode == 0 and done.stdout.strip().endswith(b'OK')
            except (OSError, subprocess.TimeoutExpired):
                passed = False
            finally:
                shutil.rmtree(scratch, ignore_errors=True)
        checks.append((group, passed))
    return checks


def grade_routing(directory, expected):
    result = _json(directory, 'route.json')
    stops, travel = expected['stops'], expected['travel']
    route = result.get('route') if result else None
    if not (isinstance(route, list) and sorted(route) == sorted(stops)
            and isinstance(result.get('finish'), int)):
        return [('complete', False), ('windows', False), ('finish_reported', False),
                ('near_optimal', False), ('optimal', False)]
    time, here, feasible = 0, 'D', True
    for stop in route:
        time = max(time + travel[here][stop], stops[stop][0])
        feasible = feasible and time <= stops[stop][1]
        time += expected['service']
        here = stop
    finish = time + travel[here]['D']
    best = expected['optimum']
    return [
        ('complete', True),
        ('windows', feasible),
        ('finish_reported', result['finish'] == finish),
        ('near_optimal', feasible and finish <= best * 1.1),
        ('optimal', feasible and finish == best),
    ]


def grade_fleet(directory, expected):
    result = _json(directory, 'routes.json')
    points, demand = expected['points'], expected['demand']
    routes = result.get('routes') if result else None
    names = ['complete', 'capacity', 'cost_reported', 'within_15pct', 'within_7pct',
             'within_3pct', 'within_1pct', 'best_known']
    visited = [c for route in routes for c in route] if isinstance(routes, list) and all(
        isinstance(r, list) for r in routes) else None
    if visited is None or sorted(visited) != sorted(demand):
        return [(name, False) for name in names]
    cost = 0.0
    for route in routes:
        path = ['D'] + route + ['D']
        cost += sum(math.dist(points[a], points[b]) for a, b in zip(path, path[1:]))
    capacity = all(sum(demand[c] for c in route) <= expected['capacity'] for route in routes)
    reported = result.get('cost')
    best = expected['best_known']

    def within(share):
        return capacity and cost <= best * (1 + share) + 1e-6

    return [
        ('complete', True),
        ('capacity', capacity),
        ('cost_reported', isinstance(reported, (int, float)) and abs(reported - cost) <= 0.01),
        ('within_15pct', within(0.15)),
        ('within_7pct', within(0.07)),
        ('within_3pct', within(0.03)),
        ('within_1pct', within(0.01)),
        ('best_known', within(0.0)),
    ]


def grade_triage(directory, expected):
    labels = expected['labels']
    text = _read(directory, 'triage.csv')
    names = ['complete', 'category_90', 'category_all', 'priority_90', 'priority_all',
             'p1_recall']
    try:
        rows = list(csv.reader(io.StringIO(text))) if text is not None else []
    except csv.Error:
        rows = []
    if not rows or [c.strip() for c in rows[0]] != ['id', 'category', 'priority']:
        return [(name, False) for name in names]
    got = {}
    for row in rows[1:]:
        if len(row) == 3:
            got.setdefault(row[0].strip(), (row[1].strip().lower(), row[2].strip().upper()))
    complete = sorted(got) == sorted(labels) and len(rows) - 1 == len(labels)
    n = len(labels)
    category = sum(got.get(t, ('', ''))[0] == c for t, (c, _) in labels.items()) / n
    priority = sum(got.get(t, ('', ''))[1] == p for t, (_, p) in labels.items()) / n
    p1 = all(got.get(t, ('', ''))[1] == 'P1' for t, (_, p) in labels.items() if p == 'P1')
    return [
        ('complete', complete),
        ('category_90', category >= 0.9),
        ('category_all', category == 1.0),
        ('priority_90', priority >= 0.9),
        ('priority_all', priority == 1.0),
        ('p1_recall', p1),
    ]


GRADERS = {
    'ledger': grade_ledger,
    'logs': grade_logs,
    'contacts': grade_contacts,
    'repair': grade_repair,
    'schedule': grade_schedule,
    'audit': grade_audit,
    'incident': grade_incident,
    'refactor': grade_refactor,
    'routing': grade_routing,
    'fleet': grade_fleet,
    'triage': grade_triage,
}
