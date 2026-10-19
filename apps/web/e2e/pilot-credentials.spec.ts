import { expect, test, type Page } from '@playwright/test';

const session = 'pilot-private-credentials';
const request = { id: 'credential-1', session, turn: 'old-turn', plugin: 'service', server: 'main', field: 'SERVICE_TOKEN', status: 'configured' };
const turn = (id: string, status = 'completed') => ({ id, session, status, error: '', prompt: '配置服务', created_at: '2026-10-19T10:00:00Z' });
const unexpectedRequests = new WeakMap<Page, string[]>();

test.beforeEach(async ({ page }) => {
  const unexpected: string[] = [];
  unexpectedRequests.set(page, unexpected);
  await page.route('**/*', async route => {
    const path = new URL(route.request().url()).pathname;
    if (/^\/(graphs|runs|timeline|sessions|plugins|settings)(\/|$)/.test(path)) {
      unexpected.push(`${route.request().method()} ${path}`);
      return route.fulfill({ status: 500, json: { error: `Unexpected fixture request: ${path}` } });
    }
    return route.continue();
  });
  await page.route('**/graphs', route => route.fulfill({ json: { graphs: [] } }));
  await page.route('**/sessions', route => route.fulfill({ json: { sessions: [
    { id: session, title: '凭据会话', status: 'active', updated_at: '2026-10-19T10:00:00Z', approval: null },
    { id: 'pilot-private-other', title: '另一段对话', status: 'active', updated_at: '2026-10-18T10:00:00Z', approval: null },
  ] } }));
  await page.route('**/sessions/*/plugins', route => route.fulfill({ json: { plugins: [] } }));
  await page.route('**/sessions/pilot-private-other/messages', route => route.fulfill({ json: { messages: [] } }));
  await page.route('**/sessions/pilot-private-other/turns', route => route.fulfill({ json: { turns: [] } }));
  await page.route('**/sessions/pilot-private-other/credential-requests', route => route.fulfill({ json: { requests: [] } }));
  await page.addInitScript(id => {
    localStorage.setItem('anchor:view', 'pilot'); localStorage.setItem('pilot:selected', id);
  }, session);
});

test.afterEach(async ({ page }) => {
  expect(unexpectedRequests.get(page) ?? [], 'Every management API request must be mocked by this fixture').toEqual([]);
});

test('settled credential cards collapse at their original turn, including after reload and navigation', async ({ page }) => {
  const latest = turn('new-turn');
  await page.route(`**/sessions/${session}/messages`, route => route.fulfill({ json: { messages: [
    { role: 'user', text: '配置旧服务', turn: 'old-turn' },
    { role: 'assistant', text: '旧服务配置已处理', turn: 'old-turn' },
    { role: 'user', text: '继续新的问题', turn: latest.id },
    { role: 'assistant', text: '这是最新回复', turn: latest.id },
  ] } }));
  await page.route(`**/sessions/${session}/turns`, route => route.fulfill({ json: { turns: [latest, turn('old-turn')] } }));
  await page.route(`**/sessions/${session}/turns/${latest.id}`, route => route.fulfill({ json: { turn: latest } }));
  await page.route(`**/sessions/${session}/turns/${latest.id}/events?*`, route => route.fulfill({ contentType: 'text/event-stream', body: `event: turn\ndata: ${JSON.stringify(latest)}\n\n` }));
  await page.route(`**/sessions/${session}/credential-requests`, route => route.fulfill({ json: { requests: [
    request, { ...request, id: 'cancelled', status: 'cancelled' }, { ...request, id: 'interrupted', status: 'interrupted' },
  ] } }));
  await page.goto('/');
  const history = page.locator('[data-credential-turn="old-turn"] .pilot-credential-history');
  await expect(history.locator('summary')).toHaveText('凭据配置记录 · 3 项');
  await expect(page.getByRole('region', { name: '配置插件凭据' })).toHaveCount(0);
  await expect(page.getByText('本次配置请求已完成', { exact: true })).not.toBeVisible();
  expect(await history.evaluate(element => Boolean(element.compareDocumentPosition(
    [...document.querySelectorAll('.pilot-message')].find(message => message.textContent?.includes('这是最新回复'))!,
  ) & Node.DOCUMENT_POSITION_FOLLOWING))).toBe(true);
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(history.locator('summary')).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.setViewportSize({ width: 1440, height: 1000 });
  await history.locator('summary').click();
  await expect(page.getByText('本次配置请求已完成', { exact: true })).toBeVisible();
  await expect(page.getByText('配置已取消', { exact: true })).toBeVisible();
  await expect(page.getByText('配置请求已中断', { exact: true })).toBeVisible();
  await page.reload();
  await expect(history.locator('summary')).toBeVisible();
  await expect(page.getByText('本次配置请求已完成', { exact: true })).not.toBeVisible();
  await page.locator('.pilot-session').filter({ hasText: '另一段对话' }).click();
  await expect(page.locator('.pilot-credential-history')).toHaveCount(0);
  await page.locator('.pilot-session').filter({ hasText: '凭据会话' }).click();
  await expect(history.locator('summary')).toBeVisible();
  await expect(page.getByText('本次配置请求已完成', { exact: true })).not.toBeVisible();
});

test('older history without turn metadata keeps records collapsed without guessing their position', async ({ page }) => {
  await page.route(`**/sessions/${session}/messages`, route => route.fulfill({ json: { messages: [
    { role: 'user', text: '重复的输入' }, { role: 'assistant', text: '旧版本的历史回复' },
    { role: 'user', text: '重复的输入' }, { role: 'assistant', text: '最新的历史回复' },
  ] } }));
  await page.route(`**/sessions/${session}/turns`, route => route.fulfill({ json: { turns: [] } }));
  await page.route(`**/sessions/${session}/credential-requests`, route => route.fulfill({ json: { requests: [request] } }));
  await page.goto('/');
  await expect(page.locator('.pilot-credential-history summary')).toHaveText('凭据配置记录 · 1 项');
  await expect(page.getByText('本次配置请求已完成', { exact: true })).not.toBeVisible();
  await expect(page.locator('[data-credential-turn]')).toHaveCount(0);
  await expect(page.getByText('最新的历史回复', { exact: true })).toBeVisible();
});

test('pending credentials remain actionable and settle into collapsed history after save', async ({ page }) => {
  let current = turn('live-turn', 'running');
  let credential = { ...request, turn: current.id, status: 'pending' };
  let finish: () => void = () => undefined;
  const completed = new Promise<void>(resolve => { finish = resolve; });
  await page.route(`**/sessions/${session}/messages`, route => route.fulfill({ json: { messages: current.status === 'running'
    ? [{ role: 'user', text: '配置服务', turn: current.id }]
    : [{ role: 'user', text: '配置服务', turn: current.id }, { role: 'assistant', text: '配置接续完成', turn: current.id }] } }));
  await page.route(`**/sessions/${session}/turns`, route => route.fulfill({ json: { turns: [current] } }));
  await page.route(`**/sessions/${session}/turns/${current.id}`, route => route.fulfill({ json: { turn: current } }));
  await page.route(`**/sessions/${session}/turns/${current.id}/questions`, route => route.fulfill({ json: { questions: [] } }));
  await page.route(`**/sessions/${session}/turns/${current.id}/events?*`, async route => {
    await completed;
    await route.fulfill({ contentType: 'text/event-stream', body: `event: turn\ndata: ${JSON.stringify(current)}\n\n` });
  });
  await page.route(`**/sessions/${session}/credential-requests`, route => route.fulfill({ json: { requests: [credential] } }));
  await page.route(`**/sessions/${session}/credential-requests/${request.id}/value`, async route => {
    expect(route.request().postDataJSON()).toEqual({ value: 'browser-only-fixture-token' });
    credential = { ...credential, status: 'configured' }; current = { ...current, status: 'completed' };
    await route.fulfill({ json: { request: credential } }); finish();
  });
  await page.goto('/');
  const form = page.getByRole('region', { name: '配置插件凭据' });
  await expect(form).toBeVisible();
  await form.getByLabel('SERVICE_TOKEN', { exact: true }).fill('browser-only-fixture-token');
  await form.getByRole('button', { name: '保存凭据并继续' }).click();
  await expect(page.getByText('配置接续完成', { exact: true })).toBeVisible();
  await expect(page.locator('[data-credential-turn="live-turn"] summary')).toHaveText('凭据配置记录 · 1 项');
  await expect(page.getByRole('button', { name: '保存凭据并继续' })).toHaveCount(0);
  await expect(page.getByText('本次配置请求已完成', { exact: true })).not.toBeVisible();
  expect(await page.evaluate(() => JSON.stringify({ local: { ...localStorage }, session: { ...sessionStorage } }))).not.toContain('browser-only-fixture-token');
});
