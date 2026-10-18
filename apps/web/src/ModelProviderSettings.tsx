import { useEffect, useRef, useState, type FormEvent } from 'react';
import { ApiError, api } from './api';
import './ModelProviderSettings.css';

type Slot = 'default' | 'backup';
type ModelFields = { endpoint: string; wire_api: 'chat' | 'responses'; flash: string; pro: string };
type ProviderStatus = { fields: ModelFields; api_key_present: boolean; configured: boolean; source: 'legacy' | 'host' | 'empty' };
type RetainedProvider = { id: string; endpoint: string; wire_api: string; api_key_present: boolean; current: boolean; deployment: boolean; slot?: Slot };
type ExecutionStatus = { default_reference: 'models.default.flash'; source: 'legacy' | 'host' | 'empty'; configured: boolean; fields: { endpoint: string; wire_api: 'chat' | 'responses'; model: string } | null; api_key_present: boolean; error: string | null };
type ModelStatus = { providers: Record<Slot, ProviderStatus>; retained_providers: RetainedProvider[]; execution: ExecutionStatus };
type Notice = { kind: 'ok' | 'bad'; text: string } | null;
type Change = { slot: Slot; provider?: never } | { slot?: never; provider: RetainedProvider };
type SaveSettings = (change: Change, save: () => Promise<ModelStatus>) => Promise<ModelStatus>;
const slots: Slot[] = ['default', 'backup'];
const sameFields = (left: ModelFields, right: ModelFields) => left.endpoint === right.endpoint && left.wire_api === right.wire_api && left.flash === right.flash && left.pro === right.pro;
const endpointKey = (endpoint: string) => endpoint.trim().replace(/\/+$/, '');
const sameConnection = (left: { endpoint: string; wire_api: string }, right: { endpoint: string; wire_api: string }) => endpointKey(left.endpoint) === endpointKey(right.endpoint) && left.wire_api === right.wire_api;
const affectedCredential = (row: RetainedProvider, target: RetainedProvider) => target.slot ? row.slot === target.slot && sameConnection(row, target) : row.id === target.id;
const withKey = (provider: ProviderStatus, present: boolean): ProviderStatus => ({ ...provider, api_key_present: present, configured: !!(provider.fields.endpoint && provider.fields.flash && provider.fields.pro && present) });

// Recognize only Host-generated diagnostics. Never display arbitrary error text
// or confuse the management API's status with the upstream provider's status.
const failure = (action: string, error: unknown) => {
  if (error instanceof ApiError) {
    if (error.message === 'model connection test timed out') return '测试超时，模型服务未及时返回结果。请稍后重试。';
    if (error.message === 'model provider response timed out') return '已连接到模型服务，但在 20 秒测试时限内没有收到完整模型结果。本次尚未确认模型可用，请稍后重试。';
    if (error.message === 'the selected provider has no API key configured') return '请先填写此服务的 API key，再测试连接。';
    if (error.message === 'the selected tier has no model configured') return '请先填写要测试的模型名称。';
    if (error.message === 'model connection test failed; check the endpoint and network') return '无法连接模型服务，请检查服务地址和网络。';
    const upstream = /^model provider returned HTTP (\d{3}); check the endpoint, key, model and protocol$/.exec(error.message);
    if (upstream) {
      const status = Number(upstream[1]);
      if (status === 401) return '模型服务拒绝了此 API key（401）。请填写该模型服务商提供的密钥，确认密钥有效；这里不使用登录 Anchor 的密钥。';
      if (status === 403) return '模型服务拒绝访问（403）。请检查账户、密钥权限和模型访问权限。';
      if (status === 404 || status === 405) return `模型服务不支持当前请求地址或接口（${status}）。请检查服务地址、接口协议和模型名称。`;
      if (status === 429) return '模型服务限制了请求（429）。请检查账户额度或稍后重试。';
      if (status >= 500) return `模型服务暂时出错（${status}），请稍后重试。`;
      return `模型服务拒绝了测试请求（${status}）。请检查接口协议、模型名称及服务支持的请求参数。`;
    }
  }
  return `${action}未完成。请检查填写的设置和服务连接后重试。`;
};

function ProviderSettings({ slot, status, onSave }: { slot: Slot; status: ProviderStatus; onSave: SaveSettings }) {
  const [draft, setDraft] = useState(status.fields);
  const [keyDraft, setKeyDraft] = useState(false);
  const [busy, setBusy] = useState('');
  const [notice, setNotice] = useState<Notice>(null);
  const form = useRef<HTMLFormElement>(null);
  const privateKey = useRef<HTMLInputElement>(null);
  const draftRef = useRef(draft);
  const baseline = useRef(status);
  const pending = useRef<AbortController | null>(null);
  const alive = useRef(true);
  const dirty = !sameFields(draft, status.fields) || keyDraft;
  const sameConnection = endpointKey(draft.endpoint) === endpointKey(status.fields.endpoint) && draft.wire_api === status.fields.wire_api;
  const savedKeyAvailable = sameConnection && status.api_key_present;

  useEffect(() => {
    const wasDirty = !sameFields(draftRef.current, baseline.current.fields) || !!privateKey.current?.value;
    baseline.current = status;
    if (!wasDirty) { draftRef.current = status.fields; setDraft(status.fields); }
  }, [status]);
  useEffect(() => {
    alive.current = true;
    const input = privateKey.current;
    return () => { alive.current = false; pending.current?.abort(); if (input) input.value = ''; };
  }, []);

  const update = <K extends keyof ModelFields>(field: K, value: ModelFields[K]) => {
    const next = { ...draftRef.current, [field]: value };
    draftRef.current = next; setDraft(next); setNotice(null);
  };
  const discard = () => {
    baseline.current = status; draftRef.current = status.fields; setDraft(status.fields);
    if (privateKey.current) privateKey.current.value = '';
    setKeyDraft(false); setNotice(null);
  };
  const execute = async (action: 'save' | 'clear' | 'flash' | 'pro') => {
    if (pending.current || (action !== 'clear' && !form.current?.reportValidity())) return;
    const controller = new AbortController(); pending.current = controller;
    setBusy(action); setNotice(null);
    const key = privateKey.current?.value ?? '';
    const body = action === 'clear'
      ? { ...status.fields, api_key: { action: 'clear' } }
      : { ...draftRef.current, api_key: key ? { action: 'set', value: key } : { action: 'keep' } };
    try {
      if (action === 'save' || action === 'clear') {
        const next = await onSave({ slot }, () => api<ModelStatus>(`/settings/model/providers/${slot}`, 'PUT', body, controller.signal));
        if (!alive.current || controller.signal.aborted) return;
        if (action === 'save') { baseline.current = next.providers[slot]; draftRef.current = next.providers[slot].fields; setDraft(next.providers[slot].fields); }
        if (privateKey.current) privateKey.current.value = '';
        setKeyDraft(false);
        setNotice({ kind: 'ok', text: action === 'clear'
          ? '此服务的密钥已清除。当前轮继续使用原设置。'
          : slot === 'default'
            ? next.execution.configured ? '配置已保存，下一轮使用新设置；当前轮继续使用原设置。连通性需单独测试。' : '配置已保存。请补全服务地址、Flash 模型和密钥后开始下一轮；当前轮继续使用原设置。'
            : '配置已保存。工作流明确选择 backup 时，下一轮使用新设置；当前轮继续使用原设置。' });
      } else {
        const result = await api<{ ok: boolean; elapsed_ms: number; saved: false }>(`/settings/model/providers/${slot}/test`, 'POST', { ...body, tier: action }, controller.signal);
        if (!alive.current || controller.signal.aborted) return;
        setNotice(result.ok && result.saved === false
          ? { kind: 'ok', text: `${action === 'flash' ? 'Flash' : 'Pro'} 测试通过（${result.elapsed_ms} ms）。未保存配置或密钥。` }
          : { kind: 'bad', text: '测试未通过，请检查服务地址、协议、模型名称和密钥。' });
      }
    } catch (error) {
      if (alive.current && !controller.signal.aborted) setNotice({ kind: 'bad', text: failure(action === 'save' ? '保存' : action === 'clear' ? '清除密钥' : '测试', error) });
    } finally {
      if (pending.current === controller) pending.current = null;
      if (alive.current) setBusy('');
    }
  };
  const submit = (event: FormEvent) => { event.preventDefault(); void execute('save'); };

  return <form ref={form} onSubmit={submit} autoComplete="off" className="model-provider-form">
    <fieldset className="model-provider" aria-label={slot} data-testid={`model-provider-${slot}`} disabled={!!busy}>
      <legend><span>{slot === 'default' ? '默认服务' : '备用服务'}</span><code>{slot}</code></legend>
      <div className="model-provider-heading"><span className={`settings-state ${status.configured ? 'ready' : ''}`}><span />{status.configured ? '已配置' : '待补全'}</span>{dirty && <span className="model-provider-dirty">未保存</span>}</div>
      <div className="model-provider-grid">
        <label className="settings-endpoint">服务地址<input type="url" value={draft.endpoint} placeholder="https://api.example.com/v1" onChange={event => update('endpoint', event.target.value)} /></label>
        <label>接口协议<select value={draft.wire_api} onChange={event => update('wire_api', event.target.value as ModelFields['wire_api'])}><option value="responses">Responses</option><option value="chat">Chat Completions</option></select></label>
        <label className="model-provider-model">Flash 模型<input value={draft.flash} placeholder="服务提供的模型名称" onChange={event => update('flash', event.target.value)} /></label>
        <label className="model-provider-model">Pro 模型<input value={draft.pro} placeholder="服务提供的模型名称" onChange={event => update('pro', event.target.value)} /></label>
        <label className="model-provider-key">模型服务 API key<input ref={privateKey} type="password" name={`model-${slot}-key`} autoComplete="off" spellCheck={false} aria-describedby={`model-${slot}-key-help`} placeholder={savedKeyAvailable ? '已保存；输入新密钥以更换' : '输入模型服务商提供的 API key'} onChange={event => { setKeyDraft(!!event.target.value); setNotice(null); }} /></label>
      </div>
      <p id={`model-${slot}-key-help`} className="settings-field-help">填写模型服务商提供的密钥，例如 DeepSeek 控制台创建的 API key；登录 Anchor 的密钥不能用于调用模型。</p>
      {status.source === 'legacy' && <p className="settings-field-help">已从原配置预填；Pro 模型可稍后补充。</p>}
      {!sameConnection && status.api_key_present && <p className="settings-field-help">地址或协议已更改，请填写新服务的密钥。保存时会移除此槽的原密钥。</p>}
      <div className="settings-actions model-provider-actions">
        <button type="submit" className="settings-primary">{busy === 'save' ? '正在保存…' : '保存配置'}</button>
        <button type="button" onClick={() => void execute('flash')}>{busy === 'flash' ? '正在测试…' : '测试 Flash'}</button>
        <button type="button" onClick={() => void execute('pro')}>{busy === 'pro' ? '正在测试…' : '测试 Pro'}</button>
        {dirty && <button type="button" className="settings-text-button" onClick={discard}>放弃修改</button>}
        <button type="button" className="settings-text-button danger model-provider-clear" disabled={!status.api_key_present} onClick={() => void execute('clear')}>{busy === 'clear' ? '正在清除…' : '清除密钥'}</button>
      </div>
      {notice && <p className={`settings-notice ${notice.kind}`} role={notice.kind === 'bad' ? 'alert' : 'status'}>{notice.text}</p>}
    </fieldset>
  </form>;
}

function RetainedCredentials({ providers, onSave }: { providers: RetainedProvider[]; onSave: SaveSettings }) {
  const [busy, setBusy] = useState('');
  const [notice, setNotice] = useState<Notice>(null);
  const inputs = useRef(new Map<string, HTMLInputElement>());
  const inputRefs = useRef(new Map<string, (input: HTMLInputElement | null) => void>());
  const pending = useRef<AbortController | null>(null);
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    const keys = inputs.current;
    return () => { alive.current = false; pending.current?.abort(); keys.forEach(input => { input.value = ''; }); };
  }, []);
  const inputRef = (id: string) => {
    let ref = inputRefs.current.get(id);
    if (!ref) {
      ref = input => {
        const previous = inputs.current.get(id);
        if (previous && previous !== input) previous.value = '';
        if (input) inputs.current.set(id, input); else inputs.current.delete(id);
      };
      inputRefs.current.set(id, ref);
    }
    return ref;
  };
  const changeKey = async (provider: RetainedProvider, action: 'save' | 'clear') => {
    if (pending.current) return;
    const input = inputs.current.get(provider.id);
    const value = input?.value ?? '';
    if (action === 'save' && !value) { input?.focus(); return; }
    const controller = new AbortController(); pending.current = controller; setBusy(provider.id); setNotice(null);
    try {
      await onSave({ provider }, () => api<ModelStatus>(`/settings/model/credentials/${encodeURIComponent(provider.id)}`, action === 'save' ? 'PUT' : 'DELETE', action === 'save' ? { value } : undefined, controller.signal));
      if (alive.current && !controller.signal.aborted) {
        setNotice({ kind: 'ok', text: action === 'save' ? '旧任务的服务密钥已保存。' : '旧任务的服务密钥已清除；恢复使用时需重新配置。' });
      }
    } catch (error) {
      if (alive.current && !controller.signal.aborted) setNotice({ kind: 'bad', text: failure(action === 'save' ? '保存密钥' : '清除密钥', error) });
    } finally {
      if (input) input.value = '';
      if (pending.current === controller) pending.current = null;
      if (alive.current) setBusy('');
    }
  };
  if (!providers.length) return null;
  return <details className="settings-retained"><summary>旧任务保留的服务凭据</summary>
    <p className="settings-field-help">用于恢复尚未完成的旧任务。普通对话的下一轮使用上方最新配置。</p>
    {providers.map(provider => <div className="settings-provider-row" key={provider.id}>
      <div><code>{provider.endpoint}</code><span>{provider.slot && `${provider.slot} · `}{provider.wire_api} · {provider.api_key_present ? '密钥已保存' : '密钥未配置'}{provider.deployment ? ' · 部署管理' : ''}</span></div>
      <div className="settings-provider-key"><input ref={inputRef(provider.id)} type="password" autoComplete="off" spellCheck={false} aria-label={`${provider.slot ? `${provider.slot} ` : ''}${provider.endpoint} 已有会话 API key`} placeholder={provider.api_key_present ? '输入以更换密钥' : '输入服务密钥'} disabled={!!busy || provider.deployment} /><button type="button" disabled={!!busy || provider.deployment} onClick={() => void changeKey(provider, 'save')}>保存密钥</button><button type="button" disabled={!!busy || provider.deployment || !provider.api_key_present} onClick={() => void changeKey(provider, 'clear')}>清除密钥</button></div>
    </div>)}
    {notice && <p className={`settings-notice ${notice.kind}`} role={notice.kind === 'bad' ? 'alert' : 'status'}>{notice.text}</p>}
  </details>;
}

export function ModelProviderSettings({ refreshVersion = 0 }: { refreshVersion?: number }) {
  const [status, setStatus] = useState<ModelStatus | null>(null);
  const [problem, setProblem] = useState('');
  const [loading, setLoading] = useState(true);
  const [retry, setRetry] = useState(0);
  const mutationVersion = useRef(0);
  const mutationOrder = useRef(0);
  const activeMutations = useRef(0);
  const reconcileOverlap = useRef(false);
  const credentialWrites = useRef(new Map<string, { provider: RetainedProvider; order: number }>());
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => {
    const controller = new AbortController(); setLoading(true);
    const version = mutationVersion.current;
    void api<ModelStatus>('/settings/model', 'GET', undefined, controller.signal).then(next => {
      if (controller.signal.aborted || version !== mutationVersion.current) return;
      if (!next.providers?.default || !next.providers.backup || !next.execution) { setStatus(null); setProblem('当前服务版本尚未支持这项设置，请更新并重启服务。'); return; }
      setStatus(next); setProblem('');
    }).catch(() => {
      if (!controller.signal.aborted) setProblem('模型设置读取失败，请检查服务连接后重试。');
    }).finally(() => { if (!controller.signal.aborted) setLoading(false); });
    return () => controller.abort();
  }, [refreshVersion, retry]);
  const accept = (next: ModelStatus, change: Change, order: number) => {
    if (change.provider) credentialWrites.current.set(change.provider.id, { provider: change.provider, order });
    setStatus(previous => {
      if (!previous) return next;
      const providers = { ...previous.providers };
      let retained = previous.retained_providers;
      let execution = previous.execution;
      if (change.slot) {
        const slot = change.slot;
        const newerCredentials = [...credentialWrites.current.values()].filter(write => write.order > order);
        const preserve = (row: RetainedProvider) => newerCredentials.some(write => affectedCredential(row, write.provider));
        const preserveKey = newerCredentials.some(write => write.provider.slot === slot && sameConnection(write.provider, next.providers[slot].fields));
        providers[slot] = preserveKey ? withKey(next.providers[slot], previous.providers[slot].api_key_present) : next.providers[slot];
        retained = [...previous.retained_providers.filter(row => row.slot !== slot), ...next.retained_providers.filter(row => row.slot === slot).map(row => preserve(row) ? previous.retained_providers.find(prior => prior.id === row.id) ?? row : row)];
        previous.retained_providers.filter(row => row.slot === slot && preserve(row)).forEach(row => { if (!retained.some(item => item.id === row.id)) retained.push(row); });
        if (slot === 'default' && !preserveKey) execution = next.execution;
      } else {
        const target = change.provider;
        const nextRows = next.retained_providers.filter(row => affectedCredential(row, target));
        retained = previous.retained_providers.map(row => affectedCredential(row, target) ? nextRows.find(item => item.id === row.id) ?? row : row);
        nextRows.forEach(row => { if (!retained.some(item => item.id === row.id)) retained.push(row); });
        slots.forEach(slot => {
          const matches = target.slot ? target.slot === slot : providers[slot].source === 'legacy';
          if (matches && sameConnection(target, providers[slot].fields) && sameConnection(target, next.providers[slot].fields)) {
            providers[slot] = withKey(providers[slot], next.providers[slot].api_key_present);
            if (slot === 'default' && sameFields(previous.providers.default.fields, next.providers.default.fields) && previous.providers.default.source === next.providers.default.source) execution = next.execution;
          }
        });
      }
      return { providers, execution, retained_providers: retained };
    });
  };
  const save: SaveSettings = async (change, request) => {
    const order = ++mutationOrder.current; mutationVersion.current += 1;
    activeMutations.current += 1;
    if (activeMutations.current > 1) reconcileOverlap.current = true;
    try { const next = await request(); if (alive.current) accept(next, change, order); return next; }
    finally {
      mutationVersion.current += 1; activeMutations.current -= 1;
      if (alive.current && !activeMutations.current && reconcileOverlap.current) { reconcileOverlap.current = false; setRetry(value => value + 1); }
    }
  };
  return <section className="settings-section settings-model-section" aria-labelledby="settings-model-heading">
    <div className="settings-section-heading"><h2 id="settings-model-heading">模型服务</h2>{loading && <span className="settings-state">正在读取</span>}</div>
    <p className="settings-help">Pilot 和未指定模型的节点使用 default.flash。保存后，下一轮使用新设置，当前轮保持原设置。backup 仅在工作流明确选择时使用。</p>
    {status && <p className="settings-field-help">{status.execution.configured ? `下一轮默认模型：${status.execution.fields?.model ?? 'default.flash'}` : '下一轮默认模型尚不可用，请补全 default 的服务地址、Flash 模型和密钥。'}</p>}
    {problem && <div className="model-provider-problem"><p className="settings-notice bad" role="alert">{problem}</p><button type="button" disabled={loading} onClick={() => setRetry(value => value + 1)}>重试读取模型设置</button></div>}
    {status && <>{slots.map(slot => <ProviderSettings key={slot} slot={slot} status={status.providers[slot]} onSave={save} />)}<p className="settings-field-help model-provider-footnote">模型服务和 API key 在此配置，由 Anchor 保存；不读取环境变量中的模型配置，读取设置时不返回密钥。测试会发送一条小请求，可能产生少量费用。</p><RetainedCredentials providers={status.retained_providers.filter(provider => !provider.slot || !provider.current)} onSave={save} /></>}
  </section>;
}
