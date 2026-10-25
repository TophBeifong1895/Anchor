import { expect, test, type Page } from '@playwright/test';
import type { CredentialRequest } from '../src/PilotCredentials';
import type { Turn } from '../src/PilotTurn';

const stamp = '2026-10-12T08:00:00Z';
const session = { id: 'initialize', title: '初始化 Anchor', status: 'active', updated_at: stamp, approval: null };

async function openSession(page: Page) {
  await page.addInitScript(() => {
    localStorage.setItem('anchor:view', 'pilot');
    localStorage.setItem('pilot:selected', 'initialize');
  });
  await page.route('**/graphs', route => route.fulfill({ json: { graphs: [] } }));
  await page.route('**/runs', route => route.fulfill({ json: { runs: [] } }));
  await page.route('**/timeline*', route => route.fulfill({ json: { runs: [], scheduled: [], schedules: [] } }));
  await page.route('**/sessions', route => route.fulfill({ json: { sessions: [session] } }));
  await page.route('**/sessions/*/plugins', route => route.fulfill({ json: { plugins: [] } }));
  await page.route('**/sessions/*/turns/*/questions', route => route.fulfill({ json: { questions: [] } }));
}

function credential(): CredentialRequest {
  return { id: 'service-key', session: session.id, turn: 'prepare-service', plugin: 'service', server: 'api',
    field: 'SERVICE_API_KEY', purpose: '配置服务访问凭据', status: 'pending' };
}

test('Pilot saves a credential through the dedicated input without putting its value in conversation or browser storage', async ({ page }) => {
  await openSession(page);
  let current = credential();
  const submissions: Array<{ path: string; body: unknown }> = [];
  await page.route('**/sessions/*/messages', route => route.fulfill({ json: { messages: [
    { role: 'assistant', text: '请在下方配置服务凭据，然后我会继续。' },
  ] } }));
  await page.route('**/sessions/*/turns', route => {
    expect(route.request().method()).toBe('GET');
    return route.fulfill({ json: { turns: [] } });
  });
  await page.route('**/sessions/*/credential-requests', route => route.fulfill({ json: { requests: [current] } }));
  await page.route('**/sessions/*/credential-requests/*/value', route => {
    submissions.push({ path: new URL(route.request().url()).pathname, body: route.request().postDataJSON() });
    current = { ...current, status: 'configured' };
    return route.fulfill({ headers: { 'Cache-Control': 'no-store' }, json: { request: current } });
  });
  await page.goto('/');
  const form = page.getByRole('region', { name: '配置插件凭据' });
  const secret = 'acceptance-only-password-value-12';
  const input = form.getByLabel('SERVICE_API_KEY', { exact: true });
  await expect(input).toHaveAttribute('type', 'password');
  await input.fill(secret);
  await expect(page.getByRole('log', { name: '对话消息' })).not.toContainText(secret);
  await form.getByRole('button', { name: '保存凭据并继续', exact: true }).click();
  // A settled request is history: it folds into the record, where its state is read once opened.
  await expect(input).toHaveCount(0);
  await page.getByText(/凭据配置记录/).click();
  await expect(page.getByText('本次配置请求已完成', { exact: true })).toBeVisible();
  expect(submissions).toEqual([{ path: '/sessions/initialize/credential-requests/service-key/value', body: { value: secret } }]);
  const storage = await page.evaluate(() => JSON.stringify({ local: { ...localStorage }, session: { ...sessionStorage }, history: history.state }));
  expect(storage).not.toContain(secret);
  await expect(page.locator('body')).not.toContainText(secret);
  await page.reload();
  await page.getByText(/凭据配置记录/).click();
  await expect(page.getByText('本次配置请求已完成', { exact: true })).toBeVisible();
  await expect(page.locator('body')).not.toContainText(secret);
});

test('Pilot cancels a credential request and removes its unsaved value', async ({ page }) => {
  await openSession(page);
  let current = credential();
  let saved = false;
  await page.route('**/sessions/*/messages', route => route.fulfill({ json: { messages: [] } }));
  await page.route('**/sessions/*/turns', route => route.fulfill({ json: { turns: [] } }));
  await page.route('**/sessions/*/credential-requests', route => route.fulfill({ json: { requests: [current] } }));
  await page.route('**/sessions/*/credential-requests/*/value', route => { saved = true; return route.abort(); });
  await page.route('**/sessions/*/credential-requests/*/cancel', route => {
    expect(route.request().postData()).toBeNull();
    current = { ...current, status: 'cancelled' };
    return route.fulfill({ json: { request: current } });
  });
  await page.goto('/');
  const form = page.getByRole('region', { name: '配置插件凭据' });
  await form.getByLabel('SERVICE_API_KEY', { exact: true }).fill('discarded-secret');
  await form.getByRole('button', { name: '取消', exact: true }).click();
  await expect(page.getByRole('region', { name: '配置插件凭据' }).locator('input')).toHaveCount(0);
  await page.getByText(/凭据配置记录/).click();
  await expect(page.getByText('配置已取消', { exact: true })).toBeVisible();
  expect(saved).toBe(false);
});

test('Pilot follows a delayed admitted continuation and shows the next Turn without a page reload', async ({ page }) => {
  await openSession(page);
  await page.route('**/sessions/*/credential-requests', route => route.fulfill({ json: { requests: [] } }));
  const source: Turn = { id: 'prepare', session: session.id, status: 'running', error: '', created_at: stamp, prompt: '准备 Python' };
  const next: Turn = { ...source, id: 'continued', prompt: null };
  let current: Turn = source;
  let polls = 0;
  let continued = false;
  const final = { ...source, status: 'completed', continuation: { status: 'scheduled' as const, request_id: 'continuation:prepare' } };
  await page.route('**/sessions/*/messages', route => route.fulfill({ json: { messages: continued ? [
    { role: 'user', text: '准备 Python' }, { role: 'assistant', text: '环境已就绪，继续运行测试。' },
  ] : [{ role: 'user', text: '准备 Python' }] } }));
  await page.route('**/sessions/*/turns', route => route.fulfill({ json: { turns: [current] } }));
  await page.route('**/sessions/*/turns/prepare', route => {
    polls++;
    if (polls >= 3) { current = next; continued = true; }
    return route.fulfill({ json: { turn: final, continuation: continued
      ? { status: 'admitted', request_id: 'continuation:prepare', turn: next.id }
      : final.continuation } });
  });
  await page.route('**/sessions/*/turns/continued', route => route.fulfill({ json: { turn: current, continuation: { status: 'none' } } }));
  await page.route(/\/sessions\/[^/]+\/turns\/[^/]+\/events/, route => {
    const isNext = route.request().url().includes('/continued/');
    current = isNext ? { ...next, status: 'completed' } : final;
    return route.fulfill({ contentType: 'text/event-stream', body: isNext
      ? `id: 1\ndata: ${JSON.stringify({ type: 'text-delta', id: 'after', delta: '环境已就绪，继续运行测试。' })}\n\nevent: turn\ndata: ${JSON.stringify(current)}\n\n`
      : `event: turn\ndata: ${JSON.stringify(final)}\n\n` });
  });
  await page.goto('/');
  await expect(page.getByText('正在应用配置，Pilot 将继续当前任务…')).toBeVisible();
  await expect(page.locator('.pilot-message.assistant')).toContainText('环境已就绪，继续运行测试。');
  await expect(page.getByText('正在应用配置，Pilot 将继续当前任务…')).toHaveCount(0);
  expect(polls).toBeGreaterThanOrEqual(3);
  await expect(page.getByLabel('发送给 Anchor Pilot')).toBeEnabled();
});
