import { useCallback, useEffect, useRef, useState } from 'react';
import { ArrowUpRight, ChevronDown, Radio, RefreshCw, Trash2 } from 'lucide-react';
import { api, reason } from './api';
import type { RunProjection } from './model';
import { RunControls } from './RunControls';
import { runPhase, runPhaseLabel, runQuiescent } from './runPresentation';
import { Modal } from './ui';
import './Channels.css';

type ChannelSession = { id: string; title: string; graph: string; platform: string };
type Assistant = { run_id: string; wait_node: string; work_node: string; reply_node: string };
type AssistantStatus = { assistant: Assistant | null; needs_recovery: boolean;
  recovery_blocked?: string | null;
  plugin_update?: { state: 'current' | 'pending' | 'restart_required' | 'unavailable'; message?: string } };
type RunStatus = { run: string; active: boolean; projection?: RunProjection; state: { cursor?: { node: string } | null } };
type Delivery = { key: string; status: string; error?: string | null };
type SessionDetail = { session: { run_ids: string[] } };
const enc = encodeURIComponent;

/** Message-triggered conversations. Each one owns a lineage of Runs; stop, retire and cleanup act on
 *  the conversation, while each Run stays visible on the board like any other Run. */
export function ChannelSessions({ onOpenRun, close }: { onOpenRun?: (run: string, graph: string) => void; close: () => void }) {
  const [sessions, setSessions] = useState<ChannelSession[]>([]);
  const [sessionsRead, setSessionsRead] = useState<'loading' | 'ready' | 'failed'>('loading');
  const [readError, setReadError] = useState('');
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    const controller = new AbortController();
    let disposed = false;
    const load = async () => {
      try {
        const result = await api<{ sessions: ChannelSession[] }>('/channel-sessions', 'GET', undefined, controller.signal);
        if (!disposed) { setSessions(result.sessions); setSessionsRead('ready'); setReadError(''); }
      } catch (cause) { if (!disposed) { setSessionsRead('failed'); setReadError(`消息会话：${reason(cause)}`); } }
    };
    void load();
    const timer = window.setInterval(() => { void load(); }, 8000);
    return () => { disposed = true; controller.abort(); window.clearInterval(timer); };
  }, [revision]);
  const refresh = () => setRevision(value => value + 1);
  const open = onOpenRun && ((run: string, graph: string) => { close(); onOpenRun(run, graph); });
  return <Modal title="消息会话" close={close} className="channels-dialog channels-sessions-dialog">
    <div className="channels-dialog-intro"><p>每段可信对话拥有独立运行实例与工作现场。消息触发在工作流编辑页配置。</p>
      <span className="channels-count">{sessionsRead === 'ready' ? sessions.length : '未知'}</span>
      <button type="button" onClick={refresh}><RefreshCw size={14} />刷新状态</button></div>
    {readError && <div className="channels-message bad" role="alert"><strong>读取未完成</strong><p>{readError}</p><span>检查服务状态后重试。</span></div>}
    <section className="channels-assistants" aria-label="消息会话列表">
      {sessions.length ? <div className="channels-session-list">{sessions.map(session => <AssistantRow key={session.id} session={session} revision={revision} onChanged={refresh} onOpenRun={open} />)}</div>
        : <div className="channels-empty"><Radio size={24} /><strong>{sessionsRead === 'failed' ? '消息会话读取失败' : sessionsRead === 'ready' ? '还没有消息会话' : '正在读取会话…'}</strong><p>{sessionsRead === 'failed' ? '无法确认当前消息会话。请刷新状态重试。' : '在工作流编辑页配置并启用消息触发后，首条可信消息会创建会话；绑定的 Graph 会惰性创建运行实例。'}</p></div>}
      <p className="channels-footer-note">支持热更新的 Graph 保存插件配置后，各会话从下一条消息使用新配置。退役会解除当前绑定，下一条可信消息创建新实例。</p>
    </section>
  </Modal>;
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
    : assistant.recovery_blocked && !run.active ? '恢复已阻止'
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
  return <article className="channels-assistant-row" aria-label={`消息会话 ${session.title || session.id}`}>
    <div className="channels-assistant-top"><div><strong>{session.title || session.id}</strong><span>{session.platform || '渠道'} · {session.graph}</span></div><div className="channels-assistant-status"><span className={`channels-instance-status ${run?.active ? 'active' : ''}`}>{status}</span>
      {pluginUpdateLabel && <small className={`channels-plugin-state ${pluginUpdate?.state}`} title={pluginUpdate?.message} role={pluginUpdate?.state === 'unavailable' ? 'alert' : 'status'}>{pluginUpdateLabel}</small>}</div></div>
    {bound && <button type="button" className="channels-run-link" disabled={!onOpenRun} onClick={() => onOpenRun?.(bound.run_id, session.graph)}><code>{bound.run_id}</code><ArrowUpRight size={13} /></button>}
    {bound && assistant?.recovery_blocked && <p role="status">此历史实例不再匹配当前渠道身份或绑定，不会自动恢复。历史记录仍保留；请核查后退役。<small>{assistant.recovery_blocked}</small></p>}
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
    {action && <Modal title={action === 'retire' ? '退役当前实例？' : '永久清理这段会话历史？'} close={() => { if (!pending) setAction(null); }} initialFocus={cancel} className="channels-confirm">
      {action === 'retire' ? <><p>解除 <code>{retiringRun}</code> 的当前绑定，保留运行、产物与工作现场。旧实例不能再恢复。</p><p>下一条可信消息会按当前配置创建新实例；现有现场不会自动迁移到新版本。</p>{bound?.run_id !== retiringRun && <p className="channels-message warn">当前实例已变化，请关闭此窗口后重新核查。</p>}</>
        : <><p>永久删除“{session.title || session.id}”的消息会话及下列 {cleanupIds.length} 条运行，包含运行产物和工作区文件。Graph 与插件仍会保留。</p>{cleanupIds.length > 0 && <ul className="channels-delete-list">{cleanupIds.map(id => <li key={id}><code>{id}</code></li>)}</ul>}<p>宿主会逐项核查执行、血缘和投递。遇到阻断立即停止，已删除的项目无法恢复；请核查状态后继续。</p></>}
      {error && <p className="channels-message bad" role="alert">{error}</p>}
      <div className="modal-actions"><button ref={cancel} type="button" disabled={pending} onClick={() => setAction(null)}>取消</button><button type="button" className="channels-danger" disabled={pending || (action === 'retire' && (!canRetire || bound?.run_id !== retiringRun))} onClick={() => {
        if (action === 'retire') void perform(() => api(`/channel-sessions/${enc(session.id)}/assistant/retire`, 'POST', { run_id: retiringRun }), '实例已退役；下一条可信消息才会创建新实例。', true);
        else void perform(cleanup, '消息会话与历史已清理。', true);
      }}>{pending ? '正在处理…' : action === 'retire' ? '确认退役' : '永久清理'}</button></div>
    </Modal>}
  </article>;
}
