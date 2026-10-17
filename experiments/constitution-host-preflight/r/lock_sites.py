#!/usr/bin/env python3
"""Production file-lock sites, excluding test modules, comments and literals."""
import importlib.util
import json
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location(
    'p_measure', ROOT / 'experiments/constitution-host-preflight/p/measure.py')
p = importlib.util.module_from_spec(spec)
spec.loader.exec_module(p)
BASE = '1d1ba54'


def production(text, literals=True):
    masked = p.mask_source(text)
    chars = list(p.mask_source(text, literals))
    for hit in re.finditer(r'#\[cfg\(test\)\]', masked):
        start = hit.start()
        a, b = masked.find('{', hit.end()), masked.find(';', hit.end())
        if a < 0 or 0 <= b < a:
            end = b + 1
        else:
            depth, end = 1, a + 1
            while depth and end < len(masked):
                depth += (masked[end] == '{') - (masked[end] == '}')
                end += 1
        for i in range(start, end):
            if chars[i] != '\n':
                chars[i] = ' '
    return ''.join(chars)


def paths(revision):
    tracked = subprocess.check_output(
        ['git', 'ls-tree', '-r', '--name-only',
         'HEAD' if revision == 'WORKTREE' else revision, 'rust'], cwd=ROOT, text=True).splitlines()
    if revision == 'WORKTREE':
        tracked += subprocess.check_output(
            ['git', 'ls-files', '--others', '--exclude-standard', 'rust'],
            cwd=ROOT, text=True).splitlines()
    return sorted(set(tracked))


def read(revision, path):
    if revision == 'WORKTREE':
        return (ROOT / path).read_text()
    return subprocess.check_output(['git', 'show', f'{revision}:{path}'], cwd=ROOT, text=True)


def is_rust_source(path):
    return (path.endswith('.rs') and '/tests/' not in path
            and not re.search(r'(?:^|/|_)tests\.rs$', path))


def scan(revision):
    found = []
    for path in paths(revision):
        if not is_rust_source(path):
            continue
        text = read(revision, path)
        masked = production(text)
        patterns = [r'\bflock\s*\(', r'\b(?:file|lock|held|lease)\s*\.\s*try_lock\(\)',
                    r'\bfile\.lock\(\)', r'\b(?:file|lock|self\._file)\.unlock\(\)']
        for hit in re.finditer('|'.join(patterns), masked):
            # Capture only the invocation, not an enclosing match/closure body.
            start = hit.start()
            depth, end = 1, masked.index('(', start) + 1
            while depth:
                depth += (masked[end] == '(') - (masked[end] == ')')
                end += 1
            source = text[start:end]
            found.append(dict(file=path, line=masked[:start].count('\n') + 1,
                              release='Unlock' in source or '.unlock()' in source, source=source))
    return found


if __name__ == '__main__':
    import sys
    after = sys.argv[1] if len(sys.argv) > 1 else 'HEAD'
    result = dict(baseline=BASE, implementation=after, before=scan(BASE), after=scan(after))
    callers = []
    for path in paths(after):
        if not is_rust_source(path) or path == 'rust/anchor-file-lease/src/lib.rs':
            continue
        text = read(after, path)
        masked = production(text)
        for hit in re.finditer(r'\bFileLease::(?:new|acquire)\s*\(', masked):
            line = masked[:hit.start()].count('\n') + 1
            callers.append(dict(file=path, line=line, source=text.splitlines()[line - 1].strip()))
    result['after_callers'] = callers
    # This lock is created and owned inside an executed shell subshell, rather
    # than by a Rust File passed to a child. Keep its separate lifecycle.
    shell_path = 'rust/anchor-runner-host/src/tool_environment.rs'
    shell_source = read(after, shell_path)
    shell_lock = '/usr/bin/flock -x 9'
    assert shell_source.count(shell_lock) == 1
    result['retained_embedded_shell_locks'] = [dict(
        file=shell_path, line=shell_source[:shell_source.index(shell_lock)].count('\n') + 1,
        source=shell_lock,
        reason='Sandbox Cargo cache seeding; fd 9 is opened/locked by the subshell, not held by the Rust Host.')]
    for name in ['before', 'after']:
        sites = result[name]
        print(name, 'acquisition', sum(not s['release'] for s in sites),
              'release', sum(s['release'] for s in sites))
    (Path(__file__).parent / 'lock-sites.json').write_text(json.dumps(result, indent=2) + '\n')
