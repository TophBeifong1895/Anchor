# profile: what the data hub computes over all recorded Runs, written once
# for every later node. The JSON is whole; profile.md is the page a reader
# starts from.

signals = tool('signals', {'limit': 40})
profile = tool('report', {'analyzer': 'profile'})
health = tool('report', {'analyzer': 'health', 'limit': 200})
save('signals.json', signals)
save('profile.json', profile)
save('health.json', health)

lines = ['# Profile', '',
         'Computed by the data hub over every recorded Run. Numbers only; '
         'the reader decides what they mean. Drill down with the harness-hub tools.', '']

lines += ['## Signals', '',
          'Ranked by tier (quality, then waste, then slowness), then by Runs reached.', '',
          '| id | code | subject | Runs | weight | detail |', '|---|---|---|---|---|---|']
for s in signals.get('signals', []):
    lines.append(f"| {s['id']} | {s['code']} | {cell(s['subject'], 60)} | {s.get('runs', '')} "
                 f"| {s['weight']} | {cell(s['detail'], 180)} |")
if signals.get('omitted'):
    lines.append(f"\nOmitted: {json.dumps(signals['omitted'])}")
exemplars = signals.get('exemplars', [])
if exemplars:
    lines += ['', '### Exemplars', '', '| id | code | subject | detail |', '|---|---|---|---|']
    for s in exemplars:
        lines.append(f"| {s['id']} | {s['code']} | {cell(s['subject'], 60)} | {cell(s['detail'], 220)} |")

summary = profile.get('summary', {})
everything = summary.get('all', {})
lines += ['', '## Cost per task', '',
          f"All Runs: {everything.get('tokens')} tokens; {everything.get('above_median_share')} of them "
          'lie above their task median (an upper bound on what stopping at the median buys).', '',
          '| scope | Runs | score | tokens median | spread max/min | turns median | corr(turns,tokens) '
          '| corr(score,tokens) | context growth | model share | tool share | cheapest | dearest |',
          '|---|---|---|---|---|---|---|---|---|---|---|---|---|']


def run_line(run):
    if not run:
        return ''
    return f"{run['tokens']} tok, {run['turns']} turns, score {run['score']}"


for scope, p in summary.get('scopes', {}).items():
    growth = p.get('context_growth') or {}
    lines.append(
        f"| {scope} | {p['runs']} | {p.get('mean_score')} | {(p.get('tokens') or {}).get('median')} "
        f"| {p.get('spread')} | {(p.get('turns') or {}).get('median')} | {p.get('turns_tokens_correlation')} "
        f"| {p.get('score_tokens_correlation')} | {growth.get('median')} | {p.get('model_share')} "
        f"| {p.get('tool_share')} | {run_line(p.get('cheapest'))} | {run_line(p.get('dearest'))} |")

health_summary = health.get('summary', {})
lines += ['', '## Tools', '',
          '`done` counts calls the host accepted that, for commands, also ran to a clean exit.', '',
          '| tool | calls | ok | done | failures | commands that did not end cleanly |',
          '|---|---|---|---|---|---|']
for name, t in sorted(health_summary.get('tools', {}).items(), key=lambda kv: -kv[1]['calls']):
    unclean = {program: {k: v for k, v in ends.items() if k != 'clean'}
               for program, ends in (t.get('commands') or {}).items()}
    unclean = {p: e for p, e in unclean.items() if e}
    lines.append(f"| {name} | {t['calls']} | {t['ok']} | {t.get('done', '')} "
                 f"| {cell(json.dumps(t.get('failures', {})), 80)} | {cell(json.dumps(unclean), 160)} |")

turns = health_summary.get('turns', {})
time = health_summary.get('time', {})
lines += ['', '## Model turns and time', '',
          f"- turns per invocation: {json.dumps(turns.get('per_invocation'))}",
          f"- cache read share: {turns.get('cache_read_share')}; compactions: {turns.get('compactions')}",
          f"- output tokens per turn: {json.dumps(turns.get('output_tokens'))}",
          f"- time to first token (ms): {json.dumps(turns.get('time_to_first_token_ms'))}",
          f"- invocation time: model {time.get('model_share')}, tools {time.get('tool_share')}, "
          f"unobserved {time.get('unobserved_share')}"]

lines += ['', '## Quality', '', '| task | Runs | mean score | pass rate | checks below 1 |', '|---|---|---|---|---|']
for task, q in health_summary.get('quality', {}).items():
    below = {name: rate for name, rate in q.get('checks', {}).items() if rate is not None and rate < 1}
    lines.append(f"| {task} | {q['runs']} | {q['mean_score']} | {q['pass_rate']} | {cell(json.dumps(below), 120)} |")

Path('profile.md').write_text('\n'.join(lines) + '\n')
print(f"profile: {len(signals.get('signals', []))} signals, {len(summary.get('scopes', {}))} scopes")
