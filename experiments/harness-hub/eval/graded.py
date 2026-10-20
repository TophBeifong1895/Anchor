"""The graded tier: tasks whose quality comes in degrees. The exact tier
(tasks.py, hard.py) asks for answers a careful program computes exactly,
and the baseline harness almost always gets them; here quality is how close
a solution comes to the best known (fleet) or how many judgments out of
many are right (triage), so it has room to vary.
"""

import hashlib
import json
import math
from pathlib import Path
import random

from spec import COMPLETION, HERE, SEED, Task

# ---------------------------------------------------------------------------
# fleet: capacitated vehicle routing, judged against the best known cost
# ---------------------------------------------------------------------------

BEST_KNOWN = HERE / 'fleet_best.json'


def _fleet_instance():
    rng = random.Random(SEED + 20)
    points = {'D': (50, 50)}
    demand = {}
    for n in range(1, 61):
        points[f'C{n}'] = (rng.randint(0, 100), rng.randint(0, 100))
        demand[f'C{n}'] = rng.randint(1, 30)
    return {'points': points, 'demand': demand, 'capacity': 100}


def route_cost(routes, points):
    total = 0.0
    for route in routes:
        path = ['D'] + list(route) + ['D']
        total += sum(math.dist(points[a], points[b]) for a, b in zip(path, path[1:]))
    return total


def _improve(routes, points, demand, capacity):
    """Local search to a local optimum: 2-opt inside routes, then moving or
    swapping customers between routes."""
    def cost(route):
        return route_cost([route], points)

    improved = True
    while improved:
        improved = False
        for r, route in enumerate(routes):
            best = cost(route)
            for i in range(len(route) - 1):
                for j in range(i + 2, len(route) + 1):
                    candidate = route[:i] + route[i:j][::-1] + route[j:]
                    c = cost(candidate)
                    if c < best - 1e-9:
                        route, best, improved = candidate, c, True
            routes[r] = route
        load = [sum(demand[c] for c in route) for route in routes]
        for a in range(len(routes)):
            for i in range(len(routes[a])):
                if i >= len(routes[a]):
                    break
                customer = routes[a][i]
                base_a = cost(routes[a])
                without = routes[a][:i] + routes[a][i + 1:]
                gain_a = base_a - cost(without)
                best_move = None
                for b in range(len(routes)):
                    if b == a or load[b] + demand[customer] > capacity:
                        continue
                    base_b = cost(routes[b])
                    for k in range(len(routes[b]) + 1):
                        with_ = routes[b][:k] + [customer] + routes[b][k:]
                        delta = cost(with_) - base_b - gain_a
                        if delta < -1e-9 and (best_move is None or delta < best_move[0]):
                            best_move = (delta, b, with_)
                if best_move:
                    _, b, with_ = best_move
                    routes[a], routes[b] = without, with_
                    load[a] -= demand[customer]
                    load[b] += demand[customer]
                    improved = True
        routes[:] = [route for route in routes if route]
    return routes


def _savings(points, demand, capacity, rng, noise):
    """Clarke-Wright savings, with the savings perturbed by `noise` so that
    repeated starts explore different solutions."""
    customers = [p for p in points if p != 'D']
    route_of = {c: [c] for c in customers}
    pairs = []
    for i, a in enumerate(customers):
        for b in customers[i + 1:]:
            saving = (math.dist(points['D'], points[a]) + math.dist(points['D'], points[b])
                      - math.dist(points[a], points[b]))
            pairs.append((saving * (1 + rng.uniform(-noise, noise)), a, b))
    pairs.sort(reverse=True)
    for _, a, b in pairs:
        ra, rb = route_of[a], route_of[b]
        if ra is rb or sum(demand[c] for c in ra + rb) > capacity:
            continue
        if ra[-1] == a and rb[0] == b:
            merged = ra + rb
        elif rb[-1] == b and ra[0] == a:
            merged = rb + ra
        elif ra[0] == a and rb[0] == b:
            merged = ra[::-1] + rb
        elif ra[-1] == a and rb[-1] == b:
            merged = ra + rb[::-1]
        else:
            continue
        for c in merged:
            route_of[c] = merged
    unique = []
    for route in route_of.values():
        if not any(route is r for r in unique):
            unique.append(route)
    return [list(r) for r in unique]


def search_best_known(starts=300):
    """Multi-start savings plus local search. Run offline; the result is
    kept in fleet_best.json with the digest of the instance it solves."""
    instance = _fleet_instance()
    points, demand, capacity = instance['points'], instance['demand'], instance['capacity']
    rng = random.Random(1)
    best = None
    for start in range(starts):
        routes = _improve(_savings(points, demand, capacity, rng, 0.0 if start == 0 else 0.3),
                          points, demand, capacity)
        cost = route_cost(routes, points)
        if best is None or cost < best[0] - 1e-9:
            best = (cost, routes)
    return {'instance': _digest(instance), 'cost': round(best[0], 6), 'routes': best[1],
            'method': f'Clarke-Wright savings with 30% noise, 2-opt, relocate; {starts} starts'}


def ruin_and_recreate(routes, points, demand, capacity, rng, rounds):
    """Large neighborhood search: remove a few nearby customers, reinsert
    each where it costs least, polish with local search, keep the result
    when it is no worse."""
    best = [list(r) for r in routes]
    best_cost = route_cost(best, points)
    customers = [c for r in best for c in r]
    for _ in range(rounds):
        seed = rng.choice(customers)
        k = rng.randint(3, 12)
        removed = sorted(customers, key=lambda c: math.dist(points[c], points[seed]))[:k]
        trial = [[c for c in r if c not in removed] for r in best]
        trial = [r for r in trial if r]
        rng.shuffle(removed)
        for c in removed:
            choice = None
            for i, r in enumerate(trial):
                if sum(demand[x] for x in r) + demand[c] > capacity:
                    continue
                base = route_cost([r], points)
                for k2 in range(len(r) + 1):
                    delta = route_cost([r[:k2] + [c] + r[k2:]], points) - base
                    if choice is None or delta < choice[0]:
                        choice = (delta, i, k2)
            alone = 2 * math.dist(points['D'], points[c])
            if choice is None or alone < choice[0]:
                trial.append([c])
            else:
                _, i, k2 = choice
                trial[i] = trial[i][:k2] + [c] + trial[i][k2:]
        trial = _improve(trial, points, demand, capacity)
        cost = route_cost(trial, points)
        if cost <= best_cost + 1e-9:
            best, best_cost = trial, cost
    return best


def _digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


def fleet():
    instance = _fleet_instance()
    known = json.loads(BEST_KNOWN.read_text())
    if known['instance'] != _digest(instance):
        raise SystemExit('fleet_best.json solves another instance; run python3 graded.py')
    points = instance['points']
    instructions = (
        'Plan deliveries for a fleet. /in/prepare/data/fleet.json gives `points` (the depot D and '
        'customers C1 to C60 with x, y coordinates), `demand` (units each customer needs) and '
        '`capacity` (units a vehicle carries). Any number of vehicles may be used. Every route '
        'starts at D, visits customers and returns to D; every customer is visited by exactly one '
        'route; the demand on a route may not exceed capacity. The cost of a solution is the total '
        'Euclidean distance driven. Find a solution of as low a cost as you can: solutions are '
        'judged by how close they come to the best known. Write /workspace/routes.json as '
        '{"routes": [[customers of one route in order], ...], "cost": the total distance as a '
        'number}. ' + COMPLETION)
    expected = {'points': points, 'demand': instance['demand'], 'capacity': instance['capacity'],
                'best_known': known['cost']}
    return Task('fleet', 2, {'fleet.json': json.dumps(instance, indent=1)}, instructions,
                ['routes.json'], expected,
                {'routes.json': json.dumps({'routes': known['routes'], 'cost': known['cost']})},
                tier='graded')


# ---------------------------------------------------------------------------
# triage: judge many tickets by a written policy
# ---------------------------------------------------------------------------

POLICY = """# Support triage policy

Label every ticket with one category and one priority.

## Category: what the customer is asking us to do

- security: suspected unauthorized access, leaked credentials or keys, leaked data, phishing.
- outage: part of the product does not work or is unavailable.
- billing: charges, invoices, refunds, fees.
- account: access to one's own account (login, password reset, lockout) or profile changes.
- feature: a request for something the product does not do yet.

Judge by the request, not by words mentioned in passing: a ticket that thanks us for fixing
an earlier outage and asks for a refund is billing.

## Priority

- security: P1.
- outage: P1 when production or all users are affected; otherwise P2.
- billing: P2 when the amount in question is 1,000 USD or more, or the customer is on the
  Enterprise plan; otherwise P3.
- account: P2 when the customer says they need access within 24 hours; otherwise P3.
- feature: P3.

Then raise the priority one level (P3 to P2, P2 to P1) when the ticket mentions legal action,
lawyers or a regulator. P1 stays P1.
"""

_SECURITY = [
    'I think someone else got into our account: there are logins from countries none of us has '
    'ever been to.',
    'We received an email pretending to be from you asking for our admin password, and one '
    'colleague clicked the link and entered it.',
    'Personal data of our customers from your export feature appeared on a public paste site.',
    'Our API key ended up in a public repository and we now see calls we never made.',
    'An ex-employee still seems to be able to read our private projects after we removed them.',
]
_OUTAGE_WIDE = [
    'Since 09:10 none of our users can load the dashboard; production is completely down.',
    'Every request to the production API returns 502 for all of our customers.',
    'Nobody in the company can sign documents in production since the last release.',
]
_OUTAGE_NARROW = [
    'The CSV export fails for a few of our users; everything else works fine.',
    'Our staging environment has returned errors since this morning; production looks fine.',
    'Search is slow for some users in one region, the rest of the product works.',
    'Image previews do not render in the mobile app for two of our team members.',
]
_BILLING = [
    'We were charged {amount} twice this month and want the duplicate refunded.',
    'The invoice shows {amount} for seats we removed in August; please correct it.',
    'Please explain the overage fee of {amount} on our last bill.',
    'Our card was billed {amount} after we cancelled the subscription.',
]
_ACCOUNT = [
    'I cannot reset my password; the reset email never arrives.',
    'Please change the email address on my profile to my new work address.',
    'My account is locked after too many sign-in attempts.',
    'I lost the phone with my two-factor codes and need to sign in to my account again.',
]
_URGENT = [
    'I need access before our board presentation tomorrow morning.',
    'Our filing is due in a few hours and I am stuck outside my account.',
    'I have to send the contract out tonight, so this cannot wait until next week.',
]
_CALM = ['No rush, whenever you get a chance.', 'Thanks in advance.', '']
_FEATURE = [
    'It would be great if reports could be scheduled to arrive every Monday.',
    'Could you add a dark mode to the editor?',
    'Please support single sign-on with Okta.',
    'We would love an export to Parquet in addition to CSV.',
]
_LEGAL = [
    'If this is not resolved this week we will involve our lawyers.',
    'We are obliged to report this to the regulator if it is not fixed.',
    'Our legal team is copied on this message.',
]
_DISTRACTORS = [
    'Unrelated, but thanks for fixing last week\'s outage so quickly.',
    'My colleague once asked about a dark mode, but that is not why I am writing.',
    'Our billing contact is copied for awareness only.',
    'We changed our password policy internally last month, nothing to do with you.',
]


def _bump(priority):
    return {'P3': 'P2', 'P2': 'P1', 'P1': 'P1'}[priority]


def triage():
    rng = random.Random(SEED + 21)
    tickets, labels = [], {}
    categories = ['security'] * 8 + ['outage'] * 14 + ['billing'] * 14 + ['account'] * 14 \
        + ['feature'] * 10
    rng.shuffle(categories)
    for n, category in enumerate(categories, start=1):
        tid = f'T{n:03d}'
        plan = rng.choice(['Free', 'Pro', 'Enterprise'])
        sentences = []
        if category == 'security':
            sentences.append(rng.choice(_SECURITY))
            priority = 'P1'
        elif category == 'outage':
            wide = rng.random() < 0.5
            sentences.append(rng.choice(_OUTAGE_WIDE if wide else _OUTAGE_NARROW))
            priority = 'P1' if wide else 'P2'
        elif category == 'billing':
            amount = rng.choice([rng.randint(20, 999), rng.randint(1000, 5000)])
            sentences.append(rng.choice(_BILLING).format(amount=f'${amount:,}.00'))
            priority = 'P2' if amount >= 1000 or plan == 'Enterprise' else 'P3'
        elif category == 'account':
            urgent = rng.random() < 0.4
            sentences.append(rng.choice(_ACCOUNT))
            sentences.append(rng.choice(_URGENT if urgent else _CALM))
            priority = 'P2' if urgent else 'P3'
        else:
            sentences.append(rng.choice(_FEATURE))
            priority = 'P3'
        if rng.random() < 0.35:
            sentences.insert(rng.randint(0, len(sentences)), rng.choice(_DISTRACTORS))
        if rng.random() < 0.2:
            sentences.append(rng.choice(_LEGAL))
            priority = _bump(priority)
        body = ' '.join(s for s in sentences if s)
        tickets.append(f'--- {tid}\nPlan: {plan}\n\n{body}\n')
        labels[tid] = [category, priority]
    instructions = (
        'Triage the support tickets in /in/prepare/data/tickets.txt by the policy in '
        '/in/prepare/data/policy.md. Each ticket starts with a line `--- <id>`, then the '
        'customer\'s plan, then the message. Read each ticket and judge it; do not label by '
        'keywords alone. Write /workspace/triage.csv with the header `id,category,priority` and '
        'one line per ticket, for example `T001,billing,P3`. ' + COMPLETION)
    reference = 'id,category,priority\n' + ''.join(f'{t},{c},{p}\n' for t, (c, p) in labels.items())
    return Task('triage', 1, {'tickets.txt': '\n'.join(tickets), 'policy.md': POLICY},
                instructions, ['triage.csv'], {'labels': labels}, {'triage.csv': reference},
                tier='graded')


def graded_tasks():
    return [fleet(), triage()]


if __name__ == '__main__':
    found = search_best_known()
    BEST_KNOWN.write_text(json.dumps(found, indent=1) + '\n')
    print(found['cost'], len(found['routes']), 'routes')
