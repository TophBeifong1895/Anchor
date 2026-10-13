import { useEffect, useState } from 'react';
import { RefreshCw } from 'lucide-react';
import { api, reason } from './api';
import type { Plugin } from './model';
import { PluginCard, PluginDetails, PluginToolbar } from './PluginPanel';
import { LibraryPluginDetails } from './LibraryPluginDetails';

type PluginFilter = 'all' | 'selected' | 'available';

/** Graph owns draft references; Run owns captured metadata; the Library owns current definitions. */
export function Plugins({ selected, onChange, disabled = false, recorded }:
  { selected?: string[]; onChange?: (ids: string[]) => void; disabled?: boolean; recorded?: Plugin[] }) {
  const [catalog, setCatalog] = useState<Plugin[]>([]);
  const [catalogKnown, setCatalogKnown] = useState(false);
  const [error, setError] = useState('');
  const [opened, setOpened] = useState<Plugin | null>(null);
  const [loading, setLoading] = useState(recorded === undefined);
  const [revision, setRevision] = useState(0);
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState<PluginFilter>('all');
  useEffect(() => {
    if (recorded !== undefined) return;
    let active = true;
    setLoading(true); setError('');
    api<{ plugins: Plugin[] }>('/plugins').then(data => {
      if (!Array.isArray(data.plugins)) throw new Error('插件列表响应无效，请重试。');
      if (active) { setCatalog(data.plugins); setCatalogKnown(true); }
    }).catch(problem => { if (active) setError(reason(problem)); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [recorded, revision]);
  useEffect(() => { setOpened(null); setQuery(''); setFilter('all'); }, [recorded !== undefined]);
  type CatalogItem = Plugin & { unconfirmed?: boolean };
  const items: CatalogItem[] = recorded ?? [...catalog, ...(selected ?? []).filter(id => !catalog.some(plugin => plugin.id === id))
    .map(id => ({ id, name: id, description: '', available: catalogKnown ? false : undefined,
      error: catalogKnown ? '插件未在库中找到' : undefined, unconfirmed: !catalogKnown }))];
  const selectedIds = new Set(selected ?? []);
  const needle = query.trim().toLocaleLowerCase();
  const visible = items.filter(plugin => recorded !== undefined || filter === 'all'
    || (filter === 'selected' ? selectedIds.has(plugin.id) : plugin.available !== false))
    .filter(plugin => !needle || [plugin.name, plugin.id, plugin.description, ...(plugin.skills ?? []), ...Object.keys(plugin.mcpServers ?? {})]
      .some(value => value?.toLocaleLowerCase().includes(needle)))
    .sort((left, right) => (recorded === undefined ? Number(selectedIds.has(right.id)) - Number(selectedIds.has(left.id)) : 0)
      || left.name.localeCompare(right.name, 'zh-CN'));
  const status = (plugin: CatalogItem) => recorded !== undefined ? '运行时绑定记录'
    : plugin.unconfirmed ? '已选择 · 状态未确认'
    : plugin.available === false ? selectedIds.has(plugin.id) ? '已选择 · 不可用' : '不可用'
      : selectedIds.has(plugin.id) ? '已选择' : '未选择';
  return <section className="plugin-panel plugins-panel" aria-label={recorded !== undefined ? '运行插件记录' : '节点插件配置'}>
    <div className="plugin-panel-header"><div><p className="plugin-panel-context">{recorded !== undefined ? '本次运行' : '节点能力'}</p><h3>插件</h3></div>
      {recorded === undefined && <button className="plugin-panel-refresh" type="button" disabled={loading}
        onClick={() => setRevision(value => value + 1)}><RefreshCw size={14} />刷新</button>}
    </div>
    <p className="plugin-panel-note">{recorded !== undefined ? '本次运行保存的绑定记录，不代表实际调用或任务成功。' : '更改保存后用于后续运行。'}</p>
    {!!items.length && <PluginToolbar query={query} onQuery={setQuery} filter={filter}
      onFilter={value => setFilter(value as PluginFilter)} filters={recorded !== undefined ? [] : [
        { value: 'all', label: '全部' }, { value: 'selected', label: '已选择' }, { value: 'available', label: '可用' },
      ]} />}
    {loading && <p className="plugin-message" role="status">正在读取插件库…</p>}
    {error && <p role="alert" className="plugin-message is-error">插件库读取失败：{error}</p>}
    {!loading && !error && !items.length && <div className="plugin-empty"><strong>{recorded !== undefined ? '没有插件绑定记录' : '还没有安装插件'}</strong>
      <p>{recorded !== undefined ? '此运行没有保存插件绑定信息。' : '告诉 Pilot 你需要什么能力。安装完成后刷新，即可为节点选择插件。'}</p></div>}
    {!!items.length && !visible.length && <div className="plugin-empty"><strong>没有匹配的插件</strong><p>试试其他名称，或清空搜索。</p>
      <button type="button" onClick={() => { setQuery(''); setFilter('all'); }}>查看全部插件</button></div>}
    <div className="plugin-card-list">{visible.map(plugin => {
      const isSelected = selectedIds.has(plugin.id);
      return <PluginCard key={plugin.id} plugin={plugin} status={status(plugin)}
        active={recorded === undefined && isSelected} unavailable={recorded === undefined && plugin.available === false}
        onDetails={() => setOpened(plugin)} control={recorded === undefined && onChange ? <input className="plugin-selection" type="checkbox"
          aria-label={`${isSelected ? '取消选择' : '选择'} ${plugin.name || plugin.id}`} checked={isSelected}
          disabled={disabled || plugin.available === false && !isSelected}
          onChange={event => onChange(event.target.checked ? [...(selected ?? []), plugin.id]
            : (selected ?? []).filter(id => id !== plugin.id))} /> : undefined}>
        {plugin.error && <p className="plugin-message is-error">不可用：{plugin.error}</p>}
        {!!plugin.unsupported?.length && <p className="plugin-message is-error">暂不支持：{plugin.unsupported.join('、')}</p>}
      </PluginCard>;
    })}</div>
    {opened && (recorded !== undefined ? <PluginDetails plugin={opened} status={status(opened)} recorded close={() => setOpened(null)} />
      : <LibraryPluginDetails key={opened.id} plugin={opened} status={status(opened)} close={() => setOpened(null)} />)}
  </section>;
}
