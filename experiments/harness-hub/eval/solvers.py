"""Independent solutions computed from the input files alone, the way a
careful solver would read them. Expected values come from the generators'
internal state; agreement between the two is the check that the expected
answers are what the instructions ask for.
"""

import csv
from datetime import datetime, timedelta, timezone
from decimal import Decimal, ROUND_HALF_UP
import io
import json
import math
import re


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


def _shard_minor(text, column, decimal, grouped=True):
    if text in ('', 'NULL', 'N/A'):
        return None
    if column == 'amount_minor':
        return int(text)
    if decimal == ',':
        if not grouped and '.' in text:
            return None
        text = text.replace('.', '').replace(',', '.')
    return int(Decimal(text) * 100)


def shards(files, mistakes=lambda name: ()):
    """`mistakes` names, per shard file, quirks to mishandle the way a
    careless reader would: the tests use it to show what each one costs."""
    manifest = json.loads(files['manifest.json'])
    blocked = set(manifest['blocked_merchants'])
    headers = (['event_id', 'ts', 'merchant', 'amt', 'status'],
               ['event_id', 'ts', 'status', 'amount_minor', 'merchant'])
    seen, duplicates, hours, out = set(), 0, {}, {}
    for shard in manifest['shards']:
        wrong = set(mistakes(shard['file']))
        raw = files[shard['file']]
        if 'latin1' in wrong:
            text = raw.decode('utf-8', errors='replace')
        else:
            text = raw.decode(shard['encoding'])
        if 'bom' not in wrong:
            text = text.removeprefix('\ufeff')
        if 'crlf' in wrong:
            text = text.replace('\r\n', '\x00\n')
        sign = -1 if shard['utc_offset'][0] == '-' else 1
        hh, mm = shard['utc_offset'][1:].split(':')
        offset = timedelta(minutes=sign * (int(hh) * 60 + int(mm)))
        if 'offset' in wrong:
            offset = timedelta(0)
        local = set() if 'dup' in wrong else seen
        columns, accepted, rejected, total = None, 0, 0, 0
        for row in csv.reader(io.StringIO(text, newline=''), delimiter=shard['delimiter']):
            if row in headers:
                if columns is None or 'v2' not in wrong:
                    columns = row
                continue
            if columns is None:  # a header behind a byte order mark, read as a record
                columns = headers[0]
                rejected += 1
                continue
            if row[0].startswith('#TOTAL') and 'total' not in wrong:
                continue
            if 'qnl' in wrong and any('\n' in f for f in row):
                rejected += 2
                continue
            record = dict(zip(columns, row))
            event = record.get('event_id')
            if event in local:
                duplicates += 1
                rejected += 1
                continue
            seen.add(event)
            local.add(event)
            amount_column = 'amt' if 'amt' in record else 'amount_minor'
            amount = record.get(amount_column)
            if amount is None:
                minor = None
            elif 'null' in wrong and amount in ('', 'NULL', 'N/A'):
                minor = 0
            else:
                try:
                    minor = _shard_minor(amount, amount_column, shard['decimal'],
                                         'decimal' not in wrong)
                except (ArithmeticError, ValueError):
                    minor = None
            if minor is None or record.get('status') != 'ok' or record.get('merchant') in blocked:
                rejected += 1
                continue
            accepted += 1
            total += minor
            hour = (datetime.strptime(record['ts'], '%Y-%m-%d %H:%M:%S') - offset) \
                .strftime('%Y-%m-%dT%H')
            hours[hour] = hours.get(hour, 0) + 1
        out[shard['file']] = {'accepted': accepted, 'rejected': rejected, 'sum_minor': total}
    # A careless reader still counts each record it reads as new or as a
    # duplicate, so its distinct count follows from its own record counts.
    records = sum(s['accepted'] + s['rejected'] for s in out.values())
    distinct = len(seen)
    if any(mistakes(s['file']) for s in manifest['shards']):
        distinct = records - duplicates
    return {'shards': out, 'distinct_events': distinct, 'duplicates': duplicates,
            'peak_hour': max(sorted(hours), key=hours.get)}


def _up(value):
    return int(Decimal(value).quantize(Decimal('1'), rounding=ROUND_HALF_UP))


def payroll(files, amended=(True, True, True)):
    """The policy's rules and amendments, applied to the two CSV files.
    `amended` switches each amendment off for the tests."""
    people = list(csv.DictReader(io.StringIO(files['employees.csv'])))
    known = {p['employee_id'] for p in people}
    days, outside = {}, set()
    for r in csv.DictReader(io.StringIO(files['timesheets.csv'])):
        if r['employee_id'] in known and not '2026-09-01' <= r['date'] <= '2026-09-30':
            outside.add(r['employee_id'])
        if r['employee_id'] in known and '2026-09-01' <= r['date'] <= '2026-09-30':
            day = days.setdefault(r['employee_id'], {}).setdefault(r['date'], [])
            day.append((Decimal(r['hours']), r['shift']))
    columns = ['regular_hours', 'overtime_hours', 'base_pay', 'overtime_pay', 'night_premium',
               'weekend_premium', 'holiday_pay', 'attendance_bonus', 'seniority_bonus', 'gross',
               'pension', 'union_dues', 'taxable', 'income_tax', 'health_levy', 'meal_allowance',
               'deductions', 'net']
    rows, extra = {}, {}
    populations = {'amendment_1': [], 'amendment_2': [], 'amendment_3': []}
    for p in people:
        pid, hourly, rate = p['employee_id'], p['pay_type'] == 'hourly', Decimal(p['rate'])
        per_hour = rate if hourly else rate / 160
        mine = days.get(pid, {})
        reg = ot = otp = night = weekend = holiday = Decimal(0)
        meal = night_days = 0
        for d, items in mine.items():
            late = d >= '2026-09-16'
            total = sum(h for h, _ in items)
            over = max(total - 8, Decimal(0))
            reg, ot = reg + total - over, ot + over
            otp += over * per_hour * (2 if late and amended[0] and p['department'] == 'logistics'
                                      else Decimal('1.5'))
            night += sum(h for h, s in items if s == 'night') * rate * (
                Decimal('0.20') if late and amended[1] and p['union'] == 'yes' else Decimal('0.15'))
            night_days += any(s == 'night' for _, s in items)
            if datetime.strptime(d, '%Y-%m-%d').weekday() >= 5:
                weekend += total * rate * Decimal('0.25')
            if d in ('2026-09-07', '2026-09-25'):
                holiday += total * per_hour
            if total >= 6:
                meal += 2000 if late and amended[2] and p['location'] == 'east' else 1500
        hire = datetime.strptime(p['hire_date'], '%Y-%m-%d')
        if hourly:
            base, otp, night, weekend = _up(reg * rate), _up(otp), _up(night), _up(weekend)
        else:
            first = max(hire, datetime(2026, 9, 1))
            last = min(datetime.strptime(p['end_date'], '%Y-%m-%d') if p['end_date']
                       else datetime(2026, 9, 30), datetime(2026, 9, 30))
            base = _up(rate * ((last - first).days + 1) / 30)
            otp = _up(otp) if int(p['grade']) <= 2 else 0
            night, weekend = 2000 * night_days, 0
        years = 2026 - hire.year - ((9, 30) < (hire.month, hire.day))
        seniority = _up(Decimal(base) * min(years, 10) / 100)
        attendance = 5000 if len(mine) >= 18 else 0
        gross = base + otp + night + weekend + _up(holiday) + attendance + seniority
        pension = _up(Decimal(gross) * int(p['pension_pct']) / 100)
        dues = min(_up(Decimal(gross) * Decimal('0.012')), 3000) if p['union'] == 'yes' else 0
        taxable = max(0, gross - pension - 50000 * int(p['dependents']))
        tax = _up(Decimal('0.1') * min(taxable, 300000)
                  + Decimal('0.2') * max(0, min(taxable, 800000) - 300000)
                  + Decimal('0.3') * max(0, taxable - 800000))
        levy = _up(Decimal(gross) * {'north': Decimal('0.02'), 'south': Decimal('0.015'),
                                     'east': Decimal(0)}[p['location']])
        deductions = pension + dues + tax + levy
        values = [f'{reg:.2f}', f'{ot:.2f}', base, otp, night, weekend, _up(holiday), attendance,
                  seniority, gross, pension, dues, taxable, tax, levy, meal, deductions,
                  gross - deductions + meal]
        rows[pid] = dict(zip(columns, values))
        extra[pid] = {'pay_type': p['pay_type'], 'rate': int(p['rate']),
                      'pension_pct': int(p['pension_pct']), 'union': p['union'],
                      'dependents': int(p['dependents']), 'location': p['location'],
                      'years': years}
        if p['department'] == 'logistics':
            populations['amendment_1'].append(pid)
        if p['union'] == 'yes' and hourly:
            populations['amendment_2'].append(pid)
        if p['location'] == 'east':
            populations['amendment_3'].append(pid)
    return {'columns': columns, 'rows': rows, 'people': extra, 'populations': populations,
            'idle': sorted(p for p in rows if p not in days), 'outside': sorted(outside)}


_WORD_VALUES = dict(
    {w: n for n, w in enumerate('zero one two three four five six seven eight nine ten eleven '
                                'twelve thirteen fourteen fifteen sixteen seventeen eighteen '
                                'nineteen'.split())},
    **{w: 10 * n for n, w in enumerate('twenty thirty forty fifty sixty seventy eighty '
                                       'ninety'.split(), start=2)})


def _from_words(text):
    total = current = 0
    for word in text.replace('-', ' ').split():
        if word in _WORD_VALUES:
            current += _WORD_VALUES[word]
        elif word == 'hundred':
            current *= 100
        elif word in ('thousand', 'million'):
            total += current * (1000 if word == 'thousand' else 1_000_000)
            current = 0
    return total + current


def _contract_date(text, forms=('%Y-%m-%d', '%d %B %Y', '%B %d, %Y', '%d/%m/%Y')):
    text = text.strip()
    for form in forms:
        try:
            return datetime.strptime(text, form).date().isoformat()
        except ValueError:
            continue
    raise ValueError(text)


def _contract(text, wrong=()):
    lines = text.splitlines()
    joined = []
    for line in lines:
        if joined and re.search(r'[A-Za-z]-$', joined[-1]) and 'hyphen' not in wrong:
            joined[-1] = joined[-1][:-1] + line
        else:
            joined.append(line)
    text = '\n'.join(joined)
    flat = ' '.join(text.split())
    amendment = ''
    if 'AMENDMENT No. 1' in flat and 'amendment' not in wrong:
        flat, amendment = flat.split('AMENDMENT No. 1', 1)
    roles = {}
    for name, role in re.findall(r'\(\d\) ([^,]+), a company .*?\(the "(\w+)"\)', flat):
        roles[role] = name
    supplier_role = next(r for r in roles if r in ('Supplier', 'Provider', 'Vendor', 'Licensor'))
    customer_role = next(r for r in roles if r in ('Customer', 'Client', 'Buyer', 'Licensee'))

    def amount(phrase):
        m = re.search(r'((?:[a-z]+[ -])+?)(US Dollars|euros|pounds sterling) \((\w{3}) ([\d,]+)',
                      phrase)
        if 'figures' in wrong:
            return int(m.group(4).replace(',', '')) * 100, m.group(3)
        return _from_words(m.group(1)) * 100, m.group(3)

    def number_of(after, unit):
        m = re.search(re.escape(after) + r' ([a-z -]+?) \((\d+)\) ' + unit, flat)
        return _from_words(m.group(1))

    term = re.search(r'commences on (.+?) \(the "Effective Date"\) and continues for an initial '
                     r'term of ([a-z -]+?) \(\d+\) (months|years)', flat)
    months = _from_words(term.group(2)) * (12 if term.group(3) == 'years' else 1)
    fee, currency = amount(flat.split('an annual fee of', 1)[1])
    if amendment:
        m = re.search(r'initial term in the Term clause is replaced by ([a-z -]+?) \(\d+\) months',
                      amendment)
        if m:
            months = _from_words(m.group(1))
        if 'annual fee in the Fees clause is replaced by' in amendment:
            fee, _ = amount(amendment.split('is replaced by', 1)[1])
    signature = flat.split('For and on behalf of the ' + supplier_role, 1)[1]
    return {
        'reference': re.search(r'Agreement No\. (\S+)', flat).group(1),
        'supplier': roles[supplier_role], 'customer': roles[customer_role],
        'effective_date': _contract_date(term.group(1), *(
            [('%Y-%m-%d', '%d %B %Y', '%B %d, %Y', '%m/%d/%Y', '%d/%m/%Y')]
            if 'month_first' in wrong else [])), 'term_months': months,
        'auto_renew': 'renews automatically for' in flat, 'annual_fee_cents': fee,
        'currency': currency,
        'payment_days': number_of('undisputed invoice within', 'days'),
        'late_interest_pct': json.loads(re.search(r'interest at ([\d.]+) percent', flat).group(1)),
        'notice_days': number_of('not less than', 'days'),
        'liability_cap_cents': amount(flat.split('shall not exceed', 1)[1])[0],
        'confidentiality_years': number_of('survive for', 'years'),
        'governing_law': re.search(r'governed by the laws of (.+?)\.(?: |$)', flat).group(1),
        'signatory_supplier': re.search(r'Name: (.+?) Title:', signature).group(1),
    }


def contracts(files, mistakes=lambda name: ()):
    """`mistakes` names, per file, rules to get wrong, for the tests."""
    fields = ['reference', 'supplier', 'customer', 'effective_date', 'term_months', 'auto_renew',
              'annual_fee_cents', 'currency', 'payment_days', 'late_interest_pct', 'notice_days',
              'liability_cap_cents', 'confidentiality_years', 'governing_law',
              'signatory_supplier']
    records = {name.split('/')[1]: _contract(data.decode(), mistakes(name.split('/')[1]))
               for name, data in sorted(files.items()) if name.startswith('contracts/')}
    return {'fields': fields, 'records': records}


SOLVERS = {'ledger': ledger, 'logs': logs, 'contacts': contacts, 'audit': audit,
           'incident': incident, 'shards': shards, 'payroll': payroll, 'contracts': contracts}
