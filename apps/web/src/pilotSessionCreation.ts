import { api } from './api';

const pending = new Map<string, string>();

export async function createPilotSession<Session extends { id: string }>(): Promise<{ session: Session }> {
  // One installation, one creation request per pending session id.
  const scope = 'pilot:creation';
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
