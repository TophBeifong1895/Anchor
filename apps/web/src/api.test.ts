/** @vitest-environment jsdom */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { api, apiBlob } from './api';

describe('Anchor API transport', () => {
  let fetchMock: ReturnType<typeof vi.fn>;
  beforeEach(() => { fetchMock = vi.fn(); vi.stubGlobal('fetch', fetchMock); });
  afterEach(() => { vi.unstubAllGlobals(); });

  it('sends no credentials and surfaces the server message on failure', async () => {
    fetchMock.mockResolvedValueOnce(Response.json({ error: 'no such graph' }, { status: 404 }));
    await expect(api('/graphs/missing')).rejects.toThrow('no such graph');
    expect(new Headers(fetchMock.mock.calls[0][1].headers).has('Authorization')).toBe(false);
  });

  it('treats an empty success as no body and a missing body as a transport problem', async () => {
    fetchMock.mockResolvedValueOnce(new Response(null, { status: 204 }));
    await expect(api('/runs/x', 'DELETE')).resolves.toBeUndefined();
    fetchMock.mockResolvedValueOnce(new Response('', { status: 200 }));
    await expect(api('/graphs')).rejects.toThrow('未返回有效 JSON');
  });

  it('reports a blob download failure like any other request', async () => {
    fetchMock.mockResolvedValueOnce(Response.json({ error: 'gone' }, { status: 404 }));
    await expect(apiBlob('/runs/x/files/a.txt')).rejects.toThrow('gone');
  });
});
