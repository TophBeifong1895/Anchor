"""Can the graders be trusted? Every reference solution passes every check;
each targeted corruption fails exactly the checks it should; broken or
missing output fails without raising; inputs and graders are deterministic;
and the grade script embedded in the Graph behaves like the grader itself.

Run: python3 -m unittest test_eval   (from this directory)
"""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import graded  # noqa: E402
import graders  # noqa: E402
import hard  # noqa: E402
import solvers  # noqa: E402
import tasks  # noqa: E402


def grade(task, outputs):
    with tempfile.TemporaryDirectory() as directory:
        for name, text in outputs.items():
            Path(directory, name).write_text(text)
        return dict(graders.GRADERS[task.id](directory, task.expected))


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

    def test_the_embedded_grade_script_agrees_with_the_grader(self):
        for task in tasks.TASKS.values():
            with tempfile.TemporaryDirectory() as directory:
                for name, text in task.reference.items():
                    Path(directory, name).write_text(text)
                script = tasks.grade_command(task)
                body = script.split('\n', 1)[1].rsplit('\n', 1)[0]
                body = body.replace(repr('/in/' + tasks.AGENT), repr(directory))
                done = subprocess.run([sys.executable, '-c', body], capture_output=True,
                                      text=True, check=True)
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
