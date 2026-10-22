# Shared by every Op of the RSI Graph; build.py prepends it to each one.
# Inside the sandbox upstream artifacts are under /in and operator grants
# under /local-inputs; tests point RSI_IN and RSI_LOCAL elsewhere.
import json
import os
import subprocess
import tempfile
from pathlib import Path

IN = Path(os.environ.get('RSI_IN', '/in'))
LOCAL = Path(os.environ.get('RSI_LOCAL', '/local-inputs'))
HUB = str(LOCAL / 'hub-bin' / 'anchor-hub')
DATA = str(LOCAL / 'harness-hub-data')


def tool(name, args=None):
    """One read of the data hub, the same an agent makes over MCP."""
    with tempfile.NamedTemporaryFile('w', suffix='.json', delete=False) as stream:
        json.dump(args or {}, stream)
    try:
        done = subprocess.run([HUB, 'tool', name, '--args', '@' + stream.name, DATA],
                              capture_output=True, text=True)
    finally:
        os.unlink(stream.name)
    if done.returncode != 0:
        raise SystemExit(f'anchor-hub tool {name}: {done.stderr.strip()}')
    return json.loads(done.stdout)


def file_check(spec):
    """A check on source: `file` contains `contains`, within `lines` [a, b]
    (1-based, inclusive) when given."""
    path = Path(str(spec['file']))
    if not str(path).startswith(str(LOCAL)) or '..' in path.parts:
        return {'status': 'error', 'actual': None, 'message': 'file must be under /local-inputs'}
    try:
        text = path.read_text(errors='replace').splitlines()
    except OSError as error:
        return {'status': 'error', 'actual': None, 'message': str(error)}
    lines = spec.get('lines')
    if isinstance(lines, list) and len(lines) == 2:
        first, last = int(lines[0]), int(lines[1])
        text = text[max(first - 1, 0):last]
    wanted = str(spec.get('contains', ''))
    found = any(wanted in line for line in text) or wanted in '\n'.join(text)
    return {'status': 'pass' if found and wanted else 'fail', 'actual': found}


def run_checks(specs):
    """Results of `specs` in order: hub checks in batches, file checks here."""
    results = [None] * len(specs)
    hub = [(i, s) for i, s in enumerate(specs) if isinstance(s, dict) and 'file' not in s]
    for i, spec in enumerate(specs):
        if not isinstance(spec, dict):
            results[i] = {'status': 'error', 'actual': None, 'message': 'a check is an object'}
        elif 'file' in spec:
            results[i] = file_check(spec)
    for start in range(0, len(hub), 200):
        batch = hub[start:start + 200]
        answers = tool('check', {'checks': [s for _, s in batch]})['results']
        for (i, _), answer in zip(batch, answers):
            results[i] = answer
    return results


def load(path):
    """A JSON input, or None with the reason when it is missing or unreadable."""
    try:
        return json.loads(Path(path).read_text()), None
    except FileNotFoundError:
        return None, f'{path} is missing'
    except (OSError, ValueError) as error:
        return None, f'{path} is unreadable: {error}'


def save(name, value):
    Path(name).write_text(json.dumps(value, ensure_ascii=False, indent=1) + '\n')


def cut(text, n=160):
    text = ' '.join(str(text).split())
    return text if len(text) <= n else text[:n - 1] + '…'


def cell(text, n=160):
    return cut(text, n).replace('|', '\\|')
