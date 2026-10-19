#!/usr/bin/env python3
"""R2 census: reuse R's complete I/M/W envelopes, including moved rules.

Usage: python3 experiments/constitution-host-preflight/r2/measure.py COMMIT
WORKTREE is supported for drafts. No historical R output is overwritten.
"""
import argparse
import importlib.util
import json
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
sys.path.insert(0, str(HERE.parent / 'r'))
spec = importlib.util.spec_from_file_location('r_measure', HERE.parent / 'r/measure.py')
r = importlib.util.module_from_spec(spec)
spec.loader.exec_module(r)
BASE = r.BASE
R = '54e664e'
PATTERNS = r.p.PATTERNS
Census, span, merge = r.Census, r.span, r.merge

# R2 changes the JSON contract, not the operation envelope. In particular the
# Session callback moved from legacy_run_view back to conversations is retained.
# from_view is the new external serializer and belongs to projection, not free
# shared infrastructure. run_json remains an internal bridge and is counted.
SCOPES = {
    'run_projection': {r.V: ['project_run', 'from_view']},
    'liveness': {r.L: ['from_view', 'from_phase', 'observed', 'with_registry', 'is_quiescent']},
    'conversations': {r.C: ['settle_channel_turn', 'settle_channel_run',
                           'ensure_wait_closure_settled', 'successor_reads_predecessor',
                           'check_execution_scope', 'admit_conversation']},
    'kernel_runs_projection': {r.K: ['record', 'view'], r.JSON: None},
}
SUPPORT = {r.VIEW: None, r.LEGACY: ['phase', 'view'],
           r.Q: ['read_view', 'observed_liveness']}
R_SUPPORT = {r.VIEW: None, r.LEGACY: ['phase', 'view'],
             r.Q: ['read_view', 'observed_liveness']}


def resolve(revision):
    return revision if revision == 'WORKTREE' else subprocess.check_output(
        ['git', 'rev-parse', revision], cwd=ROOT, text=True).strip()


def owner(path, line, census):
    if path == r.C:
        return 'Session (including unchanged legacy write-back callback)'
    if path == r.V:
        a, b = span(census.source(path)[0], 'from_view') if census.revision != resolve(R) and census.revision != resolve(BASE) else (0, 0)
        offset = sum(len(row) for row in census.source(path)[0].splitlines(keepends=True)[:line - 1])
        return ('external Phase/Waiter/Outcome serialization' if a <= offset < b
                else 'Session/authorization and existing projection observation')
    if path == r.LEGACY:
        return 'legacy lifecycle mapping/compatibility (not Session authorization)'
    if path == r.JSON:
        return 'internal Kernel-to-legacy record bridge (not external JSON)'
    if path == r.K:
        return 'Kernel read projection/legacy bridge'
    if path == r.Q:
        return 'read ownership/Host observation (timestamp is not a control input)'
    if path == r.L:
        return 'Host worker observation and Phase quiescence'
    if path == r.VIEW:
        return 'shared Phase-derived view/actions'
    return 'separate implementation cost'


def measured(census, scopes, added=None):
    row = census.count(scopes)
    if added is not None:
        row['added_glue_lines'] = len(set(map(tuple, row['code_locations'])) & added)
    for hit in row['hits']:
        hit['owner'] = owner(hit['file'], hit['line'], census)
    return row


def optional_cost(census, path, names):
    try:
        return measured(census, {path: names})
    except AssertionError as error:
        # A deleted helper has zero cost, not an omitted hidden scope. The main
        # scopes never tolerate a missing function.
        return {'points': 0, 'counts': dict.fromkeys(PATTERNS, 0),
                'code_lines': 0, 'hits': [], 'absent': str(error)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('revision', help='source commit or WORKTREE')
    parser.add_argument('--output', type=Path, default=HERE / 'counts-draft.json')
    args = parser.parse_args()
    revisions = {'baseline': resolve(BASE), 'r': resolve(R), 'r2': resolve(args.revision)}
    censuses = {key: Census(revision) for key, revision in revisions.items()}
    scopes = {
        'baseline': {key: value['old'] for key, value in r.SCOPES.items()},
        'r': {key: value['new'] for key, value in r.SCOPES.items()},
        'r2': SCOPES,
    }
    supports = {'baseline': {}, 'r': R_SUPPORT, 'r2': SUPPORT}
    selected = {key: merge(*rows.values(), supports[key]) for key, rows in scopes.items()}
    added = r.additions(revisions['r2'], selected['r2'])
    result = {'baseline': BASE, 'revisions': revisions,
              'method': 'Identical R/P I + M + W patterns; complete operation envelopes; moved rules included; deduplicated union',
              'patterns': PATTERNS, 'consumers': {}, 'shared_support': {}, 'total': {}}
    for name in SCOPES:
        result['consumers'][name] = {
            version: measured(censuses[version], scopes[version][name], added if version == 'r2' else None)
            for version in revisions}
    for name, expected in {'run_projection': (13, 13), 'liveness': (0, 0),
                           'conversations': (59, 58), 'kernel_runs_projection': (10, 10)}.items():
        assert tuple(result['consumers'][name][version]['points'] for version in ['baseline', 'r']) == expected
    for version in revisions:
        result['shared_support'][version] = measured(censuses[version], supports[version], added if version == 'r2' else None)
        result['total'][version] = measured(censuses[version], selected[version], added if version == 'r2' else None)
    assert result['total']['baseline']['points'] == 82
    assert result['total']['r']['points'] == 90
    new = censuses['r2']
    result['separate_costs'] = {
        'legacy_adapter': measured(new, {r.LEGACY: None}),
        'internal_json_record_bridge': measured(new, {r.JSON: None}),
        'external_serializer': measured(new, {r.V: ['from_view']}),
        'host_time_file': measured(new, {r.TIME: None}),
        'host_time_query': optional_cost(new, r.Q, ['run_updated']),
        'scheduler_observation_hook': measured(new, {r.S: ['submit_with_retry']}),
    }
    result['status_inventory'] = {version: r.status_inventory(revision)
                                  for version, revision in revisions.items()}
    result['limits'] = [
        'I/M/W is syntax, not semantic complexity: boolean expressions and matches! contribute no point.',
        'External vocabulary serialization and the still-needed internal record bridge are both counted.',
        'Existing Session callback RunStatus rules stay in the full conversations envelope.',
        'The cfg(test) observed helper is excluded; it is not a second production consumer.',
        'The separate costs overlap the consumer union and must not be added to it again.',
    ]
    args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')
    for name, rows in result['consumers'].items():
        print(name, ' / '.join(f'{version}={row["points"]} {row["counts"]}' for version, row in rows.items()))
    print('shared_support', {key: row['points'] for key, row in result['shared_support'].items()})
    print('total', {key: (row['points'], row['code_lines'], row.get('added_glue_lines')) for key, row in result['total'].items()})
    for version, files in result['status_inventory'].items():
        print('RunStatus', version, sum(row['occurrences'] for row in files.values()), 'in', len(files), 'files')
    print('output', args.output.relative_to(ROOT) if args.output.is_relative_to(ROOT) else args.output)


if __name__ == '__main__':
    main()
