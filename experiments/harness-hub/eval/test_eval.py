"""Can the graders be trusted? Every reference solution passes every check;
each targeted corruption fails exactly the checks it should; broken or
missing output fails without raising; inputs and graders are deterministic;
and the grade script embedded in the Graph behaves like the grader itself.

Run: python3 -m unittest test_eval   (from this directory)
"""

import dataclasses
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import bugtrail  # noqa: E402
import generators  # noqa: E402
import graded  # noqa: E402
import hard  # noqa: E402
import harder  # noqa: E402
import solvers  # noqa: E402
import tasks  # noqa: E402

GRADERS, GRADING = tasks.grading()


def grade(task, outputs):
    with tempfile.TemporaryDirectory() as directory:
        for name, text in outputs.items():
            path = Path(directory, name)
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
        return dict(GRADERS[task.id](directory, task.expected))


def failing(task, outputs):
    return sorted(name for name, passed in grade(task, outputs).items() if not passed)


class Graders(unittest.TestCase):
    def test_every_reference_passes_every_check(self):
        for task in tasks.TASKS.values():
            checks = grade(task, task.reference)
            self.assertTrue(all(checks.values()), (task.id, checks))
            self.assertGreaterEqual(len(checks), 5, task.id)

    def test_missing_or_broken_output_fails_every_check(self):
        for task in tasks.TASKS.values():
            self.assertEqual(failing(task, {}), sorted(grade(task, task.reference)), task.id)
            broken = {name: 'not { valid' for name in task.outputs}
            self.assertEqual(failing(task, broken), sorted(grade(task, task.reference)), task.id)

    def test_ledger_checks_are_independent(self):
        task = tasks.TASKS['ledger']
        result = json.loads(task.reference['result.json'])
        result['short'] = result['short'][1:]
        result['total_received_cents'] += 1
        self.assertEqual(failing(task, {'result.json': json.dumps(result)}),
                         ['short', 'total_received_cents'])
        result = json.loads(task.reference['result.json'])
        result['extra'] = 1
        self.assertEqual(failing(task, {'result.json': json.dumps(result)}), ['shape'])
        # Lists must be sorted as instructed.
        result = json.loads(task.reference['result.json'])
        result['unpaid'] = list(reversed(result['unpaid']))
        self.assertEqual(failing(task, {'result.json': json.dumps(result)}), ['unpaid'])

    def test_ledger_traps_are_present(self):
        # Orders split over several payments, exact or short, and payments
        # of unknown orders: what a careless reconciliation gets wrong.
        task = tasks.TASKS['ledger']
        payments = task.files['payments.csv'].splitlines()[1:]
        orders = [line.split(',')[1] for line in payments]
        self.assertGreaterEqual(sum(orders.count(o) > 1 for o in set(orders)), 9)
        self.assertEqual(len(task.expected['unknown_payments']), 3)

    def test_logs_checks_are_independent(self):
        task = tasks.TASKS['logs']
        result = json.loads(task.reference['result.json'])
        result['errors_by_service']['api'] += 1
        result['busiest_minute'] = '2026-10-01T07:00'
        self.assertEqual(failing(task, {'result.json': json.dumps(result)}),
                         ['busiest_minute', 'errors_by_service'])

    def test_contacts_checks_are_independent(self):
        task = tasks.TASKS['contacts']
        lines = task.reference['contacts.csv'].splitlines()
        swapped = [lines[0], lines[2], lines[1]] + lines[3:]
        self.assertEqual(failing(task, {'contacts.csv': '\n'.join(swapped) + '\n'}), ['order'])
        shouted = [lines[0], lines[1].upper()] + lines[2:]
        self.assertEqual(failing(task, {'contacts.csv': '\n'.join(shouted) + '\n'}),
                         ['emails', 'names', 'order', 'phones'])
        no_phone = [lines[0]] + [','.join(l.split(',')[:2] + ['']) for l in lines[1:]]
        self.assertEqual(failing(task, {'contacts.csv': '\n'.join(no_phone) + '\n'}), ['phones'])
        # Keeping a later duplicate instead of the first line is caught.
        self.assertIn('emails', failing(task, {'contacts.csv': '\n'.join(lines + [lines[1]]) + '\n'}))

    def test_repair_credits_each_fixed_behavior(self):
        task = tasks.TASKS['repair']
        self.assertEqual(failing(task, {'inventory.py': tasks.BUGGY}),
                         ['merge', 'page', 'parse', 'value', 'visible'])
        # Fixing only what the visible tests show leaves the rest failing.
        visible_only = (tasks.BUGGY
                        .replace('totals[sku] = qty', 'totals[sku] = totals.get(sku, 0) + qty')
                        .replace('start = number * size', 'start = (number - 1) * size'))
        self.assertEqual(failing(task, {'inventory.py': visible_only}),
                         ['merge', 'parse', 'value'])
        self.assertEqual(failing(task, {'inventory.py': 'raise SystemExit(0)'}),
                         sorted(task.expected['tests']))

    def test_schedule_checks_each_constraint(self):
        task = tasks.TASKS['schedule']
        reference = json.loads(task.reference['schedule.json'])
        talks = task.expected['talks']
        # Put the largest talk in the smallest room's free cell, if one exists.
        big = max(talks, key=lambda t: talks[t]['audience'])
        if talks[big]['audience'] > 30:
            used = {tuple(v) for v in reference.values()}
            free = [['C', s] for s in task.expected['slots'] if ('C', s) not in used]
            if free:
                moved = dict(reference, **{big: free[0]})
                self.assertIn('capacity', failing(task, {'schedule.json': json.dumps(moved)}))
        a, b = task.expected['before'][0]
        swapped = dict(reference, **{a: reference[b], b: reference[a]})
        self.assertIn('order', failing(task, {'schedule.json': json.dumps(swapped)}))
        clash = dict(reference)
        first, second = sorted(clash)[:2]
        clash[second] = clash[first]
        self.assertIn('rooms_free', failing(task, {'schedule.json': json.dumps(clash)}))
        missing = dict(reference)
        missing.pop(first)
        self.assertEqual(failing(task, {'schedule.json': json.dumps(missing)}),
                         sorted(grade(task, task.reference)))


class HardTier(unittest.TestCase):
    def test_refactor_credits_each_fixed_behavior(self):
        task = tasks.TASKS['refactor']
        buggy = {'money.py': hard.MONEY, 'dates.py': hard.DATES, 'invoices.py': hard.INVOICES}
        self.assertEqual(failing(task, buggy), ['days_between', 'parse', 'ranking', 'split',
                                                'to_cents', 'totals', 'visible'])
        visible_only = dict(buggy)
        visible_only['dates.py'] = hard.FIXED_MODULES['dates.py']
        visible_only['invoices.py'] = (hard.FIXED_MODULES['invoices.py']
                                       .replace("into=None", "into={}")
                                       .replace("    into = {} if into is None else into\n", ''))
        self.assertEqual(failing(task, visible_only), ['ranking', 'split', 'to_cents', 'totals'])
        partial = dict(hard.FIXED_MODULES)
        del partial['dates.py']
        self.assertEqual(failing(task, partial), sorted(task.expected['tests']))

    def test_routing_tells_feasible_near_and_exact_optima_apart(self):
        task = tasks.TASKS['routing']
        reference = json.loads(task.reference['route.json'])
        wrong_finish = dict(reference, finish=reference['finish'] + 1)
        self.assertEqual(failing(task, {'route.json': json.dumps(wrong_finish)}),
                         ['finish_reported'])
        stops, travel, service = task.expected['stops'], task.expected['travel'], 5
        # The greedy route by earliest possible arrival is infeasible or worse.
        here, left, t, greedy = 'D', set(stops), 0, []
        while left:
            s = min(left, key=lambda x: (max(t + travel[here][x], stops[x][0]), x))
            t = max(t + travel[here][s], stops[s][0]) + service
            greedy.append(s)
            left.discard(s)
            here = s
        finish = hard._finish(greedy, stops, travel, service)
        failed = failing(task, {'route.json': json.dumps({'route': greedy, 'finish': finish or 0})})
        self.assertIn('optimal', failed)
        # Brute force agrees with the dynamic program on the optimum.
        import itertools
        best = min(f for f in (hard._finish(p, stops, travel, service)
                               for p in itertools.permutations(sorted(stops))) if f is not None)
        self.assertEqual(best, task.expected['optimum'])

    def test_audit_and_incident_fields_are_graded_independently(self):
        for name, key in (('audit', 'refund_due'), ('incident', 'first_error_utc')):
            task = tasks.TASKS[name]
            result = json.loads(task.reference['result.json'])
            result[key] = 'wrong'
            self.assertEqual(failing(task, {'result.json': json.dumps(result)}), [key])


SHARD_MISTAKE = {'bom': 'bom', 'crlf': 'crlf', 'latin1': 'latin1', 'dcomma': 'decimal',
                 'qnl': 'qnl', 'dup': 'dup', 'v2': 'v2', 'null': 'null', 'total': 'total'}


def shard_kinds():
    return {f'shard_{i + 1:02d}.csv': k for i, k in enumerate(generators.SHARD_LAYOUT)}


def first_quirk_line(data, kind, shard, earlier_ids):
    """The 1-based line of the first occurrence of a shard's quirk."""
    base = kind.rsplit('_', 1)[0] if kind.endswith(('_early', '_late')) else kind
    for number, line in enumerate(data.split(b'\n'), start=1):
        text = line.decode(shard['encoding'])
        fields = text.split(shard['delimiter'])
        if base == 'v2' and number > 1 and text.startswith('event_id'):
            return number
        if base == 'latin1' and not line.isascii():
            return number
        if base == 'crlf' and line.endswith(b'\r'):
            return number
        if base == 'qnl' and text.count('"') % 2:
            return number
        if base == 'null' and len(fields) > 3 and fields[3] in ('NULL', 'N/A', ''):
            return number
        if base == 'dcomma' and number > 1 and '.' in fields[3]:
            return number
        if base == 'dup' and number > 1 and fields[0] in earlier_ids:
            return number
        if base == 'total' and text.startswith('#TOTAL'):
            return number
    return None


PAYROLL_DERIVED = ['seniority_bonus', 'gross', 'pension', 'union_dues', 'taxable', 'income_tax',
                   'health_levy', 'deductions', 'net']


def payroll_csv(rows):
    columns = harder.PAYROLL_COLUMNS
    return 'employee_id,' + ','.join(columns) + '\n' + ''.join(
        pid + ',' + ','.join(str(rows[pid][c]) for c in columns) + '\n' for pid in sorted(rows))


def payroll_after(rows, people, column):
    """Recompute the derived columns after `column` by their rules over each
    row's own values: an agent that erred in `column` alone."""
    order = harder.PAYROLL_COLUMNS
    for pid, row in rows.items():
        for later in PAYROLL_DERIVED:
            if order.index(later) > order.index(column):
                row[later] = GRADING['_payroll_derived'](
                    later, {c: str(v) for c, v in row.items()}, people[pid])
    return rows


class HarderTier(unittest.TestCase):
    def test_mishandling_one_shard_quirk_fails_that_shard_alone(self):
        task = tasks.TASKS['shards']
        for name, kind in shard_kinds().items():
            if kind == 'clean':
                continue
            mistake = SHARD_MISTAKE[kind.split('_')[0]]
            summary = solvers.shards(task.files, lambda f: [mistake] if f == name else ())
            want = [name[:-4]] + (['duplicates'] if mistake == 'dup' else [])
            self.assertEqual(failing(task, {'summary.json': json.dumps(summary)}), sorted(want),
                             (name, kind))
        summary = solvers.shards(task.files, lambda f: ['offset'])
        self.assertEqual(failing(task, {'summary.json': json.dumps(summary)}), ['peak_hour'])
        summary = json.loads(task.reference['summary.json'])
        summary['distinct_events'] += 1
        self.assertEqual(failing(task, {'summary.json': json.dumps(summary)}), ['distinct_events'])
        self.assertEqual(len(grade(task, task.reference)), 28)

    def test_shards_hold_their_quirks_half_of_them_deep(self):
        task = tasks.TASKS['shards']
        manifest = {s['file']: s for s in json.loads(task.files['manifest.json'])['shards']}
        ids, late, lines = set(), 0, 0
        for name, kind in shard_kinds().items():
            data, shard = task.files[name], manifest[name]
            lines += data.count(b'\n')
            self.assertTrue(1500 <= data.count(b'\n') <= 4200, name)
            if kind == 'clean':
                self.assertTrue(not data.startswith(b'\xef') and b'\r' not in data and b'"' not in data
                                and b'#TOTAL' not in data and b'NULL' not in data, name)
                self.assertEqual(data.count(b'event_id'), 1, name)
                data.decode('utf-8')
            elif kind == 'bom':
                self.assertTrue(data.startswith(b'\xef\xbb\xbf'))
            else:
                line = first_quirk_line(data, kind, shard, ids)
                self.assertIsNotNone(line, name)
                if kind.endswith('_late') or kind == 'total':
                    self.assertGreater(line, 1500, name)
                    late += 1
                else:
                    self.assertLess(line, 400, name)
            ids.update(l.split(shard['delimiter'])[0]
                       for l in data.decode(shard['encoding']).splitlines())
        self.assertGreaterEqual(late, 8)
        self.assertTrue(55000 <= lines <= 75000, lines)
        records = sum(s['accepted'] + s['rejected'] for s in task.expected['shards'].values())
        self.assertEqual(records, task.expected['distinct_events'] + task.expected['duplicates'])

    def test_payroll_ignoring_one_amendment_fails_that_amendment_alone(self):
        task = tasks.TASKS['payroll']
        for n, switch in enumerate([(False, True, True), (True, False, True),
                                    (True, True, False)], start=1):
            rows = solvers.payroll(task.files, amended=switch)['rows']
            self.assertEqual(failing(task, {'payroll.csv': payroll_csv(rows)}), [f'amendment_{n}'])

    def test_payroll_one_wrong_rule_fails_its_column_alone(self):
        task = tasks.TASKS['payroll']
        people, expected = task.expected['people'], task.expected

        def fresh():
            return {pid: dict(r) for pid, r in expected['rows'].items()}

        plain = [p for p in sorted(expected['rows']) if people[p]['pay_type'] == 'hourly'
                 and p not in expected['idle'] + expected['outside']
                 and not any(p in v for v in expected['populations'].values())]
        for column in harder.PAYROLL_COLUMNS[2:]:
            rows = fresh()
            rows[plain[0]][column] += 1
            rows = payroll_after(rows, people, column)
            self.assertEqual(failing(task, {'payroll.csv': payroll_csv(rows)}), [column], column)
        rows = fresh()
        pid = next(p for p in plain if float(rows[p]['overtime_hours']) > 0)
        rows[pid]['overtime_hours'] = '0.00'
        self.assertEqual(failing(task, {'payroll.csv': payroll_csv(rows)}), ['overtime_hours'])
        # Counting a row outside the pay period, or leaving out an employee
        # without rows.
        rows = fresh()
        rows[expected['outside'][0]]['holiday_pay'] += 100
        rows = payroll_after(rows, people, 'holiday_pay')
        self.assertEqual(failing(task, {'payroll.csv': payroll_csv(rows)}), ['pay_period'])
        rows = fresh()
        del rows[expected['idle'][0]]
        self.assertEqual(failing(task, {'payroll.csv': payroll_csv(rows)}), ['complete'])
        self.assertEqual(len(grade(task, task.reference)), 23)

    def test_payroll_traps_lie_deep_in_its_documents(self):
        task = tasks.TASKS['payroll']
        policy = task.files['policy.md'].splitlines()
        self.assertTrue(650 <= len(policy) <= 800, len(policy))
        self.assertEqual(sum(l.startswith('### Rule ') for l in policy), 28)
        amendments = [n for n, l in enumerate(policy, 1) if l.startswith('### Amendment')]
        self.assertEqual(len(amendments), 3)
        self.assertGreater(min(amendments), 400)
        sheet = task.files['timesheets.csv'].splitlines()
        self.assertTrue(3000 <= len(sheet) <= 4500, len(sheet))
        known = {l.split(',')[0] for l in task.files['employees.csv'].splitlines()}
        unknown = [n for n, l in enumerate(sheet, 1) if l.split(',')[0] not in known]
        self.assertTrue(unknown and min(unknown) > 400)
        october = [n for n, l in enumerate(sheet, 1) if ',2026-10-' in l]
        self.assertTrue(october and min(october) > 400)

    def test_contracts_one_misread_rule_fails_its_field_alone(self):
        task = tasks.TASKS['contracts']
        _, truth = generators.contracts(*task.generator[1:])

        def jsonl(records):
            return ''.join(json.dumps(dict({'file': n}, **records[n])) + '\n'
                           for n in sorted(records))

        def files_of(indices):
            return {f'c{i + 1:02d}.txt' for i in indices}

        for mistake, indices, field in (
                ('hyphen', truth['hyphenated'], 'governing_law'),
                ('figures', truth['contradicted'], 'liability_cap_cents'),
                ('amendment', truth['term_amended'], 'term_months'),
                ('amendment', truth['fee_amended'], 'annual_fee_cents'),
                ('month_first', range(48), 'effective_date')):
            chosen = files_of(indices)
            records = solvers.contracts(task.files,
                                        lambda n: [mistake] if n in chosen else ())['records']
            self.assertEqual(failing(task, {'contracts.jsonl': jsonl(records)}), [field],
                             mistake)
            # Every one of these contracts holds the trap: one wrong is a fail.
            for name in sorted(chosen)[:2] if mistake != 'month_first' else []:
                one = solvers.contracts(task.files,
                                        lambda n: [mistake] if n == name else ())['records']
                self.assertEqual(failing(task, {'contracts.jsonl': jsonl(one)}), [field], name)
        records = json.loads('[' + ','.join(task.reference['contracts.jsonl'].split('\n')[:-1])
                             + ']')
        lines = [json.dumps(r) for r in records]
        self.assertEqual(failing(task, {'contracts.jsonl': '\n'.join(lines[1:] + lines[:1])}),
                         ['sorted'])
        self.assertEqual(failing(task, {'contracts.jsonl': '\n'.join(lines[:-1])}),
                         sorted(['complete'] + harder.CONTRACT_FIELDS))
        extra = dict(records[0], note='x')
        self.assertEqual(failing(task, {'contracts.jsonl': '\n'.join(
            [json.dumps(extra)] + lines[1:])}), ['valid'])
        self.assertEqual(len(grade(task, task.reference)), 18)

    def test_contracts_hold_their_traps_deep(self):
        task = tasks.TASKS['contracts']
        _, truth = generators.contracts(*task.generator[1:])
        texts = {int(n[11:13]) - 1: d.decode() for n, d in task.files.items()}
        total = sum(t.count('\n') for t in texts.values())
        self.assertTrue(11000 <= total <= 16000, total)
        self.assertTrue(all(100 <= t.count('\n') <= 500 for t in texts.values()))

        def line_of(i, marker):
            return next(n for n, l in enumerate(texts[i].splitlines(), 1) if marker(l))

        for i in truth['term_amended'] + truth['fee_amended'] + truth['idle_amended']:
            self.assertGreater(line_of(i, lambda l: l == 'AMENDMENT No. 1'), 400, i)
        for i in truth['contradicted']:
            self.assertGreater(line_of(i, lambda l: 'shall not exceed' in l), 400, i)
            words, figures = re.search(r'shall not exceed ([a-z -]+?) (?:US Dollars|euros|'
                                       r'pounds sterling) \(\w+ ([\d,]+)\.00\)',
                                       ' '.join(texts[i].split())).groups()
            self.assertNotEqual(solvers._from_words(words), int(figures.replace(',', '')))
        for i in truth['hyphenated']:
            self.assertGreater(line_of(i, lambda l: re.search(r'[a-z]-$', l)), 400, i)
        self.assertEqual(len(truth['contradicted']), 5)
        self.assertEqual(len(truth['hyphenated']), 3)
        # The other contracts have no such trap.
        for i in set(range(48)) - set(truth['hyphenated']):
            self.assertFalse(re.search(r'[a-z]-$', texts[i], re.M), i)
        styles = {re.search(r'commences on (\S+ ?\S*,? ?\S*) \(the', ' '.join(t.split())).group(1)
                  [:2].isdigit() for t in texts.values()}
        self.assertEqual(styles, {True, False})
        ambiguous = [i for i, t in texts.items() if re.search(
            r'commences on (0\d|1[0-2])/(0\d|1[0-2])/', t)]
        self.assertGreaterEqual(len(ambiguous), 5)

    def test_bugtrail_credits_root_causes_over_symptom_patches(self):
        task = tasks.TASKS['bugtrail']
        buggy = {n: t for n, t in task.files.items() if n.startswith('shop/')}
        issues = [f'issue{n}' for n in range(1, 8)]
        every = set(issues + [i + '_other_caller' for i in issues])
        self.assertEqual(failing(task, buggy), sorted(every))

        def patched(path, old, new):
            self.assertIn(old, buggy[path])
            return dict(buggy, **{path: buggy[path].replace(old, new)})

        half_up = ('(lambda v: int(__import__("math").floor(abs(v) + 0.5) '
                   '* (1 if v >= 0 else -1)))(')
        # A patch where the symptom shows fixes the report and nothing else.
        symptom_patches = {
            'issue1': patched('shop/pricing.py', '@memoize\ndef quote', 'def quote'),
            'issue2': patched('shop/shipping.py', '@memoize\ndef rate', 'def rate'),
            'issue3': patched('shop/orders.py', "{'order': order_id})",
                              "{'order': order_id}, [])"),
            'issue4': patched('shop/notifications.py', "'message': message})",
                              "'message': message}, [])"),
            'issue5': patched('shop/tax.py', 'return round_cents(', 'return ' + half_up),
            'issue6': patched('shop/discounts.py', 'cents - round_cents(', 'cents - ' + half_up),
            'issue7': patched('shop/checkout.py', 'card  # noqa: F401  the default method',
                              'card, voucher  # noqa: F401'),
        }
        for issue, files in symptom_patches.items():
            self.assertEqual(failing(task, files), sorted(every - {issue}), issue)
        # Each root cause fixed alone clears both tests of each of its issues.
        causes = {'shop/util/cache.py': ['issue1', 'issue2'],
                  'shop/util/events.py': ['issue3', 'issue4'],
                  'shop/util/money.py': ['issue5', 'issue6'],
                  'shop/registry.py': ['issue7']}
        for path, fixed in causes.items():
            files = dict(buggy, **{path: bugtrail.FIXED_ROOTS[path]})
            cleared = set(fixed + [i + '_other_caller' for i in fixed])
            self.assertEqual(failing(task, files), sorted(every - cleared), path)
        # Careless fixes at the cause break what worked.
        reference = task.reference
        careless = {
            'regression_cache': ('shop/util/cache.py', 'if key in _CACHE:', 'if False:'),
            'regression_events': ('shop/util/events.py', 'tags = list(tags or []) + [kind]',
                                  'tags = [kind]'),
            'regression_money': ('shop/util/money.py',
                                 '    whole = math.floor(abs(value) + 0.5)\n'
                                 '    return int(whole if value >= 0 else -whole)\n',
                                 '    return int(value + 0.5)\n'),
            'regression_registry': ('shop/registry.py',
                                    "        raise ValueError(f'payment method {name!r} is "
                                    "already registered')\n", '        return\n'),
        }
        for group, (path, old, new) in careless.items():
            self.assertIn(old, reference[path], group)
            files = dict(reference, **{path: reference[path].replace(old, new)})
            # Dropping given tags also breaks urgent notifications (issue4).
            self.assertIn(group, failing(task, files), group)
        self.assertEqual(len(grade(task, reference)), 18)

    def test_bugtrail_reports_what_the_package_does(self):
        task = tasks.TASKS['bugtrail']
        modules = [n for n in task.files if n.startswith('shop/') and n.endswith('.py')]
        self.assertGreaterEqual(len(modules), 30)
        self.assertGreaterEqual(sum(task.files[n].count('\n') for n in modules), 3000)
        for label, files, column in (('buggy', task.files, 3), ('fixed', task.reference, 4)):
            with tempfile.TemporaryDirectory() as directory:
                for name, text in files.items():
                    if name.startswith('shop/'):
                        Path(directory, name).parent.mkdir(parents=True, exist_ok=True)
                        Path(directory, name).write_text(text)
                Path(directory, 'test_shop.py').write_text(bugtrail.VISIBLE)
                done = subprocess.run([sys.executable, 'test_shop.py'], cwd=directory,
                                      capture_output=True, text=True)
                self.assertEqual(done.stdout.strip(), 'OK', label)
                for issue in bugtrail.ISSUES:
                    done = subprocess.run([sys.executable, '-c', issue[2]], cwd=directory,
                                          capture_output=True, text=True)
                    printed = done.stdout.strip() or done.stderr.strip().splitlines()[-1]
                    self.assertEqual(printed, issue[column], (label, issue[0]))
        # No report names the module that holds its cause.
        where = {'cache': 'util.cache', 'events': 'util.events', 'money': 'util.money',
                 'registry': 'registry'}
        for _, cause, code, *_ in bugtrail.ISSUES:
            self.assertNotIn(where[cause], code)

    def test_large_inputs_are_generated_in_the_sandbox_and_checked(self):
        task = tasks.TASKS['shards']
        script = tasks.graph(task)['ops']['prepare']['run']
        self.assertLess(len(script), 64 * 1024)
        with tempfile.TemporaryDirectory() as directory:
            subprocess.run(['sh', '-c', script], cwd=directory, check=True)
            for name, data in task.files.items():
                self.assertEqual(Path(directory, 'data', name).read_bytes(), data, name)
        tampered = dataclasses.replace(task, files=dict(task.files, **{'manifest.json': b'{}'}))
        with tempfile.TemporaryDirectory() as directory:
            done = subprocess.run(['sh', '-c', tasks.graph(tampered)['ops']['prepare']['run']],
                                  cwd=directory, capture_output=True)
            self.assertNotEqual(done.returncode, 0)
            self.assertIn(b'differ', done.stderr)

    def test_every_op_fits_one_argument(self):
        for task in tasks.TASKS.values():
            for graph in (tasks.graph(task), tasks.reference_graph(task)):
                for op in graph['ops'].values():
                    self.assertLess(len(op['run'].encode()), tasks.MAX_RUN, task.id)

    def test_reference_solutions_run_quickly(self):
        for name in ('shards', 'payroll', 'contracts'):  # bugtrail has no computed answer
            task = tasks.TASKS[name]
            started = time.monotonic()
            self.assertEqual(solvers.SOLVERS[name](task.files), task.expected)
            self.assertLess(time.monotonic() - started, 5, name)


class GradedTier(unittest.TestCase):
    def test_the_best_known_fleet_solution_is_valid_and_costs_what_it_says(self):
        task = tasks.TASKS['fleet']
        known = json.loads(graded.BEST_KNOWN.read_text())
        self.assertAlmostEqual(graded.route_cost(known['routes'], task.expected['points']),
                               known['cost'], places=5)
        self.assertEqual(failing(task, task.reference), [])

    def test_fleet_credit_grows_with_solution_quality(self):
        task = tasks.TASKS['fleet']
        instance = graded._fleet_instance()
        plain = graded._savings(instance['points'], instance['demand'], instance['capacity'],
                                __import__('random').Random(0), 0.0)
        cost = graded.route_cost(plain, instance['points'])
        failed = failing(task, {'routes.json': json.dumps({'routes': plain, 'cost': cost})})
        self.assertIn('best_known', failed)
        self.assertNotIn('within_15pct', failed)
        self.assertNotIn('capacity', failed)
        # One vehicle per customer: valid, but far from good.
        alone = [[c] for c in instance['demand']]
        cost = graded.route_cost(alone, instance['points'])
        self.assertEqual(failing(task, {'routes.json': json.dumps({'routes': alone, 'cost': cost})}),
                         ['best_known', 'within_15pct', 'within_1pct', 'within_3pct',
                          'within_7pct'])
        # Too much on one vehicle: no credit for its cost.
        known = json.loads(task.reference['routes.json'])
        overloaded = [known['routes'][0] + known['routes'][1]] + known['routes'][2:]
        self.assertIn('capacity', failing(task, {'routes.json': json.dumps(
            {'routes': overloaded, 'cost': 0})}))
        self.assertIn('best_known', failing(task, {'routes.json': json.dumps(
            {'routes': overloaded, 'cost': 0})}))

    def test_triage_needs_reading_not_keywords(self):
        task = tasks.TASKS['triage']
        keywords = [('pretending', 'security'), ('repository', 'security'), ('paste', 'security'),
                    ('logins', 'security'), ('ex-employee', 'security'), ('outage', 'outage'),
                    ('production', 'outage'), ('users', 'outage'), ('staging', 'outage'),
                    ('dark mode', 'feature'), ('would love', 'feature'), ('support single', 'feature'),
                    ('every monday', 'feature'), ('billing', 'billing'), ('$', 'billing'),
                    ('password', 'account'), ('locked', 'account'), ('login', 'account'),
                    ('profile', 'account')]
        tickets = task.files['tickets.txt'].split('--- ')[1:]
        lines = ['id,category,priority']
        for ticket in tickets:
            tid, text = ticket.split('\n', 1)
            category = next((c for k, c in keywords if k in text.lower()), 'feature')
            lines.append(f'{tid.strip()},{category},P3')
        failed = failing(task, {'triage.csv': '\n'.join(lines) + '\n'})
        self.assertIn('category_all', failed)
        self.assertIn('p1_recall', failed)
        labels = task.expected['labels']
        swapped = dict(labels)
        first_p1 = next(t for t, (_, p) in labels.items() if p == 'P1')
        swapped[first_p1] = [labels[first_p1][0], 'P2']
        csv_text = 'id,category,priority\n' + ''.join(f'{t},{c},{p}\n' for t, (c, p) in swapped.items())
        self.assertEqual(failing(task, {'triage.csv': csv_text}), ['p1_recall', 'priority_all'])

    def test_triage_mixes_what_changes_a_label(self):
        task = tasks.TASKS['triage']
        text = task.files['tickets.txt']
        for marker in ('Plan: Enterprise', 'lawyers', 'regulator', 'Unrelated', 'tomorrow'):
            self.assertIn(marker, text)
        priorities = [p for _, p in task.expected['labels'].values()]
        self.assertTrue({'P1', 'P2', 'P3'} <= set(priorities))


class ExpectedAnswers(unittest.TestCase):
    def test_independent_solutions_from_the_files_agree_with_the_expected_values(self):
        for name, solve in solvers.SOLVERS.items():
            task = tasks.TASKS[name]
            self.assertEqual(solve(task.files), task.expected, name)

    def test_hard_tasks_hold_their_traps(self):
        audit = tasks.TASKS['audit']
        payments = [json.loads(l) for l in audit.files['payments.jsonl'].splitlines()]
        self.assertTrue(any(p['order'] != p['order'].strip().upper() for p in payments))
        self.assertTrue(any(',' in p['amount'] for p in payments))
        self.assertTrue(any(p['kind'] == 'refund' for p in payments))
        weekend = [p for p in payments if p['currency'] != 'CNY'
                   and p['date'] not in audit.files['rates.csv']]
        self.assertTrue(weekend, 'some foreign payments fall on days without a rate')
        incident = tasks.TASKS['incident']
        self.assertIn('+08:00', incident.files['payments.log'])
        self.assertIn('caused by:', incident.files['payments.log'])
        starts = [json.loads(l)['rid'] for l in incident.files['gateway.jsonl'].splitlines()
                  if json.loads(l)['event'] == 'start']
        self.assertGreater(len(starts), len(set(starts)), 'retries present')


class Determinism(unittest.TestCase):
    def test_inputs_expected_values_and_digests_are_stable(self):
        again = {t.id: t for t in tasks.all_tasks()}
        for task in tasks.TASKS.values():
            self.assertEqual(task.files, again[task.id].files, task.id)
            self.assertEqual(task.expected, again[task.id].expected, task.id)
            self.assertEqual(tasks.grader_digest(task), tasks.grader_digest(again[task.id]))

    def test_generated_inputs_are_stable_and_carried_by_their_digest(self):
        for task in tasks.TASKS.values():
            if task.generator:
                function, seed = task.generator
                again, _ = getattr(generators, function)(seed)
                self.assertEqual(generators.digest(again), generators.digest(task.files))
                self.assertIn(generators.digest(task.files),
                              tasks.graph(task)['ops']['prepare']['run'])

    def test_the_embedded_grade_script_agrees_with_the_grader(self):
        for task in tasks.TASKS.values():
            with tempfile.TemporaryDirectory() as directory:
                for name, text in task.reference.items():
                    Path(directory, name).parent.mkdir(parents=True, exist_ok=True)
                    Path(directory, name).write_text(text)
                # The Op's own path: one `sh -c` argument, script on stdin.
                done = subprocess.run(['sh', '-c', tasks.grade_command(task, directory)],
                                      capture_output=True, text=True, check=True)
                printed = json.loads(done.stdout.strip().splitlines()[-1])
                self.assertEqual(printed['task'], task.id)
                self.assertTrue(all(c['passed'] for c in printed['checks']), printed)

    def test_the_prepare_script_writes_the_inputs(self):
        task = tasks.TASKS['logs']
        with tempfile.TemporaryDirectory() as directory:
            script = tasks.graph(task)['ops']['prepare']['run']
            subprocess.run(['sh', '-c', script], cwd=directory, check=True)
            for name, text in task.files.items():
                self.assertEqual(Path(directory, 'data', name).read_text(), text)

    def test_a_task_carries_only_its_own_graders(self):
        # Graders added for the harder tasks must not change the grade Op,
        # and so the grader digest, of a task recorded before them.
        harder = (tasks.HERE / 'graders_harder.py').read_text()
        for task in tasks.TASKS.values():
            grade = tasks.graph(task)['ops']['grade']['run']
            self.assertEqual(task.id in tasks.HARDER, harder in tasks.grader_source(task), task.id)
            if task.id not in tasks.HARDER:
                self.assertNotIn('def grade_shards', grade)

    def test_graders_have_no_dependency_beyond_the_standard_library(self):
        source = tasks.grader_source()
        imports = {line.split()[1] for line in source.splitlines()
                   if line.startswith('import ')}
        self.assertLessEqual(imports, {'csv', 'io', 'json', 'math', 'os', 'shutil', 'subprocess', 'sys', 'tempfile'})
        self.assertNotIn('from ', ''.join(l for l in source.splitlines() if l.startswith('from')))


class Stopping(unittest.TestCase):
    """experiment.py stops once the gate has decided every arm."""

    def test_an_arm_the_gate_has_not_seen_is_open_and_only_decided_arms_stop(self):
        import experiment
        report = {'summary': {'arms': {'bbb': {'decision': 'admitted'},
                                       'ccc': {'decision': 'rejected'}}}}
        self.assertEqual(experiment.decisions(report, {'B': 'bbb', 'C': 'ccc', 'D': 'ddd'}),
                         {'B': 'admitted', 'C': 'rejected', 'D': 'open'})
        self.assertTrue(experiment.settled({'B': 'admitted', 'C': 'rejected'}))
        self.assertFalse(experiment.settled({'B': 'admitted', 'D': 'open'}))
        # A report the gate could not make (no grades yet) decides nothing.
        self.assertEqual(experiment.decisions({'summary': {}}, {'B': 'bbb'}), {'B': 'open'})


if __name__ == '__main__':
    unittest.main()
