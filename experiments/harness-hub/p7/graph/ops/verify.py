# verify: re-run every check a research node attached to its claims, and
# judge each claim. A claim's own checks come first: any failing check
# refutes it, one that cannot run leaves it unverified. An observed claim
# with passing checks is verified; without checks it is unverified. An
# inferred claim is then judged by the claims it rests on: supported when
# every one of them is verified or supported, unsupported when one fell,
# unverified otherwise. Whether the inference holds is the reviewer's call;
# nobody downstream has to take the facts under it on trust.

SLOTS = [f'investigate-{n}' for n in range(1, 5)] + ['explore']
FELL = ('refuted', 'unsupported')
STANDS = ('verified', 'supported')


def reference(slot, name):
    """A claim id as rests_on names it: `c1` in the same slot, or `slot:c1`."""
    return name if ':' in name else f'{slot}:{name}'


claims, specs = [], []
for slot in SLOTS:
    name = 'explore' if slot == 'explore' else f'findings-{slot.split("-")[1]}'
    data, problem = load(IN / slot / f'{name}.json')
    if problem or not isinstance(data, dict) or data.get('skip'):
        continue
    for index, claim in enumerate(data.get('claims') or []):
        if not isinstance(claim, dict):
            continue
        checks = [c for c in (claim.get('checks') or []) if isinstance(c, dict)]
        basis = [reference(slot, str(r)) for r in (claim.get('rests_on') or []) if r]
        # Ids are the slot's own; the slot makes them unique across slots.
        claims.append({'slot': slot, 'id': f"{slot}:{claim.get('id') or index + 1}",
                       'kind': claim.get('kind'), 'text': claim.get('text', ''),
                       'rests_on': basis, 'checks': len(checks), 'first': len(specs)})
        specs.extend(checks)

results = run_checks(specs)
by_id = {claim['id']: claim for claim in claims}


def checked(claim):
    """The verdict of a claim's own checks, or None when it has none."""
    statuses = [r['status'] for r in results[claim['first']:claim['first'] + claim['checks']]]
    if not statuses:
        return None
    if 'fail' in statuses:
        return 'refuted'
    return 'unverified' if 'error' in statuses else 'verified'


verdicts = {}


def judge(cid, path=()):
    if cid in verdicts:
        return verdicts[cid]
    claim = by_id.get(cid)
    if claim is None or cid in path:
        return 'unverified'  # names nothing, or rests on itself
    own = checked(claim)
    if own in ('refuted', 'unverified') or claim['kind'] != 'inferred' or not claim['rests_on']:
        verdict = own or 'unverified'
    else:
        basis = [judge(r, path + (cid,)) for r in claim['rests_on']]
        if any(b in FELL for b in basis):
            verdict = 'unsupported'
        elif all(b in STANDS for b in basis):
            verdict = 'supported'
        else:
            verdict = 'unverified'
    verdicts[cid] = verdict
    return verdict


verified = []
for claim in claims:
    own = results[claim['first']:claim['first'] + claim['checks']]
    verified.append({'slot': claim['slot'], 'id': claim['id'], 'kind': claim['kind'],
                     'text': claim['text'], 'verdict': judge(claim['id']),
                     'rests_on': [{'id': r, 'verdict': judge(r)} for r in claim['rests_on']],
                     'checks': [dict(spec=specs[claim['first'] + i], **r) for i, r in enumerate(own)]})
save('verified.json', {'claims': verified})

counts = {}
for claim in verified:
    counts[claim['verdict']] = counts.get(claim['verdict'], 0) + 1
lines = ['# Verified claims', '',
         'Each check was re-run against the data hub by this Op. A claim is refuted when one '
         'of its checks failed and unverified when one could not run. An observed claim is '
         'verified when its checks passed; an inferred claim is supported when everything it '
         'rests on is verified or supported (the inference itself is for the review), '
         'unsupported when something under it fell, unverified otherwise.', '',
         f'Totals: {json.dumps(counts)}', '',
         '| claim | verdict | kind | text | checks / rests on |', '|---|---|---|---|---|']
for claim in verified:
    shown = '; '.join(f"{c['status']}: {cell(c['spec'].get('path', ''), 60)} {c['spec'].get('op', '')} "
                      f"{cell(json.dumps(c['spec'].get('value')), 40)} → {cell(json.dumps(c.get('actual')), 60)}"
                      + (f" ({cell(c['message'], 80)})" if c.get('message') else '')
                      for c in claim['checks'])
    if claim['rests_on']:
        shown += ('; ' if shown else '') + 'rests on ' + ', '.join(
            f"{r['id']} ({r['verdict']})" for r in claim['rests_on'])
    lines.append(f"| {claim['id']} | {claim['verdict']} | {claim['kind']} "
                 f"| {cell(claim['text'], 220)} | {cell(shown, 400)} |")
Path('verified.md').write_text('\n'.join(lines) + '\n')
print(json.dumps(counts))
