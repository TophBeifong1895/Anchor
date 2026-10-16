import { useRef, useState } from 'react';
import { Modal } from './ui';
import './WorkbenchDialogs.css';

export function CreateGraphDialog({ create, close }: { create: (name: string) => Promise<void>; close: () => void }) {
  const [name, setName] = useState('');
  const [pending, setPending] = useState(false);
  const [problem, setProblem] = useState('');
  const input = useRef<HTMLInputElement>(null);
  return <Modal title="新建工作流" close={() => { if (!pending) close(); }} initialFocus={input} className="workbench-dialog">
    <form onSubmit={event => {
      event.preventDefault(); if (pending) return;
      setPending(true); setProblem('');
      void create(name.trim()).then(close).catch(error => setProblem((error as Error).message)).finally(() => setPending(false));
    }}>
      <label>工作流名称<input ref={input} value={name} required maxLength={160} onChange={event => setName(event.target.value)} placeholder="例如 weekly-review" disabled={pending} /></label>
      <p className="hint">创建后可以添加节点、配置输入并运行。</p>
      {problem && <p className="notice bad" role="alert">{problem}</p>}
      <div className="modal-actions"><button type="button" disabled={pending} onClick={close}>取消</button><button className="primary" disabled={pending || !name.trim()}>{pending ? '正在创建…' : '创建工作流'}</button></div>
    </form>
  </Modal>;
}

function savedInput(graph: string, initial: Record<string, unknown>) {
  try { return localStorage.getItem(`anchor:run-input:${graph}`) ?? JSON.stringify(initial, null, 2); }
  catch { return JSON.stringify(initial, null, 2); }
}

export function RunInputDialog({ graph, initial, run, close }: {
  graph: string; initial: Record<string, unknown>; run: (input: Record<string, unknown>) => Promise<void>; close: () => void;
}) {
  const [text, setText] = useState(() => savedInput(graph, initial));
  const [pending, setPending] = useState(false);
  const [problem, setProblem] = useState('');
  const input = useRef<HTMLTextAreaElement>(null);
  const change = (value: string) => {
    setText(value); setProblem('');
    try { localStorage.setItem(`anchor:run-input:${graph}`, value); } catch { /* Keep the draft in memory when storage is unavailable. */ }
  };
  const parse = (): Record<string, unknown> => {
    let value: unknown;
    try { value = text.trim() ? JSON.parse(text) : {}; }
    catch { throw new Error('输入不是有效的 JSON，请检查引号、逗号和括号。'); }
    if (!value || Array.isArray(value) || typeof value !== 'object') throw new Error('运行输入需要一个 JSON 对象，例如 {"topic": "本周进展"}。');
    return value as Record<string, unknown>;
  };
  return <Modal title={`运行 ${graph}`} close={() => { if (!pending) close(); }} initialFocus={input} className="workbench-dialog">
    <form onSubmit={event => {
      event.preventDefault(); if (pending) return;
      let value: Record<string, unknown>;
      try { value = parse(); } catch (error) { setProblem((error as Error).message); return; }
      setPending(true); setProblem('');
      void run(value).then(close).catch(error => setProblem((error as Error).message)).finally(() => setPending(false));
    }}>
      <label>本次运行输入<textarea ref={input} className="json-editor" value={text} spellCheck={false} rows={10} disabled={pending} onChange={event => change(event.target.value)} /></label>
      <div className="input-helpers"><span className="hint">草稿保存在此浏览器，留空时使用空对象。</span><button type="button" disabled={pending} onClick={() => {
        try { change(JSON.stringify(parse(), null, 2)); } catch (error) { setProblem((error as Error).message); }
      }}>格式化 JSON</button><button type="button" disabled={pending} onClick={() => change(JSON.stringify(initial, null, 2))}>使用工作流默认值</button></div>
      {problem && <p className="notice bad" role="alert">{problem}</p>}
      <div className="modal-actions"><button type="button" disabled={pending} onClick={close}>取消</button><button className="primary" disabled={pending}>{pending ? '正在启动…' : '开始运行'}</button></div>
    </form>
  </Modal>;
}
