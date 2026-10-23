"""Graders of the harder tasks (harder.py, bugtrail.py). Same contract as
graders.py, whose helpers they use: the grade Op of these tasks runs
graders.py followed by this file, while the other tasks' grade Ops carry
graders.py alone, so adding a task here leaves their grader digests as
they were. Locally, tasks.grading() loads both into one namespace the
same way.
"""


def grade_shards(directory, expected):
    """One check per shard (its three counts together), the shape, and
    three global values. `duplicates` and `peak_hour` are judged against the
    expected values; `distinct_events` against the agent's own counts: every
    record is either a first occurrence or a duplicate, so it must equal the
    sum of its records less its duplicates. A record miscounted in one shard
    then fails that shard alone."""
    result = _json(directory, 'summary.json')
    got = result.get('shards') if result else None
    got = got if isinstance(got, dict) else {}
    want = expected['shards']
    shape = result is not None and sorted(result) == ['distinct_events', 'duplicates',
                                                      'peak_hour', 'shards'] \
        and sorted(got) == sorted(want)
    counts = [g.get(k) for g in got.values() if isinstance(g, dict)
              for k in ('accepted', 'rejected')]
    distinct = (result is not None and len(counts) == 2 * len(want)
                and all(type(c) is int for c in counts)
                and type(result.get('duplicates')) is int
                and result.get('distinct_events') == sum(counts) - result['duplicates'])
    return ([('shape', shape)]
            + [(name.rsplit('.', 1)[0], got.get(name) == want[name]) for name in sorted(want)]
            + [('distinct_events', distinct)]
            + _fields(result, expected, ['duplicates', 'peak_hour']))


def _int(text):
    try:
        return int(text.strip()) if text.strip().lstrip('-').isdigit() else None
    except AttributeError:
        return None


def _half_up(a, b):
    """a / b rounded half up, for integers a and b > 0."""
    return (2 * a + b) // (2 * b)


def _payroll_derived(column, row, person):
    """What `column` must be, given the employee's own upstream values:
    downstream columns are judged on their own rule, not on inherited
    errors."""
    v = {c: _int(row.get(c, '')) for c in row}
    if column == 'base_pay':
        hours = row['regular_hours'].strip()
        whole, _, cents = hours.partition('.')
        if not (whole.isdigit() and len(cents) == 2 and cents.isdigit()):
            return None
        return _half_up(int(whole + cents) * person['rate'], 100)
    if column == 'seniority_bonus':
        return _half_up(v['base_pay'] * min(person['years'], 10), 100)
    if column == 'gross':
        return sum(v[c] for c in ('base_pay', 'overtime_pay', 'night_premium', 'weekend_premium',
                                  'holiday_pay', 'attendance_bonus', 'seniority_bonus'))
    gross = v['gross']
    if column == 'pension':
        return _half_up(gross * person['pension_pct'], 100)
    if column == 'union_dues':
        return min(_half_up(gross * 12, 1000), 3000) if person['union'] == 'yes' else 0
    if column == 'taxable':
        return max(0, gross - v['pension'] - 50000 * person['dependents'])
    if column == 'income_tax':
        t = v['taxable']
        return _half_up(min(t, 300000) + 2 * max(0, min(t, 800000) - 300000)
                        + 3 * max(0, t - 800000), 10)
    if column == 'health_levy':
        return {'north': _half_up(gross * 2, 100), 'south': _half_up(gross * 15, 1000),
                'east': 0}[person['location']]
    if column == 'deductions':
        return v['pension'] + v['union_dues'] + v['income_tax'] + v['health_levy']
    if column == 'net':
        return gross - v['deductions'] + v['meal_allowance']
    raise KeyError(column)


def grade_payroll(directory, expected):
    """Upstream columns against the expected values; downstream columns
    against their rule applied to the agent's own upstream values. Each
    amended column is graded outside the amendment's population, and the
    population gets its own check. Column checks judge the employees with
    timesheet rows in the pay period only; `pay_period` judges every column
    of the employees who also have rows outside it, and `complete` that
    every employee has exactly one line, in order, and that those without
    rows are right."""
    columns, want, people = expected['columns'], expected['rows'], expected['people']
    amended = {'overtime_pay': 'amendment_1', 'night_premium': 'amendment_2',
               'meal_allowance': 'amendment_3'}
    names = columns + sorted(set(amended.values())) + ['pay_period', 'complete']
    text = _read(directory, 'payroll.csv')
    try:
        rows = list(csv.reader(io.StringIO(text))) if text is not None else []
    except csv.Error:
        rows = []
    if not rows or rows[0] != ['employee_id'] + columns:
        return [(name, False) for name in names]
    got = {}
    for row in rows[1:]:
        if len(row) == len(columns) + 1:
            got.setdefault(row[0], dict(zip(columns, row[1:])))
    idle = set(expected['idle'])
    skip = idle | set(expected['outside'])

    def holds(column, ids):
        for pid in ids:
            row = got.get(pid)
            if row is None:
                return False
            if column in ('regular_hours', 'overtime_hours'):
                if row[column] != want[pid][column]:
                    return False
                continue
            value = _int(row[column])
            try:
                hourly_base = column == 'base_pay' and people[pid]['pay_type'] == 'hourly'
                target = want[pid][column] if not hourly_base and column in (
                    'base_pay', 'overtime_pay', 'night_premium', 'weekend_premium', 'holiday_pay',
                    'attendance_bonus', 'meal_allowance') else _payroll_derived(column, row,
                                                                                people[pid])
            except TypeError:
                return False
            if value is None or value != target:
                return False
        return True

    checks = []
    for column in columns:
        outside = set(expected['populations'][amended[column]]) if column in amended else set()
        checks.append((column, holds(column, [p for p in sorted(want)
                                              if p not in outside and p not in skip])))
    for column, population in sorted(amended.items(), key=lambda kv: kv[1]):
        checks.append((population, holds(column, [p for p in expected['populations'][population]
                                                  if p not in skip])))
    checks.append(('pay_period', all(
        holds(c, [p for p in expected['outside']
                  if c not in amended or p not in expected['populations'][amended[c]]])
        for c in columns)))
    checks.append(('complete', [row[0] for row in rows[1:]] == sorted(want)
                   and all(holds(c, sorted(idle)) for c in columns)))
    return checks


def grade_contracts(directory, expected):
    """One check per field, passed when that field is right in every
    agreement, and checks that the file is valid, complete and in order.
    Records are matched by `file`, so order affects `sorted` alone; a
    missing agreement has every field wrong."""
    fields, want = expected['fields'], expected['records']
    names = ['valid', 'complete', 'sorted'] + fields
    text = _read(directory, 'contracts.jsonl')
    records = []
    for line in (text or '').splitlines():
        if not line.strip():
            continue
        try:
            value = json.loads(line)
        except ValueError:
            value = None
        records.append(value)
    if not records or not any(isinstance(r, dict) for r in records):
        return [(name, False) for name in names]
    valid = all(isinstance(r, dict) and sorted(r) == sorted(['file'] + fields) for r in records)
    by_file = {}
    for r in records:
        if isinstance(r, dict) and isinstance(r.get('file'), str):
            by_file.setdefault(r['file'], r)
    files = [r.get('file') if isinstance(r, dict) else None for r in records]
    checks = [('valid', valid),
              ('complete', sorted(f for f in files if f is not None) == sorted(want)
               and len(files) == len(want)),
              ('sorted', None not in files and files == sorted(files))]
    for field in fields:
        right = True
        for name, record in want.items():
            got = by_file.get(name, {}).get(field)
            if isinstance(record[field], bool) or isinstance(got, bool):
                right = right and got is record[field]
            elif isinstance(record[field], (int, float)):
                right = right and isinstance(got, (int, float)) and got == record[field]
            else:
                right = right and got == record[field]
        checks.append((field, right))
    return checks


def grade_bugtrail(directory, expected):
    """Run each hidden test group in a fresh interpreter against the agent's
    package, copied into a scratch directory."""
    source = os.path.join(directory, expected['package'])
    checks = []
    for group, test in expected['tests'].items():
        passed = False
        if os.path.isdir(source):
            scratch = tempfile.mkdtemp()
            try:
                shutil.copytree(source, os.path.join(scratch, expected['package']),
                                ignore=shutil.ignore_patterns('__pycache__'))
                done = subprocess.run([sys.executable, '-c', test], cwd=scratch,
                                      capture_output=True, timeout=20, check=False)
                passed = done.returncode == 0 and done.stdout.strip().endswith(b'OK')
            except (OSError, shutil.Error, subprocess.TimeoutExpired):
                passed = False
            finally:
                shutil.rmtree(scratch, ignore_errors=True)
        checks.append((group, passed))
    return checks


GRADERS.update({
    'shards': grade_shards,
    'payroll': grade_payroll,
    'contracts': grade_contracts,
    'bugtrail': grade_bugtrail,
})
