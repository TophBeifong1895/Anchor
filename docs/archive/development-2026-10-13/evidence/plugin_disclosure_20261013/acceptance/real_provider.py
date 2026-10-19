#!/usr/bin/env python3
"""Isolated real-provider Plugin discovery acceptance; no deployment mutation."""
import hashlib
import json
import os
from pathlib import Path
import secrets
import shlex
import shutil
import signal
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request

REPO = Path(__file__).resolve().parents[2]
NONCE = secrets.token_hex(8)
OUTPUT = REPO / '.local/plugin-disclosure-acceptance-20261013'
OUTPUT.mkdir(parents=True, exist_ok=True)
ROOT = OUTPUT / ('real-' + NONCE)
ROOT.mkdir(mode=0o700)
PRIVATE = []
PLUGIN = 'zz-validation'
MARKER = 'validation-' + NONCE
TOKEN = secrets.token_hex(16)
EXPECTED = {'marker': MARKER, 'token': TOKEN, 'total': 42, 'format': 'json'}
REPORT = {'passed': False, 'root': str(ROOT), 'real_provider': True,
          'fixture_plugin_preseeded': True, 'plugin_installation_tested': False,
          'deployment_updated': False, 'business_api_calls': 0, 'plugin_count': 1000,
          'plugin_tool_count': 1001, 'pilot_mounted_plugin_count': 10,
          'node_mounted_plugin_count': 1000, 'expected': EXPECTED}

SERVER = r'''import json
import sys
from pathlib import Path

target = {'name': 'produce_report', 'description': '生成校验报告 validation report，核对 token、marker 和两笔金额后返回 JSON。先读取 Plugin 的校验 Skill 和完整参数定义。', 'inputSchema': {
    'type': 'object', 'properties': {
        'payload': {'type': 'object', 'properties': {'marker': {'type': 'string'}, 'token': {'type': 'string'}}, 'required': ['marker', 'token'], 'additionalProperties': False},
        'entries': {'type': 'array', 'minItems': 2, 'maxItems': 2, 'items': {'type': 'object', 'properties': {'amount': {'type': 'integer'}}, 'required': ['amount'], 'additionalProperties': False}},
        'format': {'type': 'string', 'enum': ['json']}}, 'required': ['payload', 'entries', 'format'], 'additionalProperties': False}}
noise = [{'name': f'noise_{i:04}', 'description': '其他无关能力。This operation is unrelated to validation reports. ' + 'metadata ' * 24, 'inputSchema': {'type': 'object', 'properties': {'value': {'type': 'string'}, 'mode': {'type': 'string', 'enum': ['a', 'b']}}, 'required': ['value'], 'additionalProperties': False}} for i in range(1000)]
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request:
        continue
    method = request.get('method')
    if method == 'initialize':
        result = {'protocolVersion': '2024-11-05', 'capabilities': {'tools': {}}, 'serverInfo': {'name': 'validation-fixture', 'version': '1'}}
    elif method == 'tools/list':
        result = {'tools': noise + [target]}
    elif method == 'tools/call':
        params = request['params']
        args = params.get('arguments', {})
        if params.get('name') != 'produce_report' or set(args) != {'payload', 'entries', 'format'} or set(args.get('payload', {})) != {'marker', 'token'} or args.get('format') != 'json' or len(args.get('entries', [])) != 2 or any(set(e) != {'amount'} or type(e['amount']) != int for e in args.get('entries', [])):
            result = {'isError': True, 'content': [{'type': 'text', 'text': 'Read the complete produce_report schema and correct the arguments.'}]}
        else:
            result_value = dict(args['payload'], total=sum(e['amount'] for e in args['entries']), format=args['format'])
            with Path('/workspace/plugin-calls.jsonl').open('a') as f:
                f.write(json.dumps({'tool': 'produce_report', 'arguments': args, 'result': result_value}) + '\n')
            result = {'content': [{'type': 'text', 'text': json.dumps(result_value)}], 'structuredContent': result_value}
    else:
        result = {}
    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)
'''


def redact(text):
    for value in PRIVATE:
        text = text.replace(value, '<redacted>')
    return text


def save(name, value):
    (ROOT / name).write_text(redact(json.dumps(value, indent=2, ensure_ascii=False)) + '\n')


def digest(path):
    with Path(path).open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def environment():
    allowed = {'ANCHOR_MODEL_URL', 'ANCHOR_MODEL_API_KEY', 'ANCHOR_MODEL_NAME',
               'ANCHOR_MODEL_WIRE_API', 'ANCHOR_MODEL_CONTEXT_WINDOW',
               'ANCHOR_MODEL_MAX_TOKENS', 'ANCHOR_MODEL_DIRECT', 'ANCHOR_MODEL_ALIASES',
               'ANCHOR_DEPENDENCIES_PYTHON'}
    result = {'PATH': '/usr/bin:/bin', 'LANG': 'C.UTF-8', 'TZ': 'UTC'}
    for line in (REPO / '.env').read_text().splitlines():
        line = line.strip()
        if not line or line.startswith('#') or '=' not in line:
            continue
        key, value = line.removeprefix('export ').split('=', 1)
        if key.strip() in allowed:
            values = shlex.split(value, comments=True)
            assert len(values) <= 1, 'Expected one environment value'
            result[key.strip()] = values[0] if values else ''
    for key in ('ANCHOR_MODEL_URL', 'ANCHOR_MODEL_API_KEY', 'ANCHOR_MODEL_NAME', 'ANCHOR_DEPENDENCIES_PYTHON'):
        assert result.get(key), 'Missing configured ' + key
    PRIVATE.append(result['ANCHOR_MODEL_API_KEY'])
    return result


def api(method, path, body=None):
    request = urllib.request.Request(REPORT['base_url'] + path, method=method,
        data=None if body is None else json.dumps(body).encode(),
        headers={'Content-Type': 'application/json', 'Authorization': 'Bearer ' + API_KEY})
    try:
        response = urllib.request.urlopen(request, timeout=15)
    except urllib.error.HTTPError as error:
        raise RuntimeError(f'{method} {path}: HTTP {error.code}: {redact(error.read().decode())}') from error
    with response:
        raw = response.read().decode()
        try:
            value = json.loads(raw)
        except ValueError:
            value = raw
        return response.status, value


def wait(read, done, description, timeout=360):
    deadline = time.monotonic() + timeout
    progress = time.monotonic()
    while True:
        value = read()
        if done(value):
            return value
        assert time.monotonic() < deadline, description + ' timed out'
        if time.monotonic() - progress > 20:
            print(description + ' still running', flush=True)
            progress = time.monotonic()
        time.sleep(0.5)


def events(stream):
    return [json.loads(line[6:]) for line in stream.splitlines() if line.startswith('data: ')]


def actual_calls(workspace):
    path = Path(workspace) / 'plugin-calls.jsonl'
    assert path.is_file(), 'No independent MCP call evidence'
    rows = [json.loads(line) for line in path.read_text().splitlines()]
    assert rows and all(row['result'] == EXPECTED for row in rows), rows
    return rows


def live_descendants(parent):
    rows = {}
    for entry in Path('/proc').iterdir():
        if not entry.name.isdigit():
            continue
        try:
            stat = (entry / 'stat').read_text().rsplit(')', 1)[1].split()
            rows[int(entry.name)] = (int(stat[1]), stat[0])
        except (OSError, ValueError, IndexError):
            pass
    known = {parent}
    while True:
        more = {pid for pid, (ppid, _) in rows.items() if ppid in known}
        if more <= known:
            return {pid for pid in known if pid in rows and rows[pid][1] != 'Z'}
        known |= more


API_KEY = secrets.token_hex(24)
PRIVATE.append(API_KEY)
process = None
log = None
sid = 'disclosure-' + NONCE
session_created = False
try:
    before_env = digest(REPO / '.env')
    bin_root = ROOT / 'bin'
    bin_root.mkdir()
    host = bin_root / 'anchor-runner-host'
    shutil.copy2(REPO / 'rust/target/debug/anchor-runner-host', host)
    goose = REPO / '.local/rust/bin/goose'
    assert digest(goose) == '71e76c412597b2ecd96ed20d0706e7666f31c018216e7cb5d65c5ca5c44824a7'
    REPORT.update(host_binary_sha256=digest(host), goose_binary_sha256=digest(goose))
    for name in ('state', 'catalog', 'workspaces', 'library', 'home'):
        (ROOT / name).mkdir()
    ids = []
    for index in range(999):
        identifier = f'noise-{index:04}'
        ids.append(identifier)
        p = ROOT / 'library/plugins' / identifier
        p.mkdir(parents=True)
        (p / 'plugin.json').write_text(json.dumps({'name': f'无关能力 {index}', 'description': '其他任务，不用于生成校验报告。'}))
        (p / 'instructions.md').write_text('This Plugin serves an unrelated task. ' * 20)
    ids.append(PLUGIN)
    plugin = ROOT / 'library/plugins' / PLUGIN
    (plugin / 'skills/validation').mkdir(parents=True)
    (plugin / 'skills/unrelated').mkdir(parents=True)
    (plugin / 'resources').mkdir()
    (plugin / 'plugin.json').write_text(json.dumps({'name': '校验报告', 'description': '验证报告 validation report，核对随机标记和两笔金额。',
        'skills': ['skills/validation', 'skills/unrelated'],
        'mcpServers': {'service': {'command': 'python3', 'args': [f'/plugins/{PLUGIN}/server.py']}}}, ensure_ascii=False))
    (plugin / 'instructions.md').write_text('生成校验报告时先读 skills/validation/SKILL.md。其他 Skill 与本任务无关。所需参数必须从工具详情读取。')
    (plugin / 'skills/validation/SKILL.md').write_text('---\nname: validation\ndescription: 生成校验报告，读取随机 token，使用 MCP 工具核对金额。\n---\n'
        '读取 resources/proof.json 获得 token 和 amounts。用 produce_report 工具，payload 包含任务给出的 marker 及资源里的 token；entries 对应每个 amount；format=json。报告必须使用真实工具返回值。')
    (plugin / 'skills/unrelated/SKILL.md').write_text('---\nname: unrelated\ndescription: Other tasks only.\n---\nUNRELATED_BODY_MUST_NOT_BE_LOADED_' + NONCE)
    (plugin / 'resources/proof.json').write_text(json.dumps({'token': TOKEN, 'amounts': [17, 25]}))
    (plugin / 'server.py').write_text(SERVER)
    env = environment()
    interpreter = Path(env['ANCHOR_DEPENDENCIES_PYTHON']).resolve()
    REPORT['shared_interpreter'] = str(interpreter)
    before_python = digest(interpreter)
    with socket.socket() as probe:
        probe.bind(('127.0.0.1', 0))
        port = probe.getsockname()[1]
    REPORT['base_url'] = f'http://127.0.0.1:{port}'
    REPORT['model'] = env['ANCHOR_MODEL_NAME']
    env['ANCHOR_MODEL_ALIASES'] = json.dumps({'models.worker': env['ANCHOR_MODEL_NAME']})
    env.update({'HOME': str(ROOT / 'home'), 'XDG_CONFIG_HOME': str(ROOT / 'home/config'),
        'XDG_DATA_HOME': str(ROOT / 'home/data'), 'XDG_STATE_HOME': str(ROOT / 'home/state'),
        'XDG_CACHE_HOME': str(ROOT / 'home/cache'), 'ANCHOR_API_KEYS': json.dumps([API_KEY]),
        'ANCHOR_RUNNER_STATE_ROOT': str(ROOT / 'state'), 'ANCHOR_RUNNER_WORKSPACE_ROOT': str(ROOT / 'workspaces'),
        'ANCHOR_RUNNER_CATALOG_ROOT': str(ROOT / 'catalog'), 'ANCHOR_RUNNER_LIBRARY_ROOT': str(ROOT / 'library'),
        'ANCHOR_RUNNER_LISTEN': f'127.0.0.1:{port}', 'ANCHOR_RUNNER_ALLOWED_COMMANDS': 'sh',
        'ANCHOR_GOOSE_BINARY': str(goose), 'ANCHOR_GOOSE_BINARY_SHA256': digest(goose),
        'ANCHOR_GOOSE_LOCAL_NETWORK': '1', 'ANCHOR_GOOSE_RELAY_BINARY': str(REPO / '.local/rust/bin/anchor-net-relay'),
        'ANCHOR_SANDBOX_NETWORK_BINARY': str(REPO / '.local/rust/bin/anchor-sandbox-network'),
        'ANCHOR_PILOT_PYTHON_ENABLED': '0', 'ANCHOR_NODE_TOOL_DISCLOSURE': '1'})
    log = (ROOT / 'host.log').open('wb')
    process = subprocess.Popen([str(host), 'serve'], env=env, cwd=ROOT, stdout=log, stderr=log, start_new_session=True)
    REPORT['host_pid'] = process.pid
    def ready():
        assert process.poll() is None, 'Host exited during startup'
        try:
            return api('GET', '/ready')
        except (urllib.error.URLError, RuntimeError):
            return 0, {}
    wait(ready, lambda value: value[0] == 200, 'Host ready', 30)
    status, _ = api('POST', '/sessions', {'id': sid})
    assert status == 201
    session_created = True
    for identifier in ids[:9] + [PLUGIN]:
        api('POST', f'/sessions/{sid}/plugins/{identifier}', {'on': True})
    prompt = ('请通过已挂载 Plugin 的能力发现入口找到适合生成校验报告的 Plugin，读取它的使用说明、正确的 Skill 和必要资源，'
        '并在实际调用前取得完整工具参数定义。任务 marker=' + MARKER + '。'
        '按说明调用真实 MCP 工具，最后给出返回的 JSON。不要用 shell 或自行计算来替代 Plugin 工具。')
    status, accepted = api('POST', f'/sessions/{sid}/turns', {'request_id': 'disclosure', 'message': prompt})
    assert status == 202
    tid = accepted['turn']['id']
    turn = wait(lambda: api('GET', f'/sessions/{sid}/turns/{tid}')[1]['turn'],
                lambda value: value['status'] not in ('pending', 'running'), 'Real Pilot disclosure')
    save('pilot-turn.json', turn)
    assert turn['status'] == 'completed', turn['status']
    _, stream = api('GET', f'/sessions/{sid}/turns/{tid}/events')
    rows = events(stream)
    save('pilot-events.json', rows)
    names = [row['toolName'] for row in rows if row.get('type') == 'tool-input-available']
    for name in ('anchor_plugins', 'anchor_plugin_read', 'anchor_tool_detail'):
        assert name in names, 'Pilot did not exercise ' + name
    assert any(name.endswith('_produce_report') for name in names), 'Pilot trace did not retain actual target'
    assert 'UNRELATED_BODY_MUST_NOT_BE_LOADED_' not in json.dumps(rows), 'Unrelated Skill loaded'
    binding = json.loads((ROOT / 'state/platform/pilot-resources' / (turn['goose']['scope'] + '.json')).read_text())
    REPORT['pilot_actual_calls'] = actual_calls(binding['workspace'])
    _, messages = api('GET', f'/sessions/{sid}/messages')
    save('pilot-messages.json', messages)
    assert TOKEN in json.dumps(messages), 'Pilot final conversation lacks actual resource token'
    graph = {'entry': 'worker', 'agents': {'worker': {'model': 'models.worker', 'writes': ['notes.md'],
        'instructions': prompt + ' 将真实工具返回的 JSON 保存到 notes.md，检查后用原生 final_result 单独提交完成。'}},
        'nodes': [{'id': 'worker', 'agent': 'worker', 'plugins': ids}], 'edges': []}
    status, _ = api('POST', '/graphs', {'name': 'disclosure-proof', 'definition': graph})
    assert status == 201
    status, accepted = api('POST', '/trigger', {'graph': 'disclosure-proof'})
    assert status == 202
    run = accepted['run']
    detail = wait(lambda: api('GET', f'/runs/{run}')[1], lambda value: not value['active'], 'Real Node disclosure')
    save('run.json', detail)
    assert detail['state']['status'] == 'completed', detail['state'].get('error')
    _, artifact = api('GET', f'/runs/{run}/files/worker/notes.md')
    save('artifact.json', artifact)
    text = artifact['text']
    assert MARKER in text and TOKEN in text and '42' in text, 'Artifact does not contain actual returned facts'
    paths = list((ROOT / 'state/goose-acp').glob('*.evidence.json'))
    assert len(paths) == 1
    native = json.loads(paths[0].read_text())
    save('node-native-evidence.json', native)
    calls = native['tool_calls']
    for name in ('anchor_plugins', 'anchor_plugin_read', 'anchor_tool_detail', 'final_result'):
        assert any(call['tool'] == name for call in calls), 'Node did not exercise ' + name
    actual = [call for call in calls if call['tool'].endswith('_produce_report')]
    assert actual and all(call['result']['ok'] is True for call in actual), 'Node target call observation missing'
    node_call_files = [path for path in (ROOT / 'workspaces').rglob('plugin-calls.jsonl')
                       if path.parent != Path(binding['workspace'])]
    assert len(node_call_files) == 1, 'Expected one independent Node MCP call log'
    REPORT['node_actual_calls'] = actual_calls(node_call_files[0].parent)
    REPORT.update(pilot_status=turn['status'], node_status=detail['state']['status'], node_tool_count=len(calls),
                  node_actual_target=actual, artifact_checked=True, original_env_unchanged=digest(REPO / '.env') == before_env,
                  shared_interpreter_unchanged=digest(interpreter) == before_python, passed=True)
except Exception as error:
    REPORT['error'] = redact(f'{type(error).__name__}: {error}')
finally:
    if session_created and process is not None and process.poll() is None:
        try:
            api('POST', f'/sessions/{sid}/stop', {})
            REPORT['session_delete_http'], _ = api('DELETE', f'/sessions/{sid}')
        except Exception as error:
            REPORT['cleanup_error'] = redact(str(error))
            REPORT['passed'] = False
    if process is not None:
        owned_children = live_descendants(process.pid)
        if process.poll() is None:
            process.send_signal(signal.SIGTERM)
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=5)
                REPORT['shutdown_forced'] = True
        REPORT['host_exit'] = process.returncode
        if log is not None:
            log.close()
        with socket.socket() as probe:
            REPORT['listener_closed'] = probe.connect_ex(('127.0.0.1', port)) != 0
        REPORT['host_log_contains_panic'] = 'panicked' in (ROOT / 'host.log').read_text(errors='replace')
        REPORT['remaining_child_pids'] = sorted(pid for pid in owned_children if pid != process.pid and Path(f'/proc/{pid}/stat').exists() and Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[0] != 'Z')
        REPORT['passed'] = REPORT['passed'] and process.returncode == 0 and REPORT['listener_closed'] and not REPORT['host_log_contains_panic'] and not REPORT['remaining_child_pids']
    save('evidence.json', REPORT)
    print(redact(json.dumps(REPORT, ensure_ascii=False)), flush=True)
sys.exit(0 if REPORT['passed'] else 1)
