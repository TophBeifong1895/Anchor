"""The hard tier of the evaluation task set. The basic tier (tasks.py) is
solved almost always by the baseline harness, so its quality signal sits at
the ceiling; these tasks carry more exact rules, several formats and time
zones, behavior the visible tests do not show, and an exact optimum, so
that quality has room to vary.
"""

from datetime import date, datetime, timedelta, timezone
from decimal import Decimal, ROUND_HALF_UP
import json
import math
import random

from spec import COMPLETION, SEED, Task

# ---------------------------------------------------------------------------
# audit: reconcile multi-currency payments and refunds
# ---------------------------------------------------------------------------


def _cents(value):
    return int(value.quantize(Decimal('1'), rounding=ROUND_HALF_UP))


def audit():
    rng = random.Random(SEED + 10)
    customers = ['acme', 'globex', 'initech', 'umbrella', 'hooli', 'stark', 'wayne', 'tyrell',
                 'soylent', 'cyberdyne']
    usd, eur, rates = Decimal('7.1000'), Decimal('7.8000'), {}
    for day in range(1, 31):
        d = date(2026, 9, day)
        usd += Decimal(rng.randint(-150, 150)) / 10000
        eur += Decimal(rng.randint(-150, 150)) / 10000
        if d.weekday() < 5:
            rates[d.isoformat()] = {'USD': usd, 'EUR': eur}

    def rate(day, currency):
        if currency == 'CNY':
            return Decimal(1)
        return rates[max(d for d in rates if d <= day)][currency]

    def day():
        return date(2026, 9, rng.randint(2, 30)).isoformat()

    orders, payments = [], []

    def pay(order, cny_cents, currency, kind='payment'):
        d = day()
        amount = (Decimal(cny_cents) / 100 / rate(d, currency)).quantize(Decimal('0.01'))
        text = f'{amount:.2f}'
        if currency == 'CNY' and amount >= 1000 and rng.random() < 0.5:
            text = f'{amount:,.2f}'
        ref = rng.choice([order, order.lower(), f' {order}', f'{order} '])
        payments.append({'order': ref, 'amount': text, 'currency': currency, 'kind': kind,
                         'date': d})

    open_kinds = (['exact_cny'] * 18 + ['exact_foreign'] * 14 + ['unpaid'] * 6 + ['short'] * 6
                  + ['over'] * 4 + ['split_mixed'] * 8 + ['refund_part'] * 5 + ['refund_all'] * 3
                  + ['duplicate'] * 4)
    closed_kinds = ['cancelled_paid'] * 4 + ['cancelled_refunded'] * 3 + ['cancelled_none'] * 3
    kinds = open_kinds + closed_kinds
    rng.shuffle(kinds)
    for n, kind in enumerate(kinds):
        order = f'O{2001 + n}'
        cents = rng.randint(5000, 500000)
        status = 'cancelled' if kind.startswith('cancelled') else 'open'
        orders.append((order, rng.choice(customers), f'{cents / 100:.2f}', status))
        foreign = rng.choice(['USD', 'EUR'])
        if kind == 'exact_cny':
            pay(order, cents, 'CNY')
        elif kind == 'exact_foreign':
            pay(order, cents, foreign)
        elif kind == 'short':
            pay(order, cents * rng.randint(30, 80) // 100, rng.choice(['CNY', foreign]))
        elif kind == 'over':
            pay(order, cents + rng.randint(500, 5000), 'CNY')
        elif kind == 'split_mixed':
            part = rng.randint(1000, cents - 1000)
            pay(order, part, 'CNY')
            pay(order, cents - part, foreign)
        elif kind == 'refund_part':
            pay(order, cents, 'CNY')
            pay(order, cents * rng.randint(10, 50) // 100, 'CNY', 'refund')
        elif kind in ('refund_all', 'cancelled_refunded'):
            pay(order, cents, 'CNY')
            pay(order, cents, 'CNY', 'refund')
        elif kind == 'duplicate':
            pay(order, cents, 'CNY')
            pay(order, cents, 'CNY')
        elif kind == 'cancelled_paid':
            pay(order, cents, foreign)
    for n in range(3):
        pay(f'O{9001 + n}', rng.randint(5000, 50000), 'CNY')
    rng.shuffle(payments)
    for n, p in enumerate(payments):
        p['id'] = f'P{7001 + n}'

    amount = {o: int(Decimal(a) * 100) for o, _, a, _ in orders}
    status = {o: s for o, _, _, s in orders}
    customer = {o: c for o, c, _, _ in orders}
    net, seen, unknown = {}, set(), []
    for p in payments:
        order = p['order'].strip().upper()
        if order not in amount:
            unknown.append(p['id'])
            continue
        seen.add(order)
        cny = _cents(Decimal(p['amount'].replace(',', '')) * rate(p['date'], p['currency']) * 100)
        net[order] = net.get(order, 0) + (cny if p['kind'] == 'payment' else -cny)
    opened = [o for o in amount if status[o] == 'open']
    outstanding = {}
    for o in opened:
        owed = max(0, amount[o] - net.get(o, 0))
        outstanding[customer[o]] = outstanding.get(customer[o], 0) + owed
    top = sorted(outstanding.items(), key=lambda kv: (-kv[1], kv[0]))[:3]
    expected = {
        'unpaid': sorted(o for o in opened if o not in seen),
        'short': sorted(o for o in opened if o in seen and net[o] < amount[o] - 100),
        'over': sorted(o for o in opened if o in seen and net[o] > amount[o] + 100),
        'refund_due': sorted(o for o in amount if status[o] == 'cancelled' and net.get(o, 0) > 0),
        'unknown_payments': sorted(unknown),
        'total_net_cny_cents': sum(net.values()),
        'top_outstanding': [[c, v] for c, v in top],
    }
    files = {
        'orders.csv': 'order_id,customer,amount_cny,status\n' + ''.join(
            f'{o},{c},{a},{s}\n' for o, c, a, s in orders),
        'payments.jsonl': ''.join(json.dumps({k: p[k] for k in
                                              ('id', 'order', 'amount', 'currency', 'kind', 'date')})
                                  + '\n' for p in payments),
        'rates.csv': 'date,currency,rate_to_cny\n' + ''.join(
            f'{d},{c},{r[c]:.4f}\n' for d, r in sorted(rates.items()) for c in ('USD', 'EUR')),
    }
    instructions = (
        'Audit the payments of September 2026. Inputs in /in/prepare/data: orders.csv '
        '(order_id, customer, amount_cny with two decimals, status open or cancelled), '
        'payments.jsonl (one JSON object per line: id, order, amount as a decimal string that may '
        'contain thousands separators, currency CNY, USD or EUR, kind payment or refund, date) and '
        'rates.csv (date, currency, rate_to_cny; business days only). Rules: a payment\'s order '
        'reference matches order_id after trimming spaces and upper-casing. Convert each payment '
        'or refund to CNY cents on its own: amount times the rate of its date, or of the latest '
        'earlier date in rates.csv when its date has none (CNY has rate 1), times 100, rounded half '
        'up to a whole cent. An order\'s net is the sum of its converted payments minus its '
        'converted refunds. Write /workspace/result.json, a JSON object with exactly these keys: '
        '`unpaid`: open orders with no payment or refund record at all; `short`: open orders with '
        'at least one record whose net is more than 100 cents below amount_cny in cents; `over`: '
        'open orders whose net is more than 100 cents above it; `refund_due`: cancelled orders '
        'whose net is above 0; `unknown_payments`: ids of records whose order matches no order; '
        '`total_net_cny_cents`: the sum of the nets of all orders, an integer; `top_outstanding`: '
        'the three customers owed most over their open orders, where an order owes amount minus '
        'net when that is positive and 0 otherwise, as [customer, cents] pairs, largest first, '
        'ties by customer name ascending. Lists of ids are sorted ascending. ' + COMPLETION)
    return Task('audit', 1, files, instructions, ['result.json'], expected,
                {'result.json': json.dumps(expected)}, tier='exact')


# ---------------------------------------------------------------------------
# incident: requests across four log formats and time zones
# ---------------------------------------------------------------------------

def _iso(ms, offset_hours=0):
    moment = datetime.fromtimestamp(ms / 1000, timezone(timedelta(hours=offset_hours)))
    text = moment.strftime('%Y-%m-%dT%H:%M:%S.') + f'{ms % 1000:03d}'
    return text + ('Z' if offset_hours == 0 else f'+{offset_hours:02d}:00')


def _incident(seed):
    rng = random.Random(seed)
    base = int(datetime(2026, 10, 1, 8, tzinfo=timezone.utc).timestamp() * 1000)
    services = ['gateway', 'orders', 'payments', 'inventory']
    causes = ['TimeoutError', 'CardDeclined', 'LedgerLocked', 'RateLimited']
    weights = [rng.randint(1, 9) for _ in causes]
    events = {s: [] for s in services}
    starts, ends, traces = {}, {}, []
    for n in range(1, 241):
        rid, service = f'R{n:04d}', rng.choice(services)
        start = base + rng.randint(0, 2 * 3600 * 1000)
        duration = int(math.exp(rng.uniform(math.log(20), math.log(6000))))
        roll = rng.random()
        status = 503 if roll < 0.1 else 404 if roll < 0.15 else 200
        events[service].append((start, 'start', rid, None))
        starts[rid] = (service, start)
        if rng.random() < 0.08:
            events[service].append((start + rng.randint(100, 900), 'start', rid, None))
        if rng.random() < 0.04:
            continue
        end = start + duration
        events[service].append((end, 'end', rid, status))
        ends[rid] = (end, status)
        if status == 503 and rng.random() < 0.5:
            later = end + rng.randint(200, 3000)
            events[service].append((later, 'end', rid, 200))
            ends[rid] = (later, 200)
    lines = {}
    for service, items in events.items():
        out = []
        for ms, kind, rid, status in sorted(items):
            if service == 'gateway':
                record = {'time': _iso(ms), 'event': kind, 'rid': rid}
                if status is not None:
                    record['status'] = status
                out.append(json.dumps(record))
            elif service == 'inventory':
                record = {'ts': ms, 'type': 'begin' if kind == 'start' else 'finish', 'id': rid}
                if status is not None:
                    record['code'] = status
                out.append(json.dumps(record))
            else:
                offset = 8 if service == 'payments' else 0
                word = 'START' if kind == 'start' else 'END'
                out.append(f'{_iso(ms, offset)} {word} {rid}' + (f' {status}' if status else ''))
                if service == 'payments' and status == 503:
                    cause = rng.choices(causes, weights)[0]
                    traces.append(cause)
                    out += ['    at payments.charge(Charge.java:42)',
                            '    at payments.handle(Handler.java:17)',
                            f'    caused by: {cause}']
        lines[service] = out
    first_start = {}
    for service, items in events.items():
        for ms, kind, rid, _ in items:
            if kind == 'start':
                first_start[rid] = min(first_start.get(rid, ms), ms)
    durations = {rid: ends[rid][0] - first_start[rid] for rid in ends}
    errors = sorted((ms, rid) for items in events.values() for ms, kind, rid, status in items
                    if kind == 'end' and status >= 500)
    ordered = sorted(durations.values())
    by_cause = {c: traces.count(c) for c in set(traces)}
    expected = {
        'requests_per_service': {s: len({r for _, _, r, _ in events[s]}) for s in services},
        'incomplete': sorted(r for r in starts if r not in ends),
        'server_errors_by_service': {
            s: sum(1 for r, (svc, _) in starts.items() if svc == s and r in ends
                   and ends[r][1] >= 500) for s in services},
        'slowest_request': max(durations, key=durations.get),
        'p90_duration_ms': ordered[math.ceil(0.9 * len(ordered)) - 1],
        'top_cause': max(by_cause, key=by_cause.get),
        'first_error_utc': _iso(errors[0][0]),
    }
    unique = (sorted(durations.values())[-1] != sorted(durations.values())[-2]
              and sorted(by_cause.values())[-1] != sorted(by_cause.values())[-2]
              and errors[0][0] != errors[1][0])
    files = {
        'gateway.jsonl': '\n'.join(lines['gateway']) + '\n',
        'orders.log': '\n'.join(lines['orders']) + '\n',
        'payments.log': '\n'.join(lines['payments']) + '\n',
        'inventory.jsonl': '\n'.join(lines['inventory']) + '\n',
    }
    return files, expected, unique


def incident():
    seed = SEED + 11
    files, expected, unique = _incident(seed)
    while not unique:
        seed += 1
        files, expected, unique = _incident(seed)
    instructions = (
        'Investigate the request logs of four services in /in/prepare/data. gateway.jsonl: JSON '
        'lines {time, event start|end, rid, status on end}. inventory.jsonl: JSON lines {ts as Unix '
        'epoch milliseconds, type begin|finish, id, code on finish}. orders.log and payments.log: '
        'text lines `<time> START <rid>` or `<time> END <rid> <status>`; orders.log times are UTC '
        '(Z), payments.log times carry a +08:00 offset, and a payments END line may be followed by '
        'indented stack trace lines, one of which reads `caused by: <Exception>`. A request may '
        'have several start events (retries) and several end events; its duration is its last end '
        'minus its first start, in milliseconds, and its final status is the status of its last '
        'end. Write /workspace/result.json, a JSON object with exactly these keys: '
        '`requests_per_service`: each service (gateway, orders, payments, inventory) mapped to its '
        'number of distinct request ids; `incomplete`: request ids with no end event, sorted; '
        '`server_errors_by_service`: each service mapped to the number of its requests whose final '
        'status is 500 or more; `slowest_request`: the id of the request with the longest duration; '
        '`p90_duration_ms`: the 90th percentile of the durations of all requests that have an end, '
        'by nearest rank (sort ascending, take the value at 1-based position ceil(0.9 * n)), an '
        'integer; `top_cause`: the exception named most often in caused-by lines; '
        '`first_error_utc`: the time of the earliest end event with status 500 or more, in UTC as '
        '`YYYY-MM-DDTHH:MM:SS.mmmZ`. ' + COMPLETION)
    return Task('incident', 1, files, instructions, ['result.json'], expected,
                {'result.json': json.dumps(expected)}, tier='exact')


# ---------------------------------------------------------------------------
# refactor: repair three modules against their docstrings
# ---------------------------------------------------------------------------

MONEY = '''"""Money helpers. Amounts are integer cents."""

from decimal import Decimal


def to_cents(text):
    """Parse a decimal amount such as '1,234.5', '-0.07' or ' 12 ' into
    integer cents. Thousands separators and surrounding spaces are ignored.
    Half a cent rounds away from zero: '0.005' is 1, '-0.005' is -1."""
    value = Decimal(text.strip().replace(',', ''))
    return int(round(value * 100))


def split(cents, parts):
    """Split `cents` into `parts` integer shares (parts >= 1) that sum to
    `cents` and differ by at most one cent, the shares of largest magnitude
    first. A negative amount splits into negative shares the same way:
    split(-10, 3) is [-4, -3, -3]."""
    share = cents // parts
    rest = cents - share * parts
    return [share + 1] * rest + [share] * (parts - rest)
'''

DATES = '''"""Date helpers over ISO date strings."""

from datetime import date, timedelta


def days_between(start, end):
    """The number of days from start to end counting both, so a single day
    is 1; 0 when end is before start."""
    a, b = date.fromisoformat(start), date.fromisoformat(end)
    return max(0, (b - a).days)


def month_end(day):
    """The last day of the month of `day`."""
    d = date.fromisoformat(day)
    following = d.replace(day=28) + timedelta(days=4)
    return (following - timedelta(days=following.day)).isoformat()
'''

INVOICES = '''"""Invoices: parse, total and rank."""

from money import to_cents


def parse(text):
    """Parse CSV text with the header `id,customer,amount,date` into a list
    of dicts with keys id, customer, amount (integer cents) and date. Fields
    may be quoted, and a quoted amount may contain commas: "1,200.00"."""
    rows = []
    for line in text.strip().splitlines()[1:]:
        id_, customer, amount, day = line.split(',')
        rows.append({'id': id_, 'customer': customer, 'amount': to_cents(amount), 'date': day})
    return rows


def totals(rows, by='customer', into={}):
    """The sum of amount per value of field `by`. Adds into `into` when it
    is given; otherwise returns a new dict."""
    for row in rows:
        into[row[by]] = into.get(row[by], 0) + row['amount']
    return into


def ranking(rows):
    """Customers as (customer, cents) by total amount, largest first, ties
    by customer name ascending."""
    return sorted(totals(rows).items(), key=lambda item: -item[1])
'''

VISIBLE_TESTS = '''from dates import days_between
from invoices import parse

assert days_between('2026-09-01', '2026-09-01') == 1
assert days_between('2026-09-01', '2026-09-30') == 30
rows = parse('id,customer,amount,date\\nI1,acme,"1,200.00",2026-09-01\\n')
assert rows == [{'id': 'I1', 'customer': 'acme', 'amount': 120000, 'date': '2026-09-01'}]
print('OK')
'''

REFACTOR_TESTS = {
    'visible': VISIBLE_TESTS,
    'to_cents': '''from money import to_cents
assert to_cents('0.005') == 1
assert to_cents('-0.005') == -1
assert to_cents('2.675') == 268
assert to_cents(' 1,234.5 ') == 123450
print('OK')
''',
    'split': '''from money import split
assert split(10, 3) == [4, 3, 3]
assert split(-10, 3) == [-4, -3, -3]
assert split(-9, 3) == [-3, -3, -3]
assert split(7, 1) == [7]
print('OK')
''',
    'days_between': '''from dates import days_between
assert days_between('2026-02-27', '2026-03-02') == 4
assert days_between('2026-09-02', '2026-09-01') == 0
print('OK')
''',
    'month_end': '''from dates import month_end
assert month_end('2028-02-10') == '2028-02-29'
assert month_end('2026-12-31') == '2026-12-31'
print('OK')
''',
    'parse': '''from invoices import parse
text = 'id,customer,amount,date\\nI1,"acme, inc",12.5,2026-09-01\\nI2,hooli,"-1,000.25",2026-09-02\\n'
assert parse(text) == [
    {'id': 'I1', 'customer': 'acme, inc', 'amount': 1250, 'date': '2026-09-01'},
    {'id': 'I2', 'customer': 'hooli', 'amount': -100025, 'date': '2026-09-02'},
]
print('OK')
''',
    'totals': '''from invoices import totals
rows = [{'customer': 'a', 'amount': 5}, {'customer': 'b', 'amount': 1}]
assert totals(rows) == {'a': 5, 'b': 1}
assert totals(rows) == {'a': 5, 'b': 1}
into = {'a': 1}
assert totals(rows, into=into) == {'a': 6, 'b': 1} and into == {'a': 6, 'b': 1}
assert totals([{'date': 'x', 'amount': 2}], by='date') == {'x': 2}
print('OK')
''',
    'ranking': '''from invoices import ranking
rows = [{'customer': 'zeta', 'amount': 5}, {'customer': 'beta', 'amount': 5},
        {'customer': 'alpha', 'amount': 2}, {'customer': 'beta', 'amount': 0}]
assert ranking(rows) == [('beta', 5), ('zeta', 5), ('alpha', 2)]
assert ranking(rows) == [('beta', 5), ('zeta', 5), ('alpha', 2)]
print('OK')
''',
}

FIXED_MODULES = {
    'money.py': (MONEY
                 .replace('from decimal import Decimal', 'from decimal import ROUND_HALF_UP, Decimal')
                 .replace('return int(round(value * 100))',
                          "return int((value * 100).quantize(Decimal('1'), rounding=ROUND_HALF_UP))")
                 .replace('''    share = cents // parts
    rest = cents - share * parts
    return [share + 1] * rest + [share] * (parts - rest)''',
                          '''    sign = -1 if cents < 0 else 1
    share, rest = divmod(abs(cents), parts)
    return [sign * (share + 1)] * rest + [sign * share] * (parts - rest)''')),
    'dates.py': DATES.replace('return max(0, (b - a).days)', 'return max(0, (b - a).days + 1)'),
    'invoices.py': (INVOICES
                    .replace('from money import to_cents', 'import csv\nimport io\n\nfrom money import to_cents')
                    .replace('''    for line in text.strip().splitlines()[1:]:
        id_, customer, amount, day = line.split(',')''',
                             '''    for id_, customer, amount, day in list(csv.reader(io.StringIO(text.strip())))[1:]:''')
                    .replace("def totals(rows, by='customer', into={}):",
                             "def totals(rows, by='customer', into=None):")
                    .replace('''    for row in rows:
        into[row[by]]''', '''    into = {} if into is None else into
    for row in rows:
        into[row[by]]''')
                    .replace('key=lambda item: -item[1]', 'key=lambda item: (-item[1], item[0])')),
}


def refactor():
    modules = {'money.py': MONEY, 'dates.py': DATES, 'invoices.py': INVOICES}
    instructions = (
        'The Python modules money.py, dates.py and invoices.py in /in/prepare/data do not all do '
        'what their docstrings say. test_billing.py there holds some tests (copy all four files '
        'to one directory and run python3.12 test_billing.py there); the tests are not complete. '
        'Fix the modules so that every function meets its docstring. Do not change docstrings, '
        'function names or signatures except where a signature itself contradicts its docstring. '
        'Write the three fixed modules to /workspace/money.py, /workspace/dates.py and '
        '/workspace/invoices.py. ' + COMPLETION)
    return Task('refactor', 1, dict(modules, **{'test_billing.py': VISIBLE_TESTS}), instructions,
                list(modules), {'modules': list(modules), 'tests': REFACTOR_TESTS},
                dict(FIXED_MODULES), tier='exact')


# ---------------------------------------------------------------------------
# routing: an exact optimum under time windows
# ---------------------------------------------------------------------------

def _finish(order, stops, travel, service):
    time, here = 0, 'D'
    for stop in order:
        time = max(time + travel[here][stop], stops[stop][0])
        if time > stops[stop][1]:
            return None
        time += service
        here = stop
    return time + travel[here]['D']


def _optimum(stops, travel, service):
    """Held-Karp over (visited set, last stop). Waiting is allowed and
    windows only bound arrival, so the earliest time at a state dominates."""
    names = sorted(stops)
    best = {}
    for i, s in enumerate(names):
        t = max(travel['D'][s], stops[s][0])
        if t <= stops[s][1]:
            best[(1 << i, i)] = (t + service, [s])
    for mask in range(1, 1 << len(names)):
        for last in range(len(names)):
            if (mask, last) not in best:
                continue
            time, path = best[(mask, last)]
            for j, s in enumerate(names):
                if mask & (1 << j):
                    continue
                t = max(time + travel[names[last]][s], stops[s][0])
                if t > stops[s][1]:
                    continue
                key = (mask | (1 << j), j)
                if key not in best or t + service < best[key][0]:
                    best[key] = (t + service, path + [s])
    full = (1 << len(names)) - 1
    ends = [(time + travel[names[last]]['D'], path) for (mask, last), (time, path) in best.items()
            if mask == full]
    return min(ends)


def routing():
    seed = SEED + 13
    while True:
        rng = random.Random(seed)
        names = [f'S{n}' for n in range(1, 11)]
        points = {'D': (10, 10)}
        points.update({s: (rng.randint(0, 20), rng.randint(0, 20)) for s in names})
        travel = {a: {} for a in points}
        for a in points:
            for b in points:
                if a < b:
                    minutes = 2 * (abs(points[a][0] - points[b][0])
                                   + abs(points[a][1] - points[b][1])) + rng.randint(1, 4)
                    travel[a][b] = travel[b][a] = minutes
        service, tour = 5, names[:]
        rng.shuffle(tour)
        stops, time, here = {}, 0, 'D'
        for s in tour:
            time += travel[here][s]
            stops[s] = [max(0, time - rng.randint(0, 25)), time + rng.randint(15, 45)]
            time += service
            here = s
        best, path = _optimum(stops, travel, service)
        greedy, here, left, t = [], 'D', set(names), 0
        while left:
            s = min(left, key=lambda x: (max(t + travel[here][x], stops[x][0]), x))
            t = max(t + travel[here][s], stops[s][0]) + service
            greedy.append(s)
            left.discard(s)
            here = s
        naive = _finish(greedy, stops, travel, service)
        # Keep instances where the obvious greedy route is not already optimal.
        if naive is None or naive > best:
            break
        seed += 1
    instructions = (
        'Plan a delivery route. /in/prepare/data/routing.json gives `travel` (minutes between any '
        'two of the depot D and the stops), `stops` (stop to its time window [open, close] in '
        'minutes after the start) and `service` (minutes spent at each stop). The vehicle leaves D '
        'at minute 0, visits every stop exactly once and returns to D. Arriving before a stop '
        'opens, it waits until open; it must arrive no later than close; it then spends `service` '
        'minutes there before driving on. Find a route that returns to D as early as possible; the '
        'earliest possible return is what counts. Write /workspace/route.json as {"route": [stops '
        'in visiting order], "finish": the minute the vehicle is back at D, an integer}. '
        + COMPLETION)
    expected = {'stops': stops, 'travel': travel, 'service': service, 'optimum': best}
    return Task('routing', 1,
                {'routing.json': json.dumps({'travel': travel, 'stops': stops, 'service': service},
                                            indent=1)},
                instructions, ['route.json'], expected,
                {'route.json': json.dumps({'route': path, 'finish': best})}, tier='exact')


def hard_tasks():
    return [audit(), incident(), refactor(), routing()]
