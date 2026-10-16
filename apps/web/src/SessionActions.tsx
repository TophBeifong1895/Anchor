import { useEffect, useRef, useState } from 'react';
import { Pencil, Trash2 } from 'lucide-react';
import { Modal } from './ui';
import { ActionMenu } from './ActionMenu';

export function SessionActions({ title, busy, onRename, onDelete }: {
  title: string; busy: boolean; onRename: (title: string) => Promise<void>; onDelete: () => Promise<void>;
}) {
  const trigger = useRef<HTMLButtonElement>(null);
  const field = useRef<HTMLInputElement>(null);
  const cancel = useRef<HTMLButtonElement>(null);
  const [action, setAction] = useState<'rename' | 'delete' | null>(null);
  const [name, setName] = useState(title);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState('');
  useEffect(() => {
    if (action === 'rename') { field.current?.focus(); field.current?.select(); }
    else if (action === 'delete') cancel.current?.focus();
  }, [action]);
  const close = () => {
    if (pending) return;
    setAction(null);
    requestAnimationFrame(() => trigger.current?.focus());
  };
  const choose = (value: 'rename' | 'delete', dismiss: () => void) => {
    dismiss();
    setName(title); setError(''); setAction(value);
  };
  return <>
    <ActionMenu label={`管理对话 ${title}`} menuLabel={`${title} 的操作`} triggerRef={trigger}>{dismiss => <>
      <button role="menuitem" onClick={() => choose('rename', dismiss)}><Pencil size={15} />改名</button>
      <button role="menuitem" className="danger" disabled={busy} onClick={() => choose('delete', dismiss)}><Trash2 size={15} />删除</button>
      {busy && <small>请先停止此对话的回复</small>}
    </>}</ActionMenu>
    {action && <Modal title={action === 'rename' ? '修改对话名称' : '删除对话？'} close={close} className="session-action-dialog">
      <form onSubmit={async event => {
        event.preventDefault();
        if (pending || (action === 'delete' && busy)) return;
        setPending(true); setError('');
        try {
          if (action === 'rename') await onRename(name.trim());
          else await onDelete();
          setAction(null);
          requestAnimationFrame(() => trigger.current?.focus());
        } catch (cause) { setError((cause as Error).message); }
        finally { setPending(false); }
      }}>
        {action === 'rename' ? <label>对话名称<input ref={field} required maxLength={120} value={name}
          disabled={pending} onChange={event => setName(event.target.value)} /></label> : <>
          <p>删除“{title}”及其对话历史？此操作无法撤销。</p>
          <p className="pilot-muted">关联的工作流、运行记录和产物会保留。</p>
        </>}
        {error && <p role="alert" className="pilot-error">{error}</p>}
        <div className="modal-actions"><button ref={cancel} type="button" className="cancel-delete" disabled={pending} onClick={close}>取消</button>
          <button type="submit" className={action === 'delete' ? 'confirm-delete' : 'primary'}
            disabled={pending || (action === 'rename' ? !name.trim() : busy)}>{pending ? (action === 'rename' ? '正在保存…' : '正在删除…') : action === 'rename' ? '保存' : '删除对话'}</button></div>
      </form>
    </Modal>}
  </>;
}
