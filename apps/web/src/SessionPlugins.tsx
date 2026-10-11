/** Which Plugins this Pilot session can already use, and mounting them.
 *
 * The Host owns the facts: a session lists only installed Plugins, and a switch is written by POST,
 * never by guessing locally. Mounting takes effect from the next Turn, so this panel says that in
 * one place instead of promising something the current Turn cannot deliver.
 *
 * It polls while it is open because Pilot installs a Plugin itself in the middle of a session: the
 * new one has to appear without a page reload, and the repository has no push channel to use.
 */

import { useEffect, useRef, useState } from 'react';
import { Plug } from 'lucide-react';
import { api, reason } from './api';

/** One installed Plugin as the Host reports it for a session. */
export type SessionPlugin = {
  id: string; name: string; description?: string; skills?: string[]; mcpServerNames?: string[];
  required?: boolean; on?: boolean; available?: boolean; error?: string | null;
};

/** Long enough that a session is not one request per blink, short enough that a fresh install shows up. */
export const POLL_MS = 4000;

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

export function SessionPlugins({ session }: { session: string }) {
  const [open, setOpen] = useState(true);
  const [plugins, setPlugins] = useState<SessionPlugin[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [listError, setListError] = useState('');
  const [rowError, setRowError] = useState<Record<string, string>>({});
  const [switched, setSwitched] = useState('');
  const [pending, setPending] = useState('');
  // A second click before the Host answers must not become a second switch of the same row.
  const inFlight = useRef(false);
  useEffect(() => {
    if (!open || !session) { setPlugins([]); setLoaded(false); setListError(''); return; }
    let active = true;
    const load = async () => {
      try {
        const data = await api<{ plugins: SessionPlugin[] }>(`/sessions/${encodeURIComponent(session)}/plugins`);
        if (!active) return;
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
    setSwitched('');
    setRowError(current => { const next = { ...current }; delete next[plugin.id]; return next; });
    try {
      const result = await api<{ plugin: SessionPlugin }>(
        `/sessions/${encodeURIComponent(session)}/plugins/${encodeURIComponent(plugin.id)}`, 'POST', { on: !plugin.on });
      setPlugins(current => withPlugin(current, result.plugin));
      setSwitched(plugin.id);
    } catch (error) {
      // No optimistic flip: the row keeps the state the Host last confirmed, and the reason is the
      // Host's own sentence rather than a paraphrase written here.
      setRowError(current => ({ ...current, [plugin.id]: mountProblem(error) }));
    } finally { inFlight.current = false; setPending(''); }
  };
  const count = plugins.filter(plugin => plugin.on).length;
  return <section className="pilot-plugins" aria-label="会话插件挂载">
    <div className="pilot-plugins-heading">
      <div>
        <div className="plugins-kicker">本会话能力</div>
        <h3><Plug size={15} />插件挂载{plugins.length > 0 && <span className="count">{count}/{plugins.length}</span>}</h3>
      </div>
      <button type="button" aria-expanded={open} onClick={() => setOpen(!open)}>{open ? '收起' : '展开'}</button>
    </div>
    {open && <div className="pilot-plugins-body">
      <p className="pilot-plugins-note" role="note">开关从下一个回合起生效；当前回合已经在用的能力不会中途改变。</p>
      {!session && <p className="inspector-note">先开始或选择一个对话，这里会列出它已安装的插件。</p>}
      {session && !loaded && !plugins.length && !listError && <p role="status">正在读取本会话的插件…</p>}
      {listError && <p className="plugin-error" role="alert">插件列表读取失败：{listError}</p>}
      {session && loaded && !plugins.length && !listError && <p className="inspector-note">
        这个会话还没有已安装的插件。Pilot 装好插件后会自动出现在这里，不用刷新页面。
      </p>}
      <div className="plugin-list">{plugins.map(plugin => {
        const skills = plugin.skills ?? [];
        const servers = plugin.mcpServerNames ?? [];
        const unavailable = plugin.available === false;
        return <article key={plugin.id} className={`plugin-row${plugin.on ? ' attached' : ''}${unavailable ? ' unavailable' : ''}`}>
          <div className="plugin-row-label">
            <span className="plugin-row-copy">
              <strong>{plugin.name || plugin.id}</strong><small>{plugin.id}</small>
              {plugin.description && <span>{plugin.description}</span>}
              {!plugin.description && !skills.length && !servers.length && !unavailable
                && <span className="inspector-note">没有声明 Skills 或 MCP 服务。</span>}
            </span>
          </div>
          <div className="plugin-row-actions">
            {plugin.required && <span className="plugin-required" title="图里把这个插件声明为必需，不能关闭">必需</span>}
            {!!skills.length && <span className="plugin-tool-count" title={skills.join('、')}>{skills.length} Skills</span>}
            {!!servers.length && <span className="plugin-tool-count" title={servers.join('、')}>MCP {servers.join('、')}</span>}
            <button type="button" role="switch" aria-checked={plugin.on === true} aria-label={`${plugin.on ? '卸载' : '挂载'} ${plugin.name || plugin.id}`}
              className={`session-plugin-switch${plugin.on ? ' on' : ''}`} disabled={plugin.required === true || pending === plugin.id}
              title={plugin.required ? '图里声明为必需，无法关闭' : undefined}
              onClick={() => void toggle(plugin)}>{plugin.on ? '已挂载' : '未挂载'}</button>
          </div>
          {plugin.required && <p className="inspector-note">图里声明为必需，开关固定为开，点不动。</p>}
          {unavailable && <p className="plugin-error">不可用：{plugin.error || 'Host 未给出原因'}</p>}
          {rowError[plugin.id] && <p className="plugin-error" role="alert">{rowError[plugin.id]}</p>}
          {switched === plugin.id && !rowError[plugin.id] && <p className="plugin-note" role="status">已切换 · 将在下一个回合生效</p>}
        </article>;
      })}</div>
    </div>}
  </section>;
}
