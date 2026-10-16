import { useEffect, useRef, useState } from 'react';
import { api, ApiError, reason } from './api';
import { PluginCredentials } from './PluginCredentials';
import './PluginManagement.css';

type ConfigurationField = {
  id: string; label: string; type: 'string' | 'url' | 'secret'; required: boolean;
  state: 'missing' | 'configured' | 'stale' | 'deployment'; source: 'managed' | 'external'; editable: boolean; value?: string;
};
export type PluginConfigurationData = {
  plugin: string; authorization_digest: string; revision: number; fields: ConfigurationField[];
  targets: { target: string; transport: string }[];
};
type ConnectionTest = {
  plugin: string; revision: number; current: boolean;
  results: { target: string; status: 'verified' | 'failed' | 'unsupported'; tool_count?: number; message: string }[];
};
type Props = { pluginId: string; target?: string; onChanged?: () => void };
const labels: Record<ConfigurationField['state'], string> = {
  configured: '已保存', missing: '未配置', stale: '插件权限已变化，需重新配置', deployment: '由部署管理',
};

/** One current-plugin configuration flow for Library, Graph and Pilot. */
export function PluginConfiguration(props: Props) {
  return <CurrentPluginConfiguration key={props.pluginId} {...props} />;
}

function CurrentPluginConfiguration({ pluginId, target, onChanged }: Props) {
  const [data, setData] = useState<PluginConfigurationData | null>(null);
  const [formVersion, setFormVersion] = useState(0);
  const [loading, setLoading] = useState(true);
  const [readError, setReadError] = useState('');
  const [error, setError] = useState('');
  const [message, setMessage] = useState('');
  const [reload, setReload] = useState(0);
  const [edits, setEdits] = useState<Record<string, 'set' | 'clear'>>({});
  const [pending, setPending] = useState<'' | 'save' | 'test'>('');
  const [test, setTest] = useState<ConnectionTest | null>(null);
  const inputs = useRef(new Map<string, HTMLInputElement>());
  const inputCallbacks = useRef(new Map<string, (input: HTMLInputElement | null) => void>());
  const busy = useRef(false);
  const dirty = useRef(false);
  const sequence = useRef(0);
  const request = useRef<AbortController | null>(null);
  const onChangedRef = useRef(onChanged); onChangedRef.current = onChanged;
  const clearSecrets = () => { for (const input of inputs.current.values()) if (input.type === 'password') input.value = ''; };
  const inputRef = (id: string) => {
    let callback = inputCallbacks.current.get(id);
    if (!callback) {
      callback = input => {
        const previous = inputs.current.get(id);
        if (previous?.type === 'password' && previous !== input) previous.value = '';
        if (input) inputs.current.set(id, input); else inputs.current.delete(id);
      };
      inputCallbacks.current.set(id, callback);
    }
    return callback;
  };
  const valid = (value: PluginConfigurationData) => {
    if (value.plugin !== pluginId || !Number.isSafeInteger(value.revision) || value.revision < 0 || !Array.isArray(value.fields)
      || !Array.isArray(value.targets) || typeof value.authorization_digest !== 'string') throw new Error('插件配置响应无效，请重试。');
    // Secrets never become React state, even if a malformed projection includes a value.
    return { ...value, fields: value.fields.map(field => {
      if (field.type !== 'secret') return field;
      const { value: _secret, ...metadata } = field;
      return metadata;
    }) };
  };
  const startRequest = () => {
    request.current?.abort();
    const controller = new AbortController(); request.current = controller;
    return controller;
  };
  const accept = (response: PluginConfigurationData) => {
    setData(response); setFormVersion(current => current + 1);
  };
  useEffect(() => {
    let active = true;
    const refresh = async () => {
      const controller = startRequest();
      const version = ++sequence.current;
      setLoading(true); setReadError(''); setTest(null);
      try {
        const response = valid(await api<PluginConfigurationData>(`/plugins/${encodeURIComponent(pluginId)}/configuration`, 'GET', undefined, controller.signal));
        if (active && version === sequence.current && !dirty.current) accept(response);
      } catch (problem) { if (active && version === sequence.current) setReadError(reason(problem)); }
      finally {
        if (request.current === controller) request.current = null;
        if (active && version === sequence.current) setLoading(false);
      }
    };
    const focus = () => { if (!busy.current && !dirty.current) void refresh(); };
    void refresh(); window.addEventListener('focus', focus);
    return () => { active = false; sequence.current += 1; request.current?.abort(); clearSecrets(); window.removeEventListener('focus', focus); };
  }, [pluginId, reload]);
  const edit = (field: ConfigurationField, action: 'set' | 'clear') => {
    if (!field.editable || busy.current) return;
    if (action === 'clear') { const input = inputs.current.get(field.id); if (input) input.value = ''; }
    dirty.current = true; setEdits(current => ({ ...current, [field.id]: action })); setTest(null); setMessage(''); setError('');
  };
  const undo = (field: ConfigurationField) => {
    const input = inputs.current.get(field.id); if (input) input.value = field.type === 'secret' ? '' : field.value ?? '';
    setEdits(current => { const next = { ...current }; delete next[field.id]; dirty.current = !!Object.keys(next).length; return next; });
  };
  const resetDraft = () => {
    clearSecrets(); dirty.current = false; setEdits({});
  };
  const save = async () => {
    if (!data || loading || readError || busy.current || !dirty.current) return;
    busy.current = true; const version = ++sequence.current; const controller = startRequest();
    setPending('save'); setError(''); setMessage(''); setTest(null);
    const fields = Object.fromEntries(Object.entries(edits).map(([id, action]) => [id,
      action === 'clear' ? { action } : { action, value: inputs.current.get(id)?.value ?? '' }]));
    clearSecrets();
    try {
      const response = valid(await api<PluginConfigurationData>(`/plugins/${encodeURIComponent(pluginId)}/configuration`, 'PUT', {
        authorization_digest: data.authorization_digest, revision: data.revision, fields,
      }, controller.signal));
      if (version !== sequence.current || controller.signal.aborted) return;
      resetDraft(); accept(response); setMessage('配置已保存，后续调用使用新配置。保存状态不代表认证通过。'); onChangedRef.current?.();
    } catch (problem) {
      if (version !== sequence.current || controller.signal.aborted) return;
      resetDraft(); setData(null);
      setError(problem instanceof ApiError && problem.status === 409
        ? '配置未保存。当前配置或插件权限可能已被其他操作更新，正在重新读取。请检查字段和当前状态后重试。'
        : '未能确认配置已保存，已清空密钥输入并重新读取状态。请检查后重试。');
      setReload(current => current + 1);
    } finally {
      if (request.current === controller) request.current = null;
      if (version === sequence.current) { clearSecrets(); busy.current = false; setPending(''); }
    }
  };
  const testConnection = async () => {
    if (!data || loading || readError || busy.current || dirty.current) return;
    busy.current = true; const version = ++sequence.current; const controller = startRequest();
    setPending('test'); setError(''); setMessage(''); setTest(null);
    try {
      const result = await api<ConnectionTest>(`/plugins/${encodeURIComponent(pluginId)}/configuration/test`, 'POST', {
        authorization_digest: data.authorization_digest, revision: data.revision,
      }, controller.signal);
      if (result.plugin !== pluginId || result.revision !== data.revision || typeof result.current !== 'boolean' || !Array.isArray(result.results)) throw new Error('连接测试响应无效，请重试。');
      if (version === sequence.current && !controller.signal.aborted) setTest(result);
    } catch (problem) {
      if (version === sequence.current && !controller.signal.aborted) {
        setError(problem instanceof ApiError && problem.status === 409 ? '无法测试当前配置，请刷新状态并检查必填字段后重试。' : '连接测试未完成，请检查配置后重试。');
      }
    } finally {
      if (request.current === controller) request.current = null;
      if (version === sequence.current) { busy.current = false; setPending(''); }
    }
  };
  const credentialsChanged = () => {
    setTest(null); setMessage('');
    if (pending === 'test') {
      sequence.current += 1; request.current?.abort(); request.current = null;
      busy.current = false; setPending('');
    }
    if (!dirty.current && pending !== 'save') setReload(current => current + 1);
    onChangedRef.current?.();
  };
  const changed = !!Object.keys(edits).length;
  const testButton = !!data?.targets.length && <button type="button" disabled={changed || !!pending || loading || !!readError} onClick={() => void testConnection()}>
    {pending === 'test' ? '正在测试…' : '测试连接'}</button>;
  return <>
    {(loading || readError || error || !!data?.fields.length || !!data?.targets.length) && <section className="plugin-details-section plugin-credentials plugin-configuration" aria-label="插件连接配置">
      <div className="plugin-management-heading"><h3>连接配置</h3><button type="button" disabled={loading || !!pending || changed}
        onClick={() => setReload(current => current + 1)}>刷新配置</button></div>
      <p>在此管理当前账户的插件连接。凭据只显示保存状态；保存后供后续调用使用。</p>
      {loading && <p className="plugin-message" role="status">正在读取连接配置…</p>}
      {(error || readError) && <p className="plugin-message is-error" role="alert">{error || readError}</p>}
      {message && <p className="plugin-message is-success" role="status">{message}</p>}
      {data && (!!data.fields.length || !!data.targets.length) && <>
        {!!data.targets.length && <p className="plugin-management-note">工具服务：{data.targets.map(item => item.target).join('、')}。</p>}
        {!!data.fields.length && <form key={formVersion} className="plugin-credential-form" onSubmit={event => { event.preventDefault(); void save(); }}>
          {data.fields.map(field => <div className="plugin-credential-field" key={`${data.authorization_digest}/${data.revision}/${field.id}`}>
            <div className="plugin-credential-summary"><strong>{field.label}{field.required ? '（必填）' : ''}</strong>
              <span>{edits[field.id] === 'clear' ? '保存时清除' : edits[field.id] === 'set' ? '待保存' : labels[field.state]}</span></div>
            {field.editable ? <>
              <label>{field.type === 'secret' ? '新凭据' : '值'}<input type={field.type === 'secret' ? 'password' : field.type === 'url' ? 'url' : 'text'}
                aria-label={field.label} defaultValue={field.type === 'secret' ? '' : field.value ?? ''}
                ref={inputRef(field.id)}
                placeholder={field.type === 'secret' && field.state === 'configured' ? '已保存；输入新值进行轮换' : undefined}
                autoComplete="off" spellCheck={false} maxLength={65536} disabled={!!pending || loading || !!readError || edits[field.id] === 'clear'}
                onChange={() => edit(field, 'set')} /></label>
              <div className="plugin-authorization-actions">
                {!edits[field.id] && (field.state === 'configured' || field.state === 'stale') && <button type="button" disabled={!!pending || loading || !!readError}
                  onClick={() => edit(field, 'clear')}>清除 {field.label}</button>}
                {!!edits[field.id] && <button type="button" disabled={!!pending} onClick={() => undo(field)}>撤销修改 {field.label}</button>}
              </div>
            </> : <p className="plugin-management-note">{field.type !== 'secret' && field.value && <><code>{field.value}</code> · </>}
              {field.source === 'external' ? '由部署管理，需由部署维护者修改。' : '当前字段不可编辑，请刷新配置。'}</p>}
          </div>)}
          <div className="plugin-authorization-actions"><button type="submit" disabled={!changed || !!pending || loading || !!readError}>
            {pending === 'save' ? '保存中…' : '保存配置'}</button>
            {testButton}</div>
          {data.fields.some(field => field.type === 'url' && edits[field.id]) && data.fields.some(field => field.type === 'secret')
            && <p className="plugin-management-note">更换服务地址后需重新填写访问凭据；如需继续使用同一凭据，请在本次保存时重新输入。</p>}
          {changed && <p className="plugin-management-note">有未保存的修改。保存后可测试连接；清除字段后，后续调用将无法使用其已保存的值。</p>}
        </form>}
        {!data.fields.length && <div className="plugin-authorization-actions">{testButton}</div>}
        {!!data.targets.length && <p className="plugin-management-note">连接测试只建立 MCP 连接并读取工具列表，不执行业务工具。</p>}
        {test && <div className="plugin-connection-results" aria-label="连接测试结果" role="status">
          {!test.current && <p className="plugin-message">配置已变化，本次测试结果已失效。请刷新配置后重新测试。</p>}
          {test.current && test.results.map(result => <div key={result.target} className={`plugin-connection-result ${result.status === 'failed' ? 'is-error' : ''}`}>
            <strong>{result.target}</strong><span>{result.status === 'verified' ? '连接通过' : result.status === 'failed' ? '连接失败' : '暂不支持测试'}
              {result.status === 'verified' && result.tool_count !== undefined ? ` · ${result.tool_count} 个工具` : ''}</span><p>{result.message}</p>
          </div>)}
        </div>}
      </>}
    </section>}
    {data && !readError && <PluginCredentials pluginId={pluginId} target={target} onChanged={credentialsChanged} hideEmpty={!!data.fields.length} />}
  </>;
}
