/** @vitest-environment jsdom */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MessageTrigger } from './MessageTrigger';

vi.mock('./PluginCredentials', () => ({ PluginCredentials: ({ pluginId, target }: { pluginId: string; target: string }) => <div data-testid="credentials">{pluginId}/{target}</div> }));
const binding = { graph: 'assistant-graph', reply_node: 'assistant', plugin: 'gateway', users: ['alice'], send_users: ['alice'] };
const response = (status: number, data: unknown) => ({ ok: status >= 200 && status < 300, status, json: async () => data }) as Response;

describe('message trigger configuration', () => {
  let container: HTMLDivElement;
  let root: Root;
  let fetchMock: ReturnType<typeof vi.fn>;
  let status: Record<string, unknown>;
  let closed: number;
  const requests = () => fetchMock.mock.calls.map(([path, init]) => ({ path, method: init?.method ?? 'GET', body: init?.body ? JSON.parse(init.body) : undefined }));
  const button = (name: string) => [...container.querySelectorAll<HTMLButtonElement>('button')].find(item => item.textContent === name)!;
  const click = async (name: string) => { await act(async () => button(name).click()); };
  const render = async (graph = binding.graph) => { await act(async () => root.render(<MessageTrigger graph={graph} close={() => { closed += 1; }} />)); };
  beforeEach(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    Object.defineProperty(HTMLDialogElement.prototype, 'showModal', { configurable: true, value() { this.open = true; } });
    Object.defineProperty(HTMLDialogElement.prototype, 'close', { configurable: true, value() { this.open = false; } });
    container = document.createElement('div'); document.body.appendChild(container); root = createRoot(container);
    closed = 0;
    status = { platform: 'wecom', configured: true, configuration_owner: 'host', mutable: true, owner_matches: true, enabled: true, binding, listener: 'connecting' };
    fetchMock = vi.fn(async (path: string, init: RequestInit = {}) => {
      if (path === '/channels/wecom/configuration') return response(200, status);
      if (path === '/channels/wecom/enabled') { const body = JSON.parse(String(init.body)); status = { ...status, enabled: body.enabled, listener: body.enabled ? 'connecting' : 'stopped' }; return response(200, status); }
      if (path === '/plugins') return response(200, { plugins: [{ id: binding.plugin }] });
      return response(500, { error: `unexpected ${init.method ?? 'GET'} ${path}` });
    });
    vi.stubGlobal('fetch', fetchMock);
  });
  afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });

  it('shows actual connection state and reuses plugin credentials for the channel target', async () => {
    await render();
    expect(container.textContent).toContain('正在连接');
    expect(container.textContent).not.toContain('常驻监听');
    expect(container.textContent).toContain('配置已保存');
    expect(container.querySelector('[data-testid="credentials"]')?.textContent).toBe('gateway/channel_wecom');
    expect(container.querySelector('fieldset')?.disabled).toBe(true);
    expect(button('停用消息入口')).toBeDefined();
  });

  it('marks a previously authenticated connection as unconfirmed when the configuration read fails, then recovers', async () => {
    status.listener = 'authenticated';
    await render();
    expect(container.textContent).toContain('常驻监听');
    expect(container.querySelector('.channels-signal')?.classList.contains('connected')).toBe(true);
    const initial = fetchMock.getMockImplementation() as (path: string, init?: RequestInit) => Promise<Response>;
    fetchMock.mockImplementation((path: string, init?: RequestInit) => path === '/channels/wecom/configuration'
      ? Promise.resolve(response(503, { error: 'configuration temporarily unavailable' })) : initial(path, init));
    await click('刷新状态');
    expect(container.textContent).toContain('连接状态无法确认');
    expect(container.textContent).toContain('上次读取：已启用消息入口');
    expect(container.textContent).not.toContain('常驻监听');
    expect(container.querySelector('form')).not.toBeNull();
    fetchMock.mockImplementation(initial);
    await click('刷新状态');
    expect(container.textContent).toContain('常驻监听');
    expect(container.textContent).not.toContain('连接状态无法确认');
  });

  it('binds the open Graph and offers to replace a binding that belongs to another one', async () => {
    status.enabled = false;
    await render('other-graph');
    expect(container.textContent).toContain('当前触发工作流 assistant-graph');
    expect(button('改为触发此工作流')).toBeDefined();
    expect(button('启用消息入口').disabled).toBe(true);
    await act(async () => (container.querySelector('form') as HTMLFormElement).requestSubmit());
    expect(requests().find(item => item.path === '/channels/wecom/configuration' && item.method === 'PUT')?.body)
      .toEqual({ ...binding, graph: 'other-graph' });
  });

  it('sends the saved binding when toggling the entry without stopping its instances', async () => {
    await render(); await click('停用消息入口');
    expect(requests().find(item => item.path === '/channels/wecom/enabled')?.body).toEqual({ enabled: false, expected_binding: binding });
    expect(container.textContent).toContain('监听已停止');
  });

  it('keeps deployment management read only and hides bindings belonging to another owner', async () => {
    status = { ...status, configuration_owner: 'deployment', mutable: false };
    await render();
    expect(container.textContent).toContain('此消息入口由部署配置管理');
    expect(container.querySelector('fieldset')?.disabled).toBe(true);
    expect(container.querySelector('[data-testid="credentials"]')).toBeNull();
    expect(button('启用消息入口')).toBeUndefined();
    status = { platform: 'wecom', configuration_owner: 'host', configured: true, owner_matches: false, mutable: false };
    await click('刷新状态');
    expect(container.textContent).toContain('属于另一位管理者');
    expect(container.querySelector('form')).toBeNull();
  });
});
