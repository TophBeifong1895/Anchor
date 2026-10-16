import { useEffect, useRef, useState } from 'react';
import { api, reason } from './api';
import './PluginManagement.css';

export type CredentialField = { field: string; state: 'configured' | 'missing' | 'stale' | 'deployment'; editable: boolean };
export type CredentialTarget = { target: string; kind: 'mcp' | 'channel'; authorization_digest: string; fields: CredentialField[] };
type Credentials = { plugin: string; targets: CredentialTarget[]; warning?: string | null };
const labels: Record<CredentialField['state'], string> = {
  configured: '已保存', missing: '未配置', stale: '插件权限已变化，需重新配置', deployment: '由部署管理',
};

/** Owner-bound metadata and ephemeral inputs; values never enter browser storage. */
export function PluginCredentials({ pluginId, target, onChanged }: {
  pluginId: string; target?: string; onChanged?: () => void;
}) {
  const [data, setData] = useState<Credentials | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [readError, setReadError] = useState('');
  const [message, setMessage] = useState('');
  const [editing, setEditing] = useState('');
  const input = useRef<HTMLInputElement>(null);
  const clearInput = () => { if (input.current) input.current.value = ''; };
  const [pending, setPending] = useState('');
  const [revision, setRevision] = useState(0);
  const sequence = useRef(0);
  const busy = useRef(false);
  const onChangedRef = useRef(onChanged); onChangedRef.current = onChanged;
  useEffect(() => {
    setData(null); setEditing(''); clearInput(); setMessage(''); setError(''); setPending(''); busy.current = false;
  }, [pluginId, target]);
  useEffect(() => {
    let alive = true;
    const refresh = async () => {
      const version = ++sequence.current;
      setLoading(true); setReadError('');
      try {
        const response = await api<Credentials>(`/plugins/${encodeURIComponent(pluginId)}/credentials`);
        if (response.plugin !== pluginId || !Array.isArray(response.targets)) throw new Error('凭据状态响应无效，请重试。');
        if (alive && version === sequence.current) setData(response);
      } catch (problem) { if (alive && version === sequence.current) setReadError(reason(problem)); }
      finally { if (alive && version === sequence.current) setLoading(false); }
    };
    const focus = () => { if (!busy.current) void refresh(); };
    void refresh(); window.addEventListener('focus', focus);
    return () => { alive = false; sequence.current += 1; window.removeEventListener('focus', focus); };
  }, [pluginId, revision]);
  const change = async (record: CredentialTarget, field: CredentialField, clear: boolean) => {
    if (busy.current) return;
    busy.current = true; const version = ++sequence.current;
    const key = `${record.target}/${field.field}`;
    setPending(key); setError(''); setMessage('');
    try {
      const response = await api<Credentials>(`/plugins/${encodeURIComponent(pluginId)}/credentials/${encodeURIComponent(record.target)}/${encodeURIComponent(field.field)}`,
        clear ? 'DELETE' : 'PUT', { authorization_digest: record.authorization_digest, ...(clear ? {} : { value: input.current?.value ?? '' }) });
      if (version !== sequence.current) return;
      setData(response); setEditing(''); clearInput();
      setMessage(clear ? `已清除 ${field.field}` : `已保存 ${field.field}`);
      if (response.warning) setError(response.warning);
      onChangedRef.current?.();
    } catch (problem) {
      if (version === sequence.current) { setError(reason(problem)); setData(null); setRevision(current => current + 1); }
    } finally {
      if (version === sequence.current) { busy.current = false; setPending(''); clearInput(); }
    }
  };
  const targets = data?.targets.filter(record => !target || record.target === target) ?? [];
  return <section className="plugin-details-section plugin-credentials" aria-label="插件凭据">
    <div className="plugin-management-heading"><h3>凭据</h3><button type="button" disabled={loading || !!pending}
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
            <label>新值<input type="password" aria-label={`${field.field} 新值`} ref={input} required autoComplete="off"
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
