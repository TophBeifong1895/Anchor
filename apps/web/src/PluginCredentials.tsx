import { useCallback, useEffect, useRef, useState } from 'react';
import { api, ApiError, reason } from './api';
import './PluginManagement.css';

export type CredentialField = { field: string; state: 'configured' | 'missing' | 'stale' | 'deployment'; editable: boolean };
export type CredentialTarget = { target: string; kind: 'mcp' | 'channel'; authorization_digest: string; fields: CredentialField[] };
type Credentials = { plugin: string; targets: CredentialTarget[]; warning?: string | null };
const labels: Record<CredentialField['state'], string> = {
  configured: '已保存', missing: '未配置', stale: '插件权限已变化，需重新配置', deployment: '由部署管理',
};

/** `endpoint` defaults to the Plugin's shared targets; a bot Graph passes its own credentials path. */
type Props = { pluginId: string; target?: string; endpoint?: string; onChanged?: () => void; hideEmpty?: boolean };

/** Owner-bound metadata and ephemeral inputs; values never enter browser storage. */
export function PluginCredentials(props: Props) {
  return <CurrentPluginCredentials key={`${props.pluginId}/${props.target ?? ''}/${props.endpoint ?? ''}`} {...props} />;
}

function CurrentPluginCredentials({ pluginId, target, endpoint, onChanged, hideEmpty = false }: Props) {
  const base = endpoint ?? `/plugins/${encodeURIComponent(pluginId)}/credentials`;
  const [data, setData] = useState<Credentials | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [readError, setReadError] = useState('');
  const [message, setMessage] = useState('');
  const [editing, setEditing] = useState('');
  const input = useRef<HTMLInputElement | null>(null);
  const inputRef = useCallback((element: HTMLInputElement | null) => {
    if (input.current && input.current !== element) input.current.value = '';
    input.current = element;
  }, []);
  const clearInput = () => { if (input.current) input.current.value = ''; };
  const [pending, setPending] = useState('');
  const [revision, setRevision] = useState(0);
  const sequence = useRef(0);
  const busy = useRef(false);
  const request = useRef<AbortController | null>(null);
  const editingRef = useRef(editing); editingRef.current = editing;
  const onChangedRef = useRef(onChanged); onChangedRef.current = onChanged;
  const startRequest = () => {
    request.current?.abort();
    const controller = new AbortController(); request.current = controller;
    return controller;
  };
  const valid = (response: Credentials) => {
    if (response.plugin !== pluginId || !Array.isArray(response.targets)) throw new Error('凭据状态响应无效，请重试。');
    return response;
  };
  useEffect(() => {
    let alive = true;
    const refresh = async () => {
      const controller = startRequest();
      const version = ++sequence.current;
      setLoading(true); setReadError('');
      try {
        const response = valid(await api<Credentials>(base, 'GET', undefined, controller.signal));
        if (alive && version === sequence.current) setData(response);
      } catch (problem) { if (alive && version === sequence.current) setReadError(reason(problem)); }
      finally {
        if (request.current === controller) request.current = null;
        if (alive && version === sequence.current) setLoading(false);
      }
    };
    const focus = () => { if (!busy.current && !editingRef.current) void refresh(); };
    void refresh(); window.addEventListener('focus', focus);
    return () => { alive = false; sequence.current += 1; request.current?.abort(); clearInput(); window.removeEventListener('focus', focus); };
  }, [base, revision]);
  const change = async (record: CredentialTarget, field: CredentialField, clear: boolean) => {
    if (busy.current || loading || readError || !field.editable) return;
    busy.current = true; const version = ++sequence.current; const controller = startRequest();
    const key = `${record.target}/${field.field}`;
    setPending(key); setError(''); setMessage('');
    const value = input.current?.value ?? ''; clearInput();
    try {
      const response = valid(await api<Credentials>(endpoint ? `${endpoint}/${encodeURIComponent(field.field)}` : `${base}/${encodeURIComponent(record.target)}/${encodeURIComponent(field.field)}`,
        clear ? 'DELETE' : 'PUT', { authorization_digest: record.authorization_digest, ...(clear ? {} : { value }) }, controller.signal));
      if (version !== sequence.current || controller.signal.aborted) return;
      setData(response); setEditing(''); clearInput();
      setMessage(clear ? `已清除 ${field.field}` : `已保存 ${field.field}`);
      if (response.warning) setError(response.warning);
      onChangedRef.current?.();
    } catch (problem) {
      if (version === sequence.current && !controller.signal.aborted) {
        setError(problem instanceof ApiError && problem.status === 409
          ? '凭据未保存。插件权限可能已变化，正在重新读取状态；请检查后重试。'
          : '未能确认凭据已保存，已清空密钥输入并重新读取状态。请检查后重试。');
        setData(null); setEditing(''); setRevision(current => current + 1);
      }
    } finally {
      if (request.current === controller) request.current = null;
      if (version === sequence.current) { busy.current = false; setPending(''); clearInput(); }
    }
  };
  const targets = data?.targets.filter(record => (!target || record.target === target) && (!hideEmpty || !!record.fields.length)) ?? [];
  if (hideEmpty && !loading && !error && !readError && data && !targets.some(record => record.fields.length)) return null;
  return <section className="plugin-details-section plugin-credentials" aria-label="插件凭据">
    <div className="plugin-management-heading"><h3>凭据</h3><button type="button" disabled={loading || !!pending || !!editing}
      onClick={() => setRevision(current => current + 1)}>刷新状态</button></div>
    <p>这里只显示保存状态，不验证密钥是否有效。保存后供后续调用使用。</p>
    {loading && <p className="plugin-message" role="status">正在读取凭据状态…</p>}
    {(error || readError) && <p className="plugin-message is-error" role="alert">{error || readError}</p>}
    {message && <p className="plugin-message is-success" role="status">{message}</p>}
    {!loading && !readError && data && !targets.length && <p className="plugin-message">没有需要填写的凭据。</p>}
    {targets.map(record => <div className="plugin-credential-target" key={record.target}>
      <h4>{record.kind === 'channel' ? '渠道' : 'MCP'} <code>{record.target}</code></h4>
      {record.fields.map(field => {
        const key = `${record.target}/${field.field}`;
        return <div className="plugin-credential-field" key={field.field}>
          <div className="plugin-credential-summary"><code>{field.field}</code><span>{labels[field.state]}</span></div>
          {field.editable && editing !== key && <div className="plugin-authorization-actions">
            <button type="button" disabled={!!pending || loading || !!readError} onClick={() => { setEditing(key); clearInput(); setMessage(''); }}>
              {field.state === 'configured' ? `轮换 ${field.field}` : `设置 ${field.field}`}</button>
            {(field.state === 'configured' || field.state === 'stale') && <button type="button" disabled={!!pending || loading || !!readError}
              onClick={() => { setEditing(`clear:${key}`); clearInput(); setMessage(''); }}>清除 {field.field}</button>}
          </div>}
          {editing === key && <form className="plugin-credential-form" onSubmit={event => { event.preventDefault(); void change(record, field, false); }}>
            <label>新值<input type="password" aria-label={`${field.field} 新值`} ref={inputRef} required autoComplete="off"
              spellCheck={false} maxLength={65536} disabled={!!pending} autoFocus /></label>
            <div className="plugin-authorization-actions"><button type="submit" disabled={!!pending || loading || !!readError}>{pending === key ? '保存中…' : '保存凭据'}</button>
              <button type="button" disabled={!!pending} onClick={() => { setEditing(''); clearInput(); }}>取消</button></div>
          </form>}
          {editing === `clear:${key}` && <div className="plugin-credential-form"><p>清除后，后续调用将无法使用已保存的 {field.field}。</p>
            <div className="plugin-authorization-actions"><button type="button" disabled={!!pending || loading || !!readError} onClick={() => void change(record, field, true)}>
              {pending === key ? '清除中…' : '确认清除'}</button><button type="button" disabled={!!pending} onClick={() => setEditing('')}>取消</button></div></div>}
        </div>;
      })}
    </div>)}
  </section>;
}
