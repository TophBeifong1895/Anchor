# verify: re-run every check a research node attached to its claims, and
# judge each claim by them. A claim stands when all its checks pass,
# falls when any fails, and is unverified when it has none or one cannot
# run. Nobody downstream has to take a claim on trust.

SLOTS = [f'investigate-{n}' for n in range(1, 5)] + ['explore']
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
        # Ids are the slot's own; the slot makes them unique across slots.
        claims.append({'slot': slot, 'id': f"{slot}:{claim.get('id') or index + 1}",
                       'kind': claim.get('kind'), 'text': claim.get('text', ''),
                       'checks': len(checks), 'first': len(specs)})
        specs.extend(checks)

results = run_checks(specs)

verified = []
for claim in claims:
    own = results[claim['first']:claim['first'] + claim['checks']]
    statuses = [r['status'] for r in own]
    if not own:
        verdict = 'unverified'
    elif 'fail' in statuses:
        verdict = 'refuted'
    elif 'error' in statuses:
        verdict = 'unverified'
    else:
        verdict = 'verified'
    verified.append({'slot': claim['slot'], 'id': claim['id'], 'kind': claim['kind'],
                     'text': claim['text'], 'verdict': verdict,
                     'checks': [dict(spec=specs[claim['first'] + i], **r) for i, r in enumerate(own)]})
save('verified.json', {'claims': verified})

counts = {}
for claim in verified:
    counts[claim['verdict']] = counts.get(claim['verdict'], 0) + 1
lines = ['# Verified claims', '',
         'Each check was re-run against the data hub by this Op. A claim is verified when '
         'every check passed, refuted when one failed, unverified when it had none or one '
         'could not run.', '', f'Totals: {json.dumps(counts)}', '',
         '| claim | verdict | kind | text | checks |', '|---|---|---|---|---|']
for claim in verified:
    shown = '; '.join(f"{c['status']}: {cell(c['spec'].get('path', ''), 60)} {c['spec'].get('op', '')} "
                      f"{cell(json.dumps(c['spec'].get('value')), 40)} → {cell(json.dumps(c.get('actual')), 60)}"
                      + (f" ({cell(c['message'], 80)})" if c.get('message') else '')
                      for c in claim['checks'])
    lines.append(f"| {claim['id']} | {claim['verdict']} | {claim['kind']} "
                 f"| {cell(claim['text'], 220)} | {cell(shown, 400)} |")
Path('verified.md').write_text('\n'.join(lines) + '\n')
print(json.dumps(counts))
