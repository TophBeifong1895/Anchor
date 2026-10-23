"""The harder tier. The exact and hard tiers are solved almost always by a
capable baseline that writes one program, so their scores sit at the
ceiling. These tasks are as precisely specified, but large: what decides
them is reading whole inputs, special cases buried deep in data, rules at the
end of long documents, root causes away from their symptoms. Each check
rests on one rule or quirk, so the score moves with each one a harness helps
the agent catch.
"""

from datetime import date, timedelta
from fractions import Fraction
import json
import math
import random

import bugtrail as bugtrail_package
import generators
from spec import COMPLETION, SEED, Task

# ---------------------------------------------------------------------------
# shards: 24 CSV exports, each with its own quirk or none
# ---------------------------------------------------------------------------


def shards():
    seed = SEED + 30
    files, truth = generators.shards(seed)
    expected = {'shards': truth['shards'], 'distinct_events': truth['distinct_events'],
                'duplicates': truth['duplicates'], 'peak_hour': truth['peak_hour']}
    instructions = (
        'Summarize a payment-event export split into 24 CSV shards in /in/prepare/data. '
        'manifest.json lists the shards in file order under `shards`, each with `file`, '
        '`encoding` (the text encoding of the file), `delimiter` (the field separator), `decimal` '
        '(the decimal mark of its amounts) and `utc_offset` (the offset of its local timestamps '
        'from UTC, as +HH:MM or -HH:MM), and lists `blocked_merchants`. Format: fields follow CSV '
        'quoting, so a field in double quotes may hold the delimiter, doubled quotes or line '
        'breaks; lines end in LF or CRLF, possibly mixed within one file; a file may begin with a '
        'UTF-8 byte order mark, which is not part of the data. A shard begins with a header line, '
        'either v1 `event_id,ts,merchant,amt,status` or v2 `event_id,ts,status,amount_minor,'
        'merchant` (fields separated by the shard\'s delimiter); a header line may appear again '
        'later in a shard, and the records after it follow its columns. `amt` is an amount in '
        'major units with two decimals written with the shard\'s decimal mark; where the decimal '
        'mark is `,`, a `.` groups thousands (1.234,56 is 123456 minor units). `amount_minor` is '
        'an integer amount in minor units (1/100). `ts` is local time `YYYY-MM-DD HH:MM:SS` at '
        'the shard\'s utc_offset. A line whose first field starts with `#TOTAL` is a summary, not '
        'a record; header lines are not records; every other CSV row is one record. Rules, taking '
        'records in file order (shards in manifest order, then rows in order): a record whose '
        'event_id an earlier record already had is a duplicate and is rejected; a record is also '
        'rejected when its amount is empty, `NULL` or `N/A`, when its status is not `ok`, or when '
        'its merchant is exactly one of blocked_merchants; every other record is accepted. Write '
        '/workspace/summary.json, a JSON object with exactly these keys: `shards`: an object '
        'mapping each of the 24 file names to {"accepted": its accepted records, "rejected": its '
        'rejected records, "sum_minor": the sum of its accepted amounts in minor units}, all '
        'integers, where accepted plus rejected is the number of its records; `distinct_events`: '
        'the number of distinct event_id values over all records of all shards; `duplicates`: '
        'the number of duplicate records; `peak_hour`: the UTC hour holding the most accepted '
        'records, as `YYYY-MM-DDTHH`. ' + COMPLETION)
    return Task('shards', 1, files, instructions, ['summary.json'], expected,
                {'summary.json': json.dumps(expected)}, generator=('shards', seed))


# ---------------------------------------------------------------------------
# payroll: 28 rules and 3 amendments over a long policy
# ---------------------------------------------------------------------------

PAYROLL_COLUMNS = ['regular_hours', 'overtime_hours', 'base_pay', 'overtime_pay', 'night_premium',
                   'weekend_premium', 'holiday_pay', 'attendance_bonus', 'seniority_bonus', 'gross',
                   'pension', 'union_dues', 'taxable', 'income_tax', 'health_levy',
                   'meal_allowance', 'deductions', 'net']
PERIOD = (date(2026, 9, 1), date(2026, 9, 30))
AMENDED_FROM = date(2026, 9, 16)
HOLIDAYS = (date(2026, 9, 7), date(2026, 9, 25))

PAYROLL_RULES = [
    ('Pay period', 'The pay period is 1 September 2026 to 30 September 2026 inclusive; timesheet '
     'rows dated outside it are ignored.'),
    ('Unknown employees', 'A timesheet row whose employee_id does not appear in employees.csv is '
     'ignored.'),
    ('Hours of a day', "An employee's hours on a day are the sum of the hours of all of their "
     'timesheet rows dated that day.'),
    ('Regular and overtime hours', "Of an employee's hours on a day, the first 8 are regular "
     'hours and any hours beyond 8 are overtime hours.'),
    ('Hour columns', "regular_hours and overtime_hours are the employee's regular and overtime "
     'hours summed over the pay period, written with exactly two decimals.'),
    ('Hourly rate', 'The hourly rate of an hourly employee is their rate; the hourly rate of a '
     'salaried employee is their rate (a monthly salary in cents) divided by 160.'),
    ('Base pay, hourly', 'For an hourly employee, base_pay is regular_hours times rate.'),
    ('Base pay, salaried', 'For a salaried employee, base_pay is rate times the number of '
     'employed days divided by 30, where the employed days run from the later of hire_date and '
     '1 September 2026 to the earlier of end_date (when end_date is given) and 30 September '
     '2026, both inclusive.'),
    ('Overtime, hourly', 'For an hourly employee, overtime_pay is the sum over days of the '
     "day's overtime hours times rate times 1.5."),
    ('Overtime, salaried', 'For a salaried employee of grade 1 or 2, overtime_pay is the sum '
     "over days of the day's overtime hours times the hourly rate of Rule 6 times 1.5; for "
     'other salaried employees overtime_pay is 0.'),
    ('Night premium, hourly', 'For an hourly employee, night_premium is the sum over timesheet '
     'rows with shift night of the hours of the row times rate times 0.15.'),
    ('Night premium, salaried', 'For a salaried employee, night_premium is 2000 cents for every '
     'day with at least one timesheet row with shift night.'),
    ('Weekend premium', 'For an hourly employee, weekend_premium is the sum of the hours worked '
     'on Saturdays and Sundays times rate times 0.25; for a salaried employee it is 0.'),
    ('Company holidays', 'The company holidays in the pay period are 7 September 2026 and '
     '25 September 2026.'),
    ('Holiday pay', 'holiday_pay is the hours worked on company holidays times the hourly rate '
     'of Rule 6, for hourly and salaried employees alike.'),
    ('Attendance bonus', 'attendance_bonus is 5000 cents when the employee has hours on at '
     'least 18 distinct days of the pay period, and 0 otherwise.'),
    ('Years of service', 'Years of service are the number of full years from hire_date to '
     '30 September 2026.'),
    ('Seniority bonus', 'seniority_bonus is base_pay times the years of service, counting at '
     'most 10 years, divided by 100.'),
    ('Gross pay', 'gross is base_pay plus overtime_pay plus night_premium plus weekend_premium '
     'plus holiday_pay plus attendance_bonus plus seniority_bonus.'),
    ('Pension', 'pension is gross times pension_pct divided by 100.'),
    ('Union dues', 'union_dues is 1.2 percent of gross, at most 3000 cents, for an employee '
     'whose union is yes, and 0 for every other employee.'),
    ('Taxable pay', 'taxable is gross minus pension minus 50000 cents for each dependent, or 0 '
     'when that is negative.'),
    ('Income tax', 'income_tax is 10 percent of the part of taxable up to 300000 cents, plus 20 '
     'percent of the part from 300000 to 800000 cents, plus 30 percent of the part above '
     '800000 cents.'),
    ('Health levy', 'health_levy is 2 percent of gross at location north, 1.5 percent of gross '
     'at location south, and 0 at location east.'),
    ('Meal allowance', 'meal_allowance is 1500 cents for every day on which the employee has at '
     'least 6 hours.'),
    ('Deductions', 'deductions is pension plus union_dues plus income_tax plus health_levy.'),
    ('Net pay', 'net is gross minus deductions plus meal_allowance.'),
    ('Rounding', 'Every money column is a whole number of cents: compute it exactly from the '
     'values its rule names and round half up to a whole cent once, at the end; a column that '
     'its rule computes from other columns uses their rounded values.'),
]

PAYROLL_AMENDMENTS = [
    'For overtime hours worked on or after 16 September 2026 by employees of the logistics '
    'department, the multiplier of Rules 9 and 10 is 2.0 instead of 1.5.',
    'For hours of timesheet rows with shift night dated on or after 16 September 2026 of hourly '
    'employees whose union is yes, the factor of Rule 11 is 0.20 instead of 0.15.',
    'For days on or after 16 September 2026, the meal allowance of Rule 25 is 2000 cents for '
    'every qualifying day of employees at location east.',
]

_NOTES = [
    'Payroll staff record every question about this rule in the payroll queue before the '
    'monthly cut-off, so that the answer reaches every site at the same time.',
    'The rule is written for the payroll program; a site manager who needs it explained should '
    'ask the payroll office rather than work it out from a payslip.',
    'This wording replaces the version in the previous handbook, which described the same '
    'calculation at more length and with examples that no longer apply.',
    'Where the rule names a column, it means the column of the payroll output, not a column of '
    'the input files, unless it says otherwise.',
    'Corrections to timesheets reach payroll as new rows; payroll never edits a submitted row, '
    'and a correction is processed like any other row.',
    'Questions about how this rule interacts with a collective agreement go to the works '
    'council liaison, who answers them in writing.',
    'Payslips show this component on its own line, with the label the payroll program gives '
    'it, so employees can compare it with their own records.',
    'Auditors check this component on a sample of employees every quarter and report any '
    'difference to the finance director.',
    'Nothing in this note changes the rule above; notes explain why the rule exists and who '
    'handles questions about it.',
    'Historical background: the component was introduced when the sites merged their payroll '
    'systems, and its name has stayed the same since.',
    'Sites may print this rule on their notice boards; the payroll office keeps the master '
    'copy and announces every change by email.',
    'Employees who believe this component was computed wrongly may raise a query within two '
    'months of the payslip date.',
]


def _wrap(text, width=78):
    lines, line = [], ''
    for word in text.split():
        if line and len(line) + 1 + len(word) > width:
            lines.append(line)
            line = word
        else:
            line = f'{line} {word}' if line else word
    return lines + [line]


def _policy(rng):
    out = ['# Payroll policy, September 2026', '',
           'This policy defines every column of the monthly payroll output. The numbered rules '
           'are the whole of the calculation; the notes under each rule explain it and change '
           'nothing. Amendments at the end of this document override the rules they name, for '
           'the work and the employees they name.', '',
           '## Inputs', '']
    out += _wrap('employees.csv has one row per employee with the columns employee_id, name, '
                  'department, pay_type (hourly or salaried), rate (cents per hour for hourly '
                  'employees, cents per month for salaried employees), grade, location, union '
                  '(yes or no), dependents, pension_pct, hire_date and end_date (empty when the '
                  'employee has not left).')
    out += [''] + _wrap('timesheets.csv has one row per shift worked, with the columns '
                        'employee_id, date, hours (a decimal number) and shift (day or night). An '
                        'employee may have several rows on one day.') + ['', '## Rules', '']
    for n, (title, sentence) in enumerate(PAYROLL_RULES, start=1):
        out += [f'### Rule {n}. {title}', ''] + _wrap(sentence) + ['', 'Notes:', '']
        for _ in range(3):
            out += _wrap(' '.join(rng.sample(_NOTES, rng.choice([2, 3])))) + ['']
    out += ['## Amendments', '']
    for n, sentence in enumerate(PAYROLL_AMENDMENTS, start=1):
        out += [f'### Amendment {n} (effective 16 September 2026)', ''] + _wrap(sentence) + ['']
    return '\n'.join(out) + '\n'


def _half_up(value):
    return math.floor(Fraction(value) + Fraction(1, 2))


def _payroll_instance(rng):
    first = ['ana', 'ben', 'chen', 'dara', 'emil', 'fay', 'gus', 'hana', 'ivan', 'jun', 'kofi',
             'lena', 'mateo', 'nadia', 'omar', 'priya', 'quinn', 'rosa', 'sven', 'tara']
    last = ['smith', 'wang', 'garcia', 'novak', 'okafor', 'silva', 'tanaka', 'muller', 'rossi',
            'kumar', 'li', 'jensen', 'haddad', 'costa', 'berg']
    departments = ['assembly', 'logistics', 'maintenance', 'office', 'quality', 'sales']
    days = [PERIOD[0] + timedelta(d) for d in range(30)]
    people, sheets = [], []
    for n in range(250):
        hourly = rng.random() < 0.7
        hire = date(2008, 1, 1) + timedelta(rng.randrange((date(2026, 8, 1) - date(2008, 1, 1)).days))
        end = None
        if n % 41 == 7:
            hire = date(2026, 9, rng.randint(2, 20))
        if n % 47 == 11:
            end = date(2026, 9, rng.randint(8, 28))
        person = {
            'employee_id': f'P{1001 + n}', 'name': f'{rng.choice(first)} {rng.choice(last)}',
            'department': rng.choice(departments), 'pay_type': 'hourly' if hourly else 'salaried',
            'rate': rng.randint(1800, 6000) if hourly else rng.randint(6000, 24000) * 100,
            'grade': rng.randint(1, 5), 'location': rng.choice(['north', 'south', 'east']),
            'union': 'yes' if rng.random() < (0.45 if hourly else 0.1) else 'no',
            'dependents': rng.choice([0, 0, 1, 1, 2, 3, 4]), 'pension_pct': rng.choice([0, 3, 5]),
            'hire_date': hire, 'end_date': end}
        people.append(person)
        open_days = [d for d in days if hire <= d and (end is None or d <= end)]
        worked = sorted(rng.sample(open_days, min(len(open_days), rng.randint(6, 22))))
        if n % 53 == 29:
            worked = []  # on leave all month
        night_worker = rng.random() < 0.3
        for d in worked:
            hours = Fraction(rng.randint(16, 46), 4)
            shift = 'night' if night_worker and rng.random() < 0.6 else 'day'
            if rng.random() < 0.15 and hours >= 4:
                part = Fraction(rng.randint(4, int(hours * 4) - 4), 4)
                sheets.append((d, person['employee_id'], part, 'day'))
                sheets.append((d, person['employee_id'], hours - part, shift))
            else:
                sheets.append((d, person['employee_id'], hours, shift))
    stray = [(date(2026, 8, 31), rng.choice(people)['employee_id'], Fraction(8), 'day')
             for _ in range(3)]
    stray += [(date(2026, 10, 1), rng.choice(people)['employee_id'], Fraction(9), 'night')
              for _ in range(3)]
    stray += [(rng.choice(days), f'P{1300 + k}', Fraction(10), 'day') for k in range(4)]
    return people, sheets, stray


def _payroll_truth(person, rows, amended=(True, True, True)):
    """One employee's columns, from the rows of the pay period that are
    theirs: (date, hours, shift). `amended` switches each amendment, so
    that tests can show what ignoring one costs."""
    hourly = person['pay_type'] == 'hourly'
    rate = Fraction(person['rate'])
    per_hour = rate if hourly else rate / 160
    by_day = {}
    for d, hours, shift in rows:
        by_day.setdefault(d, []).append((hours, shift))
    regular = overtime = ot_pay = night = weekend = holiday = Fraction(0)
    night_days = meal = 0
    logistics = person['department'] == 'logistics'
    for d, items in by_day.items():
        total = sum(h for h, _ in items)
        ot = max(total - 8, 0)
        regular += total - ot
        overtime += ot
        late = d >= AMENDED_FROM
        ot_pay += ot * per_hour * (Fraction(2) if logistics and late and amended[0] else Fraction(3, 2))
        factor = Fraction(20, 100) if late and amended[1] and person['union'] == 'yes' else Fraction(15, 100)
        night += sum(h for h, s in items if s == 'night') * rate * factor
        night_days += any(s == 'night' for _, s in items)
        if d.weekday() >= 5:
            weekend += total * rate / 4
        if d in HOLIDAYS:
            holiday += total * per_hour
        if total >= 6:
            meal += 2000 if late and amended[2] and person['location'] == 'east' else 1500
    if hourly:
        base = _half_up(regular * rate)
        ot_pay = _half_up(ot_pay)
        night = _half_up(night)
        weekend = _half_up(weekend)
    else:
        first = max(person['hire_date'], PERIOD[0])
        last = min(person['end_date'] or PERIOD[1], PERIOD[1])
        base = _half_up(rate * ((last - first).days + 1) / 30)
        ot_pay = _half_up(ot_pay) if person['grade'] <= 2 else 0
        night = 2000 * night_days
        weekend = 0
    hire = person['hire_date']
    years = PERIOD[1].year - hire.year - ((PERIOD[1].month, PERIOD[1].day) < (hire.month, hire.day))
    attendance = 5000 if len(by_day) >= 18 else 0
    seniority = _half_up(Fraction(base * min(years, 10), 100))
    holiday = _half_up(holiday)
    gross = base + ot_pay + night + weekend + holiday + attendance + seniority
    pension = _half_up(Fraction(gross * person['pension_pct'], 100))
    dues = min(_half_up(Fraction(gross * 12, 1000)), 3000) if person['union'] == 'yes' else 0
    taxable = max(0, gross - pension - 50000 * person['dependents'])
    tax = _half_up(Fraction(min(taxable, 300000), 10)
                   + Fraction(max(0, min(taxable, 800000) - 300000), 5)
                   + Fraction(max(0, taxable - 800000) * 3, 10))
    levy = _half_up(gross * {'north': Fraction(2, 100), 'south': Fraction(15, 1000),
                             'east': Fraction(0)}[person['location']])
    deductions = pension + dues + tax + levy
    values = [f'{float(regular):.2f}', f'{float(overtime):.2f}', base, ot_pay, night, weekend,
              holiday, attendance, seniority, gross, pension, dues, taxable, tax, levy, meal,
              deductions, gross - deductions + meal]
    return dict(zip(PAYROLL_COLUMNS, values)), years


def payroll():
    rng = random.Random(SEED + 31)
    policy = _policy(rng)
    people, sheets, stray = _payroll_instance(rng)
    sheets.sort(key=lambda r: (r[0], r[1]))
    rows = stray[:3] + sheets
    for row in stray[6:]:
        rows.insert(rng.randint(400, len(rows)), row)
    rows += stray[3:6]
    truth, extra, populations = {}, {}, {'amendment_1': [], 'amendment_2': [], 'amendment_3': []}
    for person in people:
        mine = [(d, h, s) for d, e, h, s in sheets if e == person['employee_id']]
        values, years = _payroll_truth(person, mine)
        pid = person['employee_id']
        truth[pid] = values
        extra[pid] = {'pay_type': person['pay_type'], 'rate': person['rate'],
                      'pension_pct': person['pension_pct'], 'union': person['union'],
                      'dependents': person['dependents'], 'location': person['location'],
                      'years': years}
        if person['department'] == 'logistics':
            populations['amendment_1'].append(pid)
        if person['union'] == 'yes' and person['pay_type'] == 'hourly':
            populations['amendment_2'].append(pid)
        if person['location'] == 'east':
            populations['amendment_3'].append(pid)
    header = ['employee_id', 'name', 'department', 'pay_type', 'rate', 'grade', 'location',
              'union', 'dependents', 'pension_pct', 'hire_date', 'end_date']
    employees = ','.join(header) + '\n' + ''.join(
        ','.join('' if p[k] is None else str(p[k]) for k in header) + '\n' for p in people)
    timesheets = 'employee_id,date,hours,shift\n' + ''.join(
        f'{e},{d.isoformat()},{float(h):.2f},{s}\n' for d, e, h, s in rows)
    expected = {'columns': PAYROLL_COLUMNS, 'rows': truth, 'people': extra,
                'populations': populations,
                'idle': sorted(p['employee_id'] for p in people
                               if not any(e == p['employee_id'] for _, e, _, _ in sheets)),
                'outside': sorted({e for d, e, _, _ in stray if e in truth})}
    reference = 'employee_id,' + ','.join(PAYROLL_COLUMNS) + '\n' + ''.join(
        pid + ',' + ','.join(str(truth[pid][c]) for c in PAYROLL_COLUMNS) + '\n'
        for pid in sorted(truth))
    instructions = (
        'Run the September 2026 payroll. /in/prepare/data/policy.md defines every output column '
        'in numbered rules, with amendments; employees.csv and timesheets.csv there are the '
        'inputs it describes. Write /workspace/payroll.csv with the header line `employee_id,'
        + ','.join(PAYROLL_COLUMNS) + '` and one line per employee of employees.csv (also those '
        'without timesheet rows), sorted by employee_id, fields separated by commas without '
        'quotes: regular_hours and overtime_hours with exactly two decimals, every other column '
        'an integer number of cents. ' + COMPLETION)
    return Task('payroll', 1, {'policy.md': policy, 'employees.csv': employees,
                               'timesheets.csv': timesheets}, instructions, ['payroll.csv'],
                expected, {'payroll.csv': reference})


# ---------------------------------------------------------------------------
# contracts: extract 15 fields from 48 long agreements
# ---------------------------------------------------------------------------

CONTRACT_FIELDS = ['reference', 'supplier', 'customer', 'effective_date', 'term_months',
                   'auto_renew', 'annual_fee_cents', 'currency', 'payment_days',
                   'late_interest_pct', 'notice_days', 'liability_cap_cents',
                   'confidentiality_years', 'governing_law', 'signatory_supplier']


def contracts():
    seed = SEED + 32
    files, truth = generators.contracts(seed)
    records = {name.split('/')[1]: record for name, record in truth['records'].items()}
    instructions = (
        'Extract the key terms of the 48 agreements in /in/prepare/data/contracts (c01.txt to '
        'c48.txt, plain text). Each agreement names its two parties in numbered paragraphs, each '
        'ending with a defined role in parentheses such as (the "Supplier"). The supplier is the '
        'party whose role is Supplier, Provider, Vendor or Licensor; the customer is the party '
        'whose role is Customer, Client, Buyer or Licensee; either may be listed first. A line '
        'that ends in a hyphen directly after a letter breaks a word: the word continues on the '
        'next line and the hyphen is not part of it. Amounts are written in words followed by '
        'figures in parentheses, as in `two thousand euros (EUR 2,000.00)`; where words and '
        'figures disagree, the words are correct. Dates are written as YYYY-MM-DD, as D Month '
        'YYYY, as Month D, YYYY, or as DD/MM/YYYY (day first). Some agreements end with an '
        'Amendment No. 1, which replaces the terms it names; report the terms as amended. Write '
        '/workspace/contracts.jsonl with one JSON object per agreement, in file name order, each '
        'with exactly these keys: `file` (for example "c01.txt"); `reference`: the Agreement No.; '
        '`supplier` and `customer`: the full party names as written before their first comma; '
        '`effective_date`: the Effective Date of the Term clause as YYYY-MM-DD (not the date the '
        'agreement was made); `term_months`: the initial term in months, an integer (a term in '
        'years is 12 months per year); `auto_renew`: true when the agreement renews '
        'automatically, otherwise false; `annual_fee_cents`: the annual fee in cents, an integer; '
        '`currency`: the three-letter currency code; `payment_days`: the days allowed to pay an '
        'invoice; `late_interest_pct`: the late payment interest rate in percent per annum, a '
        'number; `notice_days`: the notice for termination for convenience in days; '
        '`liability_cap_cents`: the limit of liability in cents, an integer; '
        '`confidentiality_years`: the years confidentiality survives termination; '
        '`governing_law`: the words after "the laws of" in the Governing Law clause, without the '
        'final full stop; `signatory_supplier`: the Name in the signature block of the supplier. '
        + COMPLETION)
    reference = ''.join(json.dumps(dict({'file': name}, **records[name])) + '\n'
                        for name in sorted(records))
    expected = {'fields': CONTRACT_FIELDS, 'records': records}
    return Task('contracts', 1, files, instructions, ['contracts.jsonl'], expected,
                {'contracts.jsonl': reference}, generator=('contracts', seed))


# ---------------------------------------------------------------------------
# bugtrail: symptoms reported in one module, causes in another
# ---------------------------------------------------------------------------

def bugtrail():
    package = bugtrail_package.package()
    files = dict(package, **{'ISSUES.md': bugtrail_package.issues_md(),
                             'test_shop.py': bugtrail_package.VISIBLE})
    instructions = (
        'The Python package shop in /in/prepare/data/shop has the open issues reported in '
        '/in/prepare/data/ISSUES.md, each with the command that shows it. test_shop.py there '
        'holds some tests (copy shop/ and test_shop.py to one directory and run python3.12 '
        'test_shop.py there); they are not complete. Fix the causes of the issues so that every '
        'function and module meets its docstring, for every caller, not only on the path an '
        'issue reports. Do not change docstrings, function names or signatures except where a '
        'signature itself contradicts its docstring, and keep everything that works now working. '
        'Write the whole fixed package, every module of it, to /workspace/shop, with the same '
        'layout as /in/prepare/data/shop. ' + COMPLETION)
    reference = dict(package, **bugtrail_package.FIXED_ROOTS)
    return Task('bugtrail', 1, files, instructions, ['shop'],
                {'package': 'shop', 'tests': bugtrail_package.hidden_tests()}, reference)


def harder_tasks():
    return [shards(), payroll(), contracts(), bugtrail()]
