# gate: judge each proposal on its own, by rules a program can check plus
# the reviewer's verdict, and decide where the Run goes. Proposals that are
# fine stay fine; only the ones with problems go back, once. A later pass
# publishes whatever is left with its status, so nothing is lost to one
# unresolved proposal.

LEVELS = range(4)
QUALITY = ('non_decreasing', 'unknown')
VERDICTS = ('keep', 'revise', 'reject')

proposals_doc, problem = load(IN / 'synthesize' / 'proposals.json')
review, review_problem = load(IN / 'review' / 'review.json')
verified, _ = load(IN / 'verify' / 'verified.json')
earlier, _ = load(IN / 'gate' / 'gate.json')
final_pass = earlier is not None

standing = {c['id']: c['verdict'] for c in (verified or {}).get('claims', [])}
verdicts = {}
if isinstance(review, dict):
    for item in review.get('proposals') or []:
        if isinstance(item, dict) and item.get('id'):
            verdicts[item['id']] = item


def structure(proposal):
    """What a program can say is wrong with one proposal."""
    found = []
    if not isinstance(proposal.get('level'), int) or proposal['level'] not in LEVELS:
        found.append('level must be 0, 1, 2 or 3')
    for key in ('title', 'change', 'scope', 'validation'):
        if not str(proposal.get(key) or '').strip():
            found.append(f'{key} is empty')
    claims = proposal.get('claims')
    if not isinstance(claims, list) or not claims:
        found.append('claims must list the verified claims it rests on')
    else:
        for claim in claims:
            if standing.get(claim) not in ('verified', 'supported'):
                found.append(f'claim {claim} is {standing.get(claim, "unknown to verify")}, '
                             'not verified or supported')
    metric = proposal.get('metric')
    if not isinstance(metric, dict):
        found.append('metric is missing')
    else:
        for key in ('name', 'baseline', 'target'):
            if metric.get(key) in (None, ''):
                found.append(f'metric.{key} is empty')
    if proposal.get('quality') not in QUALITY:
        found.append('quality must be non_decreasing or unknown; a proposal expected to lower quality is not made')
    if not isinstance(proposal.get('guardrails'), list):
        found.append('guardrails must be a list')
    return found


proposals = (proposals_doc or {}).get('proposals') if isinstance(proposals_doc, dict) else None
items, problems = [], []
if problem or not isinstance(proposals, list):
    problems.append(problem or 'proposals.json has no proposals list')
    proposals = []
if review_problem:
    problems.append(f'review: {review_problem}')
seen = set()
for proposal in proposals:
    if not isinstance(proposal, dict):
        problems.append('a proposal is not an object')
        continue
    pid = str(proposal.get('id') or '')
    found = structure(proposal)
    if not pid or pid in seen:
        found.append('id is missing or repeated')
    seen.add(pid)
    verdict = verdicts.get(pid, {})
    judged = verdict.get('verdict') if verdict.get('verdict') in VERDICTS else None
    if judged is None:
        found.append('the review gave it no verdict')
    if judged == 'reject':
        status = 'rejected'
    elif found or judged == 'revise':
        status = 'unresolved' if final_pass else 'revise'
    else:
        status = 'accepted'
    items.append({'id': pid, 'title': proposal.get('title'), 'status': status,
                  'structure': found, 'review': verdict.get('blocking') or [],
                  'notes': verdict.get('notes', '')})

# A first pass sends back what needs revising, or a proposals file that could
# not be read at all; the second pass always publishes.
back = not final_pass and (bool(problems) or any(i['status'] == 'revise' for i in items))
decision = {'pass': 2 if final_pass else 1, 'route': 'synthesize' if back else 'publish',
            'problems': problems, 'proposals': items}
save('gate.json', decision)
lines = [f"# Gate, pass {decision['pass']}", '', f"Route: {decision['route']}", '']
lines += [f'- problem: {p}' for p in problems]
for item in items:
    lines.append(f"- {item['id']} {item['status']}: {cut(item['title'] or '', 80)}")
    lines += [f'  - structure: {s}' for s in item['structure']]
    lines += [f'  - review: {cut(b, 300)}' for b in item['review']]
Path('gate.md').write_text('\n'.join(lines) + '\n')
print('ANCHOR_ROUTE: ' + decision['route'])
