import { useEffect, useRef, useState, type FormEvent } from 'react';
import { api } from './api';
import './PilotQuestion.css';

export type CredentialRequest = {
  id: string; session: string; turn: string; plugin: string; server: string; field: string;
  purpose?: string; status: 'pending' | 'configured' | 'cancelled' | 'interrupted';
};
const statusLabels = { pending: '等待配置凭据', configured: '凭据已配置', cancelled: '配置已取消', interrupted: '配置请求已中断' };

export function PilotCredentialForm({ request, onConfigured }: {
  request: CredentialRequest; onConfigured: () => void;
}) {
  // Values only live in this input. They are not chat messages, question answers
  // or browser-storage data, and response metadata never contains the secret.
  const input = useRef<HTMLInputElement>(null);
  const submission = useRef<AbortController | null>(null);
  const [submitting, setSubmitting] = useState(false);
  const [problem, setProblem] = useState('');
  const active = request.status === 'pending';
  useEffect(() => () => { submission.current?.abort(); if (input.current) input.current.value = ''; }, [request.id]);
  useEffect(() => { if (!active) { submission.current?.abort(); if (input.current) input.current.value = ''; setSubmitting(false); } }, [active]);
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (!active || submission.current || !input.current?.value) return;
    const controller = new AbortController();
    submission.current = controller; setSubmitting(true); setProblem('');
    try {
      const result = await api<{ request: CredentialRequest }>(
        `/sessions/${encodeURIComponent(request.session)}/credential-requests/${encodeURIComponent(request.id)}/value`,
        'POST', { value: input.current.value }, controller.signal,
      );
      if (result.request.id !== request.id || result.request.session !== request.session || result.request.status !== 'configured') {
        throw new Error('凭据配置结果尚未确认，请重新加载配置状态。');
      }
      if (!controller.signal.aborted) { if (input.current) input.current.value = ''; onConfigured(); }
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
    try {
      const result = await api<{ request: CredentialRequest }>(
        `/sessions/${encodeURIComponent(request.session)}/credential-requests/${encodeURIComponent(request.id)}/cancel`, 'POST', undefined, controller.signal,
      );
      if (result.request.id !== request.id || result.request.session !== request.session || result.request.status !== 'cancelled') throw new Error('unconfirmed');
      if (!controller.signal.aborted) { if (input.current) input.current.value = ''; onConfigured(); }
    } catch { if (!controller.signal.aborted) setProblem('未能确认配置已取消，请重新加载状态。'); }
    finally { if (submission.current === controller) submission.current = null; if (!controller.signal.aborted) setSubmitting(false); }
  };
  const id = `credential-${request.id}`;
  return <section className="pilot-question" aria-label="配置插件凭据">
    <p>{request.plugin} · {request.server} · {request.field}</p>
    {request.purpose && <p>{request.purpose}</p>}
    {active ? <form onSubmit={event => void submit(event)}>
      <div className="pilot-question-field">
        <label htmlFor={id}>{request.field}</label>
        <input id={id} ref={input} type="password" autoComplete="off" spellCheck={false} required disabled={submitting} aria-describedby={`${id}-description`} />
        <small id={`${id}-description`}>直接保存到 Anchor；Pilot 只会收到配置状态。</small>
      </div>
      {problem && <p className="pilot-question-error" role="alert">{problem}</p>}
      <div className="pilot-question-actions"><button type="button" disabled={submitting} onClick={() => void cancel()}>取消</button><button type="submit" className="primary" disabled={submitting}>{submitting ? '正在配置…' : '保存凭据并继续'}</button></div>
    </form> : <p className="pilot-muted" role="status">{statusLabels[request.status]}</p>}
  </section>;
}

export function PilotCredentials({ session, onConfigured }: { session: string; onConfigured: () => void }) {
  const [requests, setRequests] = useState<CredentialRequest[]>([]);
  const [problem, setProblem] = useState(false);
  const [reload, setReload] = useState(0);
  const configured = useRef(onConfigured);
  configured.current = onConfigured;
  useEffect(() => {
    let closed = false;
    let timer: ReturnType<typeof setTimeout>;
    const controller = new AbortController();
    const load = async () => {
      try {
        const result = await api<{ requests: CredentialRequest[] }>(
          `/sessions/${encodeURIComponent(session)}/credential-requests`, 'GET', undefined, controller.signal,
        );
        if (!closed) { setRequests(result.requests.filter(request => request.session === session)); setProblem(false); }
      } catch { if (!closed) setProblem(true); }
      if (!closed) timer = setTimeout(() => void load(), 3000);
    };
    void load();
    return () => { closed = true; controller.abort(); clearTimeout(timer); };
  }, [session, reload]);
  return <>
    {requests.map(request => <PilotCredentialForm key={request.id} request={request} onConfigured={() => { setReload(value => value + 1); configured.current(); }} />)}
    {problem && <p className="pilot-error" role="alert">无法读取凭据配置请求。<button type="button" onClick={() => setReload(value => value + 1)}>重新加载配置状态</button></p>}
  </>;
}
