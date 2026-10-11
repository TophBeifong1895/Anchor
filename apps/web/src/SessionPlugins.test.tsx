/** @vitest-environment jsdom */

/** The session plugin panel writes to the Host and reads it again on a timer.
 *
 * Two of these behaviours are the ones a person would notice if they were wrong: a required Plugin
 * must not move, and a refused switch must show the Host's own sentence while the row stays where
 * the Host left it. The third is the point of the panel — Pilot installs a Plugin mid-session and it
 * has to appear without a page reload.
 */

import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { maySwitch, POLL_MS, SessionPlugins, type SessionPlugin } from './SessionPlugins';

const plugin = (fields: Partial<SessionPlugin> & { id: string }): SessionPlugin => ({
  name: fields.id, description: '', skills: [], mcpServerNames: [], required: false, on: false,
  available: true, error: null, ...fields,
});
const response = (status: number, body: unknown) => ({
  ok: status >= 200 && status < 300, status, json: async () => body,
}) as unknown as Response;

describe('the session plugin panel', () => {
  let container: HTMLDivElement;
  let root: Root;
  let fetchMock: ReturnType<typeof vi.fn>;
  beforeEach(() => {
    // `act` needs to be told this is a test renderer; without it React flushes state updates late.
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    // jsdom arrives without the timer helpers `api()` composes; the test only needs a signal.
    if (typeof AbortSignal.timeout !== 'function') {
      Object.defineProperty(AbortSignal, 'timeout', { value: () => new AbortController().signal });
    }
    container = document.createElement('div');
    document.body.appendChild(container);
    root = createRoot(container);
    fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    vi.useFakeTimers();
  });
  afterEach(async () => {
    await act(async () => { root.unmount(); });
    container.remove();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });
  const draw = async () => { await act(async () => { root.render(<SessionPlugins session="s-1" />); }); };
  const switches = () => [...container.querySelectorAll<HTMLButtonElement>('.session-plugin-switch')];
  const posts = () => fetchMock.mock.calls.filter(call => (call[1] as RequestInit | undefined)?.method === 'POST');

  it('polls within three to five seconds', () => {
    expect(POLL_MS).toBeGreaterThanOrEqual(3000);
    expect(POLL_MS).toBeLessThanOrEqual(5000);
  });

  it('renders a required Plugin as a switch that cannot move, and lets no switch past the guard', async () => {
    fetchMock.mockResolvedValue(response(200, { plugins: [plugin({
      id: 'core', name: '核心能力', description: '图里预置的能力', skills: ['搜索'],
      mcpServerNames: ['search', 'fetch'], required: true, on: true,
    })] }));
    await draw();
    const [toggle] = switches();
    expect(container.textContent).toContain('核心能力');
    expect(container.textContent).toContain('core');
    expect(container.textContent).toContain('必需');
    expect(container.textContent).toContain('图里声明为必需，开关固定为开，点不动。');
    expect(container.textContent).toContain('1 Skills');
    expect(container.textContent).toContain('MCP search、fetch');
    // The disabled attribute is what makes it unclickable in a browser; jsdom would not deliver a
    // click to it anyway, so the guard that decides is asserted directly instead of through a click.
    expect(toggle.disabled).toBe(true);
    expect(toggle.getAttribute('aria-checked')).toBe('true');
    expect(maySwitch(plugin({ id: 'core', required: true }), false)).toBe(false);
    expect(maySwitch(plugin({ id: 'core' }), false)).toBe(true);
    expect(maySwitch(plugin({ id: 'core' }), true)).toBe(false);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(posts()).toEqual([]);
  });

  it('sends one switch at a time while the first is still waiting on the Host', async () => {
    let release: (value: Response) => void = () => undefined;
    fetchMock.mockResolvedValueOnce(response(200, { plugins: [plugin({ id: 'alpha', name: '阿尔法' }), plugin({ id: 'beta', name: '贝塔' })] }));
    fetchMock.mockImplementationOnce(() => new Promise<Response>(resolve => { release = resolve; }));
    await draw();
    const [alpha, beta] = switches();
    await act(async () => { alpha.click(); });
    expect(posts()).toHaveLength(1);
    // The second row is still enabled (only the row being written is), so this click really reaches
    // the handler — and the in-flight guard is the only thing that can stop the second POST.
    expect(beta.disabled).toBe(false);
    await act(async () => { beta.click(); });
    expect(posts()).toHaveLength(1);
    await act(async () => { release(response(200, { plugin: plugin({ id: 'alpha', name: '阿尔法', on: true }) })); });
    expect(switches()[0].getAttribute('aria-checked')).toBe('true');
    expect(switches()[1].getAttribute('aria-checked')).toBe('false');
  });

  it('keeps the row where the Host left it and shows the refused switch\'s own words', async () => {
    fetchMock.mockResolvedValueOnce(response(200, { plugins: [plugin({ id: 'demo', name: '示例插件' })] }));
    fetchMock.mockResolvedValueOnce(response(400, { error: '该插件在图里被声明为必需（required: true）' }));
    await draw();
    const [toggle] = switches();
    expect(toggle.getAttribute('aria-checked')).toBe('false');
    await act(async () => { toggle.click(); });
    expect(posts()).toHaveLength(1);
    expect(posts()[0][0]).toBe('/sessions/s-1/plugins/demo');
    expect(posts()[0][1]).toMatchObject({ method: 'POST', body: JSON.stringify({ on: true }) });
    expect(container.textContent).toContain('该插件在图里被声明为必需（required: true）');
    expect(switches()[0].getAttribute('aria-checked')).toBe('false');
    expect(switches()[0].textContent).toBe('未挂载');
    expect(container.textContent).not.toContain('已切换');
  });

  it('applies a successful switch from the returned Plugin and only promises the next Turn', async () => {
    fetchMock.mockResolvedValueOnce(response(200, { plugins: [plugin({ id: 'demo', name: '示例插件' })] }));
    fetchMock.mockResolvedValueOnce(response(200, { plugin: plugin({ id: 'demo', name: '示例插件', on: true }) }));
    await draw();
    const [toggle] = switches();
    await act(async () => { toggle.click(); });
    expect(switches()[0].getAttribute('aria-checked')).toBe('true');
    expect(switches()[0].textContent).toBe('已挂载');
    expect(container.textContent).toContain('已切换 · 将在下一个回合生效');
    expect(container.textContent).toContain('开关从下一个回合起生效');
    expect(container.textContent).not.toContain('已生效');
  });

  it('shows a Plugin Pilot just installed after one poll, and an unavailable one with its reason', async () => {
    fetchMock.mockResolvedValueOnce(response(200, { plugins: [plugin({ id: 'alpha', name: '阿尔法' })] }));
    fetchMock.mockResolvedValueOnce(response(200, { plugins: [
      plugin({ id: 'alpha', name: '阿尔法' }),
      plugin({ id: 'beta', name: '贝塔', available: false, error: '技能目录视图不可用' }),
    ] }));
    await draw();
    expect(container.textContent).not.toContain('贝塔');
    await act(async () => { await vi.advanceTimersByTimeAsync(POLL_MS); });
    expect(container.textContent).toContain('贝塔');
    expect(container.textContent).toContain('不可用：技能目录视图不可用');
    expect(switches()).toHaveLength(2);
  });

  it('stops polling once the panel is closed', async () => {
    fetchMock.mockResolvedValue(response(200, { plugins: [plugin({ id: 'alpha' })] }));
    await draw();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const close = [...container.querySelectorAll('button')].find(button => button.textContent === '收起');
    await act(async () => { close?.click(); });
    await act(async () => { await vi.advanceTimersByTimeAsync(POLL_MS * 3); });
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });
});
