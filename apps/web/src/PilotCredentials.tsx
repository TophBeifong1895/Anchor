import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react';
import { api } from './api';
import { PluginConfiguration } from './PluginConfiguration';
import './PilotQuestion.css';

export type CredentialRequest = {
  id: string; session: string; turn: string; plugin: string; server: string; field: string;
  purpose?: string; status: 'pending' | 'configured' | 'cancelled' | 'interrupted';
};
const statusLabels = { pending: '等待配置凭据', configured: '本次配置请求已完成', cancelled: '配置已取消', interrupted: '配置请求已中断' };

type FormProps = { request: CredentialRequest; onConfigured: () => void };

export function PilotCredentialForm(props: FormProps) {
  return <CurrentPilotCredentialForm key={`${props.request.session}/${props.request.id}`} {...props} />;
}

function CurrentPilotCredentialForm({ request, onConfigured }: FormProps) {
  // Values only live in this input. They are not chat messages, question answers
  // or browser-storage data, and response metadata never contains the secret.
  const input = useRef<HTMLInputElement | null>(null);
  const inputRef = useCallback((element: HTMLInputElement | null) => {
    if (input.current && input.current !== element) input.current.value = '';
    input.current = element;
  }, []);
  const configured = useRef(onConfigured); configured.current = onConfigured;
  const submission = useRef<AbortController | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [problem, setProblem] = useState('');
  const [showCurrent, setShowCurrent] = useState(false);
  const active = request.status === 'pending';
  const privateScope = request.session.startsWith('pilot-private-') || request.session.startsWith('responses-');
  useEffect(() => () => { submission.current?.abort(); if (input.current) input.current.value = ''; }, [request.id]);
  useEffect(() => { if (!active) { submission.current?.abort(); if (input.current) input.current.value = ''; setSubmitting(false); } }, [active]);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!active || submission.current || !input.current?.value) return;
    const controller = new AbortController();
    submission.current = controller; setSubmitting(true); setProblem('');
    const value = input.current.value; input.current.value = '';
    try {
      const result = await api<{ request: CredentialRequest }>(
        `/sessions/${encodeURIComponent(request.session)}/credential-requests/${encodeURIComponent(request.id)}/value`,
        'POST', { value }, controller.signal,
      );
      if (result.request.id !== request.id || result.request.session !== request.session || result.request.status !== 'configured') {
        throw new Error('凭据配置结果尚未确认，请重新加载配置状态。');
      }
      if (!controller.signal.aborted) { if (input.current) input.current.value = ''; configured.current(); }
    } catch {
      // Do not echo a transport/server error that might contain submitted data.
      if (!controller.signal.aborted) setProblem('未能确认凭据已配置，请重试或重新加载状态。');
    } finally {
      if (submission.current === controller) submission.current = null;
      if (!controller.signal.aborted) setSubmitting(false);
    }
  };
  const cancel = async () => {
    if (!active || submission.current) return;
    const controller = new AbortController();
    submission.current = controller; setSubmitting(true); setProblem('');
    if (input.current) input.current.value = '';
    try {
      const result = await api<{ request: CredentialRequest }>(
        `/sessions/${encodeURIComponent(request.session)}/credential-requests/${encodeURIComponent(request.id)}/cancel`, 'POST', undefined, controller.signal,
      );
      if (result.request.id !== request.id || result.request.session !== request.session || result.request.status !== 'cancelled') throw new Error('unconfirmed');
      if (!controller.signal.aborted) { if (input.current) input.current.value = ''; configured.current(); }
    } catch { if (!controller.signal.aborted) setProblem('未能确认配置已取消，请重新加载状态。'); }
    finally { if (submission.current === controller) submission.current = null; if (!controller.signal.aborted) setSubmitting(false); }
  };
  const id = `credential-${request.id}`;
  return <section className="pilot-question" aria-label="配置插件凭据">
    <p>{request.plugin} · {request.server} · {request.field}</p>
    {!privateScope && <p className="pilot-muted">旧版共享会话：本表单管理共享作用域的凭据。插件库按当前访问账户管理凭据，凭据不会跨作用域自动共享。</p>}
    {request.purpose && <p>{request.purpose}</p>}
    {active ? <form onSubmit={event => void submit(event)}>
      <div className="pilot-question-field">
        <label htmlFor={id}>{request.field}</label>
        <input id={id} ref={inputRef} type="password" autoComplete="off" spellCheck={false} required disabled={submitting} aria-describedby={`${id}-description`} />
        <small id={`${id}-description`}>直接保存到 Anchor；Pilot 只会收到配置状态。</small>
      </div>
      {problem && <p className="pilot-question-error" role="alert">{problem}</p>}
      <div className="pilot-question-actions"><button type="button" disabled={submitting} onClick={() => void cancel()}>取消</button><button type="submit" className="primary" disabled={submitting}>{submitting ? '正在配置…' : '保存凭据并继续'}</button></div>
    </form> : <p className="pilot-muted" role="status">{statusLabels[request.status]}</p>}
    {privateScope && <button type="button" onClick={() => setShowCurrent(current => !current)} aria-expanded={showCurrent}>
      {showCurrent ? '收起当前凭据' : '查看当前凭据'}</button>
    }
    {privateScope && showCurrent && <PluginConfiguration pluginId={request.plugin} target={request.server} onChanged={onConfigured} />}
  </section>;
}

export function usePilotCredentials(session: string, onConfigured: () => void, polling = true) {
  const [state, setState] = useState<{ session: string; requests: CredentialRequest[]; problem: boolean }>({ session: '', requests: [], problem: false });
  const [reload, setReload] = useState(0);
  const configured = useRef(onConfigured);
  configured.current = onConfigured;
  useEffect(() => {
    if (!session) return;
    let closed = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const controller = new AbortController();
    const load = async () => {
      let repeat = polling;
      try {
        const result = await api<{ requests: CredentialRequest[] }>(
          `/sessions/${encodeURIComponent(session)}/credential-requests`, 'GET', undefined, controller.signal,
        );
        const requests = result.requests.filter(request => request.session === session);
        repeat ||= requests.some(request => request.status === 'pending');
        if (!closed) setState({ session, requests, problem: false });
      } catch {
        repeat = true;
        if (!closed) setState(previous => ({ session, requests: previous.session === session ? previous.requests : [], problem: true }));
      }
      if (!closed && repeat) timer = setTimeout(() => void load(), 3000);
    };
    void load();
    return () => { closed = true; controller.abort(); clearTimeout(timer); };
  }, [session, reload, polling]);
  return {
    requests: state.session === session ? state.requests : [],
    problem: state.session === session && state.problem,
    retry: () => setReload(value => value + 1),
    onConfigured: () => { setReload(value => value + 1); configured.current(); },
  };
}

export function PilotCredentials({ requests, onConfigured }: { requests: CredentialRequest[]; onConfigured: () => void }) {
  const pending = requests.filter(request => request.status === 'pending');
  const history = requests.filter(request => request.status !== 'pending');
  return <>
    {pending.map(request => <PilotCredentialForm key={request.id} request={request} onConfigured={onConfigured} />)}
    {history.length > 0 && <details className="pilot-credential-history">
      <summary>凭据配置记录 · {history.length} 项</summary>
      {history.map(request => <PilotCredentialForm key={request.id} request={request} onConfigured={onConfigured} />)}
    </details>}
  </>;
}
