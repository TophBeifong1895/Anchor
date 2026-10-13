/** Which Plugins this Pilot session can already use, and mounting them.
 *
 * The Host owns the facts: a session lists only installed Plugins, and a switch is written by POST,
 * never by guessing locally. The current Turn keeps its frozen capabilities; the Host reports
 * whether a change waits for the next message or schedules continuation after normal completion.
 *
 * It polls while it is open because Pilot installs a Plugin itself in the middle of a session: the
 * new one has to appear without a page reload, and the repository has no push channel to use.
 */

import { useEffect, useRef, useState } from 'react';
import { ChevronDown, CircleAlert, LoaderCircle, LockKeyhole, PanelRightClose, Plug, Search, Sparkles } from 'lucide-react';
import { api, reason } from './api';
import { Modal } from './ui';
import type { Turn } from './PilotTurn';
import './SessionPlugins.css';

/** One installed Plugin as the Host reports it for a session. */
export type SessionPlugin = {
  id: string; name: string; description?: string; skills?: string[]; mcpServerNames?: string[];
  required?: boolean; on?: boolean; available?: boolean; error?: string | null;
  explicitOn?: boolean; requiresNetwork?: boolean; networkProvider?: boolean;
  networkAllowed?: boolean; networkPermission?: boolean; enabledBy?: string[];
};

export type PluginMountEffect = {
  status: 'next_turn' | 'after_current_turn' | 'unchanged'; source_turn?: string;
};
export type PluginContinuation = {
  session: string; source: string; state: NonNullable<Turn['continuation']>;
};
type MountChange = { effect: PluginMountEffect; continuation?: Turn['continuation'] };

function mountNotice(change: MountChange, continuation?: PluginContinuation | null): string {
  if (change.effect.status === 'unchanged') return '设置未改变';
  if (change.effect.status !== 'after_current_turn') return '已保存 · 下次发送消息时生效';
  const state = continuation && continuation.source === change.effect.source_turn ? continuation.state : change.continuation;
  switch (state?.status) {
    case 'admitted': return '已应用到接续回合';
    case 'cancelled': return '已保存 · 自动继续已取消，等待下次消息';
    case 'pending': return '已保存 · 本轮未能自动继续，等待下次消息';
    case 'superseded': return '已保存 · 原任务的自动继续已被新回合替代';
    case 'scheduled': return '已保存 · 当前回合正常结束后自动继续';
    default: return '已保存 · 下次发送消息时生效';
  }
}

/** Long enough that a session is not one request per blink, short enough that a fresh install shows up. */
export const POLL_MS = 4000;
const COMPACT_QUERY = '(max-width: 1100px)';
type PluginFilter = 'all' | 'attached' | 'detached';

/** Replace a row with what the Host returned. A row the Host no longer reports keeps its old place
 * rather than vanishing under the pointer that just clicked it. */
export function withPlugin(list: SessionPlugin[], plugin: SessionPlugin): SessionPlugin[] {
  return list.map(item => item.id === plugin.id ? plugin : item);
}

/** A required Plugin has no switch to make, and one switch is already waiting on the Host. */
export function maySwitch(plugin: SessionPlugin, inFlight: boolean): boolean {
  return !plugin.required && !inFlight;
}

/** Turn the server's `error` into the line under the row, verbatim. */
export function mountProblem(error: unknown): string {
  return reason(error);
}

export function SessionPlugins({ session, onContinuation, continuation }: {
  session: string; onContinuation?: (session: string, source: string) => void;
  continuation?: PluginContinuation | null;
}) {
  // The transcript keeps its full width on narrow screens; the same controls open in the shared
  // native dialog, which isolates focus and handles Escape.
  const [compact, setCompact] = useState(() => typeof window !== 'undefined'
    && typeof window.matchMedia === 'function' && window.matchMedia(COMPACT_QUERY).matches);
  const [open, setOpen] = useState(() => !compact);
  const [plugins, setPlugins] = useState<SessionPlugin[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [listError, setListError] = useState('');
  const [rowError, setRowError] = useState<Record<string, string>>({});
  const [changes, setChanges] = useState<Record<string, MountChange>>({});
  const [pending, setPending] = useState('');
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState<PluginFilter>('all');
  const trigger = useRef<HTMLButtonElement>(null);
  const wasOpen = useRef(open);
  // A second click before the Host answers must not become a second switch of the same row.
  const inFlight = useRef(false);
  const mounted = useRef(true);
  const sessionRef = useRef(session);
  sessionRef.current = session;
  const listRevision = useRef(0);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  useEffect(() => {
    // Removing a dialog does not reliably restore focus like dialog.close() does. Return focus
    // after React has removed the shared Modal and the compact trigger is no longer inert.
    if (compact && wasOpen.current && !open) trigger.current?.focus();
    wasOpen.current = open;
  }, [compact, open]);
  useEffect(() => {
    if (typeof window.matchMedia !== 'function') return;
    const media = window.matchMedia(COMPACT_QUERY);
    const resize = () => { setCompact(media.matches); setOpen(!media.matches); };
    media.addEventListener('change', resize);
    return () => media.removeEventListener('change', resize);
  }, []);
  useEffect(() => {
    if (!session) { setPlugins([]); setLoaded(false); setListError(''); return; }
    // Keep the last confirmed count when collapsed, while stopping all background polling.
    if (!open) return;
    let active = true;
    const load = async () => {
      const revision = listRevision.current;
      try {
        const data = await api<{ plugins: SessionPlugin[] }>(`/sessions/${encodeURIComponent(session)}/plugins`);
        if (!active || revision !== listRevision.current) return;
        setPlugins(data.plugins ?? []);
        setListError('');
      } catch (error) {
        // Keep the rows already read: a failed poll is not evidence that nothing is installed.
        if (active) setListError(mountProblem(error));
      } finally { if (active) setLoaded(true); }
    };
    void load();
    const timer = window.setInterval(() => { void load(); }, POLL_MS);
    return () => { active = false; window.clearInterval(timer); };
  }, [open, session]);
  const toggle = async (plugin: SessionPlugin) => {
    if (!maySwitch(plugin, inFlight.current)) return;
    inFlight.current = true;
    setPending(plugin.id);
    setRowError(current => { const next = { ...current }; delete next[plugin.id]; return next; });
    try {
      const result = await api<{ plugin: SessionPlugin; plugins?: SessionPlugin[]; effect?: PluginMountEffect;
        continuation?: Turn['continuation'] }>(
        `/sessions/${encodeURIComponent(session)}/plugins/${encodeURIComponent(plugin.id)}`, 'POST', { on: !(plugin.explicitOn ?? plugin.on) });
      if (!mounted.current || sessionRef.current !== session) return;
      listRevision.current += 1;
      setPlugins(current => result.plugins ?? withPlugin(current, result.plugin));
      const effect = result.effect ?? { status: 'next_turn' };
      setChanges(current => ({ ...current, [plugin.id]: { effect, continuation: result.continuation } }));
      if (effect.status === 'after_current_turn' && effect.source_turn && result.continuation?.status === 'scheduled') {
        onContinuation?.(session, effect.source_turn);
      }
    } catch (error) {
      // No optimistic flip: the row keeps the state the Host last confirmed, and the reason is the
      // Host's own sentence rather than a paraphrase written here.
      if (mounted.current && sessionRef.current === session) setRowError(current => ({ ...current, [plugin.id]: mountProblem(error) }));
    } finally {
      inFlight.current = false;
      if (mounted.current && sessionRef.current === session) setPending('');
    }
  };
  const changeNetwork = async (plugin: SessionPlugin) => {
    if (inFlight.current) return;
    inFlight.current = true;
    setPending(plugin.id);
    setRowError(current => { const next = { ...current }; delete next[plugin.id]; return next; });
    try {
      const result = await api<{ plugins: SessionPlugin[] }>(
        `/sessions/${encodeURIComponent(session)}/network`, 'POST', { allowed: !plugin.networkPermission });
      if (!mounted.current || sessionRef.current !== session) return;
      listRevision.current += 1;
      setPlugins(result.plugins);
      setChanges(current => ({ ...current, [plugin.id]: { effect: { status: 'next_turn' } } }));
    } catch (error) {
      if (mounted.current && sessionRef.current === session) setRowError(current => ({ ...current, [plugin.id]: mountProblem(error) }));
    } finally {
      inFlight.current = false;
      if (mounted.current && sessionRef.current === session) setPending('');
    }
  };
  const count = plugins.filter(plugin => plugin.on).length;
  const needle = query.trim().toLocaleLowerCase();
  const visible = plugins.filter(plugin => (filter === 'all' || (plugin.on === true) === (filter === 'attached'))
    && (!needle || [plugin.name, plugin.id, plugin.description, ...(plugin.skills ?? []), ...(plugin.mcpServerNames ?? [])]
      .some(value => value?.toLocaleLowerCase().includes(needle))));
  const knowsCount = !!session && (plugins.length > 0 || (loaded && !listError));
  const body = <>
    <div className="pilot-plugins-summary"><span>{knowsCount ? <>已安装 <strong>{plugins.length}</strong> 个插件</>
      : !session ? '为当前对话配置插件' : listError ? '暂时无法读取插件列表' : '正在读取已安装的插件…'}</span>
      {knowsCount && <span className="plugin-mounted-count"><span aria-hidden="true" />{count} 已挂载</span>}
    </div>
    <div className="pilot-plugins-body">
      <div className="pilot-plugins-intro">
        <div className="pilot-plugins-note" role="note"><Sparkles size={13} /> <span>当前回合保持原有能力</span></div>
        <p>运行中修改后，正常结束会自动继续原任务；空闲时等待下一次消息。</p>
      </div>
      {!!plugins.length && <div className="pilot-plugins-toolbar">
        <label className="session-plugin-search"><Search size={14} /><input type="search" aria-label="搜索插件" placeholder="搜索名称或能力…"
          value={query} onChange={event => setQuery(event.target.value)} /></label>
        <div className="session-plugin-filters" role="group" aria-label="插件筛选">
          {([['all', '全部', plugins.length], ['attached', '已挂载', count], ['detached', '未挂载', plugins.length - count]] as const)
            .map(([value, label, total]) => <button key={value} type="button" aria-pressed={filter === value}
              className={filter === value ? 'chosen' : ''} onClick={() => setFilter(value)}>{label}<span>{total}</span></button>)}
        </div>
      </div>}
      {!session && <div className="session-plugins-empty"><Plug size={24} /><strong>为对话选择能力</strong><p>先开始或选择一个对话，再为 Pilot 挂载需要的插件。</p></div>}
      {session && !loaded && !plugins.length && !listError && <div className="session-plugins-empty" role="status"><LoaderCircle size={22} /><p>正在读取本会话的插件…</p></div>}
      {listError && <p className="session-plugins-error" role="alert"><CircleAlert size={14} />插件列表读取失败：{listError}</p>}
      {session && loaded && !plugins.length && !listError && <div className="session-plugins-empty"><Plug size={24} /><strong>还没有安装插件</strong><p>告诉 Pilot 你需要什么能力。安装完成后，插件会自动出现在这里。</p></div>}
      {!!plugins.length && !visible.length && <div className="session-plugins-empty"><Search size={22} /><strong>{needle ? '没有找到匹配的插件' : filter === 'attached' ? '还没有挂载插件' : '所有插件均已挂载'}</strong><p>{needle ? '试试其他名称，或清空搜索。' : '选择「全部」查看已安装的插件。'}</p>
        <button type="button" onClick={() => { setQuery(''); setFilter('all'); }}>查看全部插件</button></div>}
      <div className="session-plugin-list">{visible.map(plugin => {
        const skills = plugin.skills ?? [];
        const servers = plugin.mcpServerNames ?? [];
        const unavailable = plugin.available === false;
        const initial = (plugin.name || plugin.id).trim().slice(0, 1).toUpperCase();
        const direct = plugin.explicitOn ?? plugin.on;
        const dependents = (plugin.enabledBy ?? []).map(id => plugins.find(item => item.id === id)?.name || id);
        return <article key={plugin.id} className={`session-plugin-card${plugin.on ? ' attached' : ''}${unavailable ? ' unavailable' : ''}`}>
          <div className="plugin-row-main">
            <span className="plugin-avatar" aria-hidden="true">{initial || '?'}</span>
            <div className="plugin-row-copy">
              <div className="plugin-row-name"><strong>{plugin.name || plugin.id}</strong></div>
              <small>{plugin.id}</small>
            </div>
          </div>
          <div className="plugin-row-actions">
            <button type="button" role="switch" aria-checked={direct === true} aria-label={`${direct ? '取消挂载' : '挂载'} ${plugin.name || plugin.id}`}
              className={`session-plugin-switch${direct ? ' on' : ''}`} disabled={plugin.required === true || pending === plugin.id}
              title={plugin.required ? '图里声明为必需，无法关闭' : undefined}
              aria-busy={pending === plugin.id} onClick={() => void toggle(plugin)}><span className="switch-track" aria-hidden="true" />{plugin.networkProvider ? direct ? '主动启用' : '未主动启用' : plugin.on ? '已挂载' : '未挂载'}</button>
          </div>
          {plugin.description && <p className="session-plugin-description">{plugin.description}</p>}
          {plugin.networkProvider && <div className="session-plugin-network">
            <p>{dependents.length ? `${direct ? '主动启用 · ' : ''}由${dependents.join('、')}依赖启用` : direct ? '已主动启用' : '未启用'}
              {' · '}{plugin.networkPermission === false ? '已禁止联网' : plugin.networkAllowed ? '网络访问已允许' : '网络访问未启用'}</p>
            <button type="button" disabled={pending === plugin.id} onClick={() => void changeNetwork(plugin)}>
              {plugin.networkPermission === false ? '允许联网' : '禁止联网'}</button>
          </div>}
          {plugin.requiresNetwork && !plugin.networkProvider && <p className="plugin-note">需要联网能力{plugin.on && plugin.networkAllowed ? ' · 网络访问已允许' : ''}</p>}
          {!plugin.description && !skills.length && !servers.length && !unavailable && <p className="inspector-note">没有声明 Skills 或 MCP 服务。</p>}
          {!!(skills.length || servers.length) && <details className="session-plugin-details">
            <summary aria-label={`查看 ${plugin.name || plugin.id} 的能力详情`}><span className="plugin-row-meta">
              {!!skills.length && <span><Sparkles size={11} />{skills.length} Skills</span>}
              {!!servers.length && <span><Plug size={11} />{servers.length} MCP</span>}
            </span><ChevronDown size={13} /></summary>
            <div className="session-plugin-resources">
              {!!skills.length && <div><strong>Skills</strong><p>{skills.join('、')}</p></div>}
              {!!servers.length && <div><strong>MCP 服务</strong><p>MCP {servers.join('、')}</p></div>}
            </div>
          </details>}
          {plugin.required && <p className="session-plugin-required"><LockKeyhole size={12} /><span>必需 · 工作流需要此插件，保持挂载。</span></p>}
          {unavailable && <p className="plugin-error"><CircleAlert size={12} /><span>不可用：{plugin.error || 'Host 未给出原因'}</span></p>}
          {rowError[plugin.id] && <p className="plugin-error" role="alert"><CircleAlert size={12} /><span>{rowError[plugin.id]}</span></p>}
          {changes[plugin.id] && !rowError[plugin.id] && <p className="plugin-note" role="status">{mountNotice(changes[plugin.id], continuation?.session === session ? continuation : null)}</p>}
        </article>;
      })}</div>
    </div>
    <div className="pilot-plugins-footer"><Plug size={14} /><p>需要其他能力？直接告诉 Pilot。<span>新安装的插件会自动出现在这里。</span></p></div>
  </>;
  return <aside className={`pilot-plugins${open ? ' is-open' : ' is-closed'}`} aria-label="会话插件挂载">
    {compact ? <button ref={trigger} className="session-plugins-trigger" type="button" aria-haspopup="dialog" aria-expanded={open}
      aria-label="展开插件面板" onClick={() => setOpen(true)}><Plug size={15} />插件{knowsCount && <span>{count}</span>}</button>
      : <div className="pilot-plugins-heading">
        <div className="pilot-plugins-title"><span className="plugin-heading-icon"><Plug size={18} /></span><div>
          <div className="plugins-kicker">本会话能力</div><h3>插件</h3>
        </div></div>
        <button type="button" aria-expanded={open} aria-label={open ? '收起插件面板' : '展开插件面板'} title={open ? '收起插件面板' : '展开插件面板'}
          onClick={() => setOpen(!open)}>{open ? <PanelRightClose size={16} /> : <Plug size={16} />}<span>{open ? '收起' : '插件'}</span>{!open && knowsCount && <span className="plugin-trigger-count">{count}</span>}</button>
      </div>}
    {open && (compact ? <Modal title="会话插件" className="session-plugins-drawer" close={() => setOpen(false)}>{body}</Modal> : body)}
  </aside>;
}
