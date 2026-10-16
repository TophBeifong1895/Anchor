import { useState } from 'react';
import { Plugins } from './Plugins';
import { PluginInstall, PluginOperations } from './PluginLifecycle';
import './PluginManagement.css';

/** Global Library workspace; deliberately independent of any Graph or Session. */
export function PluginManagement() {
  const [installing, setInstalling] = useState(false);
  const [revision, setRevision] = useState(0);
  return <main className="plugin-management"><header className="plugin-management-heading"><div><h2>插件库</h2>
    <p className="plugin-management-note">管理共享插件、凭据与安装版本。</p></div>
    <button type="button" onClick={() => setInstalling(true)}>安装或导入插件</button></header>
    <Plugins refreshRevision={revision} management onChanged={() => setRevision(current => current + 1)} />
    <PluginOperations onChanged={() => setRevision(current => current + 1)} />
    {installing && <PluginInstall close={() => setInstalling(false)} onChanged={() => setRevision(current => current + 1)} />}
  </main>;
}
