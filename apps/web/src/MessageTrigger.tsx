import { useEffect, useId, useRef, useState } from 'react';
import { Check, Radio, RefreshCw } from 'lucide-react';
import { api, reason } from './api';
import { listenerLabel, type GraphSummary } from './model';
import { PluginCredentials } from './PluginCredentials';
import { Modal } from './ui';
import './Channels.css';

type Binding = { graph: string; reply_node: string; plugin: string; users: string[]; send_users: string[] };
type ChannelStatus = {
  platform: string; configuration_owner: 'host' | 'deployment'; configured: boolean; mutable: boolean;
  enabled?: boolean; owner_matches?: boolean; available?: boolean; binding?: Binding;
  listener?: NonNullable<GraphSummary['listener']>['status']; account?: string | null; restart_required?: boolean;
};
const emptyBinding: Binding = { graph: '', reply_node: 'assistant', plugin: '', users: [], send_users: [] };
const recipients = (text: string) => [...new Set(text.split(/[\n,]/).map(value => value.trim()).filter(Boolean))];

/** A Graph's message trigger. Configuration, credentials and the gateway connection are separate facts;
 *  the Host holds one WeCom binding, so binding this Graph replaces a binding to another one. */
export function MessageTrigger({ graph, close }: { graph: string; close: () => void }) {
  const [status, setStatus] = useState<ChannelStatus | null>(null);
  const [configurationRead, setConfigurationRead] = useState<'loading' | 'ready' | 'failed'>('loading');
  const [plugins, setPlugins] = useState<string[]>([]);
  const [error, setError] = useState('');
  const [readError, setReadError] = useState('');
  const [notice, setNotice] = useState('');
  const [busy, setBusy] = useState(false);
  const [revision, setRevision] = useState(0);
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => {
    const controller = new AbortController();
    let disposed = false;
    const load = async () => {
      const results = await Promise.allSettled([
        api<ChannelStatus>('/channels/wecom/configuration', 'GET', undefined, controller.signal),
        api<{ plugins: { id: string }[] }>('/plugins', 'GET', undefined, controller.signal),
      ]);
      if (disposed) return;
      const [configuration, library] = results;
      if (configuration.status === 'fulfilled') setStatus(configuration.value);
      setConfigurationRead(configuration.status === 'fulfilled' ? 'ready' : 'failed');
      if (library.status === 'fulfilled') setPlugins(library.value.plugins.map(item => item.id));
      const sources = ['消息触发配置', '插件列表'];
      setReadError(results.flatMap((item, index) => item.status === 'rejected' ? [`${sources[index]}：${reason(item.reason)}`] : []).join('；'));
    };
    void load();
    const timer = window.setInterval(() => { void load(); }, 8000);
    return () => { disposed = true; controller.abort(); window.clearInterval(timer); };
  }, [revision]);
  const refresh = () => setRevision(value => value + 1);
  const mutate = async (operation: () => Promise<ChannelStatus>, message: string) => {
    if (busy) return;
    setBusy(true); setError(''); setNotice('');
    try { const result = await operation(); if (alive.current) { setStatus(result); setNotice(message); refresh(); } }
    catch (cause) { if (alive.current) { setError(reason(cause)); refresh(); } }
    finally { if (alive.current) setBusy(false); }
  };
  const listener = configurationRead === 'ready' ? status?.listener ?? 'unavailable' : 'unavailable';
  const elsewhere = status?.binding?.graph && status.binding.graph !== graph ? status.binding.graph : '';
  return <Modal title="消息触发" close={() => { if (!busy) close(); }} className="channels-dialog">
    <div className="channels-dialog-intro"><p>企业微信消息到达时唤醒此工作流，每段对话接续自己的运行实例。对话在运行看板的「消息会话」中管理。</p>
      <button type="button" onClick={refresh} disabled={busy}><RefreshCw size={14} />刷新状态</button></div>
    {(error || readError) && <div className="channels-message bad" role="alert"><strong>读取或操作未完成</strong><p>{[error, readError].filter(Boolean).join('；')}</p><span>检查服务和资源状态后重试。请求超时后请先刷新，核查实际结果。</span></div>}
    {notice && <p className="channels-message good" role="status"><Check size={15} />{notice}</p>}
    <section className="channels-configuration" aria-label="消息触发配置">
      <div className={`channels-signal ${listener === 'authenticated' ? 'connected' : ''}`}><Radio size={19} />
        <div><strong>{configurationRead === 'failed' ? '连接状态无法确认' : status ? listenerLabel(listener) : '正在读取入口状态…'}</strong><small>{status?.owner_matches === false ? '当前身份无法读取入口状态' : status ? `${configurationRead === 'failed' ? '上次读取：' : ''}${status.enabled ? '已启用消息入口' : '消息入口未启用'} · ${status.configured ? '配置已保存' : '尚未保存配置'}` : configurationRead === 'failed' ? '入口与配置状态不可用' : '正在读取入口与配置信息'}</small></div>
      </div>
      {status?.owner_matches === false ? <p className="channels-message">此 Host 的企业微信入口属于另一位管理者，当前身份无法查看或修改绑定。</p>
        : status && <>
          {status.configuration_owner === 'deployment' && <p className="channels-message">此消息入口由部署配置管理。请由部署维护者修改绑定、凭据或启停状态。</p>}
          {elsewhere && <p className="channels-message warn">企业微信消息当前触发工作流 <code>{elsewhere}</code>。每个 Host 只有一个企业微信入口，改为此工作流会替换原绑定。</p>}
          {status.restart_required && <p className="channels-message warn">账户身份已变更。请先停用入口，再启用，使网关使用新账户。</p>}
          {configurationRead === 'ready' && status.enabled && listener === 'unavailable' && <p className="channels-message warn">入口已启用，但无法确认网关连接。请检查插件、凭据和 Host 日志；可停用后重新启用。</p>}
          <BindingForm status={status} graph={graph} plugins={plugins} busy={busy} save={binding => mutate(
            () => api<ChannelStatus>('/channels/wecom/configuration', 'PUT', binding), '配置已保存；启用后将按新绑定接收消息。',
          )} />
          {status.binding?.plugin && <div className="channels-credentials"><h3>机器人账户与凭据</h3>
            <p>通过插件的私密输入设置 Bot ID 和 Secret，与插件库中的同一份凭据。更换账户或密钥后，请停用并重新启用入口。</p>
            {status.account && <p className="channels-account">当前账户指纹 <code title={status.account}>{status.account.slice(0, 22)}…</code></p>}
            {status.mutable ? <PluginCredentials pluginId={status.binding.plugin} target="channel_wecom" onChanged={refresh} /> : <p>凭据由部署维护者管理。</p>}
          </div>}
          {status.mutable && <div className="channels-enable"><button type="button" className={status.enabled ? '' : 'primary'} disabled={busy || !status.binding || !status.enabled && status.binding.graph !== graph} onClick={() => {
            if (!status.binding) return;
            void mutate(() => api<ChannelStatus>('/channels/wecom/enabled', 'POST', { enabled: !status.enabled, expected_binding: status.binding }),
              status.enabled ? '消息入口已停用。现有实例的运行状态请在运行看板的消息会话中管理。' : '启用请求已完成；连接状态以网关实际状态为准。');
          }}>{busy ? '正在处理…' : status.enabled ? '停用消息入口' : '启用消息入口'}</button><small>入口启停不会停止或退役已有实例。</small></div>}
        </>}
    </section>
  </Modal>;
}

function BindingForm({ status, graph, plugins, busy, save }: { status: ChannelStatus; graph: string; plugins: string[]; busy: boolean; save: (binding: Binding) => Promise<void> }) {
  const id = useId();
  // The trigger belongs to the open Graph; the saved binding only supplies the remaining fields.
  const saved = JSON.stringify({ ...(status.binding ?? emptyBinding), graph });
  const [draft, setDraft] = useState<Binding>(() => JSON.parse(saved));
  const [users, setUsers] = useState((status.binding?.users ?? []).join('\n'));
  const [sendUsers, setSendUsers] = useState((status.binding?.send_users ?? []).join('\n'));
  const [dirty, setDirty] = useState(false);
  useEffect(() => {
    if (!dirty) { const binding: Binding = JSON.parse(saved); setDraft(binding); setUsers(binding.users.join('\n')); setSendUsers(binding.send_users.join('\n')); }
  }, [saved, dirty]);
  const disabled = !status.mutable || Boolean(status.enabled) || busy;
  const field = (name: 'reply_node' | 'plugin', value: string) => { setDraft(current => ({ ...current, [name]: value })); setDirty(true); };
  return <form className="channels-binding" onSubmit={event => {
    event.preventDefault(); if (disabled) return;
    void save({ ...draft, graph, reply_node: draft.reply_node.trim(), plugin: draft.plugin.trim(), users: recipients(users), send_users: recipients(sendUsers) });
  }}>
    <div className="channels-section-title"><h3>消息绑定</h3>{status.enabled && <small>停用入口后可修改</small>}</div>
    <fieldset disabled={disabled}>
      <div className="channels-field-pair"><label>回复节点<input required value={draft.reply_node} onChange={event => field('reply_node', event.target.value)} /></label><label>渠道插件<input required value={draft.plugin} list={`${id}-plugins`} onChange={event => field('plugin', event.target.value)} placeholder="Plugin ID" /></label></div>
      <datalist id={`${id}-plugins`}>{plugins.map(plugin => <option key={plugin} value={plugin} />)}</datalist>
      <label>允许接收的成员<textarea rows={3} value={users} onChange={event => { setUsers(event.target.value); setDirty(true); }} placeholder="每行一个成员 ID" /></label>
      <label>允许主动发送的成员<textarea rows={3} value={sendUsers} onChange={event => { setSendUsers(event.target.value); setDirty(true); }} placeholder="每行一个成员 ID；显式填写回复所需范围" /></label>
    </fieldset><p className="channels-field-help">成员 ID 以换行或逗号分隔。空接收名单拒绝所有成员；<code>*</code> 允许所有成员。主动发送范围需单独授权。</p>
    {status.mutable && <div className="channels-form-actions"><button type="submit" disabled={disabled || !draft.plugin.trim() || !draft.reply_node.trim()}><Check size={14} />{status.binding && status.binding.graph !== graph ? '改为触发此工作流' : '保存绑定'}</button>
      {dirty && <button type="button" disabled={busy} onClick={() => setDirty(false)}>恢复已保存值</button>}</div>}
  </form>;
}
