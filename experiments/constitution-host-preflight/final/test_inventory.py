#!/usr/bin/env python3
"""Count source test functions from fixed Git trees; never execute tests.

Python 3.11+, Node, and the installed pinned apps/web TypeScript compiler. Counts physical declarations once, including
inline cfg(test) modules; compiler target expansion and parameter rows are not
expanded. Per-file counts and ignored/helper locations make the counting boundary auditable.
"""
from __future__ import annotations
import argparse
from collections import Counter
import json
from pathlib import Path
import re
import subprocess
import tomllib


def git(*args: str) -> str:
    return subprocess.check_output(['git', *args], text=True)


def mask(source: str, rust: bool) -> str:
    """Hide literal/comment contents, preserving offsets and newlines."""
    out = list(source)
    i = 0
    while i < len(source):
        start = i
        if source.startswith('//', i):
            end = source.find('\n', i)
            i = len(source) if end < 0 else end
        elif source.startswith('/*', i):
            i += 2
            depth = 1
            while i < len(source) and depth:
                if rust and source.startswith('/*', i):
                    depth += 1
                    i += 2
                elif source.startswith('*/', i):
                    depth -= 1
                    i += 2
                else:
                    i += 1
        else:
            raw = re.match(r'(?:br|cr|r)(#*)"', source[i:]) if rust else None
            if raw:
                closing = '"' + raw[1]
                end = source.find(closing, i + len(raw[0]))
                i = len(source) if end < 0 else end + len(closing)
            elif source[i] in ('"', chr(96)) or (source[i] == "'" and (not rust or re.match(r"'(?:[^'\\\n]|\\(?:[^\n]|u\{[0-9a-fA-F_]+\}))'", source[i:]))):
                quote = source[i]
                i += 1
                while i < len(source):
                    if source[i] == '\\':
                        i += 2
                    elif source[i] == quote:
                        i += 1
                        break
                    else:
                        i += 1
            else:
                i += 1
                continue
        for j in range(start, min(i, len(source))):
            if source[j] != '\n':
                out[j] = ' '
    return ''.join(out)


def close(text: str, start: int, opening: str, closing: str) -> int:
    depth = 0
    for i in range(start, len(text)):
        if text[i] == opening:
            depth += 1
        elif text[i] == closing:
            depth -= 1
            if not depth:
                return i + 1
    raise ValueError(f'unclosed {opening} at {start}')


def rust_tests(path: str, source: str) -> list[dict]:
    masked = mask(source, True)
    records, associated = [], set()
    known_pattern = r'#\s*\[\s*(?:test|tokio::test|rstest(?:::rstest)?|async_std::test)\b'
    pos = 0
    while found := re.search(r'#\s*\[', masked[pos:]):
        cursor = pos + found.start()
        attrs = []
        while attr := re.match(r'#\s*\[', masked[cursor:]):
            start = cursor
            opening = cursor + attr.end() - 1
            end = close(masked, opening, '[', ']')
            kind = re.match(r'\s*([\w:]+)', masked[opening + 1:end - 1])[1]
            attrs.append((start, kind, source[opening + 1:end - 1]))
            cursor = end
            cursor += re.match(r'\s*', masked[cursor:]).end()
        pos = cursor
        function = re.match(r'(?:(?:pub(?:\s*\([^)]*\))?|async|unsafe|const|extern)\s+)*fn\s+(\w+)', masked[cursor:])
        testing = [(start, kind) for start, kind, _ in attrs if kind in ('test', 'tokio::test', 'rstest', 'rstest::rstest', 'async_std::test')]
        if not testing or not function:
            continue
        associated.update(start for start, _ in testing)
        ignore = next((raw for _, kind, raw in attrs if kind == 'ignore'), None)
        name = function[1]
        records.append({'file': path, 'line': source.count('\n', 0, cursor) + 1, 'name': name,
                        'attributes': [kind for _, kind in testing], 'ignored': ignore is not None,
                        'ignore_attribute': ignore, 'goose_ignored': bool(ignore and 'goose' in ignore.lower()),
                        'helper': name.endswith('_process_helper') or name == 'https_client_subprocess',
                        'cfg': [raw.strip() for _, kind, raw in attrs if kind == 'cfg']})
    known = {m.start() for m in re.finditer(known_pattern, masked)}
    if known != associated:
        raise ValueError(f'{path}: unmatched test attributes at {sorted(known - associated)}')
    return records


def web_inventory(files: dict[str, str]) -> list[dict]:
    # Parse fixed Git content, not worktree TS/JS. Only the locally installed
    # pinned TypeScript compiler is used; no test module is imported/executed.
    program = r"""
const fs = require('fs');
const ts = require('./apps/web/node_modules/typescript');
const files = JSON.parse(fs.readFileSync(0, 'utf8'));
const output = [];
for (const [file, source] of Object.entries(files)) {
  const tree = ts.createSourceFile(file, source, ts.ScriptTarget.Latest, true);
  if (tree.parseDiagnostics.length) throw new Error(file + ': TypeScript parse diagnostics');
  const tests = [], conditional = [], suites = [];
  function chain(node) {
    if (ts.isIdentifier(node)) return [node.text];
    if (ts.isPropertyAccessExpression(node)) {
      const prior = chain(node.expression);
      return prior && prior.concat(node.name.text);
    }
    if (ts.isCallExpression(node)) return chain(node.expression);
    return null;
  }
  function visit(node) {
    if (ts.isCallExpression(node)) {
      const parts = chain(node.expression);
      if (parts && parts[0] === 'describe' && parts.includes('each') &&
          ts.isCallExpression(node.expression)) {
        let table = node.expression.arguments[0];
        while (table && (ts.isAsExpression(table) || ts.isParenthesizedExpression(table))) table = table.expression;
        suites.push({line: tree.getLineAndCharacterOfPosition(node.getStart(tree)).line + 1,
                     literal_parameter_rows: table && ts.isArrayLiteralExpression(table) ? table.elements.length : null});
      }
      if (parts && ['it', 'test'].includes(parts[0]) &&
          parts.slice(1).every(p => ['each', 'only', 'skip', 'todo', 'concurrent', 'fails', 'failing'].includes(p))) {
        const callback = node.arguments.find(a => ts.isArrowFunction(a) || ts.isFunctionExpression(a));
        const line = tree.getLineAndCharacterOfPosition(node.getStart(tree)).line + 1;
        if (callback) {
          let rows = null;
          if (parts.includes('each') && ts.isCallExpression(node.expression)) {
            let table = node.expression.arguments[0];
            while (table && (ts.isAsExpression(table) || ts.isParenthesizedExpression(table))) table = table.expression;
            if (table && ts.isArrayLiteralExpression(table)) rows = table.elements.length;
          }
          tests.push({line, kind: parts[0], modifiers: parts.slice(1), parameterized: parts.includes('each'), literal_parameter_rows: rows});
        } else if (parts.includes('skip')) {
          conditional.push(line);
        }
      }
    }
    ts.forEachChild(node, visit);
  }
  visit(tree);
  output.push({file, declarations: tests.length, parameterized_declarations: tests.filter(t => t.parameterized).length,
               conditional_skip_calls: conditional, parameterized_suites: suites, tests});
}
process.stdout.write(JSON.stringify(output));
"""
    return json.loads(subprocess.check_output(['node', '-e', program], input=json.dumps(files), text=True))


def inventory(ref: str) -> dict:
    resolved = git('rev-parse', ref).strip()
    paths = git('ls-tree', '-r', '--name-only', resolved).splitlines()
    members = tomllib.loads(git('show', f'{resolved}:rust/Cargo.toml'))['workspace']['members']
    records, crates, shared_goose_targets = [], [], []
    for member in members:
        own = []
        for path in paths:
            if path.startswith(f'rust/{member}/') and path.endswith('.rs'):
                source = git('show', f'{resolved}:{path}')
                own.extend(rust_tests(path, source))
                if re.match(r'rust/anchor-runner-host/tests/[^/]+\.rs$', path) and 'support/goose_fixture.rs' in source:
                    shared_goose_targets.append(path)
        records.extend(own)
        crates.append({'crate': member, 'functions': len(own), 'ignored': sum(r['ignored'] for r in own),
                       'goose_ignored': sum(r['goose_ignored'] for r in own), 'helpers': sum(r['helper'] for r in own)})
    web = web_inventory({p: git('show', f'{resolved}:{p}') for p in paths
                         if p.startswith('apps/web/') and re.search(r'\.(?:test|spec)\.[cm]?[jt]sx?$', p)})
    def web_part(prefix: str) -> dict:
        selected = [f for f in web if f['file'].startswith(prefix)]
        return {'files': len(selected), 'callback_declarations': sum(f['declarations'] for f in selected),
                'parameterized_declarations': sum(f['parameterized_declarations'] for f in selected),
                'conditional_skip_calls': sum(len(f['conditional_skip_calls']) for f in selected),
                'literal_parameter_rows': sum(t['literal_parameter_rows'] or 0 for f in selected for t in f['tests']),
                'parameterized_suites': sum(len(f['parameterized_suites']) for f in selected)}
    return {'revision': resolved, 'workspace_members': members,
            'rust': {'functions': len(records), 'ordinary_functions': sum(not r['ignored'] for r in records),
                     'ignored_functions': sum(r['ignored'] for r in records),
                     'goose_ignored_functions': sum(r['goose_ignored'] for r in records),
                     'other_ignored_functions': sum(r['ignored'] and not r['goose_ignored'] for r in records),
                     'test_attributed_helpers': [r for r in records if r['helper']],
                     'attribute_kinds': dict(Counter(k for r in records for k in r['attributes'])),
                     'cfg_gated_functions': [r for r in records if r['cfg']],
                     'by_crate': crates, 'shared_goose_fixture_direct_targets': shared_goose_targets, 'by_file': [{'file': path, 'functions': len(own), 'ignored_functions': sum(r['ignored'] for r in own)} for path in sorted({r['file'] for r in records}) for own in [[r for r in records if r['file'] == path]]], 'ignored_detail': [r for r in records if r['ignored']]},
            'web': {'unit': web_part('apps/web/src/'), 'browser': web_part('apps/web/e2e/'), 'files': web}}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--before', default='fc558e2')
    parser.add_argument('--after', default='e60374f')
    parser.add_argument('--output', type=Path, default=Path(__file__).with_name('test-counts.json'))
    args = parser.parse_args()
    import os
    os.chdir(git('rev-parse', '--show-toplevel').strip())
    result = {'method_version': 1, 'method': {
        'rust': 'Each physical fn with test/tokio::test/rstest/rstest::rstest/async_std::test attributes, once per workspace Git source path. Inline cfg(test) tests included. Doctests and ordinary helper fn excluded; target reuse and parameter instances not expanded. cfg_gated_functions records direct function attributes, not inherited module gates; all source cfg variants are counted.',
        'ignore': 'Explicit ignore on a test function; Goose subset requires Goose in the ignore reason. Other ignored tests and test-attributed helpers remain visible separately.',
        'web': 'Named it/test callbacks including each, once per physical source declaration. Configuration, hooks and conditional test.skip(condition, reason) excluded. Parameter rows and surrounding loops not expanded.',
        'input': 'Only git ls-tree/git show of resolved commits; neither worktree code nor runtime test results are input. Web parsing uses apps/web/node_modules/typescript (repository pinned compiler), never imports a test.'},
        'before': inventory(args.before), 'after': inventory(args.after)}
    args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + '\n')
    for key in ('before', 'after'):
        item = result[key]
        print(json.dumps({'side': key, 'revision': item['revision'],
                          'rust': {k: item['rust'][k] for k in ('functions', 'ordinary_functions', 'ignored_functions', 'goose_ignored_functions', 'other_ignored_functions')},
                          'web': {k: item['web'][k] for k in ('unit', 'browser')}}, ensure_ascii=False))


if __name__ == '__main__':
    main()
