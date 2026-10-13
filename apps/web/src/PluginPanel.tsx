import type { ReactNode } from 'react';
import { Search } from 'lucide-react';
import { Markdown } from './markdown';
import { Modal } from './ui';
import './PluginPanel.css';

export type PluginInformation = {
  id: string; name: string; description?: string; skills?: string[]; digest?: string;
  mcpServers?: Record<string, { transport: string; auth?: string }>;
  mcpServerNames?: string[]; instructions?: string; error?: string | null; unsupported?: string[];
};

/** Presentation only: selection, policy and library facts belong to the caller. */
export function PluginToolbar({ query, onQuery, filters = [], filter, onFilter }: {
  query: string; onQuery: (query: string) => void;
  filters?: { value: string; label: string; count?: number }[]; filter?: string;
  onFilter?: (filter: string) => void;
}) {
  return <div className="plugin-toolbar">
    <div className="plugin-search-control"><Search size={16} aria-hidden="true" />
      <input type="search" aria-label="搜索插件" placeholder="搜索名称或能力…" value={query}
        onChange={event => onQuery(event.target.value)} />
    </div>
    {!!filters.length && <div className="plugin-filter-control" role="group" aria-label="插件筛选">
      {filters.map(item => <button type="button" key={item.value} aria-pressed={filter === item.value}
        className={filter === item.value ? 'chosen' : ''} onClick={() => onFilter?.(item.value)}>
        {item.label}{item.count !== undefined && <span>{item.count}</span>}
      </button>)}
    </div>}
  </div>;
}

export function PluginCard({ plugin, status, active = false, unavailable = false, control, children, onDetails, className = '' }: {
  plugin: PluginInformation; status: string; active?: boolean; unavailable?: boolean;
  control?: ReactNode; children?: ReactNode; onDetails: () => void; className?: string;
}) {
  return <article className={`plugin-card ${className}${active ? ' attached' : ''}${unavailable ? ' unavailable' : ''}`}>
    <div className="plugin-card-heading"><h4 className="plugin-card-name">{plugin.name || plugin.id}</h4>
      {control && <div className="plugin-card-control">{control}</div>}
    </div>
    <p className="plugin-card-description">{plugin.description || '此插件尚未提供用途说明。'}</p>
    <div className="plugin-card-footer"><span className={`plugin-card-status${unavailable ? ' is-unavailable' : ''}`}>{status}</span>
      <button className="plugin-card-details" type="button" aria-label={`查看 ${plugin.name || plugin.id} 的详情`} onClick={onDetails}>详情</button>
    </div>
    {children}
  </article>;
}

export function PluginDetails({ plugin, status, recorded = false, loading = false, error = '', retry, close, children }: {
  plugin: PluginInformation; status: string; recorded?: boolean; loading?: boolean; error?: string;
  retry?: () => void; close: () => void; children?: ReactNode;
}) {
  const servers = Object.keys(plugin.mcpServers ?? {});
  const serverNames = servers.length ? servers : plugin.mcpServerNames ?? [];
  return <Modal title={`${plugin.name || plugin.id} · 插件详情`} className="plugin-details" close={close}>
    <p className="plugin-details-source">{recorded ? '历史记录 · 仅展示本次运行保存的插件信息，不代表实际调用或任务成功。' : '当前插件库 · 只读查看插件定义。'}</p>
    <div className="plugin-details-overview"><span className="plugin-card-status">{status}</span>
      <p>{plugin.description || '此插件尚未提供用途说明。'}</p>
      <dl className="plugin-details-metadata"><div><dt>标识</dt><dd><code>{plugin.id}</code></dd></div>
        {plugin.digest && <div><dt>内容摘要</dt><dd><code>{plugin.digest}</code></dd></div>}
      </dl>
    </div>
    {plugin.error && <p className="plugin-message is-error">不可用：{plugin.error}</p>}
    {!!plugin.unsupported?.length && <p className="plugin-message is-error">暂不支持：{plugin.unsupported.join('、')}</p>}
    {loading && <p className="plugin-message" role="status">正在读取插件详情…</p>}
    {error && <div className="plugin-message is-error" role="alert"><span>插件详情读取失败：{error}</span>
      {retry && <button type="button" onClick={retry}>重试</button>}</div>}
    <section className="plugin-details-section"><h3>技能 <span>Skills</span></h3>
      {plugin.skills?.length ? <ul>{plugin.skills.map((skill, index) => <li key={`${skill}-${index}`}>{skill}</li>)}</ul>
        : <p>{plugin.skills !== undefined ? '未声明技能。' : recorded ? '本次运行未保存技能信息。'
          : loading ? '正在读取技能信息…' : error ? '技能信息暂时无法读取。' : '未声明技能。'}</p>}
    </section>
    <section className="plugin-details-section"><h3>工具服务 <span>MCP</span></h3>
      {serverNames.length ? <ul>{serverNames.map(name => <li key={name}><strong>{name}</strong>
        {plugin.mcpServers?.[name]?.transport && <span className="plugin-details-transport">{plugin.mcpServers[name].transport}</span>}
      </li>)}</ul> : <p>{plugin.mcpServers !== undefined || plugin.mcpServerNames !== undefined ? '未声明工具服务。'
        : recorded ? '本次运行未保存工具服务信息。' : loading ? '正在读取工具服务信息…'
          : error ? '工具服务信息暂时无法读取。' : '未声明工具服务。'}</p>}
    </section>
    {children}
    <section className="plugin-details-section"><h3>使用说明</h3>
      {plugin.instructions ? <Markdown text={plugin.instructions} prefix={`plugin-${plugin.id}`}
        fileBase={recorded ? undefined : `/plugins/${encodeURIComponent(plugin.id)}/files/`} />
        : <p>{recorded ? '本次运行未保存使用说明。' : loading ? '正在读取使用说明…'
          : error ? '使用说明暂时无法读取。请重试。' : '此插件未提供使用说明。'}</p>}
    </section>
  </Modal>;
}
