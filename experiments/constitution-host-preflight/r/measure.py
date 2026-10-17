#!/usr/bin/env python3
"""R census: P's I/M/W points with explicit function scopes and moved rules.

Run with a committed source revision. WORKTREE is available for drafting only.
Each hit records its exact source location; comments, literals and tests do not
contribute. Boolean expressions without if/=>/field writes do not count, just
as in P. RunStatus references are a separate, non-semantic inventory.
"""
import argparse
import bisect
import json
import re
import subprocess
from pathlib import Path

from lock_sites import BASE, ROOT, is_rust_source, p, paths, production, read

H = 'rust/anchor-runner-host/src/'
A = H + 'application/'
V, L, C, K = [A + f + '.rs' for f in
              ['run_projection', 'liveness', 'conversations', 'kernel_runs']]
LEGACY, JSON, VIEW, TIME = [A + f + '.rs' for f in
                          ['legacy_run_view', 'run_json', 'run_view', 'run_observations']]
Q = A + 'queries.rs'
S = 'rust/anchor-runtime/src/graph/scheduler.rs'

# Whole behavior envelopes, including unchanged authorization/Session guards.
# These are intentionally not a census of raw enum identifiers.
SCOPES = {
    'run_projection': {
        'old': {V: ['project_run', 'observed']},
        'new': {V: ['project_run', 'observed'], LEGACY: ['projection']},
    },
    'liveness': {
        'old': {L: ['observed', 'with_registry', 'is_quiescent']},
        'new': {L: ['from_view', 'is_quiescent'], LEGACY: ['quiescent', 'observed', 'with_registry']},
    },
    'conversations': {
        'old': {C: ['settle_channel_run', 'ensure_wait_closure_settled',
                    'successor_reads_predecessor', 'check_execution_scope', 'admit_conversation']},
        'new': {C: ['settle_channel_turn', 'ensure_wait_closure_settled',
                    'successor_reads_predecessor', 'check_execution_scope', 'admit_conversation'],
                LEGACY: ['settle_channel_run']},
    },
    'kernel_runs_projection': {
        'old': {K: ['record', 'projection']},
        'new': {K: ['record', 'view', 'projection'], JSON: None},
    },
}


def span(text, name):
    masked = production(text)
    matches = list(re.finditer(r'\bfn\s+' + re.escape(name) + r'\s*(?:<[^\n]*?>)?\s*\(', masked))
    assert len(matches) == 1, (name, len(matches))
    start = masked.rfind('\n', 0, matches[0].start()) + 1
    body = masked.index('{', matches[0].end())
    depth, end = 1, body + 1
    while depth:
        depth += (masked[end] == '{') - (masked[end] == '}')
        end += 1
    return start, end


class Census:
    def __init__(self, revision):
        self.revision = revision
        self.cache = {}

    def source(self, path):
        if path not in self.cache:
            text = read(self.revision, path)
            self.cache[path] = (text, production(text), production(text, False))
        return self.cache[path]

    def ranges(self, scopes):
        result = {}
        for path, names in scopes.items():
            text, masked, _ = self.source(path)
            ranges = [(0, len(masked))] if names is None else [span(text, name) for name in names]
            result[path] = ranges
        return result

    def count(self, scopes):
        counts = dict.fromkeys(p.PATTERNS, 0)
        hits, locations, code_lines = [], [], []
        for path, ranges in self.ranges(scopes).items():
            text, masked, code = self.source(path)
            originals, code_rows = text.splitlines(), code.splitlines()
            starts = [0] + [m.end() for m in re.finditer('\n', masked)]
            selected = set()
            for a, b in ranges:
                first, last = bisect.bisect_right(starts, a), bisect.bisect_right(starts, b - 1)
                selected.update(range(first, last + 1))
                locations.append(dict(file=path, first=first, last=last))
            code_lines += [(path, n) for n in sorted(selected) if code_rows[n - 1].strip()]
            for kind, pattern in p.PATTERNS.items():
                for hit in re.finditer(pattern, masked):
                    line = bisect.bisect_right(starts, hit.start())
                    if line in selected:
                        counts[kind] += 1
                        hits.append(dict(file=path, line=line, kind=kind,
                                         column=hit.start()-starts[line-1]+1,
                                         source=originals[line-1].strip()))
        return dict(points=sum(counts.values()), counts=counts, code_lines=len(code_lines),
                    code_locations=code_lines, scopes=locations, hits=hits)


def additions(revision, selected_paths):
    result = set()
    for path in selected_paths:
        exists = subprocess.run(['git', 'cat-file', '-e', f'{BASE}:{path}'], cwd=ROOT,
                                stderr=subprocess.DEVNULL).returncode == 0
        if not exists:
            result.update((path, i) for i in range(1, len(read(revision, path).splitlines()) + 1))
            continue
        cmd = ['git', 'diff', BASE]
        if revision != 'WORKTREE':
            cmd.append(revision)
        diff = subprocess.check_output(cmd + ['--unified=0', '--', path], cwd=ROOT, text=True)
        line = 0
        for row in diff.splitlines():
            if row.startswith('@@'):
                line = int(re.search(r'\+(\d+)', row).group(1))
            elif row.startswith(('+++', '---')):
                continue
            elif row.startswith('+'):
                result.add((path, line))
                line += 1
            elif not row.startswith(('-', '\\')):
                line += 1
    return result


def merge(*scopes):
    result = {}
    for scope in scopes:
        for path, names in scope.items():
            if path in result and (result[path] is None or names is None):
                result[path] = None
            elif path in result:
                result[path] = list(dict.fromkeys(result[path] + names))
            else:
                result[path] = names
    return result


def status_inventory(revision):
    found = {}
    for path in paths(revision):
        if not path.startswith(H) or not is_rust_source(path):
            continue
        text = read(revision, path)
        masked = production(text)
        hits = [dict(line=masked[:m.start()].count('\n') + 1,
                     source=text.splitlines()[masked[:m.start()].count('\n')].strip())
                for m in re.finditer(r'\bRunStatus\b', masked)]
        if hits:
            found[path] = dict(occurrences=len(hits), lines=len({h['line'] for h in hits}), hits=hits)
    return found


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('revision', help='source commit or WORKTREE')
    args = parser.parse_args()
    revision = (args.revision if args.revision == 'WORKTREE' else
                subprocess.check_output(['git', 'rev-parse', args.revision], cwd=ROOT, text=True).strip())
    old, new = Census(BASE), Census(revision)
    support = {VIEW: None, LEGACY: ['phase', 'view'], Q: ['read_view', 'observed_liveness']}
    selected_new = merge(*(s['new'] for s in SCOPES.values()), support)
    added = additions(revision, selected_new)

    def measured(census, scopes):
        r = census.count(scopes)
        if census is new:
            r['added_glue_lines'] = len(set(map(tuple, r['code_locations'])) & added)
        return r

    result = dict(baseline=BASE, implementation=revision, method='P: I + M + W; moved rules included',
                  consumers={})
    for name, scopes in SCOPES.items():
        direct = {path: names for path, names in scopes['new'].items() if path in scopes['old']}
        result['consumers'][name] = dict(old=measured(old, scopes['old']),
                                         consumer_only=measured(new, direct),
                                         new=measured(new, scopes['new']))
    result['shared_support'] = measured(new, support)
    result['total'] = dict(old=measured(old, merge(*(s['old'] for s in SCOPES.values()))),
                           new=measured(new, selected_new))
    result['separate_costs'] = {
        'legacy_adapter': new.count({LEGACY: None}),
        'json_boundary': new.count({JSON: None}),
        'host_time_file': new.count({TIME: None}),
        'host_time_query': new.count({Q: ['run_updated']}),
        'scheduler_observation_hook': new.count({S: ['submit_with_retry']}),
    }
    result['status_inventory'] = dict(old=status_inventory(BASE), new=status_inventory(revision))
    for name, row in result['consumers'].items():
        print(name, row['old']['counts'], '->', row['new']['counts'], 'glue', row['new']['added_glue_lines'])
    print('support', result['shared_support']['counts'])
    print('total', {k: (r['points'], r['code_lines'], r.get('added_glue_lines')) for k, r in result['total'].items()})
    for name, row in result['separate_costs'].items():
        print(name, row['code_lines'], 'lines', row['points'], 'points')
    for version, files in result['status_inventory'].items():
        print('RunStatus', version, sum(f['occurrences'] for f in files.values()), 'in', len(files), 'files')
    (Path(__file__).parent / 'counts.json').write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')


if __name__ == '__main__':
    main()
