# publish: the report a human reads, assembled from what the Run produced.
# Every proposal appears with the status the gate gave it; refuted and
# unverified claims are listed, not hidden.

proposals_doc, _ = load(IN / 'synthesize' / 'proposals.json')
gate, _ = load(IN / 'gate' / 'gate.json')
review, _ = load(IN / 'review' / 'review.json')
status = {i['id']: i for i in (gate or {}).get('proposals', [])}
proposals = (proposals_doc or {}).get('proposals') or []
for proposal in proposals:
    judged = status.get(str(proposal.get('id')), {})
    proposal['status'] = judged.get('status', 'unjudged')
    proposal['open_issues'] = judged.get('structure', []) + judged.get('review', [])
save('proposals.json', {'proposals': proposals})


def section(path, title):
    try:
        return [f'## {title}', '', Path(path).read_text().strip(), '']
    except OSError:
        return [f'## {title}', '', f'({path} is missing)', '']


order = {'accepted': 0, 'unresolved': 1, 'revise': 1, 'unjudged': 2, 'rejected': 3}
lines = ['# Harness RSI report', '']
counts = {}
for proposal in proposals:
    counts[proposal['status']] = counts.get(proposal['status'], 0) + 1
lines += [f'Proposals by status: {json.dumps(counts)}. Gate pass {(gate or {}).get("pass")}.', '']
lines += ['## Proposals', '']
for proposal in sorted(proposals, key=lambda p: order.get(p['status'], 9)):
    metric = proposal.get('metric') or {}
    lines += [f"### {proposal.get('id')} [{proposal['status']}] level {proposal.get('level')}: {proposal.get('title')}", '',
              f"- change: {proposal.get('change')}",
              f"- rests on: {', '.join(map(str, proposal.get('claims') or []))}",
              f"- metric: {metric.get('name')}: baseline {metric.get('baseline')} → target {metric.get('target')}",
              f"- guardrails: {'; '.join(map(str, proposal.get('guardrails') or []))}",
              f"- scope: {proposal.get('scope')}", f"- quality: {proposal.get('quality')}",
              f"- validation: {proposal.get('validation')}"]
    lines += [f'- open: {issue}' for issue in proposal['open_issues']]
    note = status.get(str(proposal.get('id')), {}).get('notes')
    if note:
        lines.append(f'- reviewer: {note}')
    lines.append('')
lines += section(IN / 'synthesize' / 'synthesis.md', 'Synthesis')
lines += section(IN / 'verify' / 'verified.md', 'Claims and their checks')
if isinstance(review, dict) and review.get('summary'):
    lines += ['## Review', '', str(review['summary']), '']
lines += section(IN / 'profile' / 'profile.md', 'Profile the Run started from')
Path('report.md').write_text('\n'.join(lines) + '\n')
print(json.dumps(counts))
