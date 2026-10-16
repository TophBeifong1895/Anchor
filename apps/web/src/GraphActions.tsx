import { useEffect, useRef, useState } from 'react';
import { GitBranch, Trash2 } from 'lucide-react';
import { Modal } from './ui';
import { ActionMenu } from './ActionMenu';
import { api, reason } from './api';

/** Actions belong to this row, independently of the graph open in the editor. */
export function GraphActions({ graph, running, runCount, busy, onDelete }:
  { graph: string; running: boolean; runCount: number; busy: boolean;
    onDelete: (graph: string, precondition: string) => Promise<void> }) {
  const trigger = useRef<HTMLButtonElement>(null);
  const cancel = useRef<HTMLButtonElement>(null);
  const [confirming, setConfirming] = useState(false);
  const [precondition, setPrecondition] = useState('');
  const [preparing, setPreparing] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState('');
  useEffect(() => {
    if (!confirming) return;
    const controller = new AbortController();
    setPrecondition(''); setPreparing(true); setError('');
    void api<{ graph: string; precondition: string }>(`/graphs/${encodeURIComponent(graph)}/delete-precondition`, 'GET', undefined, controller.signal)
      .then(value => {
        if (controller.signal.aborted) return;
        if (value.graph !== graph || !value.precondition) throw new Error('删除范围未返回有效版本，请关闭窗口后重试。');
        setPrecondition(value.precondition);
      }).catch(cause => { if (!controller.signal.aborted) setError(`无法核对删除范围：${reason(cause)}`); })
      .finally(() => { if (!controller.signal.aborted) setPreparing(false); });
    return () => controller.abort();
  }, [confirming, graph]);
  const close = () => {
    if (deleting) return;
    setConfirming(false); setPrecondition('');
    requestAnimationFrame(() => trigger.current?.focus());
  };

  return <>
    <ActionMenu label={`管理工作流 ${graph}`} menuLabel={`${graph} 的操作`} triggerRef={trigger}>{dismiss => <>
      <button role="menuitem" className="danger" aria-disabled={busy || running} onClick={() => {
        if (busy || running) return;
        dismiss();
        setError(''); setPrecondition(''); setPreparing(true); setConfirming(true);
      }}><Trash2 size={15} />删除工作流</button>
      {running && <p>请先停止运行，再删除此工作流。</p>}
    </>}</ActionMenu>
    {confirming && <Modal title="删除工作流？" close={close} className="delete-graph-dialog" initialFocus={cancel}>
      <div className="delete-graph-target"><GitBranch size={18} /><strong>{graph}</strong></div>
      <p className="delete-graph-description">以下内容将被永久删除，无法恢复：</p>
      <ul className="delete-graph-scope">
        <li>工作流定义</li>
        <li>全部运行记录（当前 {runCount} 条）</li>
        <li>所有运行的工作区文件、产物和 Git 历史</li>
      </ul>
      {running && <p className="problem" role="alert">此工作流正在运行，请先停止运行。</p>}
      {preparing && <p className="hint" role="status">正在核对删除范围…</p>}
      {error && <p className="problem" role="alert">{error}</p>}
      <div className="modal-actions">
        <button ref={cancel} className="cancel-delete" disabled={deleting} onClick={close}>取消</button>
        <button className="confirm-delete" disabled={busy || running || deleting || preparing || !precondition} onClick={async () => {
          if (!precondition) return;
          setDeleting(true); setError('');
          try { await onDelete(graph, precondition); }
          catch (cause) { setError(`删除失败：${(cause as Error).message}`); }
          finally { setDeleting(false); }
        }}>{deleting ? '正在删除…' : '永久删除'}</button>
      </div>
    </Modal>}
  </>;
}
