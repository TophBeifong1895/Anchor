#!/usr/bin/env python3
"""Real model checks for continuous capability context and in-session mount updates.

A transparent recording proxy forwards model requests/responses unchanged. Its
one-request barrier permits a mount click while a real Pilot Turn is running.
No deployment, shared environment or external business system is modified.
"""
import hashlib
import json
import os
from pathlib import Path
import secrets
import shlex
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import time
import threading
import traceback
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import urllib.error
import urllib.request

REPO = Path(__file__).resolve().parents[2]
assert sys.argv[1:] in ([], ['--node-only']), 'Usage: real_provider.py [--node-only]'
NODE_ONLY = sys.argv[1:] == ['--node-only']
NONCE = secrets.token_hex(8)
OUTPUT = REPO / '.local/plugin-capability-context-20261013'
OUTPUT.mkdir(parents=True, exist_ok=True)
ROOT = OUTPUT / ('real-' + NONCE)
ROOT.mkdir(mode=0o700)
PRIVATE = []
PLUGIN = 'zz-lens'
REPLACEMENT = 'zz-prism'
MARKER = 'report-' + NONCE
TOKEN = secrets.token_hex(16)
SECOND_TOKEN = secrets.token_hex(16)
EXPECTED = {'marker': MARKER, 'token': TOKEN, 'total': 42, 'format': 'json'}
SECOND_EXPECTED = {'marker': 'next-' + MARKER, 'token': SECOND_TOKEN, 'total': 42, 'format': 'json'}
REPORT = {'passed': False, 'root': str(ROOT), 'real_provider': True,
          'provider_response_replaced': False, 'real_provider_compaction_tested': False,
          'fixture_plugin_preseeded': True, 'plugin_installation_tested': False,
          'deployment_updated': False, 'business_api_calls': 0, 'plugin_count': 1000,
          'plugin_tool_count': 1001, 'pilot_mounted_plugin_count': 0 if NODE_ONLY else 10,
          'scope': 'node-only' if NODE_ONLY else 'pilot-and-node',
          'node_mounted_plugin_count': 999, 'user_names_plugin': False}
CURRENT_HEADING = 'Current Anchor Plugin capabilities for this invocation.'
STAGE = 'startup'
GATE_ARMED = False
GATE_ENTERED = threading.Event()
GATE_RELEASE = threading.Event()
MODEL_REQUESTS = []
MODEL_LOCK = threading.Lock()
REMOTE_BASE = None
PROXY_ERRORS = []

class RecordingProxy(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def do_POST(self):
        global GATE_ARMED
        payload = self.rfile.read(int(self.headers['Content-Length']))
        value = json.loads(payload)
        with MODEL_LOCK:
            index = len(MODEL_REQUESTS)
            MODEL_REQUESTS.append({'stage': STAGE, 'body': value})
            hold = GATE_ARMED
            if hold:
                GATE_ARMED = False
        if hold:
            GATE_ENTERED.set()
            if not GATE_RELEASE.wait(45):
                PROXY_ERRORS.append('mount barrier timed out')
                self.send_error(504)
                return
        url = REMOTE_BASE.rstrip('/') + self.path.removeprefix('/v1')
        headers = {'Content-Type': 'application/json'}
        if self.headers.get('Authorization'):
            headers['Authorization'] = self.headers['Authorization']
        request = urllib.request.Request(url, data=payload, headers=headers, method='POST')
        try:
            response = urllib.request.urlopen(request, timeout=180)
        except urllib.error.HTTPError as error:
            response = error
        except Exception as error:
            PROXY_ERRORS.append(redact(str(error)))
            self.send_error(502)
            return
        with response:
            body = response.read()
            with MODEL_LOCK:
                MODEL_REQUESTS[index]['status'] = response.status
                # Response is recorded to verify actual model usage, never mutated.
                MODEL_REQUESTS[index]['response'] = body.decode(errors='replace')
            self.send_response(response.status)
            self.send_header('Content-Type', response.headers.get('Content-Type', 'application/json'))
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            try:
                self.wfile.write(body)
            except (BrokenPipeError, ConnectionResetError):
                pass


def system_context(request):
    blocks = []
    for message in request['body'].get('messages', []):
        if message.get('role') not in ('system', 'developer'):
            continue
        content = message.get('content')
        if isinstance(content, str):
            blocks.append(content)
        elif isinstance(content, list):
            blocks.extend(block.get('text', '') for block in content if isinstance(block, dict))
    return '\n'.join(blocks)


def capability_rows(system):
    found = []
    for line in system.splitlines():
        if not line.startswith('{'):
            continue
        try:
            item = json.loads(line)
        except ValueError:
            continue
        if 'read_with' in item or 'byte_budget' in item:
            found.append(item)
    return found


def is_summary_request(request):
    return not request['body'].get('tools') and any(
        message.get('role') == 'user' and message.get('content') ==
        'Please summarize the conversation history provided in the system prompt.'
        for message in request['body'].get('messages', []))


def regular_requests(stage):
    return [request for request in MODEL_REQUESTS
            if request['stage'] == stage and not is_summary_request(request)]


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



def turn_result(tid, label):
    value = wait(lambda: api('GET', f'/sessions/{sid}/turns/{tid}')[1],
                 lambda value: value['turn']['status'] not in ('pending', 'running'), label)
    assert value['turn']['status'] == 'completed', value
    return value


def submit(request_id, prompt):
    assert PLUGIN not in prompt and REPLACEMENT not in prompt
    assert 'anchor_plugin' not in prompt and 'Plugin' not in prompt
    status, value = api('POST', f'/sessions/{sid}/turns', {'request_id': request_id, 'message': prompt})
    assert status == 202, value
    return value['turn']['id']


def pilot_workspace(turn):
    binding = json.loads((ROOT / 'state/platform/pilot-resources' / (turn['goose']['scope'] + '.json')).read_text())
    return Path(binding['workspace'])


def native_history(turn):
    database = ROOT / 'state/platform/pilot' / turn['goose']['scope'] / 'process/data/sessions/sessions.db'
    with sqlite3.connect(database.as_uri() + '?mode=ro', uri=True) as connection:
        rows = connection.execute(
            'SELECT id,role,content_json,metadata_json FROM messages WHERE session_id=? ORDER BY id',
            (turn['goose']['session'],)).fetchall()
    return [{'id': row[0], 'role': row[1], 'content': json.loads(row[2]),
             'metadata': json.loads(row[3]) if row[3] else {}} for row in rows]


def call_rows(workspace):
    path = Path(workspace) / 'plugin-calls.jsonl'
    return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []


def report_prompt(marker, filename):
    return (f'请为任务编号 {marker} 生成一份正式的金额核验报告，并保存为 {filename}。'
            '两笔金额与校验凭据按当前核验服务的说明取得，采用服务实际返回的 JSON 和回执。'
            '文件内容请直接保存服务实际返回的 JSON 对象，核查保存结果后给出结论。')


API_KEY = secrets.token_hex(24)
PRIVATE.append(API_KEY)
process = None
proxy = None
proxy_thread = None
log = None
sid = 'capability-' + NONCE
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
    REPORT['original_env_sha256'] = before_env
    for name in ('state', 'catalog', 'workspaces', 'library', 'home'):
        (ROOT / name).mkdir()
    ids = []
    for index in range(998):
        identifier = f'noise-{index:04}'
        ids.append(identifier)
        directory = ROOT / 'library/plugins' / identifier
        directory.mkdir(parents=True)
        (directory / 'plugin.json').write_text(json.dumps({'name': f'素材整理 {index}', 'description': '整理音频素材及本地照片。'}))
    for identifier, token in ((PLUGIN, TOKEN), (REPLACEMENT, SECOND_TOKEN)):
        directory = ROOT / 'library/plugins' / identifier
        (directory / 'skills/reporting').mkdir(parents=True)
        (directory / 'skills/unrelated').mkdir(parents=True)
        (directory / 'resources').mkdir()
        (directory / 'plugin.json').write_text(json.dumps({'name': '金额核验服务', 'description': '生成正式的金额核验报告，取得随机校验凭据、核对两笔金额并出具 JSON 回执。',
            'skills': ['skills/reporting', 'skills/unrelated'],
            'mcpServers': {'service': {'command': 'python3', 'args': [f'/plugins/{identifier}/server.py']}}}, ensure_ascii=False))
        (directory / 'instructions.md').write_text('生成金额核验报告时先读 skills/reporting/SKILL.md。只读取与任务相关的内容。所需参数必须从工具详情读取。')
        (directory / 'skills/reporting/SKILL.md').write_text('---\nname: amount-verification\ndescription: 生成金额核验报告，读取校验凭据，用正式服务核对两筆金额。\n---\n'
            '读取 resources/proof.json 获得 token 和 amounts。用 produce_report，payload 含任务编号 marker 和资源 token；entries 对应每个 amount；format=json。报告只能使用实际工具返回值。保存报告到用户指定文件，先检查再答复。')
        (directory / 'skills/unrelated/SKILL.md').write_text('---\nname: unrelated\ndescription: 其他无关任务\n---\nUNRELATED_BODY_MUST_NOT_BE_LOADED_' + NONCE)
        (directory / 'resources/proof.json').write_text(json.dumps({'token': token, 'amounts': [17, 25]}))
        server_source = "import json\nimport sys\nfrom pathlib import Path\n\ntarget = {'name': 'produce_report', 'description': '生成校验报告 validation report，核对 token、marker 和两笔金额后返回 JSON。先读取 Plugin 的校验 Skill 和完整参数定义。', 'inputSchema': {\n    'type': 'object', 'properties': {\n        'payload': {'type': 'object', 'properties': {'marker': {'type': 'string'}, 'token': {'type': 'string'}}, 'required': ['marker', 'token'], 'additionalProperties': False},\n        'entries': {'type': 'array', 'minItems': 2, 'maxItems': 2, 'items': {'type': 'object', 'properties': {'amount': {'type': 'integer'}}, 'required': ['amount'], 'additionalProperties': False}},\n        'format': {'type': 'string', 'enum': ['json']}}, 'required': ['payload', 'entries', 'format'], 'additionalProperties': False}}\nnoise = [{'name': f'noise_{i:04}', 'description': '其他无关能力。This operation is unrelated to validation reports. ' + 'metadata ' * 24, 'inputSchema': {'type': 'object', 'properties': {'value': {'type': 'string'}, 'mode': {'type': 'string', 'enum': ['a', 'b']}}, 'required': ['value'], 'additionalProperties': False}} for i in range(1000)]\nfor line in sys.stdin:\n    request = json.loads(line)\n    if 'id' not in request:\n        continue\n    method = request.get('method')\n    if method == 'initialize':\n        result = {'protocolVersion': '2024-11-05', 'capabilities': {'tools': {}}, 'serverInfo': {'name': 'validation-fixture', 'version': '1'}}\n    elif method == 'tools/list':\n        result = {'tools': noise + [target]}\n    elif method == 'tools/call':\n        params = request['params']\n        args = params.get('arguments', {})\n        if params.get('name') != 'produce_report' or set(args) != {'payload', 'entries', 'format'} or set(args.get('payload', {})) != {'marker', 'token'} or args.get('format') != 'json' or len(args.get('entries', [])) != 2 or any(set(e) != {'amount'} or type(e['amount']) != int for e in args.get('entries', [])):\n            result = {'isError': True, 'content': [{'type': 'text', 'text': 'Read the complete produce_report schema and correct the arguments.'}]}\n        else:\n            result_value = dict(args['payload'], total=sum(e['amount'] for e in args['entries']), format=args['format'])\n            with Path('/workspace/plugin-calls.jsonl').open('a') as f:\n                f.write(json.dumps({'tool': 'produce_report', 'arguments': args, 'result': result_value}) + '\\n')\n            result = {'content': [{'type': 'text', 'text': json.dumps(result_value)}], 'structuredContent': result_value}\n    else:\n        result = {}\n    print(json.dumps({'jsonrpc': '2.0', 'id': request['id'], 'result': result}), flush=True)\n"
        server_source = server_source.replace('生成校验报告 validation report', '生成正式金额核验报告')
        server_source = server_source.replace("{'tool': 'produce_report',", "{'plugin': Path(__file__).parent.name, 'tool': 'produce_report',")
        (directory / 'server.py').write_text(server_source)
    env = environment()
    interpreter = Path(env['ANCHOR_DEPENDENCIES_PYTHON']).resolve()
    REPORT['shared_interpreter'] = str(interpreter)
    before_python = digest(interpreter)
    REPORT['shared_interpreter_sha256'] = before_python
    assert env.get('ANCHOR_MODEL_WIRE_API', 'chat') == 'chat', 'Recording proxy currently supports chat transport'
    REMOTE_BASE = env['ANCHOR_MODEL_URL']
    proxy = ThreadingHTTPServer(('127.0.0.1', 0), RecordingProxy)
    proxy_thread = threading.Thread(target=proxy.serve_forever, daemon=True)
    proxy_thread.start()
    env['ANCHOR_MODEL_URL'] = f'http://127.0.0.1:{proxy.server_port}/v1'
    env.pop('ANCHOR_MODEL_DIRECT', None)
    # A smaller advertised window exercises native compaction with real model
    # responses and unmodified usage. The actual deployed setting is untouched.
    env['ANCHOR_MODEL_CONTEXT_WINDOW'] = '32000'
    REPORT['isolated_context_window'] = 32000
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
    assert api('POST', '/sessions', {'id': sid})[0] == 201
    session_created = True
    for identifier in ([] if NODE_ONLY else ids[:9] + [PLUGIN]):
        _, value = api('POST', f'/sessions/{sid}/plugins/{identifier}', {'on': True})
        assert value['effect']['status'] == 'next_turn', value
    assert api('GET', f'/sessions/{sid}/turns')[1]['turns'] == [], 'Idle mount invoked the model'
    first_system = ''
    workspace = None
    if not NODE_ONLY:
        STAGE = 'pilot-initial'
        first = turn_result(submit('initial-report', report_prompt(MARKER, 'proof.json')), 'Real Pilot initial discovery')['turn']
        workspace = pilot_workspace(first)
        rows = call_rows(workspace)
        assert len(rows) == 1 and rows[0]['plugin'] == PLUGIN and rows[0]['result'] == EXPECTED, rows
        assert json.loads((workspace / 'proof.json').read_text()) == EXPECTED
        save('pilot-initial-turn.json', first)
        _, stream = api('GET', f'/sessions/{sid}/turns/{first["id"]}/events')
        save('pilot-initial-events.json', events(stream))
        initial_requests = regular_requests(STAGE)
        assert initial_requests and all(CURRENT_HEADING in system_context(request) for request in initial_requests)
        first_system = system_context(initial_requests[0])
        assert PLUGIN in first_system and REPLACEMENT not in first_system
        assert 'UNRELATED_BODY_MUST_NOT_BE_LOADED_' not in json.dumps(initial_requests), \
            'The model explicitly read an unrelated Skill body'

        STAGE = 'pilot-active-mount'
        GATE_ARMED = True
        tid = submit('inspect-current-capability', '这次先不生成报告。请根据当前可用能力，一句话说明是否可以继续进行金额核验。')
        assert GATE_ENTERED.wait(35), 'The real model request never reached the barrier'
        _, added = api('POST', f'/sessions/{sid}/plugins/{REPLACEMENT}', {'on': True})
        _, removed = api('POST', f'/sessions/{sid}/plugins/{PLUGIN}', {'on': False})
        _, same = api('POST', f'/sessions/{sid}/plugins/{PLUGIN}', {'on': False})
        assert added['effect'] == {'status': 'after_current_turn', 'source_turn': tid}, added
        assert removed['effect'] == {'status': 'after_current_turn', 'source_turn': tid}, removed
        assert same['effect']['status'] == 'unchanged', same
        save('mount-switches.json', {'added': added, 'removed': removed, 'unchanged': same})
        with MODEL_LOCK:
            paused = [request for request in MODEL_REQUESTS if request['stage'] == STAGE][0]
            assert PLUGIN in system_context(paused) and REPLACEMENT not in system_context(paused)
        GATE_RELEASE.set()
        source = turn_result(tid, 'Real Pilot running source')
        continued = wait(lambda: api('GET', f'/sessions/{sid}/turns/{tid}')[1],
                         lambda value: value['continuation']['status'] == 'admitted', 'Mount continuation admission')
        next_id = continued['continuation']['turn']
        continuation = turn_result(next_id, 'Real Pilot mounted continuation')['turn']
        assert continuation['goose'] == first['goose'], 'Mount update replaced the native Session'
        source_turns = api('GET', f'/sessions/{sid}/turns')[1]['turns']
        assert sum(turn['request_id'] == f'continuation:{tid}' for turn in source_turns) == 1
        source_calls = call_rows(workspace)
        save('pilot-mount-source.json', source)
        save('pilot-mount-continuation.json', continuation)
        save('pilot-mount-calls.json', source_calls)
        assert source_calls == rows, 'Capability inspection replayed a report'
        updated_requests = regular_requests(STAGE)
        assert any(REPLACEMENT in system_context(request) and PLUGIN not in system_context(request) for request in updated_requests)

        STAGE = 'pilot-updated'
        updated = turn_result(submit('updated-report', report_prompt(SECOND_EXPECTED['marker'], 'next-proof.json')), 'Real Pilot updated discovery')['turn']
        assert updated['goose'] == first['goose']
        rows = call_rows(workspace)
        assert len(rows) == 2 and rows[1]['plugin'] == REPLACEMENT and rows[1]['result'] == SECOND_EXPECTED, rows
        assert json.loads((workspace / 'next-proof.json').read_text()) == SECOND_EXPECTED
        latest_requests = regular_requests(STAGE)
        assert latest_requests and all(REPLACEMENT in system_context(request) and PLUGIN not in system_context(request) for request in latest_requests)
        save('pilot-updated-turn.json', updated)
        _, stream = api('GET', f'/sessions/{sid}/turns/{updated["id"]}/events')
        save('pilot-updated-events.json', events(stream))
        _, messages = api('GET', f'/sessions/{sid}/messages')
        save('pilot-messages.json', messages)
        REPORT.update(pilot_status='completed', same_native_session=True, active_mount_followed_once=True,
                      capability_inspection_effects=0, pilot_actual_calls=rows,
                      obsolete_summary_absent_from_current_system=True, pilot_artifacts_checked=True)

        before_compact = native_history(updated)
        STAGE = 'pilot-compaction-pressure'
        pressure = secrets.token_hex(30000)
        pressure_prompt = ('以下随机串是会话压缩测试的无关附录，不包含需要处理的信息，不要复述或保存。'
                           '金额核验任务已经有报告，请勿重新执行核验。只回复“已记下”。\n' + pressure)
        assert len(pressure_prompt.encode()) <= 65536, 'Pressure must fit the public Turn message limit'
        REPORT['pressure_message_bytes'] = len(pressure_prompt.encode())
        pressured = turn_result(submit('context-pressure', pressure_prompt), 'Real Pilot context pressure')['turn']
        after_pressure = native_history(pressured)
        assert any(pressure in json.dumps(row['content']) for row in after_pressure), \
            'The pressure message was not saved in native history'
        save('pilot-native-after-pressure.json', after_pressure)
        STAGE = 'pilot-after-compaction'
        compacted = turn_result(submit('inspect-after-context-pressure',
            '请核查刚才保存的 next-proof.json 与已有核验结果一致，然后简要告诉我报告已保存。不要重新生成报告或再次核验。'),
            'Real Pilot after native compaction')['turn']
        assert compacted['goose'] == first['goose']
        after_compact = native_history(compacted)
        assert any(pressure in json.dumps(row['content']) and row['metadata'].get('agentVisible') is False
                   for row in after_compact), 'Native compaction did not hide the old pressure message'
        assert any('Your context was compacted.' in json.dumps(row['content'])
                   and row['metadata'].get('agentVisible') for row in after_compact)
        summary_requests = [request for request in MODEL_REQUESTS if is_summary_request(request)
                            and request['stage'] in ('pilot-compaction-pressure', STAGE)]
        assert summary_requests, 'No actual real-model summary request observed'
        resumed = regular_requests(STAGE)
        assert resumed and all(CURRENT_HEADING in system_context(request)
                               and REPLACEMENT in system_context(request)
                               and PLUGIN not in system_context(request) for request in resumed)
        assert any('Your context was compacted.' in json.dumps(request['body']) for request in resumed)
        assert call_rows(workspace) == rows, 'Compaction caused a report replay'
        save('pilot-compaction-turn.json', compacted)
        save('pilot-native-before-compaction.json', before_compact)
        save('pilot-native-after-compaction.json', after_compact)
        REPORT.update(real_provider_compaction_tested=True, native_history_compacted=True,
                      summary_requests=len(summary_requests),
                      compaction_stages=sorted({request['stage'] for request in summary_requests}),
                      compressed_business_effects_replayed=False)

    STAGE = 'node-large-catalog'
    node_prompt = report_prompt(MARKER, 'proof.json')
    graph = {'entry': 'worker', 'agents': {'worker': {'model': 'models.worker', 'writes': ['proof.json'],
        'instructions': node_prompt + ' 检查报告后单独提交原生完成工具。'}},
        'nodes': [{'id': 'worker', 'agent': 'worker', 'plugins': ids + [PLUGIN]}], 'edges': []}
    assert api('POST', '/graphs', {'name': 'context-proof', 'definition': graph})[0] == 201
    status, accepted = api('POST', '/trigger', {'graph': 'context-proof'})
    assert status == 202, accepted
    run = accepted['run']
    detail = wait(lambda: api('GET', f'/runs/{run}')[1], lambda value: not value['active'] and value['state']['status'] in ('completed', 'failed', 'stopped', 'interrupted', 'paused'), 'Real Node bounded discovery')
    save('run.json', detail)
    REPORT['node_status'] = detail['state']['status']
    assert detail['state']['status'] == 'completed', detail['state'].get('error')
    _, artifact = api('GET', f'/runs/{run}/files/worker/proof.json')
    save('artifact.json', artifact)
    assert json.loads(artifact['text']) == EXPECTED, artifact
    node_files = [path for path in (ROOT / 'workspaces').rglob('plugin-calls.jsonl') if path.parent != workspace]
    assert len(node_files) == 1
    node_rows = call_rows(node_files[0].parent)
    save('node-calls.json', node_rows)
    REPORT['node_actual_calls'] = node_rows
    assert len(node_rows) == 1 and node_rows[0]['plugin'] == PLUGIN and node_rows[0]['result'] == EXPECTED
    node_requests = regular_requests(STAGE)
    assert node_requests and all(CURRENT_HEADING in system_context(request) for request in node_requests)
    contexts = [system_context(request) for request in node_requests]
    catalog_summary = capability_rows(contexts[0])
    summary_count = next(item for item in catalog_summary if 'omitted' in item)
    assert summary_count['omitted'] > 0 and summary_count['total'] == 999, summary_count
    assert not any(item.get('id') == PLUGIN for item in catalog_summary), 'Large fixture target should require searching omitted entries'
    paths = list((ROOT / 'state/goose-acp').glob('*.evidence.json'))
    assert len(paths) == 1
    native = json.loads(paths[0].read_text())
    save('node-native-evidence.json', native)
    assert any(call['tool'].endswith('_produce_report') and call['result']['ok'] is True for call in native['tool_calls'])
    REPORT.update(node_status='completed', node_actual_calls=node_rows, node_artifact_checked=True,
                  large_catalog_system_bytes=len(contexts[0].encode()),
                  large_catalog_summary=summary_count,
                  initial_system_bytes=len(first_system.encode()),
                  original_env_unchanged=digest(REPO / '.env') == before_env,
                  shared_interpreter_unchanged=digest(interpreter) == before_python,
                  real_model_requests=len(MODEL_REQUESTS), proxy_errors=PROXY_ERRORS, passed=not PROXY_ERRORS)
except Exception as error:
    REPORT['failed_stage'] = STAGE
    REPORT['error'] = redact(f'{type(error).__name__}: {error}')
    (ROOT / 'failure.txt').write_text(redact(traceback.format_exc()))
finally:
    GATE_RELEASE.set()
    if 'original_env_sha256' in REPORT:
        REPORT['original_env_unchanged'] = digest(REPO / '.env') == REPORT['original_env_sha256']
    if 'shared_interpreter_sha256' in REPORT:
        REPORT['shared_interpreter_unchanged'] = digest(interpreter) == REPORT['shared_interpreter_sha256']
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
    if proxy is not None:
        proxy.shutdown()
        proxy.server_close()
    with MODEL_LOCK:
        save('model-requests.json', MODEL_REQUESTS)
    save('evidence.json', REPORT)
    print(redact(json.dumps(REPORT, ensure_ascii=False)), flush=True)
sys.exit(0 if REPORT['passed'] else 1)
