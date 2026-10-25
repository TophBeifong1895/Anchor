import { Fragment, useCallback, useEffect, useRef, useState, type FormEvent } from 'react';
import { Anchor, ArrowDown, Check, Copy, LoaderCircle, MessageSquare, Paperclip, Plus, Search, Send, Square } from 'lucide-react';
import { api, ApiError } from './api';
import { createPilotSession } from './pilotSessionCreation';
import { Markdown } from './markdown';
import { PilotTurn, type Turn } from './PilotTurn';
import { SessionActions } from './SessionActions';
import { SessionPlugins, type PluginContinuation } from './SessionPlugins';
import { PilotCredentials, usePilotCredentials } from './PilotCredentials';
import type { Question } from './pilotQuestions';
import { PilotAttachments } from './PilotAttachments';
import { addDraftAttachments, attachmentProblem, encodeAttachment, type Attachment, type DraftAttachment } from './pilotAttachments';
import { clearAttachmentDraft, saveAttachmentDraft } from './pilotAttachmentDrafts';
import { usePilotDrafts } from './usePilotDrafts';
import { PilotConversationLayout } from './PilotConversationLayout';
import './Pilot.css';

type Session = {
  id: string; title?: string; status: string; waiting_reason: string; updated_at: string;
  approval: { action: string; key: string; status: string; target?: string; proposal?: unknown } | null;
};
type Message = { role: 'user' | 'assistant'; text: string; turn?: string; attachments?: Attachment[]; localAttachments?: DraftAttachment[]; question?: Question; question_answer?: Question };
const messageTurn = (message: Message) => message.turn ?? message.question?.turn ?? message.question_answer?.turn;
const labels: Record<string, string> = { active: '可以继续对话', interrupted: '回复已中断', waiting_user: '等待你的答复', archived: '已归档' };
const starters = [
  { label: '工作流', text: '帮我把每周整理资料和生成周报做成工作流。' },
  { label: '安装插件', text: '我有一个插件仓库，帮我安装并检查能否使用。' },
  { label: '开发工具', text: '帮我开发一个把 CSV 转成图表的插件，并测试它。' },
];
const title = (session: Session) => session.title || '新对话';
const date = (value: string) => new Date(value).toLocaleDateString('zh-CN', { month: 'short', day: 'numeric' });
// Browser storage holds navigation and unsent drafts only; saved messages come from the server.
function cached(key: string, value?: string) {
  try {
    if (value !== undefined) localStorage.setItem(`pilot:${key}`, value);
    return localStorage.getItem(`pilot:${key}`) ?? '';
  } catch { return ''; }
}
type Submission = { request_id: string; message?: string; resume?: boolean; attachment_ids?: string[] };
const draftScope = (id: string) => id ? `session:${id}` : 'new';
const sameFiles = (ids: string[] = [], files: DraftAttachment[]) => ids.length === files.length && files.every((file, index) => file.id === ids[index]);
function pendingSubmission(id: string): Submission | null {
  try {
    const value = JSON.parse(cached(`submission:${id}`) || 'null');
    if (value && typeof value.request_id === 'string' && (typeof value.message === 'string' || value.resume === true)
      && (value.attachment_ids === undefined || (Array.isArray(value.attachment_ids) && value.attachment_ids.every((id: unknown) => typeof id === 'string')))) {
      return { request_id: value.request_id, ...(value.resume === true ? { resume: true } : { message: value.message }),
        ...(value.attachment_ids ? { attachment_ids: value.attachment_ids } : {}) };
    }
  } catch { /* Invalid browser cache must not prevent loading the server's history. */ }
  return null;
}

export function Pilot({ session = '', onSession, initialDraft = '', onDraftConsumed, onSettings }: { session?: string; onSession?: (id: string) => void; initialDraft?: string; onDraftConsumed?: () => void; onSettings: () => void }) {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [selected, setSelected] = useState('');
  const selectedRef = useRef('');
  const [messages, setMessages] = useState<Message[]>([]);
  const [turn, setTurn] = useState<Turn | null>(null);
  const turnRef = useRef<Turn | null>(null);
  const [waitingQuestion, setWaitingQuestion] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [syncedTurn, setSyncedTurn] = useState('');
  const [continuation, setContinuation] = useState<PluginContinuation | null>(null);
  const [dragging, setDragging] = useState(false);
  const fileInput = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const [problem, setProblem] = useState('');
  const [modelRequired, setModelRequired] = useState(false);
  const [query, setQuery] = useState('');
  const [copied, setCopied] = useState<number | null>(null);
  const [atBottom, setAtBottom] = useState(true);
  const follow = useRef(true);
  // Read through a ref so `activate` stays a plain function and the effect below can depend on the id.
  const onSessionRef = useRef(onSession);
  onSessionRef.current = onSession;
  const scroller = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLTextAreaElement>(null);
  const historyRequest = useRef<AbortController | null>(null);
  const submissions = useRef(new Set<string>());
  const navigation = useRef(0);
  const mounted = useRef(true);
  const listVersion = useRef(0);
  const { draft, setDraft, files, filesRef, filesReady, setFilesReady, attachmentError, setAttachmentError,
    setCurrentFiles, storeFiles, editDraft, activate: activateDraft, clearSent } = usePilotDrafts(selected, selectedRef, mounted);
  const setCurrentTurn = useCallback((value: Turn | null) => { turnRef.current = value; setTurn(value); }, []);
  const questionState = useCallback((id: string, turnId: string, pending: boolean) => {
    if (!mounted.current || selectedRef.current !== id || turnRef.current?.id !== turnId || turnRef.current.status !== 'running') return;
    setWaitingQuestion(pending);
    setSessions(previous => previous.map(item => item.id === id && item.status !== 'archived'
      ? { ...item, status: pending ? 'waiting_user' : 'active' } : item));
  }, []);

  const activate = (id: string) => {
    navigation.current += 1;
    activateDraft(id); setDragging(false);
    historyRequest.current?.abort();
    selectedRef.current = id;
    setSelected(id); cached('selected', id); onSessionRef.current?.(id);
    setMessages([]); setCurrentTurn(null); setWaitingQuestion(false); setStopping(false); setSyncedTurn(''); setContinuation(null); setCopied(null); setProblem(''); setModelRequired(false);
    setBusy(submissions.current.has(id));
    follow.current = true; setAtBottom(true);
  };
  const refresh = useCallback(async (id: string) => {
    const version = listVersion.current;
    historyRequest.current?.abort();
    const controller = new AbortController();
    historyRequest.current = controller;
    try {
      const [history, result, execution] = await Promise.all([
        api<{ messages: Message[] }>(`/sessions/${encodeURIComponent(id)}/messages`, 'GET', undefined, controller.signal),
        api<{ sessions: Session[] }>('/sessions', 'GET', undefined, controller.signal),
        api<{ turns: Turn[] }>(`/sessions/${encodeURIComponent(id)}/turns`, 'GET', undefined, controller.signal),
      ]);
      if (!mounted.current || controller.signal.aborted || selectedRef.current !== id) return;
      let latest = execution.turns[0];
      let savedHistory = history.messages;
      // A completed source can still have a scheduled continuation when the
      // page reopens. Turn list rows do not carry that projection.
      if (latest?.status === 'completed') {
        const detail = await api<{ turn: Turn; continuation: Turn['continuation'] }>(
          `/sessions/${encodeURIComponent(id)}/turns/${encodeURIComponent(latest.id)}`, 'GET', undefined, controller.signal);
        if (!mounted.current || controller.signal.aborted || selectedRef.current !== id) return;
        latest = { ...detail.turn, continuation: detail.continuation };
        if (detail.continuation?.status === 'admitted' && detail.continuation.turn) {
          // Admission may cross the list/detail reads on a reopened page. Read
          // the saved successor and current history before showing an idle form.
          const [successor, currentExecution, currentHistory] = await Promise.all([
            api<{ turn: Turn; continuation: Turn['continuation'] }>(
              `/sessions/${encodeURIComponent(id)}/turns/${encodeURIComponent(detail.continuation.turn)}`, 'GET', undefined, controller.signal),
            api<{ turns: Turn[] }>(`/sessions/${encodeURIComponent(id)}/turns`, 'GET', undefined, controller.signal),
            api<{ messages: Message[] }>(`/sessions/${encodeURIComponent(id)}/messages`, 'GET', undefined, controller.signal),
          ]);
          if (!mounted.current || controller.signal.aborted || selectedRef.current !== id) return;
          latest = currentExecution.turns[0] ?? successor.turn;
          if (latest.id === successor.turn.id) latest = { ...successor.turn, continuation: successor.continuation };
          savedHistory = currentHistory.messages;
        }
      }
      // The running Turn owns live output and question forms; history joins after it ends.
      const savedMessages = savedHistory.filter(message => latest?.status !== 'running'
        || (message.question?.turn !== latest.id && message.question_answer?.turn !== latest.id
          && !(message.role === 'assistant' && message.turn === latest.id)));
      const pending = pendingSubmission(id);
      if (pending && execution.turns.some(item => item.request_id === pending.request_id)) {
        if (pending.attachment_ids?.length) {
          await clearSent(id, pending.attachment_ids);
          if (!mounted.current || controller.signal.aborted || selectedRef.current !== id) return;
        }
        cached(`submission:${id}`, '');
        if (pending.message !== undefined && cached(`draft:${id}`) === pending.message) {
          cached(`draft:${id}`, ''); setDraft('');
        }
      }
      // The turn is accepted before its prompt is saved. Keep that accepted message visible
      // when reopening a session during this window, without duplicating an already saved prompt.
      const last = savedMessages.at(-1);
      const running = latest?.status === 'running';
      const prompt = running ? latest.prompt ?? '' : pendingSubmission(id)?.message;
      const attachments = running ? latest.attachments : undefined;
      const hasSameAttachments = (message: Message) => !!attachments?.length && attachments.every(attachment =>
        message.attachments?.some(saved => saved.id === attachment.id && saved.url === attachment.url));
      // Resumed Turns can reference the original input. Those references belong to the saved
      // upload message, while a newly admitted attachment-only Turn has its own original URLs.
      const alreadySaved = running && savedMessages.some(message => message.role === 'user'
        && (message.turn === latest.id || (!latest.prompt && hasSameAttachments(message))));
      const sameLastInput = last?.role === 'user' && last.text === prompt
        && (attachments?.length ? hasSameAttachments(last) : !last.attachments?.length);
      setMessages(prompt !== undefined && (prompt || attachments?.length) && !alreadySaved && !sameLastInput
        ? [...savedMessages, { role: 'user', text: prompt, ...(running ? { turn: latest.id, attachments } : {}) }] : savedMessages);
      if (version === listVersion.current) setSessions(result.sessions);
      setCurrentTurn(latest ?? null);
      // Keep the watched continuation identical to its previous value when nothing changed: the
      // follow-up effect keys on it, and a new object every poll would restart the wait instead of
      // letting it run on its interval.
      const settled = (previous: PluginContinuation, state: PluginContinuation['state']) =>
        previous.state.status === state.status && previous.state.request_id === state.request_id
          && previous.state.turn === state.turn ? previous : { ...previous, state };
      setContinuation(previous => {
        if (latest?.continuation?.status === 'scheduled') {
          const state = latest.continuation;
          return previous && previous.session === id && previous.source === latest.id
            ? settled(previous, state) : { session: id, source: latest.id, state };
        }
        if (!previous || previous.session !== id) return previous;
        if (latest?.id === previous.source) return latest.continuation
          ? settled(previous, latest.continuation) : previous;
        return settled(previous, { status: latest?.request_id === `continuation:${previous.source}` ? 'admitted' : 'superseded',
          ...(latest?.request_id === `continuation:${previous.source}` ? { turn: latest.id } : {}) });
      });
      if (latest?.status !== 'running') { setWaitingQuestion(false); setStopping(false); }
      setSyncedTurn(latest && latest.status !== 'running' ? latest.id : '');
      setBusy(submissions.current.has(id) || latest?.status === 'running');
    } catch (error) {
      if (!controller.signal.aborted) throw error;
    }
  }, [setCurrentTurn]);
  const complete = useCallback((final: Turn) => {
    if (!mounted.current || selectedRef.current !== final.session || turnRef.current?.id !== final.id) return;
    setCurrentTurn(final); setBusy(false); setWaitingQuestion(false); setStopping(false);
    if (final.continuation) setContinuation(previous => final.continuation?.status === 'scheduled'
      ? { session: final.session, source: final.id, state: final.continuation }
      : previous?.source === final.id ? { ...previous, state: final.continuation! } : previous);
    void refresh(final.session).catch(error => {
      if (mounted.current && selectedRef.current === final.session) setProblem((error as Error).message);
    });
  }, [refresh, setCurrentTurn]);
  useEffect(() => {
    if (!continuation || continuation.session !== selected || continuation.state.status !== 'scheduled') return;
    const controller = new AbortController();
    let closed = false;
    let timer: ReturnType<typeof setTimeout>;
    const followContinuation = async () => {
      try {
        const result = await api<{ turn: Turn; continuation: Turn['continuation'] }>(`/sessions/${encodeURIComponent(continuation.session)}/turns/${encodeURIComponent(continuation.source)}`, 'GET', undefined, controller.signal);
        if (closed) return;
        const latest = turnRef.current;
        if (latest && latest.id !== continuation.source && latest.request_id !== `continuation:${continuation.source}`) {
          setContinuation(previous => previous?.source === continuation.source
            ? { ...previous, state: { status: 'superseded' } } : previous);
          return;
        }
        if (submissions.current.has(continuation.session)) {
          timer = setTimeout(() => void followContinuation(), 1000);
          return;
        }
        if (result.continuation?.status === 'admitted') {
          await refresh(continuation.session);
          if (!closed) setContinuation(previous => previous?.source === continuation.source
            ? { ...previous, state: result.continuation! } : previous);
          return;
        }
        if (result.continuation?.status !== 'scheduled') {
          setContinuation(previous => previous?.source === continuation.source
            ? { ...previous, state: result.continuation ?? { status: 'none' } } : previous);
          return;
        }
      } catch { if (closed) return; }
      if (!closed) timer = setTimeout(() => void followContinuation(), 1000);
    };
    void followContinuation();
    return () => { closed = true; controller.abort(); clearTimeout(timer); };
  }, [continuation, selected, refresh]);
  const scheduleContinuation = useCallback((id: string, source: string) => {
    if (!mounted.current || selectedRef.current !== id) return;
    const latest = turnRef.current;
    const state: PluginContinuation['state'] = latest && latest.id !== source
      ? { status: latest.request_id === `continuation:${source}` ? 'admitted' : 'superseded',
        ...(latest.request_id === `continuation:${source}` ? { turn: latest.id } : {}) }
      : { status: 'scheduled', request_id: `continuation:${source}` };
    setContinuation({ session: id, source, state });
  }, []);
  const submit = async (id: string, message: string | null, attachments: DraftAttachment[] = []) => {
    const key = `submission:${id}`;
    const pending = pendingSubmission(id);
    if (pending && ((pending.message ?? null) !== message || !sameFiles(pending.attachment_ids, attachments))) throw new Error('上次提交的结果尚未确认，请先使用原文字和附件重试。');
    if (message === null && attachments.length) throw new Error('继续回复时不能附加文件。');
    const submission = pending ?? { request_id: crypto.randomUUID(), ...(message === null ? { resume: true } : { message }),
      ...(attachments.length ? { attachment_ids: attachments.map(file => file.id) } : {}) };
    if (attachments.length) await saveAttachmentDraft(draftScope(id), attachments);
    const encoded = attachments.length ? await Promise.all(attachments.map(encodeAttachment)) : [];
    cached(key, JSON.stringify(submission));
    const body = { request_id: submission.request_id, ...(message === null ? { resume: true } : message ? { message } : {}),
      ...(encoded.length ? { attachments: encoded } : {}) };
    let result: { turn: Turn };
    if (mounted.current && selectedRef.current === id) setModelRequired(false);
    try { result = await api<{ turn: Turn }>(`/sessions/${encodeURIComponent(id)}/turns`, 'POST', body); }
    catch (error) {
      // Explicit rejection means no turn was accepted. Only ambiguous delivery keeps its retry ID.
      if (error instanceof ApiError && [400, 401, 403, 404, 409, 413, 422].includes(error.status)) cached(key, '');
      if (error instanceof ApiError && error.status === 503) {
        // Ask the Host which default can execute; a provider failure alone does not mean missing settings.
        const status = await api<{ execution: { configured: boolean } }>('/settings/model').catch(() => null);
        if (status?.execution?.configured === false) {
          if (mounted.current && selectedRef.current === id) setModelRequired(true);
          throw new Error('请先在「设置 → 模型服务」配置默认服务的地址、Flash 模型和 API key，保存后再与 Pilot 对话。');
        }
      }
      throw error;
    }
    if (attachments.length) {
      await clearSent(id, attachments.map(file => file.id));
    }
    cached(key, '');
    if (message !== null && cached(`draft:${id}`).trim() === message) {
      cached(`draft:${id}`, '');
      if (mounted.current && selectedRef.current === id) setDraft('');
    }
    if (mounted.current && selectedRef.current === id) {
      historyRequest.current?.abort();
      setLoading(false); setCurrentTurn(result.turn); setWaitingQuestion(false); setStopping(false); setSyncedTurn('');
      setBusy(result.turn.status === 'running');
      setContinuation(previous => previous?.session === id && previous.source !== result.turn.id
        ? { ...previous, state: { status: result.turn.request_id === `continuation:${previous.source}` ? 'admitted' : 'superseded' } }
        : previous);
    }
  };
  useEffect(() => {
    let live = true;
    mounted.current = true;
    if (initialDraft) {
      if (selectedRef.current) activate('');
      const previous = cached('draft:new');
      editDraft(previous.includes(initialDraft) ? previous : previous ? `${previous}\n\n${initialDraft}` : initialDraft);
      onDraftConsumed?.();
      requestAnimationFrame(() => input.current?.focus());
    }
    const initialNavigation = navigation.current;
    let expectedNavigation = initialNavigation;
    const controller = new AbortController();
    void (async () => {
      try {
        const result = await api<{ sessions: Session[] }>('/sessions', 'GET', undefined, controller.signal);
        if (!live || navigation.current !== initialNavigation) return;
        setSessions(result.sessions);
        const saved = initialDraft ? '' : session || cached('selected');
        if (result.sessions.some(item => item.id === saved)) {
          if (selectedRef.current !== saved) activate(saved);
          expectedNavigation = navigation.current;
          await refresh(saved);
        } else if (selectedRef.current) {
          activate('');
          expectedNavigation = navigation.current;
        }
      } catch (error) { if (live && navigation.current === expectedNavigation) setProblem((error as Error).message); }
      finally { if (live && navigation.current === expectedNavigation) setLoading(false); }
    })();
    return () => { live = false; mounted.current = false; controller.abort(); historyRequest.current?.abort(); };
  }, []);
  const current = sessions.find(item => item.id === selected);
  const awaitingContinuation = continuation?.session === selected && continuation.state.status === 'scheduled';
  const credentials = usePilotCredentials(selected, () => {
    void refresh(selected).catch(error => { if (selectedRef.current === selected) setProblem((error as Error).message); });
  }, turn?.status === 'running' || busy || awaitingContinuation);
  const lastMessageByTurn = new Map(messages.flatMap((message, index) => {
    const id = messageTurn(message);
    return id ? [[id, index] as const] : [];
  }));
  const credentialRequests = (id: string) => credentials.requests.filter(request => request.turn === id);
  const remainingCredentials = credentials.requests.filter(request => !lastMessageByTurn.has(request.turn)
    || (turn?.status === 'running' && request.turn === turn.id));
  const pendingApproval = current?.approval?.status === 'requested';
  // An interrupted session accepts a new message: the next turn carries what the dead run recorded.
  const canSend = !selected || (current &&
    ['active', 'interrupted'].includes(current.status) && !pendingApproval && !waitingQuestion);
  // Empty history is not an idle session when a Turn or a saved control request still needs attention.
  const showStart = !messages.length && !turn && !loading && !busy && !stopping && !waitingQuestion
    && !pendingApproval && !awaitingContinuation && (!selected || current?.status === 'active');
  useEffect(() => {
    if (!input.current) return;
    input.current.style.height = 'auto';
    input.current.style.height = `${Math.min(input.current.scrollHeight, 200)}px`;
  }, [draft, selected]);
  useEffect(() => {
    if (!showStart && follow.current && scroller.current) scroller.current.scrollTop = scroller.current.scrollHeight;
  }, [messages, busy, loading, turn, waitingQuestion, showStart]);
  useEffect(() => {
    const element = scroller.current;
    if (!element) return;
    const observer = new ResizeObserver(() => {
      if (follow.current) element.scrollTop = element.scrollHeight;
    });
    observer.observe(element);
    const content = element.querySelector('.pilot-message-list');
    if (content) observer.observe(content);
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    if (!busy && !loading) input.current?.focus({ preventScroll: true });
  }, [busy, loading, selected]);

  const select = async (id: string) => {
    activate(id); setLoading(true);
    const version = navigation.current;
    try { await refresh(id); }
    catch (error) { if (mounted.current && version === navigation.current) setProblem((error as Error).message); }
    finally { if (mounted.current && version === navigation.current) setLoading(false); }
  };
  const create = async () => {
    let version = navigation.current;
    setLoading(true); setProblem('');
    try {
      const { session } = await createPilotSession<Session>();
      if (!mounted.current || version !== navigation.current) return;
      activate(session.id); version = navigation.current; await refresh(session.id);
    } catch (error) { if (mounted.current && version === navigation.current) setProblem((error as Error).message); }
    finally { if (mounted.current && version === navigation.current) setLoading(false); }
  };
  const addFiles = (incoming: File[]) => {
    if (busy || loading || stopping || !canSend || !filesReady) return;
    const next = addDraftAttachments(filesRef.current, incoming);
    const error = attachmentProblem(next);
    if (error) { setAttachmentError(error); return; }
    void storeFiles(selectedRef.current, next);
  };
  const send = async (event: FormEvent) => {
    event.preventDefault();
    if ((!draft.trim() && !files.length) || busy || loading || stopping || !canSend || !filesReady) return;
    const prompt = draft.trim();
    const attachments = [...filesRef.current];
    let id = selected;
    const sourceId = id;
    const pending = pendingSubmission(id);
    if (pending && ((pending.message ?? null) !== prompt || !sameFiles(pending.attachment_ids, attachments))) {
      setProblem('上次提交的结果尚未确认，请先使用原文字和附件重试。'); return;
    }
    const error = attachmentProblem(attachments);
    if (error) { setAttachmentError(error); return; }
    historyRequest.current?.abort();
    const version = navigation.current;
    setBusy(true); setProblem(''); setAttachmentError(''); follow.current = true;
    setCurrentTurn(null); setWaitingQuestion(false); setSyncedTurn('');
    const optimistic: Message = { role: 'user', text: prompt, localAttachments: attachments };
    setMessages(previous => pending?.message === prompt && previous.at(-1)?.role === 'user'
      && previous.at(-1)?.text === prompt ? previous : [...previous, optimistic]);
    submissions.current.add(id);
    try {
      if (attachments.length) await saveAttachmentDraft(draftScope(id), attachments);
      if (!id) {
        const { session } = await createPilotSession<Session>();
        submissions.current.delete('');
        id = session.id; submissions.current.add(id);
        cached(`draft:${id}`, prompt);
        if (mounted.current) setSessions(previous => [session, ...previous]);
        if (mounted.current && version === navigation.current) {
          activate(id);
          setMessages([optimistic]);
        }
        if (attachments.length) await saveAttachmentDraft(draftScope(id), attachments, draftScope(sourceId));
        if (cached('draft:new').trim() === prompt) cached('draft:new', '');
      }
      if (mounted.current) setSessions(previous => previous.map(item => item.id === id && !item.title
        ? { ...item, title: (prompt || attachments[0]?.name || '新对话').slice(0, 60) } : item));
      await submit(id, prompt, attachments);
      if (sourceId !== id) {
        if (cached(`draft:${sourceId || 'new'}`).trim() === prompt) cached(`draft:${sourceId || 'new'}`, '');
        await clearAttachmentDraft(draftScope(sourceId), attachments.map(file => file.id)).catch(() => undefined);
      }
    } catch (error) {
      cached(`draft:${id || 'new'}`, prompt);
      if (mounted.current && selectedRef.current === id) {
        setProblem((error as Error).message); setDraft(prompt); setCurrentFiles(attachments); setFilesReady(true); setBusy(false);
      }
    } finally { submissions.current.delete(id); }
  };
  const resume = async () => {
    if (!selected || busy) return;
    historyRequest.current?.abort();
    setBusy(true); setProblem(''); follow.current = true;
    const id = selected;
    submissions.current.add(id);
    try {
      await submit(id, null);
    } catch (error) {
      if (mounted.current && selectedRef.current === id) { setProblem((error as Error).message); setBusy(false); }
    } finally { submissions.current.delete(id); }
  };
  const stop = async () => {
    if (!selected || stopping) return;
    const id = selected;
    const version = navigation.current;
    setStopping(true);
    try {
      await api(`/sessions/${encodeURIComponent(id)}/stop`, 'POST');
      if (!mounted.current || version !== navigation.current) return;
      if (continuation?.session === id && continuation.state.status === 'scheduled') {
        const result = await api<{ continuation: Turn['continuation'] }>(
          `/sessions/${encodeURIComponent(id)}/turns/${encodeURIComponent(continuation.source)}`);
        if (!mounted.current || version !== navigation.current) return;
        setContinuation(previous => previous?.source === continuation.source
          ? { ...previous, state: result.continuation ?? { status: 'none' } } : previous);
      }
      if (turnRef.current?.status !== 'running') {
        await refresh(id);
        if (mounted.current && version === navigation.current) setStopping(false);
      }
    }
    catch (error) {
      if (mounted.current && version === navigation.current) { setProblem((error as Error).message); setStopping(false); }
    }
  };
  const decide = async (accept: boolean) => {
    const approval = current?.approval;
    if (!selected || !approval || busy) return;
    historyRequest.current?.abort();
    setBusy(true); setProblem('');
    const id = selected;
    submissions.current.add(id);
    try {
      await api(`/sessions/${encodeURIComponent(selected)}/${accept ? 'confirm' : 'reject'}`, 'POST', {
        action: approval.action, approval_key: approval.key,
      });
      // A decision does not run anything by itself: it resumes the paused run with the original call.
      await submit(id, null);
    } catch (error) {
      if (mounted.current && selectedRef.current === id) { setProblem((error as Error).message); setBusy(false); }
    } finally { submissions.current.delete(id); }
  };
  const copy = async (message: Message, index: number) => {
    try { await navigator.clipboard.writeText(message.text); setCopied(index); }
    catch { setProblem('无法访问剪贴板，请选中文字复制。'); }
  };
  const renameSession = async (id: string, title: string) => {
    const result = await api<{ session: Session }>(`/sessions/${encodeURIComponent(id)}`, 'PUT', { title });
    listVersion.current += 1;
    if (mounted.current) setSessions(items => items.map(item => item.id === id ? { ...item, title: result.session.title } : item));
  };
  const deleteSession = async (id: string) => {
    await api(`/sessions/${encodeURIComponent(id)}`, 'DELETE');
    listVersion.current += 1;
    cached(`draft:${id}`, ''); cached(`submission:${id}`, '');
    await clearAttachmentDraft(draftScope(id)).catch(() => undefined);
    if (!mounted.current) return;
    setSessions(items => items.filter(item => item.id !== id));
    if (selectedRef.current === id) { activate(''); setLoading(false); }
  };
  const heading = current?.title || messages.find(item => item.role === 'user')?.text.slice(0, 60) || '新对话';
  const filtered = sessions.filter(item => `${title(item)} ${item.id}`.toLowerCase().includes(query.toLowerCase()));
  return <main className="pilot-page">
    <aside className="pilot-sessions" aria-label="对话历史">
      <div className="pilot-session-heading"><h2><Anchor size={18} />Pilot</h2>
        <button onClick={() => void create()} disabled={loading} title="新建对话"><Plus size={16} />新对话</button></div>
      <label className="pilot-search"><Search size={15} /><input aria-label="搜索对话" placeholder="搜索对话…" value={query}
        onChange={event => setQuery(event.target.value)} /></label>
      <p className="pilot-sidebar-label">对话历史 <span>{sessions.length}</span></p>
      {!filtered.length && <p className="pilot-muted">{query ? '没有找到匹配的对话' : '你的对话会保存在这里'}</p>}
      {filtered.map(session => <div key={session.id} className={`pilot-session-row ${session.id === selected ? 'chosen' : ''}`}><button aria-current={session.id === selected ? 'page' : undefined}
        title={session.id} className={`pilot-session ${session.id === selected ? 'chosen' : ''}`}
        onClick={() => void select(session.id)}>
        <MessageSquare size={16} /><span><strong>{title(session)}</strong>
          <small>{date(session.updated_at)} · {labels[session.status] ?? session.status}</small></span>
      </button><SessionActions title={title(session)} busy={(session.id === selected && busy) || submissions.current.has(session.id)}
        onRename={name => renameSession(session.id, name)} onDelete={() => deleteSession(session.id)} /></div>)}
    </aside>
    <section className="pilot-chat" aria-label="Pilot 对话">
      <header className="pilot-chat-heading">
        <div><strong title={selected || undefined}>{heading}</strong><span>{stopping ? '正在停止执行' : waitingQuestion ? '等待你的答复' : busy ? '正在处理你的请求' : loading ? '正在加载对话' : current ? labels[current.status] : 'Anchor 的任务与管理入口'}</span></div>
        {current?.status === 'interrupted' && <button onClick={() => void resume()} disabled={busy || loading}>继续上次回复</button>}
      </header>
      <PilotConversationLayout start={showStart}>
      {showStart && <div className="pilot-intro">
        <h2>你想完成什么？</h2>
        <p>把任务交给 Pilot，搭建工作流，或扩展你需要的工具。</p>
      </div>}
      <div className="pilot-messages" ref={scroller} role="log" aria-label="对话消息" aria-live="polite" onScroll={() => {
        const el = scroller.current;
        if (!el) return;
        const near = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
        follow.current = near; setAtBottom(near);
      }}>
        <div className="pilot-message-list">
        {loading && <div className="pilot-thinking" role="status"><LoaderCircle size={16} />正在加载对话…</div>}
        {messages.map((message, index) => {
          const turnId = messageTurn(message);
          const requests = turnId && lastMessageByTurn.get(turnId) === index
            && !(turn?.status === 'running' && turnId === turn.id) ? credentialRequests(turnId) : [];
          return <Fragment key={`${selected}-${index}`}><article className={`pilot-message ${message.role}`}>
          <div className="pilot-message-author">{message.role === 'assistant' && <Anchor size={16} />}<span>{message.role === 'user' ? '你' : 'Anchor Pilot'}</span></div>
          {message.role === 'assistant' ? <Markdown text={message.text} prefix={`pilot-${selected}-${index}`} /> : <p className="pilot-user-text">{message.text}</p>}
          <PilotAttachments attachments={message.attachments} drafts={message.localAttachments} session={selected} turn={message.turn} />
          {message.question?.status === 'interrupted' && <p className="pilot-muted">这条提问已中断。</p>}
          {message.text && <button className="pilot-copy" aria-label={`复制${message.role === 'assistant' ? '回复' : '消息'} ${index + 1}`}
            onClick={() => void copy(message, index)}>{copied === index ? <Check size={14} /> : <Copy size={14} />}{copied === index ? '已复制' : '复制'}</button>}
        </article>
          {requests.length > 0 && <div data-credential-turn={turnId}><PilotCredentials requests={requests} onConfigured={credentials.onConfigured} /></div>}
        </Fragment>;
        })}
        {turn && <PilotTurn key={`${turn.session}-${turn.id}`} turn={turn} saved={syncedTurn === turn.id} stopping={stopping} onComplete={complete} onQuestions={questionState} />}
        {selected && <PilotCredentials key={`credentials-${selected}`} requests={remainingCredentials} onConfigured={credentials.onConfigured} />}
        {credentials.problem && <p className="pilot-error" role="alert">无法读取凭据配置请求。<button type="button" onClick={credentials.retry}>重新加载配置状态</button></p>}
        {busy && !turn && <div className="pilot-thinking" role="status"><LoaderCircle size={16} />正在提交…</div>}
        {awaitingContinuation && turn?.status !== 'running' && <p className="pilot-muted" role="status">正在应用配置，Pilot 将继续当前任务…</p>}
        </div>
      </div>
      {!atBottom && <button className="pilot-latest" onClick={() => {
        if (scroller.current) scroller.current.scrollTop = scroller.current.scrollHeight;
        follow.current = true; setAtBottom(true);
      }}><ArrowDown size={14} />回到最新消息</button>}
      {current?.status === 'waiting_user' && !pendingApproval && <p className="pilot-waiting" role="status">{current.waiting_reason || 'Pilot 需要你的补充信息，请回答本次执行中的问题。'}</p>}
      {pendingApproval && current.approval && <div className="pilot-approval" role="region" aria-label="待确认操作">
        <span><Check size={16} />需要你的确认 · {current.approval.target || current.approval.action}</span>
        <details><summary>查看操作内容 · {current.approval.action}</summary><pre>{JSON.stringify(current.approval.proposal, null, 2)}</pre></details>
        <div><button onClick={() => void decide(false)} disabled={busy}>拒绝</button><button onClick={() => void decide(true)} disabled={busy}>确认并继续</button></div>
      </div>}
      {problem && <p className="pilot-error" role="alert">{problem}
        {modelRequired ? <button onClick={onSettings}>前往模型设置</button>
          : selected && <button onClick={() => void select(selected)}>重新加载对话</button>}
      </p>}
      <form className={`pilot-composer${dragging ? ' is-dragging' : ''}`} onSubmit={send}
        onDragOver={event => { if (event.dataTransfer.types.includes('Files')) { event.preventDefault(); if (!busy && !loading && canSend && filesReady) setDragging(true); } }}
        onDragLeave={event => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setDragging(false); }}
        onDrop={event => { if (event.dataTransfer.types.includes('Files')) { event.preventDefault(); setDragging(false); addFiles(Array.from(event.dataTransfer.files)); } }}>
        <PilotAttachments drafts={files} disabled={busy || loading || stopping || !canSend || !filesReady} onRemove={id => void storeFiles(selectedRef.current, filesRef.current.filter(file => file.id !== id))} />
        {attachmentError && <p className="pilot-composer-error" role="alert">{attachmentError}</p>}
        <input ref={fileInput} className="pilot-file-input" type="file" multiple aria-label="选择附件" onChange={event => {
          addFiles(Array.from(event.currentTarget.files ?? [])); event.currentTarget.value = '';
        }} disabled={busy || loading || stopping || !canSend || !filesReady} />
        <textarea ref={input} rows={2} aria-label="发送给 Anchor Pilot" placeholder={pendingApproval ? '请先确认或拒绝上方操作' : waitingQuestion || current?.status === 'waiting_user' ? '请先回答、拒绝或取消上方问题' : '告诉 Pilot 你想完成什么…'} value={draft}
          onPaste={event => {
            const images = Array.from(event.clipboardData.items).filter(item => item.kind === 'file' && item.type.startsWith('image/')).map(item => item.getAsFile()).filter((file): file is File => file !== null);
            if (images.length) {
              if (!event.clipboardData.getData('text/plain')) event.preventDefault();
              addFiles(images);
            }
          }} maxLength={100000} onChange={event => editDraft(event.target.value)} onKeyDown={event => {
            if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing && event.keyCode !== 229) {
              event.preventDefault(); event.currentTarget.form?.requestSubmit();
            }
          }} disabled={busy || loading || stopping || !canSend || !filesReady} />
        <div className="pilot-composer-footer"><div className="pilot-composer-actions"><button className="pilot-attach" type="button" aria-label="添加附件" disabled={busy || loading || stopping || !canSend || !filesReady} onClick={() => fileInput.current?.click()}><Paperclip size={16} />附件</button><small>Enter 发送 · Shift + Enter 换行</small></div>
          {(busy || awaitingContinuation) && <button className="pilot-stop" type="button" disabled={stopping} onClick={() => void stop()}><Square size={14} fill="currentColor" />{stopping ? '正在停止…' : '停止'}</button>}
          {!busy && <button className="primary" type="submit" disabled={(!draft.trim() && !files.length) || stopping || loading || !canSend || !filesReady}><Send size={16} />发送</button>}
        </div>
      </form>
      {showStart && <div className="pilot-starters" aria-label="任务示例">
        {starters.map(({ label, text }) => <button key={label} type="button"
          onClick={() => { editDraft(text); input.current?.focus({ preventScroll: true }); }}>
          <span className="pilot-starter-label">{label}</span><span className="pilot-starter-text">{text}</span><span className="pilot-starter-arrow" aria-hidden="true">↗</span>
        </button>)}
      </div>}
      </PilotConversationLayout>
    </section>
    {/* Plugin mounting belongs to the session, not to one Turn. Keeping the capability rail outside
        the transcript gives the conversation a stable reading surface while the capability policy stays
        visible beside the controls. */}
    <SessionPlugins key={selected} session={selected} defaultOpen={false} onContinuation={scheduleContinuation} continuation={continuation} />
  </main>;
}
