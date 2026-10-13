/** @vitest-environment jsdom */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { api, bearerKey, registerApiKeyPrompt, setBearerKey, verifyBearerKey } from './api';

describe('Anchor API key recovery', () => {
  let fetchMock: ReturnType<typeof vi.fn>;
  let unregister: (() => void) | undefined;
  beforeEach(() => {
    sessionStorage.clear();
    fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
  });
  afterEach(() => { unregister?.(); unregister = undefined; vi.unstubAllGlobals(); });

  it('recovers from an invalid cached header and retries with the entered key', async () => {
    sessionStorage.setItem('anchor-api-key', '旧的中文密钥');
    fetchMock.mockResolvedValueOnce(Response.json({ error: 'unauthorized' }, { status: 401 }))
      .mockResolvedValueOnce(Response.json({ graphs: [] }));
    unregister = registerApiKeyPrompt(async () => 'valid-key');

    await expect(api('/graphs')).resolves.toEqual({ graphs: [] });
    expect(new Headers(fetchMock.mock.calls[0][1].headers).has('Authorization')).toBe(false);
    expect(new Headers(fetchMock.mock.calls[1][1].headers).get('Authorization')).toBe('Bearer valid-key');
    expect(bearerKey()).toBe('valid-key');
  });

  it('drops a rejected cached key even when the user cancels the prompt', async () => {
    setBearerKey('rejected-key');
    fetchMock.mockResolvedValue(Response.json({ error: 'unauthorized' }, { status: 401 }));
    unregister = registerApiKeyPrompt(async () => '');

    await expect(api('/graphs')).rejects.toMatchObject({ status: 401 });
    expect(sessionStorage.getItem('anchor-api-key')).toBeNull();
    expect(fetchMock).toHaveBeenCalledOnce();
  });

  it('does not save or send an unsafe newly entered key', async () => {
    fetchMock.mockResolvedValue(Response.json({ error: 'unauthorized' }, { status: 401 }));
    unregister = registerApiKeyPrompt(async () => '错误密钥');

    await expect(api('/graphs')).rejects.toThrow('API key 格式不正确');
    expect(fetchMock).toHaveBeenCalledOnce();
    expect(sessionStorage.getItem('anchor-api-key')).toBeNull();
  });

  it('uses a recently verified key when an older request returns a delayed 401', async () => {
    setBearerKey('old-key');
    let rejectOld: ((response: Response) => void) | undefined;
    fetchMock.mockImplementationOnce(() => new Promise<Response>(resolve => { rejectOld = resolve; }))
      .mockResolvedValueOnce(Response.json({ graphs: [] }));
    const prompt = vi.fn(async () => 'should-not-be-needed');
    unregister = registerApiKeyPrompt(prompt);
    const pending = api('/graphs');
    setBearerKey('recently-verified-key');
    rejectOld!(Response.json({ error: 'unauthorized' }, { status: 401 }));

    await expect(pending).resolves.toEqual({ graphs: [] });
    expect(prompt).not.toHaveBeenCalled();
    expect(new Headers(fetchMock.mock.calls[1][1].headers).get('Authorization')).toBe('Bearer recently-verified-key');
    expect(bearerKey()).toBe('recently-verified-key');
  });

  it.each(['中文', 'key with spaces', 'key\nvalue', 'key\u007f', 'key\u00e9'])('rejects %j before a validation request', async key => {
    await expect(verifyBearerKey(key)).rejects.toThrow('API key 格式不正确');
    expect(fetchMock).not.toHaveBeenCalled();
    expect(sessionStorage.getItem('anchor-api-key')).toBeNull();
  });

  it('distinguishes a wrong key from a service failure without exposing the key', async () => {
    fetchMock.mockResolvedValueOnce(Response.json({ error: 'do not display submitted-secret' }, { status: 401 }))
      .mockResolvedValueOnce(new Response('proxy error', { status: 502 }))
      .mockResolvedValueOnce(new Response('<html>frontend fallback</html>'));

    await expect(verifyBearerKey('submitted-secret')).rejects.toThrow('API key 不正确');
    await expect(verifyBearerKey('submitted-secret')).rejects.toThrow('服务返回 502');
    await expect(verifyBearerKey('submitted-secret')).rejects.toThrow('请检查 API 服务连接');
    expect(sessionStorage.getItem('anchor-api-key')).toBeNull();
  });

  it('normalizes a valid key and only saves it when accepted by the caller', async () => {
    fetchMock.mockResolvedValue(Response.json({ graphs: [] }));
    const key = await verifyBearerKey('  valid-key  ');
    expect(key).toBe('valid-key');
    expect(new Headers(fetchMock.mock.calls[0][1].headers).get('Authorization')).toBe('Bearer valid-key');
    expect(sessionStorage.getItem('anchor-api-key')).toBeNull();
    setBearerKey(key);
    expect(bearerKey()).toBe('valid-key');
  });
});
