/** @vitest-environment jsdom */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { PluginConfiguration, type PluginConfigurationData } from './PluginConfiguration';

const response = (status: number, body: unknown) => ({ ok: status >= 200 && status < 300, status, json: async () => body }) as Response;
const configuration = (revision = 1): PluginConfigurationData => ({
  plugin: 'docmost', authorization_digest: 'digest', revision,
  fields: [
    { id: 'endpoint', label: '服务地址', type: 'url', required: true, state: 'configured', source: 'managed', editable: true, value: 'https://docs.test/mcp' },
    { id: 'api_key', label: '访问凭据', type: 'secret', required: true, state: 'configured', source: 'managed', editable: true },
  ], targets: [{ target: 'docmost', transport: 'http' }, { target: 'attachments', transport: 'stdio' }],
});

describe('plugin connection configuration', () => {
  let container: HTMLDivElement; let root: Root; let fetchMock: ReturnType<typeof vi.fn>;
  let current: PluginConfigurationData;
  beforeEach(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    container = document.createElement('div'); document.body.appendChild(container); root = createRoot(container);
    current = configuration(); sessionStorage.clear(); localStorage.clear();
    fetchMock = vi.fn(async (path: string, options: RequestInit) => {
      if (path.endsWith('/credentials')) return response(200, { plugin: path.split('/')[2], targets: [] });
      if (options.method === 'GET') return response(200, current);
      if (options.method === 'PUT') { current = configuration(current.revision + 1); return response(200, current); }
      return response(200, { plugin: 'docmost', revision: current.revision, current: true,
        results: [{ target: 'docmost', status: 'verified', tool_count: 20, message: 'MCP 连接与工具发现成功。' }] });
    });
    vi.stubGlobal('fetch', fetchMock);
  });
  afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });
  const draw = async (pluginId = 'docmost') => { await act(async () => root.render(<PluginConfiguration pluginId={pluginId} />)); };
  const button = (name: string) => [...container.querySelectorAll<HTMLButtonElement>('button')].find(item => item.textContent === name)!;
  const click = async (name: string) => { expect(button(name), name).toBeTruthy(); await act(async () => button(name).click()); };
  const input = (label: string) => container.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`)!;
  const fill = async (label: string, value: string) => {
    const field = input(label); Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(field, value);
    await act(async () => field.dispatchEvent(new Event('input', { bubbles: true })));
  };

  it('saves shared credentials and the endpoint atomically without browser persistence or duplicate controls', async () => {
    await draw();
    expect(container.textContent).toContain('docmost、attachments'); expect(container.querySelectorAll('input[type="password"]')).toHaveLength(1);
    expect(container.querySelector('[aria-label="插件凭据"]')).toBeNull();
    await fill('服务地址', 'https://new.test/mcp'); await fill('访问凭据', 'private-rotation-value');
    expect(input('访问凭据').value).toBe('private-rotation-value'); expect(button('测试连接').disabled).toBe(true);
    await click('保存配置');
    const mutation = fetchMock.mock.calls.find(call => call[1].method === 'PUT')!;
    expect(mutation[0]).toBe('/plugins/docmost/configuration');
    expect(JSON.parse(mutation[1].body as string)).toEqual({ authorization_digest: 'digest', revision: 1,
      fields: { endpoint: { action: 'set', value: 'https://new.test/mcp' }, api_key: { action: 'set', value: 'private-rotation-value' } } });
    expect(input('访问凭据').value).toBe(''); expect(container.innerHTML).not.toContain('private-rotation-value');
    expect(container.textContent).toContain('保存状态不代表认证通过');
    expect(localStorage.length).toBe(0); expect(sessionStorage.length).toBe(0);
  });


  it('warns about changed destinations and renders the Host-cleared key as missing', async () => {
    await draw(); await fill('服务地址', 'https://other.test/mcp');
    expect(container.textContent).toContain('更换服务地址后需重新填写访问凭据');
    const updated = configuration(2); updated.fields[0].value = 'https://other.test/mcp';
    updated.fields[1].state = 'missing';
    fetchMock.mockResolvedValueOnce(response(200, updated));
    await click('保存配置');
    const mutation = fetchMock.mock.calls.find(call => call[1].method === 'PUT')!;
    expect(JSON.parse(mutation[1].body as string).fields).toEqual({ endpoint: { action: 'set', value: 'https://other.test/mcp' } });
    expect(input('访问凭据').value).toBe('');
    expect(input('访问凭据').placeholder).toBe('');
    expect(input('访问凭据').closest('.plugin-credential-field')!.textContent).toContain('未配置');
    expect(container.textContent).not.toContain('更换服务地址后需重新填写访问凭据');
  });

  it('tests only the saved version and invalidates a result on edits and on a changed remote revision', async () => {
    await draw(); await click('测试连接');
    const call = fetchMock.mock.calls.find(item => item[0].endsWith('/test'))!;
    expect(JSON.parse(call[1].body as string)).toEqual({ authorization_digest: 'digest', revision: 1 });
    expect(container.textContent).toContain('连接通过 · 20 个工具');
    await fill('访问凭据', 'new-value');
    expect(container.textContent).not.toContain('连接通过'); expect(button('测试连接').disabled).toBe(true);
    await click('撤销修改 访问凭据'); await click('测试连接');
    expect(container.textContent).toContain('连接通过');
    current = configuration(2);
    await act(async () => window.dispatchEvent(new Event('focus')));
    expect(container.textContent).not.toContain('连接通过');
  });

  it('keeps deployment configuration read only and lets the user stage and undo a clear', async () => {
    current.fields[0] = { ...current.fields[0], source: 'external', state: 'deployment', editable: false };
    await draw();
    expect(input('服务地址')).toBeNull(); expect(container.textContent).toContain('由部署管理');
    expect(container.textContent).toContain('https://docs.test/mcp');
    await click('清除 访问凭据'); expect(fetchMock.mock.calls.every(call => call[1].method === 'GET')).toBe(true);
    expect(container.textContent).toContain('保存时清除'); expect(input('访问凭据').disabled).toBe(true);
    await click('撤销修改 访问凭据'); expect(button('保存配置').disabled).toBe(true);
    await click('清除 访问凭据'); await click('保存配置');
    const mutation = fetchMock.mock.calls.find(call => call[1].method === 'PUT')!;
    expect(JSON.parse(mutation[1].body as string).fields).toEqual({ api_key: { action: 'clear' } });
  });

  it('refreshes after optimistic conflicts without echoing submitted values', async () => {
    await draw(); await fill('访问凭据', 'sensitive-conflict-value');
    fetchMock.mockResolvedValueOnce(response(409, { error: 'conflict sensitive-conflict-value' }));
    current = configuration(5); await click('保存配置');
    expect(container.textContent).toContain('已被其他操作更新'); expect(container.innerHTML).not.toContain('sensitive-conflict-value');
    expect(input('访问凭据').value).toBe('');
    await click('测试连接');
    const call = fetchMock.mock.calls.find(item => item[0].endsWith('/test'))!;
    expect(JSON.parse(call[1].body as string).revision).toBe(5);
  });

  it('clears detached secret inputs and ignores an in-flight result after switching plugins', async () => {
    await draw(); await fill('访问凭据', 'departed-secret'); const departedInput = input('访问凭据');
    current = { ...configuration(), plugin: 'other', fields: [{ ...configuration().fields[1], label: '另一个凭据' }] };
    await draw('other'); expect(departedInput.value).toBe('');
    current = configuration(); await draw();
    let complete: (value: Response) => void = () => undefined;
    fetchMock.mockImplementationOnce(() => new Promise<Response>(resolve => { complete = resolve; }));
    await click('测试连接');
    const signal = fetchMock.mock.calls.at(-1)![1].signal as AbortSignal;
    current = { ...configuration(), plugin: 'other', fields: [{ ...configuration().fields[1], label: '另一个凭据' }] };
    await draw('other'); expect(departedInput.value).toBe(''); expect(signal.aborted).toBe(true);
    await act(async () => complete(response(200, { plugin: 'docmost', revision: 1, current: true,
      results: [{ target: 'old-target', status: 'verified', message: '旧测试' }] })));
    expect(container.textContent).toContain('另一个凭据'); expect(container.textContent).not.toContain('旧测试');
  });

  it('shows read failures and refuses to present a superseded test as verified', async () => {
    fetchMock.mockResolvedValueOnce(response(503, { error: '配置存储暂时不可读' }));
    await draw(); expect(container.textContent).toContain('配置存储暂时不可读');
    expect(container.querySelector('input')).toBeNull();
    await click('刷新配置');
    fetchMock.mockResolvedValueOnce(response(200, { plugin: 'docmost', revision: 1, current: false,
      results: [{ target: 'docmost', status: 'verified', tool_count: 20, message: '旧版本通过' }] }));
    await click('测试连接');
    expect(container.textContent).toContain('本次测试结果已失效'); expect(container.textContent).not.toContain('旧版本通过');
    expect(container.textContent).not.toContain('连接通过');
  });


  it('aborts a submitted rotation on plugin switch and ignores its eventual response', async () => {
    const changed = vi.fn();
    await act(async () => root.render(<PluginConfiguration pluginId="docmost" onChanged={changed} />));
    await fill('访问凭据', 'departing-rotation');
    const departed = input('访问凭据');
    let complete: (value: Response) => void = () => undefined;
    fetchMock.mockImplementationOnce(() => new Promise<Response>(resolve => { complete = resolve; }));
    await click('保存配置');
    const signal = fetchMock.mock.calls.at(-1)![1].signal as AbortSignal;
    expect(departed.value).toBe('');
    current = { ...configuration(), plugin: 'other', fields: [{ ...configuration().fields[1], label: '另一个凭据' }] };
    await draw('other');
    expect(signal.aborted).toBe(true);
    await act(async () => complete(response(200, configuration(2))));
    expect(changed).not.toHaveBeenCalled();
    expect(container.textContent).toContain('另一个凭据');
    expect(container.textContent).not.toContain('配置已保存');
  });

  it('accepts only the latest refresh and refreshes external values even at the same revision', async () => {
    await draw(); await click('测试连接');
    let complete: (value: Response) => void = () => undefined;
    fetchMock.mockImplementationOnce(() => new Promise<Response>(resolve => { complete = resolve; }));
    await act(async () => window.dispatchEvent(new Event('focus')));
    const signal = fetchMock.mock.calls.filter(call => call[0].endsWith('/configuration')).at(-1)![1].signal as AbortSignal;
    current = configuration(); current.fields[0].value = 'https://changed.test/mcp';
    await act(async () => window.dispatchEvent(new Event('focus')));
    expect(signal.aborted).toBe(true);
    expect(input('服务地址').value).toBe('https://changed.test/mcp');
    expect(container.textContent).not.toContain('连接通过');
    await act(async () => complete(response(200, configuration())));
    expect(input('服务地址').value).toBe('https://changed.test/mcp');
  });

  it('does not refresh over an unfinished draft or display secrets from failed connection requests', async () => {
    await draw(); await fill('访问凭据', 'unfinished-rotation');
    const requests = fetchMock.mock.calls.filter(call => call[0].endsWith('/configuration')).length;
    await act(async () => window.dispatchEvent(new Event('focus')));
    expect(fetchMock.mock.calls.filter(call => call[0].endsWith('/configuration'))).toHaveLength(requests);
    expect(input('访问凭据').value).toBe('unfinished-rotation');
    await click('撤销修改 访问凭据');
    fetchMock.mockResolvedValueOnce(response(502, { error: 'provider rejected unfinished-rotation' }));
    await click('测试连接');
    expect(container.textContent).toContain('连接测试未完成');
    expect(container.textContent).not.toContain('unfinished-rotation');
  });


  it('invalidates an in-flight test when an additional legacy credential changes', async () => {
    const legacy = { plugin: 'docmost', targets: [{ target: 'docmost', kind: 'mcp', authorization_digest: 'digest',
      fields: [{ field: 'EXTRA_KEY', state: 'configured', editable: true }] }] };
    let complete: (value: Response) => void = () => undefined;
    fetchMock.mockImplementation(async (path: string, options: RequestInit) => {
      if (path.endsWith('/configuration')) return response(200, current);
      if (path.endsWith('/test')) return new Promise<Response>(resolve => { complete = resolve; });
      if (path.endsWith('/credentials') || options.method === 'PUT') return response(200, legacy);
      throw new Error('Unexpected request');
    });
    await draw(); await click('测试连接');
    const signal = fetchMock.mock.calls.find(call => call[0].endsWith('/test'))![1].signal as AbortSignal;
    await click('轮换 EXTRA_KEY');
    container.querySelector<HTMLInputElement>('input[aria-label="EXTRA_KEY 新值"]')!.value = 'extra-rotation';
    await click('保存凭据');
    expect(signal.aborted).toBe(true);
    await act(async () => complete(response(200, { plugin: 'docmost', revision: 1, current: true,
      results: [{ target: 'docmost', status: 'verified', message: '旧凭据测试通过' }] })));
    expect(container.textContent).not.toContain('旧凭据测试通过');
    expect(button('测试连接').disabled).toBe(false);
  });


  it('tests legacy MCP plugins without a configuration form and refreshes revision after credential rotation', async () => {
    current = { ...configuration(), fields: [], targets: [{ target: 'old', transport: 'http' }] };
    const legacy = { plugin: 'docmost', targets: [{ target: 'old', kind: 'mcp', authorization_digest: 'digest',
      fields: [{ field: 'LEGACY_KEY', state: 'configured', editable: true }] }] };
    fetchMock.mockImplementation(async (path: string, options: RequestInit) => {
      if (path.endsWith('/configuration')) return response(200, current);
      if (path.endsWith('/test')) return response(200, { plugin: 'docmost', revision: current.revision, current: true,
        results: [{ target: 'old', status: 'verified', tool_count: 3, message: '旧插件连接通过' }] });
      if (options.method === 'PUT') current = { ...current, revision: current.revision + 1 };
      return response(200, legacy);
    });
    await draw();
    const section = container.querySelector('[aria-label="插件连接配置"]')!;
    expect(section).not.toBeNull(); expect(section.querySelector('form')).toBeNull();
    expect([...section.querySelectorAll('button')].some(button => button.textContent === '保存配置')).toBe(false);
    await click('测试连接'); expect(container.textContent).toContain('连接通过 · 3 个工具');
    await click('轮换 LEGACY_KEY');
    container.querySelector<HTMLInputElement>('input[aria-label="LEGACY_KEY 新值"]')!.value = 'new-legacy-secret';
    await click('保存凭据');
    expect(container.textContent).not.toContain('连接通过 · 3 个工具');
    expect(fetchMock.mock.calls.filter(call => call[0].endsWith('/configuration'))).toHaveLength(2);
    await click('测试连接');
    const lastTest = fetchMock.mock.calls.filter(call => call[0].endsWith('/test')).at(-1)!;
    expect(JSON.parse(lastTest[1].body as string)).toEqual({ authorization_digest: 'digest', revision: 2 });
    expect(container.textContent).toContain('连接通过 · 3 个工具');
  });

  it('retains legacy credential management when the plugin has no configuration declaration', async () => {
    current = { ...configuration(), fields: [], targets: [] };
    fetchMock.mockImplementation(async (path: string) => path.endsWith('/configuration') ? response(200, current)
      : response(200, { plugin: 'docmost', targets: [{ target: 'old', kind: 'mcp', authorization_digest: 'digest',
        fields: [{ field: 'LEGACY_KEY', state: 'missing', editable: true }] }] }));
    await draw(); expect(container.querySelector('[aria-label="插件连接配置"]')).toBeNull();
    expect(container.textContent).toContain('设置 LEGACY_KEY');
  });
});
