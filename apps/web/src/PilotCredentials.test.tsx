/** @vitest-environment jsdom */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from 'vitest';
import { PilotCredentialForm, type CredentialRequest } from './PilotCredentials';

const request: CredentialRequest = { id: 'c-1', session: 'pilot-private-s-1', turn: 't-1', plugin: 'service', server: 'main', field: 'SERVICE_TOKEN', status: 'pending' };
const response = (status: number, body: unknown) => ({ ok: status >= 200 && status < 300, status, json: async () => body }) as Response;
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
  afterEach(async () => { await act(async () => root.unmount()); container.remove(); vi.unstubAllGlobals(); });
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
    await act(async () => root.render(<p>另一个对话</p>));
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
  it('keeps completed request history distinct from current credentials', async () => {
    await draw({ ...request, status: 'configured' });
    expect(container.textContent).toContain('本次配置请求已完成');
    expect(fetchMock).not.toHaveBeenCalled();
    fetchMock.mockResolvedValueOnce(response(200, { plugin: 'service', targets: [{ target: 'main', kind: 'mcp',
      authorization_digest: 'digest', fields: [{ field: 'SERVICE_TOKEN', state: 'missing', editable: true }] }] }));
    await act(async () => { [...container.querySelectorAll('button')].find(button => button.textContent === '查看当前凭据')!.click(); });
    expect(container.textContent).toContain('本次配置请求已完成');
    expect(container.textContent).toContain('未配置');
    expect(container.textContent).toContain('设置 SERVICE_TOKEN');
    expect(fetchMock.mock.calls[0][0]).toBe('/plugins/service/credentials');
  });
});
