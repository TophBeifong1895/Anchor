"""P2 check: are the facts P0 could not obtain now recorded, at what cost,
and without business content?

Reads every Run root under .local/harness-hub-p2/runs through the recorded
kernel histories only (state/kernel-facts), then cross-checks them with the
invocation evidence. Writes summary.json next to this script.
"""

import glob
import json
from pathlib import Path
import statistics
import sys

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(REPO / 'experiments/harness-hub/p0'))
from fixtures import t1_numbers, t2_files  # noqa: E402

# Business values that must never appear in recorded facts.
FORBIDDEN = ['red_qty', 'blue_files', 'item=', 'qty=', 'SUM=', 'TOP5', '/in/prepare',
             str(sum(t1_numbers()))] + [line for lines in t2_files().values() for line in lines[:1]]


def facts_of(root):
    records = []
    for path in sorted(glob.glob(str(root / 'state/kernel-facts/trace*/history.jsonl'))):
        for line in Path(path).read_text().splitlines():
            if line.strip():
                records.append(json.loads(line))
    return records


def main():
    roots = sorted(Path(p) for p in glob.glob(str(REPO / '.local/harness-hub-p2/runs/*')))
    rows = []
    for root in roots:
        meta = json.loads((root / 'meta.json').read_text())
        records = facts_of(root)
        facts = [f for r in records for f in r['submission']['content']['facts']]
        evidence = [json.loads(Path(p).read_text())
                    for p in glob.glob(str(root / 'state/goose-acp/*.evidence.json'))]
        recordings = [e.get('fact_recording', {}) for e in evidence]
        configs = glob.glob(str(root / 'state/kernel-facts/config*/resource.json'))
        tools = [f for f in facts if f['kind'] == 'tool_call']
        turns = [f for f in facts if f['kind'] == 'model_turn']
        lifecycles = [f for f in facts if f['kind'] == 'lifecycle']
        raw = '\n'.join(json.dumps(r) for r in records) + '\n'.join(
            Path(p).read_text() for p in configs)
        rows.append({
            'task': meta['task'], 'run': meta['run'], 'wall_seconds': meta['wall_seconds'],
            'records': len(records),
            'all_accepted': all(r['verdict']['effect'] == 'accepted' for r in records),
            'stamped_time': all(r['stamp'].get('at') for r in records),
            'stamped_config': all(r['stamp']['principal'].get('config') for r in records),
            'configs': len(configs),
            'model_turns': len(turns), 'tool_calls': len(tools), 'lifecycles': len(lifecycles),
            'evidence_turns': sum(e['usage']['messages'] for e in evidence),
            'evidence_tool_calls': sum(len(e['tool_calls']) for e in evidence),
            'tool_durations': all(t['started_at_ms'] and t['ended_at_ms'] is not None
                                  and t['ended_at_ms'] >= t['started_at_ms'] for t in tools),
            'tool_call_ids': all(t['tool_call_id'] for t in tools),
            'turn_ids_and_times': all(t['message_id'] and t['observed_at_ms'] for t in turns),
            'tool_ms': [t['ended_at_ms'] - t['started_at_ms'] for t in tools],
            'failures': [t['failure'] for t in tools if not t['ok']],
            'lifecycle_ms': [l['ended_at_ms'] - l['started_at_ms'] for l in lifecycles],
            'outcomes': [l['outcome'] for l in lifecycles],
            'recording_ms': [r.get('elapsed_ms') for r in recordings],
            'recording_errors': [r.get('error') for r in recordings if r.get('error')],
            'forbidden_hits': [w for w in FORBIDDEN if w in raw],
        })
    (HERE / 'runs.jsonl').write_text(''.join(json.dumps(r) + '\n' for r in rows))
    recording = [ms for r in rows for ms in r['recording_ms'] if ms is not None]
    summary = {
        'runs': len(rows),
        'by_task': {t: sum(r['task'] == t for r in rows) for t in sorted({r['task'] for r in rows})},
        'checks': {k: all(r[k] for r in rows) for k in
                   ('all_accepted', 'stamped_time', 'stamped_config', 'tool_durations',
                    'tool_call_ids', 'turn_ids_and_times')},
        'facts_match_evidence': all(r['model_turns'] == r['evidence_turns']
                                    and r['tool_calls'] == r['evidence_tool_calls'] for r in rows),
        'one_record_and_one_config_per_invocation': all(
            r['records'] == r['lifecycles'] == 1 and r['configs'] == 1 for r in rows),
        'recording_ms': {'n': len(recording), 'median': statistics.median(recording),
                         'max': max(recording), 'mean': statistics.fmean(recording)},
        'recording_share_of_wall': statistics.fmean(
            sum(r['recording_ms']) / 1000 / r['wall_seconds'] for r in rows),
        'tool_ms': {'n': sum(len(r['tool_ms']) for r in rows),
                    'median': statistics.median([m for r in rows for m in r['tool_ms']])},
        'failures': {f: sum(r['failures'].count(f) for r in rows)
                     for f in sorted({f for r in rows for f in r['failures']})},
        'outcomes': {o: sum(r['outcomes'].count(o) for r in rows)
                     for o in sorted({o for r in rows for o in r['outcomes']})},
        'recording_errors': [e for r in rows for e in r['recording_errors']],
        'forbidden_hits': sorted({w for r in rows for w in r['forbidden_hits']}),
    }
    (HERE / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    print(json.dumps(summary, indent=2))


if __name__ == '__main__':
    main()
