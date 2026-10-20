import { api, bearerKey } from './api';

const pending = new Map<string, string>();

export async function createPilotSession<Session extends { id: string }>(): Promise<{ session: Session }> {
  const identity = new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(bearerKey())));
  const scope = `pilot:creation:${Array.from(identity, byte => byte.toString(16).padStart(2, '0')).join('')}`;
  let key = pending.get(scope);
  try { key ??= localStorage.getItem(scope) ?? undefined; } catch {}
  key ??= crypto.randomUUID();
  pending.set(scope, key);
  try { localStorage.setItem(scope, key); } catch {}
  const result = await api<{ session: Session }>('/sessions', 'POST', undefined, undefined, { 'Idempotency-Key': key });
  if (typeof result?.session?.id !== 'string' || !result.session.id.trim()) throw new Error('创建对话未返回有效会话，请重试。');
  if (pending.get(scope) === key) pending.delete(scope);
  try { if (localStorage.getItem(scope) === key) localStorage.removeItem(scope); } catch {}
  return result;
}
