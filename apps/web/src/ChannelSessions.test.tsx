/** @vitest-environment jsdom */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ChannelSessions } from './ChannelSessions';
import type { RunProjection } from './model';

const binding = { graph: 'assistant-graph', reply_node: 'assistant', plugin: 'gateway', users: ['alice'], send_users: ['alice'] };
const response = (status: number, data: unknown) => ({ ok: status >= 200 && status < 300, status, json: async () => data }) as Response;

describe('message session management', () => {
  let container: HTMLDivElement;
  let root: Root;
  let fetchMock: ReturnType<typeof vi.fn>;
  let instance: { assistant: { run_id: string; wait_node: string; work_node: string; reply_node: string } | null; needs_recovery: boolean;
    plugin_update?: { state: 'current' | 'pending' | 'restart_required' | 'unavailable'; message?: string } };
  let execution: { run: string; active: boolean; control_requested: string | null; projection?: RunProjection; state: { status: string; cursor: { node: string } } };
  let sessions: { id: string; title: string; graph: string; platform: string }[];
  let deliveries: { key: string; status: string }[];
  let runIds: string[];
  let opened: [string, string][];
  let closed: number;
  const requests = () => fetchMock.mock.calls.map(([path, init]) => ({ path, method: init?.method ?? 'GET', body: init?.body ? JSON.parse(init.body) : undefined }));
  const button = (name: string) => [...container.querySelectorAll<HTMLButtonElement>('button')].find(item => item.textContent === name)!;
  const click = async (name: string) => { await act(async () => button(name).click()); };
  const render = async () => { await act(async () => root.render(<ChannelSessions onOpenRun={(run, graph) => opened.push([run, graph])} close={() => { closed += 1; }} />)); };
  beforeEach(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    Object.defineProperty(HTMLDialogElement.prototype, 'showModal', { configurable: true, value() { this.open = true; } });
    Object.defineProperty(HTMLDialogElement.prototype, 'close', { configurable: true, value() { this.open = false; } });
    container = document.createElement('div'); document.body.appendChild(container); root = createRoot(container);
    sessions = [{ id: 'channel-session', title: 'Alice 的对话', graph: binding.graph, platform: 'wecom' }];
    instance = { assistant: { run_id: 'current-run', wait_node: 'wait_input', work_node: 'assistant', reply_node: 'reply' }, needs_recovery: false };
    execution = { run: 'current-run', active: true, control_requested: null, state: { status: 'running', cursor: { node: 'wait_input' } } };
    deliveries = []; runIds = ['current-run']; opened = []; closed = 0;
    fetchMock = vi.fn(async (path: string, init: RequestInit = {}) => {
      const method = init.method ?? 'GET';
      if (path === '/channel-sessions') return response(200, { sessions });
      if (path === '/channel-sessions/channel-session/assistant') return response(200, instance);
      if (path === '/channel-sessions/channel-session/deliveries/unresolved') return response(200, { deliveries });
      if (path === '/channel-sessions/channel-session' && method === 'GET') return response(200, { session: { run_ids: runIds } });
      if (path === '/runs/current-run' && method === 'GET') return response(200, { ...execution, projection: execution.projection ?? {
        phase: execution.control_requested === 'stop' ? 'stopping' : execution.active ? 'running' : 'stopped',
        active: execution.active, control_requested: execution.control_requested,
        terminal: false, quiescent: !execution.active, needs_attention: false,
        actions: { pause: execution.active && !execution.control_requested, stop: execution.control_requested !== 'stop', resume: !execution.active },
      } });
      if (path === '/runs/current-run/stop') { execution = { ...execution, control_requested: 'stop' }; return response(202, { asked: 'stop' }); }
      if (path.endsWith('/assistant/retire')) { instance = { assistant: null, needs_recovery: false }; return response(200, { retired: 'current-run' }); }
      if (path.startsWith('/runs/') && method === 'DELETE') { runIds = runIds.filter(id => path !== `/runs/${id}`); return response(200, { deleted: path }); }
      if (path === '/channel-sessions/channel-session' && method === 'DELETE') { sessions = []; return response(200, { deleted: 'channel-session' }); }
      return response(500, { error: `unexpected ${method} ${path}` });
    });
    vi.stubGlobal('fetch', fetchMock);
  });
  afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });

  it('opens a Run from the board and closes the dialog behind it', async () => {
    await render();
    await click('current-run');
    expect(opened).toEqual([['current-run', binding.graph]]);
    expect(closed).toBe(1);
  });

  it.each([
    ['pending', '插件待生效', '下一条消息使用新配置'],
    ['restart_required', '插件需重建实例', '请先停止并退役现有实例'],
    ['unavailable', '插件状态不可用', '当前 Graph 读取失败'],
  ] as const)('shows server plugin state %s without claiming it has applied', async (state, label, message) => {
    instance.plugin_update = { state, message };
    await render();
    expect(container.textContent).toContain('等待消息');
    const status = container.querySelector('.channels-plugin-state')!;
    expect(status.textContent).toBe(label); expect(status.getAttribute('title')).toBe(message);
    expect(status.getAttribute('role')).toBe(state === 'unavailable' ? 'alert' : 'status');
    await click('详情'); expect(container.querySelector('.channels-instance-details')?.textContent).toContain(message);
    expect(requests().every(item => item.method === 'GET')).toBe(true);
  });

  it('shows a failed conversation read instead of an empty list until a successful retry', async () => {
    sessions = [];
    const initial = fetchMock.getMockImplementation() as (path: string, init?: RequestInit) => Promise<Response>;
    fetchMock.mockImplementation((path: string, init?: RequestInit) => path === '/channel-sessions'
      ? Promise.resolve(response(503, { error: 'conversations temporarily unavailable' })) : initial(path, init));
    await render();
    expect(container.textContent).toContain('消息会话读取失败');
    expect(container.textContent).not.toContain('还没有消息会话');
    expect(container.querySelector('.channels-count')?.textContent).toBe('未知');
    fetchMock.mockImplementation(initial);
    await click('刷新状态');
    expect(container.textContent).toContain('还没有消息会话');
    expect(container.textContent).not.toContain('消息会话读取失败');
    expect(container.querySelector('.channels-count')?.textContent).toBe('0');
  });

  it('waits for the executor to exit before allowing retirement and binds it to the current Run', async () => {
    await render();
    await click('停止');
    expect(container.textContent).toContain('正在停止…');
    expect(button('退役实例').disabled).toBe(true);
    expect(requests().filter(item => item.path.endsWith('/assistant/retire'))).toHaveLength(0);
    execution = { ...execution, active: false, control_requested: null, state: { ...execution.state, status: 'stopped' } };
    instance.needs_recovery = true;
    await click('刷新状态');
    expect(button('退役实例').disabled).toBe(false);
    await click('退役实例');
    expect(container.querySelector('dialog')?.textContent).toContain('下一条可信消息');
    await click('确认退役');
    expect(requests().find(item => item.path.endsWith('/assistant/retire'))?.body).toEqual({ run_id: 'current-run' });
    expect(container.textContent).toContain('无当前实例');
  });

  it('uses the same Run projection for an inactive execution and its allowed controls', async () => {
    execution.active = false;
    execution.projection = {
      phase: 'waiting_resume', active: false, control_requested: null, terminal: false,
      quiescent: false, needs_attention: true, actions: { pause: false, stop: true, resume: true },
    };
    await render();
    expect(container.querySelector('.channels-instance-status')?.textContent).toBe('等待接续');
    expect(button('继续').disabled).toBe(false);
    expect(button('停止').disabled).toBe(false);
    expect(button('退役实例').disabled).toBe(true);
  });

  it('does not infer resumability or retirement when the Host denies those actions', async () => {
    execution.active = false; execution.state.status = 'stopped';
    execution.projection = {
      phase: 'stopped', active: false, control_requested: null, terminal: false,
      quiescent: false, needs_attention: false, actions: { pause: false, stop: false, resume: false },
    };
    await render();
    expect(container.querySelector('.run-controls button')).toBeNull();
    expect(button('退役实例').disabled).toBe(true);
  });

  it('shows unknown deliveries and prevents retirement and cleanup without silently settling them', async () => {
    execution.active = false; execution.state.status = 'stopped';
    deliveries = [{ key: 'reply-unknown', status: 'unknown' }];
    await render();
    expect(container.textContent).toContain('结果未知');
    expect(container.textContent).toContain('核查网关账本与实际消息结果');
    expect(button('退役实例').disabled).toBe(true);
    expect(button('清理历史').disabled).toBe(true);
    expect(requests().some(item => item.method !== 'GET')).toBe(false);
  });

  it('keeps the confirmed retirement target fixed if the current instance changes', async () => {
    execution.active = false; execution.state.status = 'stopped';
    await render(); await click('退役实例');
    instance = { assistant: { run_id: 'fresh-run', wait_node: 'wait', work_node: 'work', reply_node: 'reply' }, needs_recovery: false };
    await click('刷新状态');
    expect(container.querySelector('dialog')?.textContent).toContain('解除 current-run 的当前绑定');
    expect(container.querySelector('dialog')?.textContent).toContain('当前实例已变化');
    expect(button('确认退役').disabled).toBe(true);
    expect(requests().some(item => item.path.endsWith('/assistant/retire'))).toBe(false);
  });

  it('cleans the confirmed history scope through Run deletion before Session deletion', async () => {
    instance = { assistant: null, needs_recovery: false }; runIds = ['old-run', 'new-run'];
    await render(); await click('清理历史');
    expect(container.querySelector('dialog')?.textContent).toContain('old-run');
    expect(container.querySelector('dialog')?.textContent).toContain('new-run');
    await click('永久清理');
    expect(requests().filter(item => item.method === 'DELETE').map(item => item.path)).toEqual(['/runs/new-run', '/runs/old-run', '/channel-sessions/channel-session']);
    expect(container.textContent).toContain('还没有消息会话');
  });

  it('rejects cleanup if a trusted message bound a new instance after confirmation opened', async () => {
    instance = { assistant: null, needs_recovery: false };
    await render(); await click('清理历史');
    instance = { assistant: { run_id: 'fresh-run', wait_node: 'wait', work_node: 'work', reply_node: 'reply' }, needs_recovery: false };
    await click('永久清理');
    expect(container.querySelector('dialog')?.textContent).toContain('此对话已有当前实例');
    expect(requests().filter(item => item.method === 'DELETE')).toHaveLength(0);
  });

  it('keeps the confirmed deletion scope fixed if history changes before cleanup', async () => {
    instance = { assistant: null, needs_recovery: false };
    await render(); await click('清理历史');
    runIds = ['current-run', 'newly-admitted-run'];
    await click('永久清理');
    expect(container.querySelector('dialog')?.textContent).toContain('重新确认清理范围');
    expect(requests().filter(item => item.method === 'DELETE')).toHaveLength(0);
  });
});
