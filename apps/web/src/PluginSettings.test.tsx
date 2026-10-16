/** @vitest-environment jsdom */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Plugins } from './Plugins';
import { SessionPlugins } from './SessionPlugins';

const catalog = [
  { id: 'alpha', name: 'Alpha', description: 'Search', available: true },
  { id: 'broken', name: 'Broken', description: 'Old capability', available: false, error: 'dependency view missing' },
  { id: 'zulu', name: 'Zulu', description: 'Selected capability', available: true, requiresNetwork: true },
];
const response = (status: number, body: unknown) => ({ ok: status < 300, status, json: async () => body }) as Response;

describe.each(['graph', 'session'] as const)('shared plugin settings in %s', context => {
  let container: HTMLDivElement;
  let root: Root;
  let fetchMock: ReturnType<typeof vi.fn>;
  const onChange = vi.fn();
  beforeEach(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    container = document.createElement('div'); document.body.appendChild(container); root = createRoot(container);
    fetchMock = vi.fn().mockResolvedValue(response(200, { plugins: catalog.map(plugin => ({ ...plugin, on: plugin.id === 'zulu' })) }));
    vi.stubGlobal('fetch', fetchMock); onChange.mockClear();
  });
  afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.unstubAllGlobals(); });
  const draw = async () => { await act(async () => root.render(context === 'graph'
    ? <Plugins selected={['zulu']} onChange={onChange} /> : <SessionPlugins session="session" />)); };
  const rowNames = () => [...container.querySelectorAll('.plugin-card-name')].map(node => node.textContent);

  it('uses selected-first ordering, the same switch control and shared filtering', async () => {
    await draw();
    expect(rowNames()).toEqual(['Zulu', 'Alpha', 'Broken']);
    expect(container.querySelectorAll('[role="switch"]')).toHaveLength(3);
    expect(container.querySelectorAll('input[type="checkbox"]')).toHaveLength(0);
    expect(container.textContent).toContain('需要联网');
    const broken = container.querySelector<HTMLButtonElement>('[aria-label$=" Broken"]')!;
    expect(broken.disabled).toBe(true);
    const available = [...container.querySelectorAll('button')].find(button => button.textContent === '可用')!;
    await act(async () => available.click());
    expect(rowNames()).toEqual(['Zulu', 'Alpha']);
  });

  it('keeps local draft changes and persisted Session writes with their respective owner', async () => {
    fetchMock.mockImplementation(async (_path: string, options?: RequestInit) => response(200,
      options?.method === 'POST'
        ? { plugin: { ...catalog[0], on: true } }
        : { plugins: catalog.map(plugin => ({ ...plugin, on: plugin.id === 'zulu' })) }));
    await draw();
    const select = container.querySelector<HTMLButtonElement>(`[aria-label="${context === 'graph' ? '选择' : '挂载'} Alpha"]`)!;
    await act(async () => select.click());
    const posts = fetchMock.mock.calls.filter(call => call[1]?.method === 'POST');
    if (context === 'graph') {
      expect(onChange).toHaveBeenCalledExactlyOnceWith(['zulu', 'alpha']);
      expect(posts).toHaveLength(0);
    } else {
      expect(onChange).not.toHaveBeenCalled();
      expect(posts[0][0]).toBe('/sessions/session/plugins/alpha');
      expect(container.textContent).toContain('已保存，下次发消息时使用新设置');
    }
  });

  it('offers the same failure feedback and retry instead of inventing an empty catalog', async () => {
    fetchMock.mockResolvedValueOnce(response(503, { error: 'Library is changing' }));
    await draw();
    expect(container.querySelector('[role="alert"]')?.textContent).toContain('Library is changing');
    expect(container.textContent).not.toContain('还没有安装插件');
    const retry = [...container.querySelectorAll('button')].find(button => button.textContent === '重试')!;
    await act(async () => retry.click());
    expect(rowNames()).toEqual(['Zulu', 'Alpha', 'Broken']);
  });
});
