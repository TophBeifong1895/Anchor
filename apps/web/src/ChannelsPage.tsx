import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { ArrowUpRight, Check, ChevronDown, Radio, RefreshCw, Trash2 } from 'lucide-react';
import { api, reason } from './api';
import { listenerLabel, type GraphSummary, type RunProjection } from './model';
import { RunControls } from './RunControls';
import { runPhase, runPhaseLabel, runQuiescent } from './runPresentation';
import { PluginCredentials } from './PluginCredentials';
import { Modal } from './ui';
import './ChannelsPage.css';

type Binding = { graph: string; reply_node: string; plugin: string; users: string[]; send_users: string[] };
type ChannelStatus = {
  platform: string; configuration_owner: 'host' | 'deployment'; configured: boolean; mutable: boolean;
  enabled?: boolean; owner_matches?: boolean; available?: boolean; binding?: Binding;
  listener?: NonNullable<GraphSummary['listener']>['status']; account?: string | null; restart_required?: boolean;
};
type ChannelSession = { id: string; title: string; graph: string; platform: string };
type Assistant = { run_id: string; wait_node: string; work_node: string; reply_node: string };
type AssistantStatus = { assistant: Assistant | null; needs_recovery: boolean;
  plugin_update?: { state: 'current' | 'pending' | 'restart_required' | 'unavailable'; message?: string } };
type RunStatus = { run: string; active: boolean; projection?: RunProjection; state: { cursor?: { node: string } | null } };
type Delivery = { key: string; status: string; error?: string | null };
type SessionDetail = { session: { run_ids: string[] } };
const emptyBinding: Binding = { graph: '', reply_node: 'assistant', plugin: '', users: [], send_users: [] };
const enc = encodeURIComponent;
const recipients = (text: string) => [...new Set(text.split(/[\n,]/).map(value => value.trim()).filter(Boolean))];

/** Configuration, transport connection, and assistant execution are separate facts. */
export function ChannelsPage({ onOpenRun }: { onOpenRun?: (run: string, graph: string) => void }) {
  const [status, setStatus] = useState<ChannelStatus | null>(null);
  const [sessions, setSessions] = useState<ChannelSession[]>([]);
  const [configurationRead, setConfigurationRead] = useState<'loading' | 'ready' | 'failed'>('loading');
  const [sessionsRead, setSessionsRead] = useState<'loading' | 'ready' | 'failed'>('loading');
  const [graphs, setGraphs] = useState<string[]>([]);
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
        api<{ sessions: ChannelSession[] }>('/channel-sessions', 'GET', undefined, controller.signal),
        api<{ graphs: { graph: string }[] }>('/graphs', 'GET', undefined, controller.signal),
        api<{ plugins: { id: string }[] }>('/plugins', 'GET', undefined, controller.signal),
      ]);
      if (disposed) return;
      const [configuration, conversations, catalog, library] = results;
      if (configuration.status === 'fulfilled') setStatus(configuration.value);
      if (conversations.status === 'fulfilled') setSessions(conversations.value.sessions);
      setConfigurationRead(configuration.status === 'fulfilled' ? 'ready' : 'failed');
      setSessionsRead(conversations.status === 'fulfilled' ? 'ready' : 'failed');
      if (catalog.status === 'fulfilled') setGraphs(catalog.value.graphs.map(item => item.graph));
      if (library.status === 'fulfilled') setPlugins(library.value.plugins.map(item => item.id));
      const sources = ['渠道配置', '渠道对话', 'Graph 列表', '插件列表'];
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
  return <main className="channels-page">
    <header className="channels-heading"><div><span className="channels-eyebrow">渠道管理</span><h1>企业微信与常驻助手</h1><p>管理消息入口、授权范围与每段对话的当前实例。</p></div>
      <button type="button" onClick={refresh} disabled={busy}><RefreshCw size={14} />刷新状态</button></header>
    {(error || readError) && <div className="channels-message bad" role="alert"><strong>读取或操作未完成</strong><p>{[error, readError].filter(Boolean).join('；')}</p><span>检查服务和资源状态后重试。请求超时后请先刷新，核查实际结果。</span></div>}
    {notice && <p className="channels-message good" role="status"><Check size={15} />{notice}</p>}
    <div className="channels-grid">
      <section className="channels-configuration" aria-label="企业微信配置">
        <div className={`channels-signal ${listener === 'authenticated' ? 'connected' : ''}`}><Radio size={19} />
          <div><strong>{configurationRead === 'failed' ? '连接状态无法确认' : status ? listenerLabel(listener) : '正在读取渠道状态…'}</strong><small>{status?.owner_matches === false ? '当前身份无法读取入口状态' : status ? `${configurationRead === 'failed' ? '上次读取：' : ''}${status.enabled ? '已启用消息入口' : '消息入口未启用'} · ${status.configured ? '配置已保存' : '尚未保存配置'}` : configurationRead === 'failed' ? '入口与配置状态不可用' : '正在读取入口与配置信息'}</small></div>
        </div>
        {status?.owner_matches === false ? <p className="channels-message">此 Host 的企业微信入口属于另一位管理者，当前身份无法查看或修改绑定。</p>
          : status && <>
            {status.configuration_owner === 'deployment' && <p className="channels-message">此渠道由部署配置管理。请由部署维护者修改绑定、凭据或启停状态。</p>}
            {status.restart_required && <p className="channels-message warn">账户身份已变更。请先停用入口，再启用，使网关使用新账户。</p>}
            {configurationRead === 'ready' && status.enabled && listener === 'unavailable' && <p className="channels-message warn">入口已启用，但无法确认网关连接。请检查插件、凭据和 Host 日志；可停用后重新启用。</p>}
            <BindingForm status={status} graphs={graphs} plugins={plugins} busy={busy} save={binding => mutate(
              () => api<ChannelStatus>('/channels/wecom/configuration', 'PUT', binding), '配置已保存；启用后将按新绑定接收消息。',
            )} />
            {status.binding?.plugin && <div className="channels-credentials"><h3>机器人账户与凭据</h3>
              <p>通过插件的私密输入设置 Bot ID 和 Secret。更换账户或密钥后，请停用并重新启用入口。</p>
              {status.account && <p className="channels-account">当前账户指纹 <code title={status.account}>{status.account.slice(0, 22)}…</code></p>}
              {status.mutable ? <PluginCredentials pluginId={status.binding.plugin} target="channel_wecom" onChanged={refresh} /> : <p>凭据由部署维护者管理。</p>}
            </div>}
            {status.mutable && <div className="channels-enable"><button type="button" className={status.enabled ? '' : 'primary'} disabled={busy || !status.binding} onClick={() => {
              if (!status.binding) return;
              void mutate(() => api<ChannelStatus>('/channels/wecom/enabled', 'POST', { enabled: !status.enabled, expected_binding: status.binding }),
                status.enabled ? '消息入口已停用。现有助手的运行状态请在右侧管理。' : '启用请求已完成；连接状态以网关实际状态为准。');
            }}>{busy ? '正在处理…' : status.enabled ? '停用消息入口' : '启用消息入口'}</button><small>入口启停与助手停止、退役分别管理。</small></div>}
          </>}
      </section>
      <section className="channels-assistants" aria-labelledby="channel-assistants-title"><header><div><h2 id="channel-assistants-title">常驻助手</h2><p>每段可信对话拥有独立实例与工作现场。</p></div><span className="channels-count">{sessionsRead === 'ready' ? sessions.length : '未知'}</span></header>
        {sessions.length ? <div className="channels-session-list">{sessions.map(session => <AssistantRow key={session.id} session={session} revision={revision} onChanged={refresh} onOpenRun={onOpenRun} />)}</div>
          : <div className="channels-empty"><Radio size={24} /><strong>{sessionsRead === 'failed' ? '渠道对话读取失败' : sessionsRead === 'ready' ? '还没有渠道对话' : '正在读取对话…'}</strong><p>{sessionsRead === 'failed' ? '无法确认当前渠道对话。请刷新状态重试。' : '配置并启用入口后，首条可信消息会创建会话；常驻 Graph 会惰性接纳助手实例。'}</p></div>}
        <p className="channels-footer-note">支持热更新的 Graph 保存插件配置后，各会话从下一条消息使用新配置。退役会解除当前绑定，下一条可信消息创建新实例。</p>
      </section>
    </div>
  </main>;
}

function BindingForm({ status, graphs, plugins, busy, save }: { status: ChannelStatus; graphs: string[]; plugins: string[]; busy: boolean; save: (binding: Binding) => Promise<void> }) {
  const id = useId();
  const [draft, setDraft] = useState<Binding>(status.binding ?? emptyBinding);
  const [users, setUsers] = useState((status.binding?.users ?? []).join('\n'));
  const [sendUsers, setSendUsers] = useState((status.binding?.send_users ?? []).join('\n'));
  const [dirty, setDirty] = useState(false);
  const saved = JSON.stringify(status.binding ?? emptyBinding);
  useEffect(() => {
    if (!dirty) { const binding: Binding = JSON.parse(saved); setDraft(binding); setUsers(binding.users.join('\n')); setSendUsers(binding.send_users.join('\n')); }
  }, [saved, dirty]);
  const disabled = !status.mutable || Boolean(status.enabled) || busy;
  const field = (name: 'graph' | 'reply_node' | 'plugin', value: string) => { setDraft(current => ({ ...current, [name]: value })); setDirty(true); };
  return <form className="channels-binding" onSubmit={event => {
    event.preventDefault(); if (disabled) return;
    void save({ ...draft, graph: draft.graph.trim(), reply_node: draft.reply_node.trim(), plugin: draft.plugin.trim(), users: recipients(users), send_users: recipients(sendUsers) });
  }}>
    <div className="channels-section-title"><h3>消息绑定</h3>{status.enabled && <small>停用入口后可修改</small>}</div>
    <fieldset disabled={disabled}><label>Graph<input required value={draft.graph} list={`${id}-graphs`} onChange={event => field('graph', event.target.value)} placeholder="选择或输入 Graph ID" /></label>
      <datalist id={`${id}-graphs`}>{graphs.map(graph => <option key={graph} value={graph} />)}</datalist>
      <div className="channels-field-pair"><label>回复节点<input required value={draft.reply_node} onChange={event => field('reply_node', event.target.value)} /></label><label>渠道插件<input required value={draft.plugin} list={`${id}-plugins`} onChange={event => field('plugin', event.target.value)} placeholder="Plugin ID" /></label></div>
      <datalist id={`${id}-plugins`}>{plugins.map(plugin => <option key={plugin} value={plugin} />)}</datalist>
      <label>允许接收的成员<textarea rows={3} value={users} onChange={event => { setUsers(event.target.value); setDirty(true); }} placeholder="每行一个成员 ID" /></label>
      <label>允许主动发送的成员<textarea rows={3} value={sendUsers} onChange={event => { setSendUsers(event.target.value); setDirty(true); }} placeholder="每行一个成员 ID；显式填写回复所需范围" /></label>
    </fieldset><p className="channels-field-help">成员 ID 以换行或逗号分隔。空接收名单拒绝所有成员；<code>*</code> 允许所有成员。主动发送范围需单独授权。</p>
    {status.mutable && <div className="channels-form-actions"><button type="submit" disabled={disabled || !draft.graph.trim() || !draft.plugin.trim() || !draft.reply_node.trim()}><Check size={14} />保存绑定</button>
      {dirty && <button type="button" disabled={busy} onClick={() => setDirty(false)}>恢复已保存值</button>}</div>}
  </form>;
}

function AssistantRow({ session, revision, onChanged, onOpenRun }: { session: ChannelSession; revision: number; onChanged: () => void; onOpenRun?: (run: string, graph: string) => void }) {
  const [assistant, setAssistant] = useState<AssistantStatus | null>(null);
  const [run, setRun] = useState<RunStatus | null>(null);
  const [deliveries, setDeliveries] = useState<Delivery[] | null>(null);
  const [runIds, setRunIds] = useState<string[]>([]);
  const [error, setError] = useState('');
  const [readError, setReadError] = useState('');
  const [notice, setNotice] = useState('');
  const [pending, setPending] = useState(false);
  const [action, setAction] = useState<'retire' | 'cleanup' | null>(null);
  const [retiringRun, setRetiringRun] = useState('');
  const [cleanupIds, setCleanupIds] = useState<string[]>([]);
  const [reload, setReload] = useState(0);
  const [expanded, setExpanded] = useState(false);
  const alive = useRef(true);
  const cancel = useRef<HTMLButtonElement>(null);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  useEffect(() => {
    const controller = new AbortController();
    let disposed = false;
    const load = async () => {
      try {
        const path = `/channel-sessions/${enc(session.id)}`;
        const [instance, unfinished, detail] = await Promise.all([
          api<AssistantStatus>(`${path}/assistant`, 'GET', undefined, controller.signal),
          api<{ deliveries: Delivery[] }>(`${path}/deliveries/unresolved`, 'GET', undefined, controller.signal),
          api<SessionDetail>(path, 'GET', undefined, controller.signal),
        ]);
        let execution: RunStatus | null = null;
        try {
          if (instance.assistant) execution = await api<RunStatus>(`/runs/${enc(instance.assistant.run_id)}`, 'GET', undefined, controller.signal);
        } catch (cause) {
          if (!disposed) { setAssistant(instance); setRun(null); setDeliveries(unfinished.deliveries); setRunIds(detail.session.run_ids); setReadError(`当前 Run 无法读取：${reason(cause)}。请核查 Host 中保存的运行事实。`); }
          return;
        }
        if (!disposed) { setAssistant(instance); setRun(execution); setDeliveries(unfinished.deliveries); setRunIds(detail.session.run_ids); setReadError(''); }
      } catch (cause) { if (!disposed) setReadError(`状态读取失败：${reason(cause)}`); }
    };
    void load(); const timer = window.setInterval(() => { void load(); }, 5000);
    return () => { disposed = true; controller.abort(); window.clearInterval(timer); };
  }, [session.id, revision, reload]);
  const refresh = useCallback(() => { setReload(value => value + 1); }, []);
  const perform = async (operation: () => Promise<unknown>, message: string, close = false) => {
    if (pending) return;
    setPending(true); setError(''); setNotice('');
    try { await operation(); if (alive.current) { setNotice(message); if (close) setAction(null); refresh(); onChanged(); } }
    catch (cause) { if (alive.current) { setError(reason(cause)); refresh(); } }
    finally { if (alive.current) setPending(false); }
  };
  const bound = assistant?.assistant;
  const pluginUpdate = assistant?.plugin_update;
  const pluginUpdateLabel = pluginUpdate?.state === 'pending' ? '插件待生效'
    : pluginUpdate?.state === 'restart_required' ? '插件需重建实例'
      : pluginUpdate?.state === 'unavailable' ? '插件状态不可用' : '';
  const settled = runQuiescent(run?.projection);
  const canRetire = Boolean(bound && settled && deliveries?.length === 0);
  const status = !assistant ? '正在读取…' : !bound ? '无当前实例' : !run ? '运行状态不可用'
    : runPhase(run) === 'executing' && run.projection?.active && run.state.cursor?.node === bound.wait_node ? '等待消息'
      : runPhaseLabel(run.projection);
  const cleanup = async () => {
    const path = `/channel-sessions/${enc(session.id)}`;
    // Re-read the exact deletion scope; a new trusted message may have arrived.
    const latest = await api<AssistantStatus>(`${path}/assistant`);
    if (latest.assistant) throw new Error('此对话已有当前实例，请先停止并退役，再清理历史。');
    const detail = await api<SessionDetail>(path);
    if (JSON.stringify(detail.session.run_ids) !== JSON.stringify(cleanupIds)) throw new Error('历史运行列表已变化，请关闭此窗口，刷新后重新确认清理范围。');
    const unfinished = await api<{ deliveries: Delivery[] }>(`${path}/deliveries/unresolved`);
    if (unfinished.deliveries.length) throw new Error('存在未结算投递，请先核查实际投递结果，再清理历史。');
    for (const id of [...cleanupIds].reverse()) await api(`/runs/${enc(id)}`, 'DELETE');
    await api(path, 'DELETE');
  };
  return <article className="channels-assistant-row" aria-label={`渠道对话 ${session.title || session.id}`}>
    <div className="channels-assistant-top"><div><strong>{session.title || session.id}</strong><span>{session.platform || '渠道'} · {session.graph}</span></div><div className="channels-assistant-status"><span className={`channels-instance-status ${run?.active ? 'active' : ''}`}>{status}</span>
      {pluginUpdateLabel && <small className={`channels-plugin-state ${pluginUpdate?.state}`} title={pluginUpdate?.message} role={pluginUpdate?.state === 'unavailable' ? 'alert' : 'status'}>{pluginUpdateLabel}</small>}</div></div>
    {bound && <button type="button" className="channels-run-link" disabled={!onOpenRun} onClick={() => onOpenRun?.(bound.run_id, session.graph)}><code>{bound.run_id}</code><ArrowUpRight size={13} /></button>}
    <div className="channels-row-actions">{bound && <>
      <RunControls projection={run?.projection} busy={pending} onControl={operation => void perform(
        () => api(`/runs/${enc(bound.run_id)}/${operation}`, 'POST'),
        operation === 'stop' ? '停止请求已提交；请等待执行者退出后再退役。' : '控制请求已提交；请核查执行状态与现场。',
      )} />
      <button type="button" disabled={pending || !canRetire} title={canRetire ? '' : '须等到运行已停止或终态，且没有未结算投递'} onClick={() => { setError(''); setRetiringRun(bound.run_id); setAction('retire'); }}>退役实例</button>
    </>}
      <button type="button" disabled={pending || assistant === null || Boolean(bound) || deliveries === null || deliveries.length > 0} onClick={() => { setError(''); setCleanupIds([...runIds]); setAction('cleanup'); }}><Trash2 size={12} />清理历史</button>
      <button type="button" className="channels-details-toggle" aria-expanded={expanded} onClick={() => setExpanded(value => !value)}>详情<ChevronDown size={13} /></button>
    </div>
    {deliveries && deliveries.length > 0 && <div className="channels-unresolved"><strong>{deliveries.length} 项投递未结算，暂不能退役或清理</strong><p>核查网关账本与实际消息结果。未知投递不会自动重发或结算。</p><ul>{deliveries.map(delivery => <li key={delivery.key}><code>{delivery.key}</code><span>{({ pending: '待投递', sending: '投递中', unknown: '结果未知' } as Record<string, string>)[delivery.status] ?? delivery.status}</span>{delivery.error && <small>{delivery.error}</small>}</li>)}</ul></div>}
    {expanded && <dl className="channels-instance-details"><dt>会话</dt><dd><code>{session.id}</code></dd><dt>保留的运行</dt><dd>{runIds.length} 条{runIds.length > 0 && <ul className="channels-history-links">{runIds.map(id => <li key={id}><button type="button" disabled={!onOpenRun} onClick={() => onOpenRun?.(id, session.graph)}><code>{id}</code><ArrowUpRight size={12} /></button></li>)}</ul>}</dd>{bound && <><dt>节点</dt><dd><code>{bound.wait_node} → {bound.work_node} → {bound.reply_node}</code></dd><dt>执行者</dt><dd>{run ? run.active ? '当前进程中仍有执行者' : '当前进程无活执行者' : '状态未知，请核查运行事实'}</dd></>}{pluginUpdate && <><dt>插件配置</dt><dd>{pluginUpdate.message || (pluginUpdate.state === 'current' ? '与当前 Graph 一致' : pluginUpdateLabel || '状态未知')}</dd></>}</dl>}
    {notice && <p className="channels-row-notice" role="status">{notice}</p>}
    {(error || readError) && <div className="channels-row-error" role="alert">{[error, readError].filter(Boolean).join('；')}<button type="button" disabled={pending} onClick={() => { setError(''); refresh(); }}>重读状态</button></div>}
    {action && <Modal title={action === 'retire' ? '退役当前助手？' : '永久清理这段渠道历史？'} close={() => { if (!pending) setAction(null); }} initialFocus={cancel} className="channels-confirm">
      {action === 'retire' ? <><p>解除 <code>{retiringRun}</code> 的当前绑定，保留运行、产物与工作现场。旧实例不能再恢复。</p><p>下一条可信消息会按当前配置创建新实例；现有现场不会自动迁移到新版本。</p>{bound?.run_id !== retiringRun && <p className="channels-message warn">当前实例已变化，请关闭此窗口后重新核查。</p>}</>
        : <><p>永久删除“{session.title || session.id}”的渠道会话及下列 {cleanupIds.length} 条运行，包含运行产物和工作区文件。Graph 与插件仍会保留。</p>{cleanupIds.length > 0 && <ul className="channels-delete-list">{cleanupIds.map(id => <li key={id}><code>{id}</code></li>)}</ul>}<p>宿主会逐项核查执行、血缘和投递。遇到阻断立即停止，已删除的项目无法恢复；请核查状态后继续。</p></>}
      {error && <p className="channels-message bad" role="alert">{error}</p>}
      <div className="modal-actions"><button ref={cancel} type="button" disabled={pending} onClick={() => setAction(null)}>取消</button><button type="button" className="channels-danger" disabled={pending || (action === 'retire' && (!canRetire || bound?.run_id !== retiringRun))} onClick={() => {
        if (action === 'retire') void perform(() => api(`/channel-sessions/${enc(session.id)}/assistant/retire`, 'POST', { run_id: retiringRun }), '实例已退役；下一条可信消息才会创建新实例。', true);
        else void perform(cleanup, '渠道会话与历史已清理。', true);
      }}>{pending ? '正在处理…' : action === 'retire' ? '确认退役' : '永久清理'}</button></div>
    </Modal>}
  </article>;
}
