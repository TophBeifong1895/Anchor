/** @vitest-environment jsdom */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from 'vitest';
import { PilotCredentialForm, PilotCredentials, usePilotCredentials, type CredentialRequest } from './PilotCredentials';

const request: CredentialRequest = { id: 'c-1', session: 'pilot-private-s-1', turn: 't-1', plugin: 'service', server: 'main', field: 'SERVICE_TOKEN', status: 'pending' };
const response = (status: number, body: unknown) => ({ ok: status >= 200 && status < 300, status, json: async () => body }) as Response;
function CredentialPanel({ session, onConfigured, polling = true }: { session: string; onConfigured: () => void; polling?: boolean }) {
  const credentials = usePilotCredentials(session, onConfigured, polling);
  return <PilotCredentials requests={credentials.requests} onConfigured={credentials.onConfigured} />;
}
describe('Pilot credential input', () => {
  let root: Root;
  let container: HTMLDivElement;
  let fetchMock: ReturnType<typeof vi.fn>;
  let configured: Mock<() => void>;
  beforeEach(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    container = document.createElement('div'); document.body.appendChild(container);
    root = createRoot(container); fetchMock = vi.fn(); configured = vi.fn(); vi.stubGlobal('fetch', fetchMock);
    sessionStorage.clear(); localStorage.clear();
  });
  afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.useRealTimers(); vi.unstubAllGlobals(); });
  const draw = async (value = request) => { await act(async () => root.render(<PilotCredentialForm request={value} onConfigured={configured} />)); };
  const submit = async (value: string) => {
    container.querySelector<HTMLInputElement>('input')!.value = value;
    await act(async () => { container.querySelector('form')!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })); });
  };
  it('submits directly to the secret endpoint and clears the value without recording a question answer', async () => {
    const secret = 'private-token-random-fixture';
    fetchMock.mockResolvedValue(response(200, { request: { ...request, status: 'configured' } }));
    await draw();
    const input = container.querySelector<HTMLInputElement>('input')!;
    expect(input.type).toBe('password'); expect(input.autocomplete).toBe('off');
    await submit(secret);
    expect(fetchMock.mock.calls[0][0]).toBe('/sessions/pilot-private-s-1/credential-requests/c-1/value');
    expect(fetchMock.mock.calls[0][1].body).toBe(JSON.stringify({ value: secret }));
    expect(input.value).toBe(''); expect(configured).toHaveBeenCalledOnce();
    expect(container.innerHTML).not.toContain(secret); expect(localStorage.length).toBe(0); expect(sessionStorage.length).toBe(0);
  });
  it('does not render response errors that might contain submitted secrets', async () => {
    const secret = 'sensitive-rejected-value';
    fetchMock.mockResolvedValue(response(409, { error: `cannot accept ${secret}` }));
    await draw(); await submit(secret);
    expect(container.textContent).toContain('未能确认凭据已配置');
    expect(container.textContent).not.toContain(secret); expect(configured).not.toHaveBeenCalled();
  });
  it('refuses a response for a different Session and removes inputs when the request is interrupted', async () => {
    fetchMock.mockResolvedValue(response(200, { request: { ...request, session: 'other', status: 'configured' } }));
    await draw(); await submit('token'); expect(configured).not.toHaveBeenCalled();
    await draw({ ...request, status: 'interrupted' });
    expect(container.querySelector('input')).toBeNull(); expect(container.textContent).toContain('配置请求已中断');
  });
  it('aborts an in-flight submission when switching away', async () => {
    fetchMock.mockImplementation(() => new Promise(() => undefined));
    await draw(); await submit('token');
    const signal = fetchMock.mock.calls[0][1].signal as AbortSignal;
    const input = container.querySelector<HTMLInputElement>('input')!;
    await act(async () => root.render(<p>另一个对话</p>));
    expect(input.value).toBe('');
    expect(signal.aborted).toBe(true); expect(configured).not.toHaveBeenCalled();
  });
  it('cancels the configuration without transmitting the typed secret', async () => {
    fetchMock.mockResolvedValue(response(200, { request: { ...request, status: 'cancelled' } }));
    await draw(); container.querySelector<HTMLInputElement>('input')!.value = 'do-not-send';
    await act(async () => { [...container.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === '取消')!.click(); });
    expect(fetchMock.mock.calls[0][0]).toBe('/sessions/pilot-private-s-1/credential-requests/c-1/cancel');
    expect(fetchMock.mock.calls[0][1].body).toBeUndefined();
    expect(container.querySelector<HTMLInputElement>('input')!.value).toBe('');
    expect(configured).toHaveBeenCalledOnce();
  });

  it('resets a replacement request immediately and ignores the cancelled submission response', async () => {
    let complete: (value: Response) => void = () => undefined;
    fetchMock.mockImplementationOnce(() => new Promise<Response>(resolve => { complete = resolve; }));
    await draw(); await submit('old-request-secret');
    const signal = fetchMock.mock.calls[0][1].signal as AbortSignal;
    const oldInput = container.querySelector<HTMLInputElement>('input')!;
    await draw({ ...request, id: 'c-2', field: 'NEXT_TOKEN' });
    expect(signal.aborted).toBe(true); expect(oldInput.value).toBe('');
    expect(container.querySelector<HTMLInputElement>('input')!.disabled).toBe(false);
    expect(container.textContent).toContain('NEXT_TOKEN');
    await act(async () => complete(response(200, { request: { ...request, status: 'configured' } })));
    expect(configured).not.toHaveBeenCalled(); expect(container.textContent).toContain('NEXT_TOKEN');
  });

  it('removes a previous Session request while the next Session is loading', async () => {
    fetchMock.mockResolvedValueOnce(response(200, { requests: [request] }));
    await act(async () => root.render(<CredentialPanel session={request.session} onConfigured={configured} />));
    const oldInput = container.querySelector<HTMLInputElement>('input')!;
    oldInput.value = 'departing-session-secret';
    fetchMock.mockImplementationOnce(() => new Promise<Response>(() => undefined));
    await act(async () => root.render(<CredentialPanel session="pilot-private-next" onConfigured={configured} />));
    expect(oldInput.value).toBe(''); expect(container.querySelector('input')).toBeNull();
    expect(container.textContent).not.toContain('SERVICE_TOKEN');
  });

  it('keeps completed request history distinct from current credentials', async () => {
    await draw({ ...request, status: 'configured' });
    expect(container.textContent).toContain('本次配置请求已完成');
    expect(fetchMock).not.toHaveBeenCalled();
    fetchMock.mockResolvedValueOnce(response(200, { plugin: 'service', authorization_digest: 'digest', revision: 0, fields: [], targets: [] }));
    fetchMock.mockResolvedValueOnce(response(200, { plugin: 'service', targets: [{ target: 'main', kind: 'mcp',
      authorization_digest: 'digest', fields: [{ field: 'SERVICE_TOKEN', state: 'missing', editable: true }] }] }));
    await act(async () => { [...container.querySelectorAll('button')].find(button => button.textContent === '查看当前凭据')!.click(); });
    expect(container.textContent).toContain('本次配置请求已完成');
    expect(container.textContent).toContain('未配置');
    expect(container.textContent).toContain('设置 SERVICE_TOKEN');
    expect(fetchMock.mock.calls.map(call => call[0])).toEqual(['/plugins/service/configuration', '/plugins/service/credentials']);
  });

  it('collapses settled history without hiding a pending configuration', async () => {
    const history: CredentialRequest[] = ['configured', 'cancelled', 'interrupted'].map((status, index) => ({
      ...request, id: `history-${index}`, status: status as CredentialRequest['status'],
    }));
    await act(async () => root.render(<PilotCredentials requests={[request, ...history]} onConfigured={configured} />));
    const details = container.querySelector('details')!;
    expect(details.open).toBe(false);
    expect(details.querySelector('summary')!.textContent).toBe('凭据配置记录 · 3 项');
    expect(details.querySelectorAll('.pilot-question')).toHaveLength(3);
    expect(container.querySelectorAll('form')).toHaveLength(1);
    expect(details.querySelector('form')).toBeNull();
    await act(async () => details.querySelector('summary')!.click());
    expect(details.open).toBe(true);
    expect(details.textContent).toContain('本次配置请求已完成');
    expect(details.textContent).toContain('配置已取消');
    expect(details.textContent).toContain('配置请求已中断');
  });

  it('ignores a late response from the previous Session', async () => {
    let finish: (value: Response) => void = () => undefined;
    fetchMock.mockImplementationOnce(() => new Promise<Response>(resolve => { finish = resolve; }));
    await act(async () => root.render(<CredentialPanel session={request.session} onConfigured={configured} />));
    const signal = fetchMock.mock.calls[0][1].signal as AbortSignal;
    fetchMock.mockResolvedValueOnce(response(200, { requests: [{ ...request, session: 'pilot-private-next', field: 'NEXT_TOKEN' }] }));
    await act(async () => root.render(<CredentialPanel session="pilot-private-next" onConfigured={configured} />));
    expect(signal.aborted).toBe(true);
    await act(async () => finish(response(200, { requests: [request] })));
    expect(container.textContent).toContain('NEXT_TOKEN');
    expect(container.textContent).not.toContain('SERVICE_TOKEN');
  });

  it('polls while running or pending, but stops for settled idle history', async () => {
    vi.useFakeTimers();
    fetchMock.mockResolvedValue(response(200, { requests: [{ ...request, status: 'configured' }] }));
    const panel = (polling: boolean) => <CredentialPanel session={request.session} onConfigured={configured} polling={polling} />;
    await act(async () => root.render(panel(false)));
    await act(async () => vi.advanceTimersByTimeAsync(9000));
    expect(fetchMock).toHaveBeenCalledTimes(1);
    await act(async () => root.render(panel(true)));
    await act(async () => vi.advanceTimersByTimeAsync(3000));
    expect(fetchMock).toHaveBeenCalledTimes(3);
    fetchMock.mockResolvedValue(response(200, { requests: [request] }));
    await act(async () => root.render(panel(false)));
    await act(async () => vi.advanceTimersByTimeAsync(3000));
    expect(fetchMock).toHaveBeenCalledTimes(5);
    fetchMock.mockResolvedValue(response(200, { requests: [{ ...request, status: 'configured' }] }));
    await act(async () => vi.advanceTimersByTimeAsync(3000));
    expect(fetchMock).toHaveBeenCalledTimes(6);
    await act(async () => vi.advanceTimersByTimeAsync(9000));
    expect(fetchMock).toHaveBeenCalledTimes(6);
  });
});
