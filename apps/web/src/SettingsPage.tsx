import { useEffect, useRef, useState } from 'react';
import { RefreshCw } from 'lucide-react';
import { api, reason } from './api';
import { ModelProviderSettings } from './ModelProviderSettings';
import './SettingsPage.css';

type RuntimeFacts = { python_bootstrap_available: boolean; python_prepare_available?: boolean; python_version?: string | null; python_version_error?: string | null; rust_bootstrap_available: boolean; python_configuration_owner: string; rust_configuration_owner: string; python_default_version: string; rust_default_toolchain: string; python_configured: boolean; python_enabled: boolean; python_explicitly_disabled: boolean; rust_configured: boolean; rust_toolchain: string | null };
type RuntimeOperation = { kind: string; version: string | null; status: string; phase: string; started_at: string; updated_at: string; message: string | null };
type RuntimeStatus = { runtime: RuntimeFacts; operation: RuntimeOperation | null };
type Notice = { kind: 'ok' | 'bad'; text: string } | null;
const phases: Record<string, string> = { starting: '开始初始化', waiting_for_lock: '等待当前环境操作结束', checking_configuration: '检查环境配置', installing_python: '下载并安装 Python', probing_python: '验证 Python 解释器', installing_rustup: '安装 Rust 工具链管理器', checking_rust_toolchain: '检查已有 Rust 工具链', installing_rust_toolchain: '下载并安装 Rust 工具链', probing_rust_toolchain: '验证 Rustc 与 Cargo', publishing_configuration: '保存可用环境', succeeded: '初始化完成', failed: '初始化失败', cancelled: '初始化已取消', cancelling: '正在停止初始化', interrupted: '初始化被重启中断' };
const active = (operation: RuntimeOperation | null | undefined) => operation?.status === 'running' || operation?.status === 'cancelling';
const runtimeMessage = (message: string) => message.includes('native uv bootstrap tool') ? '此前初始化未能完成：部署未配置 Python 依赖准备工具。' : message;

export function SettingsPage() {
  const [runtime, setRuntime] = useState<RuntimeStatus | null>(null);
  const [runtimeNotice, setRuntimeNotice] = useState<Notice>(null);
  const [loadProblem, setLoadProblem] = useState('');
  const [busy, setBusy] = useState('');
  const [refreshing, setRefreshing] = useState(false);
  const [refreshVersion, setRefreshVersion] = useState(0);
  const [allowNetwork, setAllowNetwork] = useState(false);
  const [pythonVersion, setPythonVersion] = useState('3.12');
  const [rustVersion, setRustVersion] = useState('stable');
  const alive = useRef(true);
  // Operation responses do not run version probes. Keep the last observation
  // until the next status read; a failed read explicitly returns null.
  const acceptOperation = (status: RuntimeStatus) => setRuntime(previous => ({ ...status, runtime: { python_version: previous?.runtime.python_version, python_version_error: previous?.runtime.python_version_error, ...status.runtime } }));
  const refresh = async (signal?: AbortSignal) => {
    setRefreshing(true);
    try {
      const status = await api<RuntimeStatus>('/settings/runtime', 'GET', undefined, signal);
      if (!signal?.aborted && alive.current) { setRuntime(status); setLoadProblem(''); }
    } catch (error) {
      if (!signal?.aborted && alive.current) setLoadProblem(`环境状态：${reason(error)}`);
    } finally { if (!signal?.aborted && alive.current) setRefreshing(false); }
  };
  useEffect(() => {
    alive.current = true;
    const controller = new AbortController(); void refresh(controller.signal);
    return () => { alive.current = false; controller.abort(); };
  }, []);
  useEffect(() => {
    if (!active(runtime?.operation)) return;
    const controller = new AbortController();
    const timer = window.setInterval(() => {
      void api<RuntimeStatus>('/settings/runtime', 'GET', undefined, controller.signal).then(status => { if (!controller.signal.aborted) { setRuntime(status); setRuntimeNotice(null); } }).catch(error => { if (!controller.signal.aborted) setRuntimeNotice({ kind: 'bad', text: `状态刷新失败：${reason(error)}。初始化仍由服务继续执行。` }); });
    }, 1500);
    return () => { window.clearInterval(timer); controller.abort(); };
  }, [runtime?.operation?.status]);
  const prepare = async (kind: 'python' | 'rust') => {
    setBusy(kind); setRuntimeNotice(null);
    try { const status = await api<RuntimeStatus>('/settings/runtime/prepare', 'POST', { kind, version: kind === 'python' ? (runtime?.runtime.python_configuration_owner === 'deployment' ? null : pythonVersion) : rustVersion, allow_network: allowNetwork }); if (alive.current) acceptOperation(status); }
    catch (error) { if (alive.current) setRuntimeNotice({ kind: 'bad', text: `初始化失败：${runtimeMessage(reason(error))}` }); }
    finally { if (alive.current) setBusy(''); }
  };
  const cancel = async () => {
    setBusy('cancel');
    try { const status = await api<RuntimeStatus>('/settings/runtime/cancel', 'POST'); if (alive.current) acceptOperation(status); }
    catch (error) { if (alive.current) setRuntimeNotice({ kind: 'bad', text: `停止失败：${reason(error)}` }); }
    finally { if (alive.current) setBusy(''); }
  };
  const facts = runtime?.runtime;
  const operation = runtime?.operation;
  const pythonPrepareAvailable = !!facts && !facts.python_explicitly_disabled && (facts.python_prepare_available ?? facts.python_bootstrap_available);
  const rustPrepareAvailable = !!facts?.rust_bootstrap_available && facts.rust_configuration_owner !== 'deployment';
  const pythonVersionFixed = facts?.python_configuration_owner === 'deployment';
  const detectedPythonVersion = facts?.python_version ?? (facts?.python_version_error ? '检测失败' : '未检测到版本');
  const pythonPrepareHelp = facts?.python_explicitly_disabled ? '部署已禁用 Python 初始化，请联系部署者调整配置。' : facts?.python_configured && facts.python_enabled ? '已有 Python 解释器仍可使用；此部署尚未配置可用的依赖准备工具，请由部署者配置后再初始化。' : '此部署尚未配置可用的 Python 初始化和依赖准备工具。请由部署者配置或安装完整平台发行包。';
  const retryAvailable = operation?.kind === 'python' ? pythonPrepareAvailable : rustPrepareAvailable;
  return <main className="settings-page">
    <div className="settings-content">
      <header className="settings-heading"><div><h1>设置</h1><p>管理模型服务和开发环境。</p></div><button type="button" onClick={() => { setRefreshVersion(value => value + 1); void refresh(); }} disabled={refreshing || !!busy}><RefreshCw size={14} />{refreshing ? '正在刷新' : '刷新状态'}</button></header>
      {loadProblem && <p className="settings-notice bad" role="alert">{loadProblem}</p>}
      <ModelProviderSettings refreshVersion={refreshVersion} />
      <section className="settings-section" aria-labelledby="settings-runtime-heading">
        <div className="settings-section-heading"><h2 id="settings-runtime-heading">开发环境</h2><span className={`settings-state ${facts?.python_configured && facts.python_enabled && facts.rust_configured ? 'ready' : ''}`}><span />{!facts ? '正在读取' : active(operation) ? '正在初始化' : '按需准备'}</span></div>
        <p className="settings-help">初始化后，后续任务可使用 Python 或 Rust。正在运行的任务保持原有环境；关闭页面不会停止初始化。</p>
        <label className="settings-network"><input type="checkbox" checked={allowNetwork} disabled={active(operation)} onChange={event => setAllowNetwork(event.target.checked)} />允许此次初始化联网下载官方解释器和工具链</label>
        <div className="settings-runtime-row">
          <div><strong>Python</strong><span>{!facts ? '正在读取状态' : facts.python_explicitly_disabled ? '部署已禁用' : facts.python_configured && facts.python_enabled ? '已就绪' : facts.python_configured ? '已配置，尚未启用' : '尚未初始化'}{pythonVersionFixed ? ' · 部署管理' : ''}</span>{facts?.python_configured && !pythonVersionFixed && <span>当前版本：{detectedPythonVersion}</span>}</div>
          {pythonVersionFixed ? <label>当前版本<output className="settings-runtime-version" aria-label="Python 版本">{detectedPythonVersion}</output></label> : <label>安装版本<input aria-label="Python 版本" value={pythonVersion} disabled={active(operation)} onChange={event => setPythonVersion(event.target.value)} /></label>}
          <button type="button" disabled={!!busy || active(operation) || !pythonPrepareAvailable || !allowNetwork} onClick={() => void prepare('python')}>{facts?.python_configured ? '检查并初始化' : '初始化 Python'}</button>
        </div>
        {pythonVersionFixed && <p className="settings-field-help">当前 Python 由服务部署指定，此处仅检查，不更换版本。</p>}
        {facts?.python_version_error && <p className="settings-field-help" role="alert">未能读取 Python 版本。点击“刷新状态”可重新检测。</p>}
        {facts && !pythonPrepareAvailable && <p className="settings-field-help">{pythonPrepareHelp}</p>}
        <div className="settings-runtime-row"><div><strong>Rust</strong><span>{!facts ? '正在读取状态' : facts.rust_configured ? `已就绪${facts.rust_toolchain ? ` · ${facts.rust_toolchain}` : ''}` : '尚未初始化'}{facts?.rust_configuration_owner === 'deployment' ? ' · 部署管理' : ''}</span></div><label>工具链<input aria-label="Rust 工具链" value={rustVersion} disabled={active(operation) || facts?.rust_configuration_owner === 'deployment'} onChange={event => setRustVersion(event.target.value)} /></label><button type="button" disabled={!!busy || active(operation) || !rustPrepareAvailable || !allowNetwork} onClick={() => void prepare('rust')}>{facts?.rust_configured ? '检查并初始化' : '初始化 Rust'}</button></div>
        {facts && !facts.rust_bootstrap_available && facts.rust_configuration_owner !== 'deployment' && <p className="settings-field-help">此部署未提供 Rust 初始化工具。请由部署者安装完整平台发行包。</p>}
        {operation && <div className={`settings-operation ${operation.status === 'failed' || operation.status === 'interrupted' ? 'bad' : ''}`} role="status"><div><strong>{operation.kind === 'python' ? 'Python' : 'Rust'} · {phases[operation.phase] ?? operation.phase}</strong><time dateTime={operation.updated_at}>{new Date(operation.updated_at).toLocaleString()}</time></div>{active(operation) && <progress aria-label="环境初始化进度" />}{operation.message && <p>{runtimeMessage(operation.message)}</p>}{active(operation) && <button type="button" disabled={!!busy || operation.status === 'cancelling'} onClick={() => void cancel()}>停止初始化</button>}{['failed', 'interrupted', 'cancelled'].includes(operation.status) && <p className="settings-field-help">此前成功的配置保留。{retryAvailable ? '允许联网后，点击上方初始化按钮重试。' : operation.kind === 'python' ? pythonPrepareHelp : '当前部署不支持在此初始化 Rust，请联系部署者配置后重试。'}</p>}</div>}
        {runtimeNotice && <p className={`settings-notice ${runtimeNotice.kind}`} role="alert">{runtimeNotice.text}</p>}
      </section>
    </div>
  </main>;
}
