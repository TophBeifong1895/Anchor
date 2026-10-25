#!/usr/bin/env python3
"""Verify H2 is a source move, preserving metadata, Session DTOs and path identity."""
from pathlib import Path
import json
import subprocess
import textwrap

ROOT = Path(__file__).resolve().parents[3]
SRC = 'rust/anchor-runner-host/src/'
BASE = '6d31798'


def before(path):
    return subprocess.check_output(['git', 'show', f'{BASE}:{SRC}{path}'], cwd=ROOT, text=True)


def formatted(source):
    return subprocess.check_output(['rustup', 'run', 'stable', 'rustfmt', '--edition', '2024', '--emit', 'stdout'], input=source, text=True)


old = before('application/metadata.rs')
new = (ROOT / SRC / 'run_metadata.rs').read_text()
identity = before('application.rs').split('    pub(crate) fn graph_identity(', 1)[1].split('    fn graph_lease(', 1)[0]
identity = textwrap.dedent('    pub(crate) fn graph_identity(' + identity)
session = before('application/session_calls.rs')
dtos = session[session.index('#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]'):session.index('pub(crate) async fn resolve')]
error = '''#[derive(Debug)]
pub(crate) enum MetadataError {
    Conflict(String),
    Invalid(String),
    Storage(String),
}

'''
helper = before('application.rs').split('fn storage(', 1)[1].split('#[cfg(test)]', 1)[0]
helper = ('fn storage(' + helper).replace('ApplicationError', 'MetadataError')
# Only the import owner, error owner and required visibility may change.
expected = old.replace('use super::{ApplicationError, storage};\n', '')
expected = expected.replace('use serde::{Deserialize, Serialize};', 'use serde::{Deserialize, Serialize};\nuse serde_json::Value;')
expected = expected.replace('use std::{\n', 'use std::{\n    collections::VecDeque,\n')
expected = expected.replace('ApplicationError', 'MetadataError').replace('pub(super) fn apply', 'pub(crate) fn apply')
expected = expected.replace('Option<super::session_calls::SessionCall>', 'Option<SessionCall>')
expected = expected.replace('#[derive(Debug, Clone, Copy, Default, Deserialize)]', error + helper + '#[derive(Debug, Clone, Copy, Default, Deserialize)]')
expected = expected.replace('pub(crate) fn now_nanos()', dtos + identity.replace('ApplicationError', 'MetadataError') + 'pub(crate) fn now_nanos()')
actual = new.split('\n', 1)[1]  # module documentation is new
assert formatted(expected) == formatted(actual), 'metadata changed beyond declared move substitutions'
assert 'crate::application' not in new
assert not (ROOT / SRC / 'application/metadata.rs').exists()
assert 'fn graph_identity' not in (ROOT / SRC / 'application.rs').read_text()
assert 'struct SessionCall' not in (ROOT / SRC / 'application/session_calls.rs').read_text()
report = {'baseline': BASE, 'metadata_and_storage_bodies': 'unchanged after owner/type/visibility substitutions', 'session_dtos': 'unchanged', 'graph_identity': 'unchanged after error owner substitution', 'lower_to_application_references': 0}

# Assertion contents may only change where the module owner changed. Ignore
# formatting outside literals; retain every literal byte in the comparison.
import importlib.util
import re
spec = importlib.util.spec_from_file_location('measure_lines', ROOT / 'experiments/constitution-host-preflight/final/measure_lines.py')
measure = importlib.util.module_from_spec(spec)
spec.loader.exec_module(measure)


def assertions(source):
    masked = measure.mask(source)
    result = []
    for match in re.finditer(r'\b(?:debug_)?assert(?:_eq|_ne)?\s*!\s*([({\[])', masked):
        end = match.end()
        depth = 1
        opener = match[1]
        closer = {'(': ')', '{': '}', '[': ']'}[opener]
        while depth:
            if masked[end] == opener:
                depth += 1
            elif masked[end] == closer:
                depth -= 1
            end += 1
        text = source[match.start():end]
        text = text.replace('crate::application::RunApplication::graph_identity', 'crate::run_metadata::graph_identity')
        text = text.replace('Self::graph_identity', 'crate::run_metadata::graph_identity')
        text = text.replace('crate::application::metadata', 'crate::run_metadata')
        for name in ('RunMetadata', 'ConversationSource', 'RunTrigger'):
            text = text.replace('crate::application::' + name, 'crate::run_metadata::' + name)
        masked_text = measure.mask(text)
        # rustfmt can insert/remove whitespace and trailing commas, but no
        # assertion literal or expression token is allowed to change.
        result.append(''.join(c for i, c in enumerate(text) if (not c.isspace() and c != ',') or masked_text[i] != c))
    return result


changed = subprocess.check_output(['git', 'diff', '--name-only', BASE, '--', SRC], cwd=ROOT, text=True).splitlines()
assertion_count = 0
for path in changed:
    current = ROOT / path
    if not current.exists() or path == SRC + 'run_metadata.rs':
        continue
    original = subprocess.check_output(['git', 'show', f'{BASE}:{path}'], cwd=ROOT, text=True)
    old_assertions = assertions(original)
    assert old_assertions == assertions(current.read_text()), f'assertions changed: {path}'
    assertion_count += len(old_assertions)
report.update(existing_assertions_checked=assertion_count, assertion_changes='module owner substitutions and formatting only')
print(json.dumps(report, indent=2))
