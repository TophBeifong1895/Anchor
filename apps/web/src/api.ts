/** The one way this page talks to the Anchor Host.
 *
 * Its own module because two components need it: the page, and the file panel it opens inside a run. A
 * second copy of this would be a second place for the error handling to be subtly different.
 */

export class ApiError extends Error {
  constructor(message: string, readonly status: number) { super(message); }
}

type ApiKeyPrompt = () => Promise<string>;
let apiKeyPrompt: ApiKeyPrompt | undefined;
let pendingApiKeyPrompt: Promise<string> | undefined;

/** Register the application's accessible key dialog for API requests that receive 401. */
export function registerApiKeyPrompt(prompt: ApiKeyPrompt): () => void {
  apiKeyPrompt = prompt;
  return () => { if (apiKeyPrompt === prompt) apiKeyPrompt = undefined; };
}

export function requestApiKey(): Promise<string> {
  if (!pendingApiKeyPrompt) {
    if (!apiKeyPrompt) return Promise.reject(new Error('需要 Anchor API key；请重新打开页面后输入密钥。'));
    pendingApiKeyPrompt = apiKeyPrompt().finally(() => { pendingApiKeyPrompt = undefined; });
  }
  return pendingApiKeyPrompt;
}

/** A delayed 401 must not discard a key another request has just verified. */
export function recoverBearerKey(rejectedKey: string): Promise<string> {
  const current = bearerKey();
  if (current && current !== rejectedKey) return Promise.resolve(current);
  setBearerKey('');
  return requestApiKey();
}

export async function api<T>(path: string, method = 'GET', body?: unknown, signal?: AbortSignal): Promise<T> {
  const send = (key: string) => {
    const timeout = AbortSignal.timeout(30000);
    return fetch(path, {
      method,
      signal: signal ? AbortSignal.any([signal, timeout]) : timeout,
      headers: { Accept: 'application/json', ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
        ...(key ? { Authorization: `Bearer ${key}` } : {}) },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  };
  let key = bearerKey();
  let response = await send(key);
  if (response.status === 401) {
    key = await recoverBearerKey(key);
    if (key) {
      setBearerKey(key);
      response = await send(bearerKey());
    }
  }
  // A successful DELETE answers 204 with no body. Only an OK response may be
  // empty; anything else without a JSON body is a transport problem.
  const empty = response.status === 204 || response.status === 205;
  const data = empty ? null : await response.json().catch(() => null);
  if (!response.ok) throw new ApiError(data?.error ?? `${method} ${path} → ${response.status}`, response.status);
  if (data === null) {
    if (empty) return undefined as T;
    throw new Error(`${method} ${path} 未返回有效 JSON，请检查 API 服务连接。`);
  }
  return data as T;
}

export function bearerKey(): string {
  const saved = sessionStorage.getItem('anchor-api-key') ?? '';
  try {
    const key = normalizeBearerKey(saved);
    if (key !== saved) setBearerKey(key);
    return key;
  } catch {
    // A stale browser value must not prevent the first request or the key dialog.
    sessionStorage.removeItem('anchor-api-key');
    return '';
  }
}

export function setBearerKey(key: string): void {
  const normalized = normalizeBearerKey(key);
  if (normalized) sessionStorage.setItem('anchor-api-key', normalized);
  else sessionStorage.removeItem('anchor-api-key');
}

function normalizeBearerKey(key: string): string {
  const normalized = key.trim();
  if (/[^\x21-\x7e]/.test(normalized)) {
    throw new Error('Anchor API key 格式不正确，请检查是否混入中文、空格或换行。');
  }
  return normalized;
}

/** Check a draft without saving it or recursively opening another key dialog. */
export async function verifyBearerKey(key: string, signal?: AbortSignal): Promise<string> {
  const normalized = normalizeBearerKey(key);
  if (!normalized) throw new Error('请输入 Anchor API key。');
  const timeout = AbortSignal.timeout(30000);
  const response = await fetch('/graphs', {
    headers: { Accept: 'application/json', Authorization: `Bearer ${normalized}` },
    signal: signal ? AbortSignal.any([signal, timeout]) : timeout,
  });
  if (response.status === 401) throw new Error('Anchor API key 不正确，请检查后重新输入。');
  if (!response.ok) throw new ApiError(`暂时无法验证 API key：服务返回 ${response.status}。`, response.status);
  const data = await response.json().catch(() => null);
  if (!Array.isArray(data?.graphs)) throw new Error('无法验证 Anchor API key，请检查 API 服务连接。');
  return normalized;
}

/** How a fetch failure reads to a person. The server's own message when there is one. */
export function reason(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** A size, at the scale a person reads it — a workspace holds both a 20-byte note and a 4MB dump. */
export function human(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}
