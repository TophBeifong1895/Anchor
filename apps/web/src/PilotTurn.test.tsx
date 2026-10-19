/** @vitest-environment jsdom */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { PilotTurn, type Turn } from './PilotTurn';
import { api } from './api';
import type { Question } from './pilotQuestions';

vi.mock('./api', async original => ({ ...await original<object>(), api: vi.fn(), bearerKey: () => '' }));
vi.mock('./markdown', () => ({ Markdown: ({ text }: { text: string }) => <div data-testid="reply">{text}</div> }));

const turn: Turn = { id: 'turn-1', session: 'session-1', status: 'running', error: '', created_at: '2026-10-19T00:00:00Z', prompt: 'Hello' };
const question: Question = { id: 'question-1', session: turn.session, turn: turn.id, message: 'Continue?',
  requested_schema: { type: 'object', properties: { confirm: { type: 'boolean' } }, required: ['confirm'] }, status: 'pending', answer: null };
const chunk = (id: number, data: object) => `id: ${id}\ndata: ${JSON.stringify(data)}\n\n`;
const terminal = (final: Turn) => `event: turn\ndata: ${JSON.stringify(final)}\n\n`;
function deferred<Value>() {
  let resolve!: (value: Value) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<Value>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}
function eventStream(signal?: AbortSignal) {
  let source!: ReadableStreamDefaultController<Uint8Array>;
  let closed = false;
  const cancel = vi.fn(() => { closed = true; });
  const body = new ReadableStream<Uint8Array>({ start(controller) { source = controller; }, cancel });
  signal?.addEventListener('abort', () => {
    if (!closed) { closed = true; source.error(new DOMException('Aborted', 'AbortError')); }
  }, { once: true });
  return {
    response: { status: 200, ok: true, body } as Response,
    send: (data: string | Uint8Array) => source.enqueue(typeof data === 'string' ? new TextEncoder().encode(data) : data),
    close: () => { if (!closed) { closed = true; source.close(); } }, cancel,
  };
}

describe('PilotTurn stream and history lifecycle', () => {
  let root: Root;
  let container: HTMLDivElement;
  let streams: ReturnType<typeof eventStream>[];
  const fetchMock = vi.fn<typeof fetch>();
  const onComplete = vi.fn();
  const onQuestions = vi.fn();
  const draw = async (value = turn, saved = false) => {
    await act(async () => root.render(<PilotTurn turn={value} saved={saved} onComplete={onComplete} onQuestions={onQuestions} />));
  };
  beforeEach(() => {
    vi.useFakeTimers();
    vi.stubGlobal('IS_REACT_ACT_ENVIRONMENT', true);
    vi.stubGlobal('fetch', fetchMock);
    container = document.createElement('div'); document.body.append(container); root = createRoot(container);
    streams = [];
    fetchMock.mockReset().mockImplementation(async (_path, options) => {
      const stream = eventStream(options?.signal ?? undefined); streams.push(stream); return stream.response;
    });
    vi.mocked(api).mockReset().mockResolvedValue({ questions: [] });
    onComplete.mockReset(); onQuestions.mockReset();
  });
  afterEach(async () => {
    await act(async () => { root.unmount(); streams.forEach(stream => stream.close()); });
    container.remove(); vi.useRealTimers(); vi.unstubAllGlobals();
  });

  it('reconnects after EOF from the last consumed cursor without replaying text or completing twice', async () => {
    await draw();
    await act(async () => {
      streams[0].send(chunk(1, { type: 'text-delta', delta: 'Hello' }) + chunk(2, { type: 'tool-input-start', toolCallId: 'tool-1', toolName: 'inspect' }));
      streams[0].close();
    });
    expect(container.textContent).toContain('正在重新连接');
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    expect(fetchMock.mock.calls.map(([path]) => path)).toEqual([
      '/sessions/session-1/turns/turn-1/events?after=0', '/sessions/session-1/turns/turn-1/events?after=2',
    ]);
    const final = { ...turn, status: 'completed' };
    await act(async () => streams[1].send(chunk(1, { type: 'text-delta', delta: 'Hello' })
      + chunk(3, { type: 'text-delta', delta: ' world' }) + chunk(4, { type: 'tool-output-available', toolCallId: 'tool-1', output: 'ok' })
      + terminal(final) + terminal(final)));
    expect(container.querySelector('[data-testid="reply"]')?.textContent).toBe('Hello world');
    expect(container.textContent).toContain('inspect已返回');
    expect(onComplete.mock.calls).toEqual([[final]]);
    expect(onQuestions).toHaveBeenLastCalledWith(turn.session, turn.id, false);
    expect(streams[1].cancel).toHaveBeenCalledTimes(1);
    await act(async () => { await vi.advanceTimersByTimeAsync(60000); });
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it('preserves UTF-8 and CRLF event boundaries split across network reads', async () => {
    await draw();
    const bytes = new TextEncoder().encode(chunk(1, { type: 'text-delta', delta: '你好' }).replace(/\n/g, '\r\n'));
    for (const value of bytes) await act(async () => streams[0].send(new Uint8Array([value])));
    await act(async () => streams[0].send(terminal({ ...turn, status: 'completed' }).trimEnd()));
    await act(async () => streams[0].close());
    expect(container.querySelector('[data-testid="reply"]')?.textContent).toBe('你好');
    expect(onComplete).toHaveBeenCalledTimes(1);
  });

  it('resets the idle watchdog on keepalives and reconnects a stalled stream without losing its cursor', async () => {
    await draw();
    await act(async () => streams[0].send(chunk(1, { type: 'text-delta', delta: 'partial' })));
    await act(async () => { await vi.advanceTimersByTimeAsync(25000); streams[0].send(': keepalive\n\n'); });
    await act(async () => { await vi.advanceTimersByTimeAsync(29999); });
    expect(fetchMock).toHaveBeenCalledTimes(1);
    await act(async () => { await vi.advanceTimersByTimeAsync(1); });
    expect(fetchMock.mock.calls[0][1]?.signal?.aborted).toBe(true);
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    expect(fetchMock).toHaveBeenLastCalledWith('/sessions/session-1/turns/turn-1/events?after=1', expect.anything());
    expect(container.querySelector('[data-testid="reply"]')?.textContent).toBe('partial');
  });

  it('shows the Host history error event without treating it as a terminal turn or advancing its cursor', async () => {
    await draw();
    await act(async () => {
      streams[0].send(chunk(1, { type: 'text-delta', delta: 'partial' })
        + 'event: error\ndata: {"type":"error","errorText":"Pilot delivery history unavailable"}\n\n');
      streams[0].close();
    });
    expect(container.querySelector('[role="alert"]')?.textContent).toBe('Pilot delivery history unavailable');
    expect(onComplete).not.toHaveBeenCalled();
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    expect(fetchMock).toHaveBeenLastCalledWith('/sessions/session-1/turns/turn-1/events?after=1', expect.anything());
  });

  it('clears the departed turn projection and ignores its late question snapshot', async () => {
    const oldRead = deferred<{ questions: Question[] }>();
    const nextRead = deferred<{ questions: Question[] }>();
    vi.mocked(api).mockImplementationOnce(() => oldRead.promise).mockImplementationOnce(() => nextRead.promise);
    await draw();
    await act(async () => streams[0].send(chunk(1, { type: 'question', question }) + chunk(2, { type: 'text-delta', delta: 'old response' })
      + chunk(3, { type: 'tool-input-start', toolCallId: 'old-tool', toolName: 'old tool' })));
    expect(container.querySelector('form')).not.toBeNull();
    const next = { ...turn, id: 'turn-2', session: 'session-2' };
    await draw(next);
    expect(fetchMock.mock.calls[0][1]?.signal?.aborted).toBe(true);
    expect(vi.mocked(api).mock.calls[0][3]?.aborted).toBe(true);
    expect(container.querySelector('.pilot-question')).toBeNull();
    expect(container.textContent).not.toContain('old response');
    expect(container.textContent).not.toContain('old tool');
    const calls = onQuestions.mock.calls.length;
    await act(async () => oldRead.resolve({ questions: [question] }));
    expect(onQuestions).toHaveBeenCalledTimes(calls);
    const nextQuestion = { ...question, id: 'question-2', session: next.session, turn: next.id, message: 'Next question' };
    await act(async () => nextRead.resolve({ questions: [nextQuestion] }));
    expect(container.querySelectorAll('.pilot-question')).toHaveLength(1);
    expect(container.textContent).toContain('Next question');
    expect(onQuestions).toHaveBeenLastCalledWith(next.session, next.id, true);
  });

  it('keeps the subscription and projection when only parent callbacks change', async () => {
    await draw();
    await act(async () => streams[0].send(chunk(1, { type: 'text-delta', delta: 'partial response' }) + chunk(2, { type: 'question', question })));
    const nextComplete = vi.fn();
    const nextQuestions = vi.fn();
    await act(async () => root.render(<PilotTurn turn={turn} onComplete={nextComplete} onQuestions={nextQuestions} />));
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(api).toHaveBeenCalledTimes(1);
    expect(container.querySelector('[data-testid="reply"]')?.textContent).toBe('partial response');
    expect(container.querySelector('form')).not.toBeNull();
    const final = { ...turn, status: 'completed' };
    await act(async () => streams[0].send(terminal(final)));
    expect(nextComplete.mock.calls).toEqual([[final]]);
    expect(nextQuestions).toHaveBeenLastCalledWith(turn.session, turn.id, false);
    expect(onComplete).not.toHaveBeenCalled();
  });

  it.each(['completed', 'failed', 'stopped', 'interrupted'])('does not reactivate pending questions from a late snapshot after %s', async status => {
    const pendingRead = deferred<{ questions: Question[] }>();
    vi.mocked(api).mockImplementationOnce(() => pendingRead.promise);
    await draw();
    const final = { ...turn, status };
    await act(async () => streams[0].send(chunk(1, { type: 'tool-input-start', toolCallId: 'tool-1', toolName: 'inspect' }) + terminal(final)));
    expect(container.textContent).toContain('执行已结束，请核查结果');
    expect(container.querySelector('.pilot-thinking')).toBeNull();
    await act(async () => pendingRead.resolve({ questions: [question] }));
    expect(container.querySelector('form')).toBeNull();
    expect(onQuestions).toHaveBeenLastCalledWith(turn.session, turn.id, false);
    expect(onComplete.mock.calls).toEqual([[final]]);
    await act(async () => { await vi.advanceTimersByTimeAsync(31000); });
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('never downgrades an answered SSE question with an older pending snapshot', async () => {
    const pendingRead = deferred<{ questions: Question[] }>();
    vi.mocked(api).mockImplementationOnce(() => pendingRead.promise);
    await draw();
    const answered: Question = { ...question, status: 'answered', answer: { action: 'accept', content: { confirm: false } } };
    await act(async () => streams[0].send(chunk(1, { type: 'question-answered', question: answered })));
    await act(async () => pendingRead.resolve({ questions: [question] }));
    expect(container.querySelector('form')).toBeNull();
    expect(container.textContent).toContain('已回答');
    expect(container.querySelector('dd')?.textContent).toBe('否');
    expect(onQuestions).toHaveBeenLastCalledWith(turn.session, turn.id, false);
  });

  it('replays saved terminal tool history without duplicating messages or projecting historical questions as pending', async () => {
    const final = { ...turn, status: 'completed' };
    await draw(final, true);
    await act(async () => streams[0].send(chunk(1, { type: 'text-delta', delta: 'saved response' }) + chunk(2, { type: 'question', question })
      + chunk(3, { type: 'question-answered', question: { ...question, status: 'answered', answer: { action: 'cancel' } } })
      + chunk(4, { type: 'tool-input-start', toolCallId: 'tool-1', toolName: 'inspect' })
      + chunk(5, { type: 'tool-output-available', toolCallId: 'tool-1', output: 'ok' }) + terminal(final)));
    expect(api).not.toHaveBeenCalled();
    expect(container.querySelector('.pilot-message')).toBeNull();
    expect(container.querySelector('.pilot-question')).toBeNull();
    expect(container.textContent).toContain('inspect已返回');
    expect(onQuestions.mock.calls.every(([, , pending]) => pending === false)).toBe(true);
    expect(onComplete.mock.calls).toEqual([[final]]);
  });

  it('shows a known terminal error before history is available and updates it without replaying the stream', async () => {
    const final = { ...turn, status: 'failed', error: 'Provider unavailable' };
    await draw(final, true);
    expect(container.querySelector('[role="alert"]')?.textContent).toBe(final.error);
    await draw({ ...final, error: 'Execution interrupted by Host' }, true);
    expect(container.querySelector('[role="alert"]')?.textContent).toBe('Execution interrupted by Host');
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('offers a scoped question reload after a read failure', async () => {
    vi.mocked(api).mockRejectedValueOnce(new Error('Question history unavailable')).mockResolvedValueOnce({ questions: [question] });
    await draw();
    expect(container.querySelector('[role="alert"]')?.textContent).toContain('Question history unavailable');
    await act(async () => (container.querySelector('button') as HTMLButtonElement).click());
    expect(container.querySelector('[role="alert"]')).toBeNull();
    expect(container.querySelector('form')).not.toBeNull();
    expect(api).toHaveBeenLastCalledWith('/sessions/session-1/turns/turn-1/questions', 'GET', undefined, expect.any(AbortSignal));
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('ignores late fetch and question completions after unmount without stopping the server task', async () => {
    const pendingFetch = deferred<Response>();
    const pendingRead = deferred<{ questions: Question[] }>();
    fetchMock.mockImplementationOnce(() => pendingFetch.promise);
    vi.mocked(api).mockImplementationOnce(() => pendingRead.promise);
    await draw();
    await act(async () => root.render(null));
    expect(fetchMock.mock.calls[0][1]?.signal?.aborted).toBe(true);
    expect(vi.mocked(api).mock.calls[0][3]?.aborted).toBe(true);
    const stream = eventStream(); streams.push(stream);
    await act(async () => { pendingFetch.resolve(stream.response); pendingRead.resolve({ questions: [question] }); });
    await act(async () => { await vi.advanceTimersByTimeAsync(60000); });
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(onComplete).not.toHaveBeenCalled();
    expect(onQuestions).not.toHaveBeenCalled();
    expect(container.textContent).toBe('');
  });
});
