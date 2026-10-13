/** @vitest-environment jsdom */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { LibraryPluginDetails } from './LibraryPluginDetails';
import { Plugins } from './Plugins';

vi.mock('./markdown', () => ({ Markdown: ({ text }: { text: string }) => <div>{text}</div> }));
const response = (status: number, data: unknown) => ({ ok: status >= 200 && status < 300, status, json: async () => data }) as Response;

describe('plugin details ownership and request safety', () => {
  let container: HTMLDivElement;
  let root: Root;
  let fetchMock: ReturnType<typeof vi.fn>;
  beforeEach(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    Object.defineProperty(HTMLDialogElement.prototype, 'showModal', { configurable: true, value() { this.open = true; } });
    Object.defineProperty(HTMLDialogElement.prototype, 'close', { configurable: true, value() { this.open = false; } });
    container = document.createElement('div'); document.body.appendChild(container); root = createRoot(container);
    fetchMock = vi.fn(); vi.stubGlobal('fetch', fetchMock);
  });
  afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });
  const click = async (name: string) => { const button = [...container.querySelectorAll('button')].find(item => item.textContent === name)!; await act(async () => button.click()); };
  const base = { id: 'demo', name: '示例', description: '完整用途' };

  it('opens captured Run metadata without a current library or authorization request', async () => {
    await act(async () => root.render(<Plugins recorded={[{ ...base, skills: ['历史技能'], digest: 'historical-digest' }]} />));
    await click('详情');
    expect(container.querySelector('dialog')?.textContent).toContain('示例 · 插件详情');
    expect(container.querySelector('dialog')?.textContent).toContain('historical-digest');
    expect(container.querySelector('dialog')?.textContent).toContain('历史技能');
    expect(container.querySelector('[aria-label="插件筛选"]')).toBeNull();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('keeps basic information on a failed read and retries only the current plugin', async () => {
    fetchMock.mockResolvedValueOnce(response(503, { error: 'Library 正在发布' }));
    await act(async () => root.render(<LibraryPluginDetails plugin={base} status="不可用" close={vi.fn()} />));
    expect(container.textContent).toContain('完整用途');
    expect(container.textContent).toContain('Library 正在发布');
    fetchMock.mockResolvedValueOnce(response(200, { ...base, instructions: '已重新读取' }));
    await click('重试');
    expect(container.textContent).toContain('已重新读取');
    expect(container.textContent).not.toContain('Library 正在发布');
  });

  it('ignores a detail response that arrives after another plugin is opened', async () => {
    let complete: (value: Response) => void = () => undefined;
    fetchMock.mockImplementationOnce(() => new Promise<Response>(resolve => { complete = resolve; }));
    await act(async () => root.render(<LibraryPluginDetails key="demo" plugin={base} status="未挂载" close={vi.fn()} />));
    fetchMock.mockResolvedValueOnce(response(200, { id: 'other', name: '另一个', description: '新详情', instructions: '当前说明' }));
    await act(async () => root.render(<LibraryPluginDetails key="other" plugin={{ id: 'other', name: '另一个' }} status="未挂载" close={vi.fn()} />));
    await act(async () => complete(response(200, { ...base, instructions: '迟到说明' })));
    expect(container.textContent).toContain('当前说明');
    expect(container.textContent).not.toContain('迟到说明');
  });

  it('reports unknown OAuth status and retains authorize and revoke after a retry', async () => {
    fetchMock.mockResolvedValueOnce(response(200, { ...base, mcpServers: { remote: { transport: 'http', auth: 'oauth' } } }));
    fetchMock.mockResolvedValueOnce(response(503, { error: '授权存储暂时不可读' }));
    await act(async () => root.render(<LibraryPluginDetails plugin={base} status="已选择" close={vi.fn()} />));
    expect(container.textContent).toContain('无法读取授权状态');
    expect(container.textContent).not.toContain('未授权');
    fetchMock.mockResolvedValueOnce(response(200, { authorized: true }));
    await click('重试授权状态');
    expect(container.textContent).toContain('已授权');
    expect(container.textContent).toContain('重新授权 remote');
    expect(container.textContent).toContain('撤销 remote');
    fetchMock.mockResolvedValueOnce(response(204, null));
    await click('撤销 remote');
    expect(container.textContent).toContain('未授权');
    expect(fetchMock.mock.calls.at(-1)?.[1]).toMatchObject({ method: 'DELETE' });
  });

  it('offers a manual authorization link when the popup is blocked', async () => {
    fetchMock.mockResolvedValueOnce(response(200, { ...base, mcpServers: { remote: { transport: 'http', auth: 'oauth' } } }));
    fetchMock.mockResolvedValueOnce(response(200, { authorized: false }));
    await act(async () => root.render(<LibraryPluginDetails plugin={base} status="已选择" close={vi.fn()} />));
    vi.spyOn(window, 'open').mockReturnValue(null);
    fetchMock.mockResolvedValueOnce(response(200, { authorization_url: 'https://example.test/oauth' }));
    await click('授权 remote');
    expect(container.querySelector('a')?.getAttribute('href')).toBe('https://example.test/oauth');
    expect(container.textContent).toContain('授权窗口未能打开');
  });
});
