/** @vitest-environment jsdom */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ModelProviderSettings } from './ModelProviderSettings';

const fields = { endpoint: 'https://provider.example/v1', wire_api: 'chat', flash: 'fast-model', pro: '' };
const original = {
  configured: true, fields: { ...fields, model: fields.flash }, api_key_present: true,
  deployment_fields: ['endpoint', 'wire_api', 'model', 'api_key'],
  providers: {
    default: { fields, api_key_present: true, configured: false, source: 'legacy' },
    backup: { fields: { endpoint: '', wire_api: 'responses', flash: '', pro: '' }, api_key_present: false, configured: false, source: 'empty' },
  },
  execution: { default_reference: 'models.default.flash', source: 'legacy', configured: true, fields: { endpoint: fields.endpoint, wire_api: fields.wire_api, model: fields.flash } as { endpoint: string; wire_api: string; model: string } | null, api_key_present: true, error: null as string | null },
  retained_providers: [{ id: 'legacy-provider', endpoint: fields.endpoint, wire_api: 'chat', current: true, api_key_present: true, deployment: false, slot: undefined as 'default' | 'backup' | undefined }],
};
type Status = typeof original;
type Slot = keyof Status['providers'];
const response = (body: unknown, status = 200) => ({ ok: status >= 200 && status < 300, status, json: async () => body }) as Response;
const deferred = <T,>() => { let resolve!: (value: T) => void; const promise = new Promise<T>(done => { resolve = done; }); return { promise, resolve }; };

describe('Model provider settings', () => {
  let root: Root;
  let container: HTMLDivElement;
  let fetchMock: ReturnType<typeof vi.fn>;
  let server: Status;
  let mounted: boolean;
  beforeEach(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    container = document.createElement('div'); document.body.appendChild(container); root = createRoot(container); mounted = true;
    sessionStorage.clear(); localStorage.clear(); server = structuredClone(original);
    fetchMock = vi.fn((path: string, options: RequestInit = {}) => {
      if (path === '/settings/model') return Promise.resolve(response(structuredClone(server)));
      if (path.endsWith('/test')) return Promise.resolve(response({ ok: true, elapsed_ms: 12, saved: false }));
      if (path.startsWith('/settings/model/credentials/')) {
        const target = server.retained_providers.find(provider => provider.id === decodeURIComponent(path.split('/').at(-1)!));
        if (target) {
          const present = options.method === 'PUT';
          server.retained_providers.forEach(provider => { if (provider.id === target.id || (target.slot && provider.slot === target.slot && provider.endpoint === target.endpoint && provider.wire_api === target.wire_api)) provider.api_key_present = present; });
          const slot = target.slot ?? 'default'; const current = server.providers[slot];
          if ((target.slot || current.source === 'legacy') && current.fields.endpoint === target.endpoint && current.fields.wire_api === target.wire_api) {
            current.api_key_present = present; current.configured = !!(current.fields.endpoint && current.fields.flash && current.fields.pro && present);
            if (slot === 'default') server.execution = { ...server.execution, api_key_present: present, configured: !!(current.fields.endpoint && current.fields.flash && present), fields: present ? { endpoint: current.fields.endpoint, wire_api: current.fields.wire_api, model: current.fields.flash } : null, error: present ? null : 'default Flash has no key' };
          }
        }
      }
      const slot = path.split('/').at(-1) as Slot;
      if (options.method === 'PUT' && (slot === 'default' || slot === 'backup')) {
        const body = JSON.parse(options.body as string);
        const prior = server.providers[slot];
        const sameConnection = body.endpoint === prior.fields.endpoint && body.wire_api === prior.fields.wire_api;
        const present = body.api_key.action === 'set' || (body.api_key.action === 'keep' && sameConnection && prior.api_key_present);
        server.providers[slot] = { fields: { endpoint: body.endpoint, wire_api: body.wire_api, flash: body.flash, pro: body.pro }, api_key_present: present, configured: !!(body.endpoint && body.flash && body.pro && present), source: 'host' };
        if (slot === 'default') {
          const configured = !!(body.endpoint && body.flash && present);
          server.execution = { default_reference: 'models.default.flash', source: 'host', configured, fields: configured ? { endpoint: body.endpoint, wire_api: body.wire_api, model: body.flash } : null, api_key_present: present, error: configured ? null : 'default Flash is incomplete' };
        }
      }
      return Promise.resolve(response(structuredClone(server)));
    });
    vi.stubGlobal('fetch', fetchMock);
  });
  afterEach(async () => { if (mounted) await act(async () => root.unmount()); container.remove(); vi.unstubAllGlobals(); });
  const draw = async (refreshVersion = 0) => { await act(async () => root.render(<ModelProviderSettings refreshVersion={refreshVersion} />)); };
  const group = (slot: Slot) => container.querySelector<HTMLFieldSetElement>(`fieldset[aria-label="${slot}"]`)!;
  const button = (text: string, within: ParentNode = container) => [...within.querySelectorAll<HTMLButtonElement>('button')].find(item => item.textContent === text)!;
  const input = (slot: Slot, label: string) => [...group(slot).querySelectorAll('label')].find(item => item.childNodes[0].textContent === label)!.querySelector<HTMLInputElement>('input')!;
  const key = (slot: Slot) => input(slot, '模型服务 API key');
  const setInput = async (element: HTMLInputElement, value: string) => {
    await act(async () => { Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(element, value); element.dispatchEvent(new Event('input', { bubbles: true })); });
  };
  const click = async (text: string, within: ParentNode = container) => { await act(async () => button(text, within).click()); };
  const writes = () => fetchMock.mock.calls.filter(([, options]) => ['PUT', 'POST', 'DELETE'].includes((options as RequestInit)?.method ?? ''));

  it('saves partial default settings independently of deployment locks without testing or starting a session', async () => {
    await draw();
    expect(input('default', 'Flash 模型').value).toBe('fast-model');
    expect(input('default', 'Pro 模型').value).toBe('');
    expect(input('default', '服务地址').readOnly).toBe(false);
    expect(key('default').disabled).toBe(false);
    expect(key('default').autocomplete).toBe('off');
    await setInput(input('backup', '服务地址'), 'https://backup.example/v1');
    await setInput(input('backup', 'Pro 模型'), 'backup-draft');
    await setInput(key('backup'), 'backup-private-draft');
    await setInput(key('default'), 'default-private-key');
    await click('保存配置', group('default'));
    expect(writes().map(([path]) => path)).toEqual(['/settings/model/providers/default']);
    expect(JSON.parse((writes()[0][1] as RequestInit).body as string)).toEqual({ ...fields, api_key: { action: 'set', value: 'default-private-key' } });
    expect(key('default').value).toBe('');
    expect(input('backup', 'Pro 模型').value).toBe('backup-draft');
    expect(key('backup').value).toBe('backup-private-draft');
    expect(group('default').textContent).toContain('配置已保存');
    expect(group('default').textContent).toContain('待补全');
    expect(container.textContent).toContain('Pilot 和未指定模型的节点使用 default.flash');
    expect(container.textContent).toContain('下一轮使用新设置，当前轮保持原设置');
    expect(container.textContent).toContain('backup 仅在工作流明确选择时使用');
    expect(group('default').textContent).toContain('下一轮使用新设置');
    expect(container.textContent).toContain('下一轮默认模型：fast-model');
    expect(container.innerHTML).not.toContain('default-private-key');
    expect(localStorage.length).toBe(0); expect(sessionStorage.length).toBe(0);
    expect(fetchMock.mock.calls.some(([path]) => String(path).startsWith('/sessions'))).toBe(false);
  });

  it('uses the execution projection for default readiness while allowing an incomplete Pro model', async () => {
    await draw();
    expect(group('default').textContent).toContain('待补全');
    expect(container.textContent).toContain('下一轮默认模型：fast-model');
    await setInput(input('default', 'Flash 模型'), '');
    await click('保存配置', group('default'));
    expect(group('default').textContent).toContain('配置已保存');
    expect(group('default').textContent).toContain('请补全服务地址、Flash 模型和密钥');
    expect(container.textContent).toContain('下一轮默认模型尚不可用');
    expect(container.textContent).not.toContain('下一轮默认模型：fast-model');
    expect(server.configured).toBe(true);
    expect(server.fields.model).toBe('fast-model');
    expect(writes()).toHaveLength(1);
  });

  it('keeps independent credentials even when both slots use the same endpoint and protocol', async () => {
    server.providers.backup.fields = { ...fields, pro: 'backup-pro' };
    await draw();
    expect(key('default').placeholder).toContain('已保存');
    expect(key('backup').placeholder).not.toContain('已保存');
    await setInput(key('backup'), 'separate-account-key');
    await click('保存配置', group('backup'));
    expect(writes()[0][0]).toBe('/settings/model/providers/backup');
    expect(key('default').placeholder).toContain('已保存');
    expect(key('backup').placeholder).toContain('已保存');
    expect(group('backup').textContent).toContain('已配置');
    expect(group('backup').textContent).toContain('工作流明确选择 backup 时');
    expect(container.textContent).toContain('下一轮默认模型：fast-model');
  });

  it.each([
    ['model connection test timed out', '测试超时，模型服务未及时返回结果'],
    ['model provider response timed out', '已连接到模型服务，但在 20 秒测试时限内没有收到完整模型结果'],
    ['the selected provider has no API key configured', '请先填写此服务的 API key'],
    ['the selected tier has no model configured', '请先填写要测试的模型名称'],
    ['model connection test failed; check the endpoint and network', '无法连接模型服务'],
    ['model provider returned HTTP 401; check the endpoint, key, model and protocol', '模型服务拒绝了此 API key（401）'],
    ['model provider returned HTTP 403; check the endpoint, key, model and protocol', '模型服务拒绝访问（403）'],
    ['model provider returned HTTP 404; check the endpoint, key, model and protocol', '不支持当前请求地址或接口（404）'],
    ['model provider returned HTTP 405; check the endpoint, key, model and protocol', '不支持当前请求地址或接口（405）'],
    ['model provider returned HTTP 429; check the endpoint, key, model and protocol', '模型服务限制了请求（429）'],
    ['model provider returned HTTP 503; check the endpoint, key, model and protocol', '模型服务暂时出错（503）'],
    ['model provider returned HTTP 400; check the endpoint, key, model and protocol', '模型服务拒绝了测试请求（400）'],
  ])('explains the known probe failure %s without exposing raw errors', async (error, expected) => {
    await draw();
    fetchMock.mockImplementationOnce(() => Promise.resolve(response({ error }, 400)));
    await click('测试 Flash', group('default'));
    expect(group('default').querySelector('[role="alert"]')?.textContent).toContain(expected);
    expect(group('default').textContent).not.toContain(error);
    expect(writes()).toHaveLength(1);
    expect((writes()[0][1] as RequestInit).method).toBe('POST');
  });

  it('refreshes clean drafts while preserving fields and keys edited during an in-flight refresh', async () => {
    await draw();
    const pending = deferred<Response>();
    fetchMock.mockImplementationOnce(() => pending.promise);
    await draw(1);
    await setInput(input('default', 'Flash 模型'), 'unsaved-flash');
    await setInput(key('default'), 'unsaved-refresh-key');
    const next = structuredClone(server);
    next.providers.default.fields.flash = 'server-default-flash';
    next.providers.backup.fields.pro = 'server-backup-pro';
    await act(async () => pending.resolve(response(next)));
    expect(input('default', 'Flash 模型').value).toBe('unsaved-flash');
    expect(key('default').value).toBe('unsaved-refresh-key');
    expect(input('backup', 'Pro 模型').value).toBe('server-backup-pro');
    await click('放弃修改', group('default'));
    expect(input('default', 'Flash 模型').value).toBe('server-default-flash');
    expect(key('default').value).toBe('');
    expect(button('放弃修改', group('default'))).toBeUndefined();
    expect(writes()).toHaveLength(0);
  });

  it('keeps an unchanged key-only draft when refreshed settings change', async () => {
    await draw(); await setInput(key('default'), 'key-only-draft');
    server.providers.default.fields.flash = 'new-server-flash';
    await draw(1);
    expect(input('default', 'Flash 模型').value).toBe('fast-model');
    expect(key('default').value).toBe('key-only-draft');
    await click('放弃修改', group('default'));
    expect(input('default', 'Flash 模型').value).toBe('new-server-flash');
    expect(key('default').value).toBe('');
  });

  it.each(['endpoint', 'protocol'])('stops advertising the saved key after a changed %s and saves keep without copying credentials', async change => {
    await draw();
    if (change === 'endpoint') await setInput(input('default', '服务地址'), 'https://new-provider.example/v1');
    else await act(async () => { const select = group('default').querySelector('select')!; select.value = 'responses'; select.dispatchEvent(new Event('change', { bubbles: true })); });
    expect(key('default').placeholder).not.toContain('已保存');
    expect(group('default').textContent).toContain('保存时会移除此槽的原密钥');
    await click('保存配置', group('default'));
    expect(JSON.parse((writes()[0][1] as RequestInit).body as string).api_key).toEqual({ action: 'keep' });
    expect(key('default').placeholder).not.toContain('已保存');
    expect(button('清除密钥', group('default')).disabled).toBe(true);
  });

  it('tests each selected tier using drafts without saving or discarding the key draft', async () => {
    await draw(); await setInput(input('backup', '服务地址'), 'https://backup.example/v1');
    await setInput(input('backup', 'Flash 模型'), 'draft-flash'); await setInput(input('backup', 'Pro 模型'), 'draft-pro');
    await setInput(key('backup'), 'test-private-key');
    await click('测试 Flash', group('backup')); await click('测试 Pro', group('backup'));
    expect(writes().map(([path]) => path)).toEqual(['/settings/model/providers/backup/test', '/settings/model/providers/backup/test']);
    expect(writes().map(([, options]) => JSON.parse((options as RequestInit).body as string))).toEqual(['flash', 'pro'].map(tier => ({ endpoint: 'https://backup.example/v1', wire_api: 'responses', flash: 'draft-flash', pro: 'draft-pro', api_key: { action: 'set', value: 'test-private-key' }, tier })));
    expect(writes().every(([, options]) => (options as RequestInit).method === 'POST')).toBe(true);
    expect(group('backup').textContent).toContain('Pro 测试通过（12 ms）。未保存配置或密钥');
    expect(key('backup').value).toBe('test-private-key');
    expect(group('backup').textContent).toContain('未保存');
  });

  it('does not show provider errors or encoded credentials returned in a failed test', async () => {
    await draw(); const secret = 'private/test?key=123';
    await setInput(key('default'), secret);
    fetchMock.mockImplementationOnce(() => Promise.resolve(response({ error: `provider echoed ${secret} and ${encodeURIComponent(secret)} and unrelated-private-secret` }, 400)));
    await click('测试 Flash', group('default'));
    expect(group('default').textContent).toContain('测试未完成');
    expect(group('default').textContent).not.toContain('服务返回 400');
    expect(container.textContent).not.toContain(secret);
    expect(container.textContent).not.toContain(encodeURIComponent(secret));
    expect(container.textContent).not.toContain('unrelated-private-secret');
    expect(writes()).toHaveLength(1);
    expect(localStorage.length).toBe(0); expect(sessionStorage.length).toBe(0);
  });

  it('clears only the saved slot key without persisting either slot’s field drafts', async () => {
    await draw();
    await setInput(input('default', 'Flash 模型'), 'default-unsaved-flash');
    await setInput(input('backup', '服务地址'), 'https://draft-backup.example/v1');
    await setInput(key('backup'), 'backup-key-draft');
    await click('清除密钥', group('default'));
    expect(writes().map(([path]) => path)).toEqual(['/settings/model/providers/default']);
    expect(JSON.parse((writes()[0][1] as RequestInit).body as string)).toEqual({ ...fields, api_key: { action: 'clear' } });
    expect(input('default', 'Flash 模型').value).toBe('default-unsaved-flash');
    expect(input('backup', '服务地址').value).toBe('https://draft-backup.example/v1');
    expect(key('backup').value).toBe('backup-key-draft');
    expect(key('default').placeholder).not.toContain('已保存');
    expect(container.textContent).toContain('下一轮默认模型尚不可用');
  });

  it('ignores an older refresh response that arrives after a successful save', async () => {
    await draw(); const older = structuredClone(server); const pending = deferred<Response>();
    fetchMock.mockImplementationOnce(() => pending.promise);
    await draw(1);
    await setInput(input('default', 'Pro 模型'), 'newly-saved-pro');
    await click('保存配置', group('default'));
    await act(async () => pending.resolve(response(older)));
    expect(input('default', 'Pro 模型').value).toBe('newly-saved-pro');
    expect(group('default').textContent).toContain('已配置');
  });

  it('allows independent pending saves and merges only the slot each response changed', async () => {
    await draw(); const pending = deferred<Response>(); const defaultResult = structuredClone(server);
    defaultResult.providers.default = { ...defaultResult.providers.default, fields: { ...fields, pro: 'saved-default-pro' }, configured: true, source: 'host' };
    await setInput(input('default', 'Pro 模型'), 'saved-default-pro');
    fetchMock.mockImplementationOnce(() => pending.promise);
    await click('保存配置', group('default'));
    expect(group('default').disabled).toBe(true); expect(group('backup').disabled).toBe(false);
    await setInput(input('backup', '服务地址'), 'https://backup.example/v1');
    await setInput(input('backup', 'Flash 模型'), 'saved-backup-flash'); await setInput(input('backup', 'Pro 模型'), 'saved-backup-pro');
    await setInput(key('backup'), 'saved-backup-key');
    await click('保存配置', group('backup'));
    server.providers.default = defaultResult.providers.default;
    await act(async () => pending.resolve(response(defaultResult)));
    expect(input('default', 'Pro 模型').value).toBe('saved-default-pro');
    expect(input('backup', 'Flash 模型').value).toBe('saved-backup-flash');
    expect(key('backup').placeholder).toContain('已保存');
    expect(group('backup').textContent).toContain('已配置');
    expect(fetchMock.mock.calls.filter(([path]) => path === '/settings/model')).toHaveLength(2);
  });

  it('offers a retry after an initial load failure', async () => {
    fetchMock.mockImplementationOnce(() => Promise.reject(new Error('untrusted-server-error')));
    await draw(); expect(container.textContent).toContain('模型设置读取失败');
    expect(container.querySelector('fieldset')).toBeNull();
    await click('重试读取模型设置');
    expect(group('default')).not.toBeNull(); expect(group('backup')).not.toBeNull();
    expect(container.textContent).not.toContain('untrusted-server-error');
  });

  it('does not let a delayed slot response restore a cleared retained-session credential', async () => {
    await draw(); const pending = deferred<Response>(); const olderSlotResponse = structuredClone(server);
    olderSlotResponse.providers.default = { ...olderSlotResponse.providers.default, fields: { ...fields, pro: 'saved-pro' }, source: 'host', configured: true };
    await setInput(input('default', 'Pro 模型'), 'saved-pro');
    fetchMock.mockImplementationOnce(() => pending.promise);
    await click('保存配置', group('default'));
    const retained = container.querySelector<HTMLDetailsElement>('.settings-retained')!;
    const cleared = structuredClone(server); cleared.retained_providers[0].api_key_present = false;
    fetchMock.mockImplementationOnce(() => Promise.resolve(response(cleared)));
    await click('清除密钥', retained);
    expect(retained.textContent).toContain('密钥未配置');
    server.retained_providers = cleared.retained_providers;
    server.providers.default = olderSlotResponse.providers.default;
    await act(async () => pending.resolve(response(olderSlotResponse)));
    expect(retained.textContent).toContain('密钥未配置');
    expect(button('清除密钥', retained).disabled).toBe(true);
    expect(input('default', 'Pro 模型').value).toBe('saved-pro');
  });

  it('does not let a delayed retained-credential response replace a newly imported slot', async () => {
    await draw(); const pending = deferred<Response>(); const olderCredentialResponse = structuredClone(server);
    olderCredentialResponse.retained_providers[0].api_key_present = false;
    olderCredentialResponse.providers.default.api_key_present = false;
    fetchMock.mockImplementationOnce(() => pending.promise);
    const retained = container.querySelector<HTMLDetailsElement>('.settings-retained')!;
    await click('清除密钥', retained);
    await setInput(input('default', 'Pro 模型'), 'new-imported-pro');
    await click('保存配置', group('default'));
    server.retained_providers = olderCredentialResponse.retained_providers;
    await act(async () => pending.resolve(response(olderCredentialResponse)));
    expect(input('default', 'Pro 模型').value).toBe('new-imported-pro');
    expect(key('default').placeholder).toContain('已保存');
    expect(group('default').textContent).toContain('已配置');
    expect(retained.textContent).toContain('密钥未配置');
  });

  it('shows an upgrade message instead of crashing on an older Host response', async () => {
    fetchMock.mockImplementationOnce(() => Promise.resolve(response({ ...original, providers: undefined })));
    await draw(); expect(container.textContent).toContain('当前服务版本尚未支持这项设置，请更新并重启服务');
    expect(container.querySelector('fieldset')).toBeNull();
    await click('重试读取模型设置'); expect(group('default')).not.toBeNull();
  });

  it('preserves retained-session key drafts through refresh and slot saves, and uses the legacy credential endpoint', async () => {
    await draw();
    const retained = container.querySelector<HTMLDetailsElement>('.settings-retained')!;
    const retainedKey = retained.querySelector<HTMLInputElement>('input[type="password"]')!;
    expect(container.querySelectorAll('fieldset')).toHaveLength(2);
    await setInput(retainedKey, 'retained-key-draft');
    await draw(1); expect(retainedKey.value).toBe('retained-key-draft');
    await click('保存配置', group('backup')); expect(retainedKey.value).toBe('retained-key-draft');
    await setInput(input('backup', 'Pro 模型'), 'independent-backup-draft');
    await click('保存密钥', retained);
    const save = writes().find(([path]) => path === '/settings/model/credentials/legacy-provider')!;
    expect((save[1] as RequestInit).method).toBe('PUT');
    expect(JSON.parse((save[1] as RequestInit).body as string)).toEqual({ value: 'retained-key-draft' });
    expect(retainedKey.value).toBe('');
    await click('清除密钥', retained);
    expect(writes().some(([path, options]) => path === '/settings/model/credentials/legacy-provider' && (options as RequestInit).method === 'DELETE')).toBe(true);
    expect(input('backup', 'Pro 模型').value).toBe('independent-backup-draft');
  });

  it('labels historical slot credentials without presenting another editable provider configuration', async () => {
    server.retained_providers[0].slot = 'backup';
    server.retained_providers[0].current = false;
    server.retained_providers[0].id = 'slot:backup:old-service';
    server.retained_providers.push({ ...server.retained_providers[0], id: 'slot:backup', current: true });
    await draw();
    const retained = container.querySelector<HTMLDetailsElement>('.settings-retained')!;
    expect(retained.textContent).toContain('backup · chat');
    expect(retained.querySelector('input')?.getAttribute('aria-label')).toBe(`backup ${fields.endpoint} 已有会话 API key`);
    expect(container.querySelectorAll('fieldset')).toHaveLength(2);
    expect(retained.querySelectorAll('.settings-provider-row')).toHaveLength(1);
  });

  it('updates the current default key when a historical credential targets the same service', async () => {
    server.providers.default = { ...server.providers.default, source: 'host' };
    server.execution.source = 'host';
    server.retained_providers[0] = { ...server.retained_providers[0], id: 'slot:default:old-service', slot: 'default', current: false };
    await draw();
    await setInput(input('backup', 'Pro 模型'), 'untouched-backup-draft');
    const retained = container.querySelector<HTMLDetailsElement>('.settings-retained')!;
    await click('清除密钥', retained);
    expect(key('default').placeholder).not.toContain('已保存');
    expect(container.textContent).toContain('下一轮默认模型尚不可用');
    expect(input('backup', 'Pro 模型').value).toBe('untouched-backup-draft');
    expect(fetchMock.mock.calls.filter(([path]) => path === '/settings/model')).toHaveLength(1);
  });

  it('keeps a new service key when an old service credential is cleared', async () => {
    server.providers.default = { ...server.providers.default, source: 'host' };
    server.execution.source = 'host';
    server.retained_providers[0] = { ...server.retained_providers[0], id: 'slot:default:old-service', slot: 'default', current: false, endpoint: 'https://old.example/v1' };
    await draw();
    await click('清除密钥', container.querySelector('.settings-retained')!);
    expect(key('default').placeholder).toContain('已保存');
    expect(container.textContent).toContain('下一轮默认模型：fast-model');
  });

  it('adds newly retained services from a slot save without replacing other credentials', async () => {
    await draw();
    const next = structuredClone(server);
    next.providers.default = { ...next.providers.default, fields: { ...fields, endpoint: 'https://new.example/v1' }, api_key_present: false, source: 'host' };
    next.retained_providers.push({ id: 'slot:default:old-service', endpoint: fields.endpoint, wire_api: 'chat', current: false, api_key_present: true, deployment: false, slot: 'default' });
    next.execution = { ...next.execution, source: 'host', configured: false, api_key_present: false, fields: null, error: 'default Flash has no key' };
    await setInput(input('default', '服务地址'), 'https://new.example/v1');
    fetchMock.mockImplementationOnce(() => Promise.resolve(response(next)));
    await click('保存配置', group('default'));
    const retained = container.querySelector('.settings-retained')!;
    expect(retained.querySelectorAll('.settings-provider-row')).toHaveLength(2);
    expect(retained.textContent).toContain('default · chat');
    expect(retained.textContent).toContain('用于恢复尚未完成的旧任务');
  });

  it('aborts pending requests and clears every private input on unmount', async () => {
    await draw(); await setInput(key('default'), 'pending-default-key'); await setInput(key('backup'), 'pending-backup-key');
    const retainedKey = container.querySelector<HTMLInputElement>('.settings-retained input[type="password"]')!;
    await setInput(retainedKey, 'pending-retained-key');
    const defaultKey = key('default'); const backupKey = key('backup'); const pending = deferred<Response>();
    fetchMock.mockImplementationOnce(() => pending.promise);
    await click('测试 Flash', group('default'));
    const signal = (writes()[0][1] as RequestInit).signal!;
    await act(async () => root.unmount()); mounted = false;
    expect(signal.aborted).toBe(true);
    expect(defaultKey.value).toBe(''); expect(backupKey.value).toBe(''); expect(retainedKey.value).toBe('');
    await act(async () => pending.resolve(response({ ok: true, elapsed_ms: 1, saved: false })));
  });
});
