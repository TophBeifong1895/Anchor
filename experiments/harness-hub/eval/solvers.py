"""Independent solutions computed from the input files alone, the way a
careful solver would read them. Expected values come from the generators'
internal state; agreement between the two is the check that the expected
answers are what the instructions ask for.
"""

import csv
from datetime import datetime, timezone
from decimal import Decimal, ROUND_HALF_UP
import io
import json
import math


def ledger(files):
    orders = {r['order_id']: int(r['amount_cents'])
              for r in csv.DictReader(io.StringIO(files['orders.csv']))}
    payments = list(csv.DictReader(io.StringIO(files['payments.csv'])))
    paid = {}
    for p in payments:
        paid[p['order_id']] = paid.get(p['order_id'], 0) + int(p['amount_cents'])
    return {
        'unpaid': sorted(o for o in orders if o not in paid),
        'short': sorted(o for o in orders if 0 < paid.get(o, 0) < orders[o]),
        'over': sorted(o for o in orders if paid.get(o, 0) > orders[o]),
        'unknown_payments': sorted(p['payment_id'] for p in payments if p['order_id'] not in orders),
        'total_received_cents': sum(int(p['amount_cents']) for p in payments
                                    if p['order_id'] in orders),
    }


def logs(files):
    lines = [line.split() for name, text in files.items() if name.startswith('logs/')
             for line in text.splitlines() if line.strip()]
    errors = [l for l in lines if l[1] == 'ERROR']
    codes, minutes = {}, {}
    for l in errors:
        codes[l[4]] = codes.get(l[4], 0) + 1
    for l in lines:
        minutes[l[0][:16]] = minutes.get(l[0][:16], 0) + 1
    search = sorted(int(l[3]) for l in lines if l[2] == 'search')
    deploys = [line.split() for line in files['deploys.txt'].splitlines()]
    last = max(d[0] for d in deploys if d[1] == 'billing')
    return {
        'errors_by_service': {s: sum(l[2] == s for l in errors)
                              for s in ('api', 'auth', 'billing', 'search')},
        'top_error_code': max(codes, key=codes.get),
        'p95_latency_ms_search': search[math.ceil(0.95 * len(search)) - 1],
        'billing_errors_after_last_deploy': sum(1 for l in errors
                                                if l[2] == 'billing' and l[0] > last),
        'busiest_minute': max(minutes, key=minutes.get),
    }


def contacts(files):
    kept = {}
    for line in files['contacts.txt'].splitlines():
        if not line.strip():
            continue
        name, email, phone = (part.strip() for part in line.split('|'))
        email = email.lower()
        if email in kept:
            continue
        name = ' '.join(w[0].upper() + w[1:].lower() for w in name.split())
        digits = ''.join(ch for ch in phone if ch.isdigit())
        if len(digits) == 13 and digits.startswith('86'):
            digits = digits[2:]
        phone = (f'+86 {digits[:3]} {digits[3:7]} {digits[7:]}'
                 if len(digits) == 11 and digits.startswith('1') else '')
        kept[email] = [name, email, phone]
    return {'rows': [kept[e] for e in sorted(kept)]}


def audit(files):
    orders = list(csv.DictReader(io.StringIO(files['orders.csv'])))
    rates = {}
    for r in csv.DictReader(io.StringIO(files['rates.csv'])):
        rates.setdefault(r['date'], {})[r['currency']] = Decimal(r['rate_to_cny'])
    amount = {o['order_id']: int(Decimal(o['amount_cny']) * 100) for o in orders}
    net, seen, unknown = {}, set(), []
    for line in files['payments.jsonl'].splitlines():
        p = json.loads(line)
        order = p['order'].strip().upper()
        if order not in amount:
            unknown.append(p['id'])
            continue
        seen.add(order)
        rate = Decimal(1) if p['currency'] == 'CNY' else \
            rates[max(d for d in rates if d <= p['date'])][p['currency']]
        cny = int((Decimal(p['amount'].replace(',', '')) * rate * 100)
                  .quantize(Decimal('1'), rounding=ROUND_HALF_UP))
        net[order] = net.get(order, 0) + (cny if p['kind'] == 'payment' else -cny)
    opened = [o['order_id'] for o in orders if o['status'] == 'open']
    owed = {}
    for o in orders:
        if o['status'] == 'open':
            owed[o['customer']] = owed.get(o['customer'], 0) + max(
                0, amount[o['order_id']] - net.get(o['order_id'], 0))
    return {
        'unpaid': sorted(o for o in opened if o not in seen),
        'short': sorted(o for o in opened if o in seen and net[o] < amount[o] - 100),
        'over': sorted(o for o in opened if o in seen and net[o] > amount[o] + 100),
        'refund_due': sorted(o['order_id'] for o in orders
                             if o['status'] == 'cancelled' and net.get(o['order_id'], 0) > 0),
        'unknown_payments': sorted(unknown),
        'total_net_cny_cents': sum(net.values()),
        'top_outstanding': [[c, v] for c, v in sorted(owed.items(), key=lambda kv: (-kv[1], kv[0]))[:3]],
    }


def _ms(text):
    return int(datetime.fromisoformat(text.replace('Z', '+00:00')).timestamp() * 1000)


def incident(files):
    events = []  # (service, ms, kind, rid, status)
    for line in files['gateway.jsonl'].splitlines():
        r = json.loads(line)
        events.append(('gateway', _ms(r['time']), r['event'], r['rid'], r.get('status')))
    for line in files['inventory.jsonl'].splitlines():
        r = json.loads(line)
        events.append(('inventory', r['ts'], 'start' if r['type'] == 'begin' else 'end', r['id'],
                       r.get('code')))
    causes = []
    for service, name in (('orders', 'orders.log'), ('payments', 'payments.log')):
        for line in files[name].splitlines():
            if line.startswith(' '):
                if 'caused by:' in line:
                    causes.append(line.split('caused by:')[1].strip())
                continue
            parts = line.split()
            status = int(parts[3]) if len(parts) > 3 else None
            events.append((service, _ms(parts[0]), parts[1].lower(), parts[2], status))
    service_of, first, last = {}, {}, {}
    for service, ms, kind, rid, status in events:
        service_of[rid] = service
        if kind == 'start':
            first[rid] = min(first.get(rid, ms), ms)
        elif rid not in last or ms > last[rid][0]:
            last[rid] = (ms, status)
    services = ['gateway', 'orders', 'payments', 'inventory']
    durations = {rid: last[rid][0] - first[rid] for rid in last}
    ordered = sorted(durations.values())
    counts = {c: causes.count(c) for c in set(causes)}
    error = min(ms for _, ms, kind, _, status in events if kind == 'end' and status >= 500)
    moment = datetime.fromtimestamp(error / 1000, timezone.utc)
    return {
        'requests_per_service': {s: sum(1 for r in service_of if service_of[r] == s)
                                 for s in services},
        'incomplete': sorted(r for r in first if r not in last),
        'server_errors_by_service': {s: sum(1 for r, (_, st) in last.items()
                                            if service_of[r] == s and st >= 500) for s in services},
        'slowest_request': max(durations, key=durations.get),
        'p90_duration_ms': ordered[math.ceil(0.9 * len(ordered)) - 1],
        'top_cause': max(counts, key=counts.get),
        'first_error_utc': moment.strftime('%Y-%m-%dT%H:%M:%S.') + f'{error % 1000:03d}Z',
    }


SOLVERS = {'ledger': ledger, 'logs': logs, 'contacts': contacts, 'audit': audit,
           'incident': incident}
