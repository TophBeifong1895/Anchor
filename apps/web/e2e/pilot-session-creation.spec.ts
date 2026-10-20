import { expect, test } from '@playwright/test';
import { blockUnmockedApi } from './fixtures/api-fallback';

test('Pilot retries a lost session creation after reload without creating another conversation', async ({ page }) => {
  await blockUnmockedApi(page);
  await page.addInitScript(() => localStorage.setItem('anchor:view', 'pilot'));
  const session = { id: 'created', title: '', status: 'active', updated_at: '2026-10-20T00:00:00Z', approval: null };
  const creationKeys: (string | undefined)[] = [];
  await page.route('**/graphs', route => route.fulfill({ json: { graphs: [] } }));
  await page.route('**/sessions', route => {
    if (route.request().method() !== 'POST') return route.fulfill({ json: { sessions: creationKeys.length ? [session] : [] } });
    creationKeys.push(route.request().headers()['idempotency-key']);
    if (creationKeys.length === 1) return route.abort('failed');
    return route.fulfill({ json: { session } });
  });
  await page.route('**/sessions/*/messages', route => route.fulfill({ json: { messages: [] } }));
  await page.route('**/sessions/*/turns', route => route.fulfill({ json: { turns: [] } }));
  await page.route('**/sessions/*/plugins', route => route.fulfill({ json: { plugins: [] } }));
  await page.route('**/sessions/*/credential-requests', route => route.fulfill({ json: { requests: [] } }));
  await page.goto('/');
  await page.getByRole('button', { name: '新对话', exact: true }).click();
  await expect(page.locator('.pilot-error')).toBeVisible();
  expect(creationKeys).toHaveLength(1);
  expect(creationKeys[0]).toBeTruthy();
  await page.reload();
  await page.getByRole('button', { name: '新对话', exact: true }).click();
  await expect(page.getByLabel('发送给 Anchor Pilot')).toBeEnabled();
  await expect.poll(() => page.evaluate(() => localStorage.getItem('pilot:selected'))).toBe(session.id);
  expect(creationKeys).toEqual([creationKeys[0], creationKeys[0]]);
  expect(await page.evaluate(() => Object.keys(localStorage).filter(key => key.startsWith('pilot:creation:')))).toEqual([]);
});

test('Pilot uses idempotent creation when the first message starts a conversation', async ({ page }) => {
  await blockUnmockedApi(page);
  await page.addInitScript(() => localStorage.setItem('anchor:view', 'pilot'));
  const session = { id: 'first-message', title: '', status: 'active', updated_at: '2026-10-20T00:00:00Z', approval: null };
  const creationKeys: (string | undefined)[] = [];
  const submissions: { path: string; message: string }[] = [];
  await page.route('**/graphs', route => route.fulfill({ json: { graphs: [] } }));
  await page.route('**/sessions', route => {
    if (route.request().method() !== 'POST') return route.fulfill({ json: { sessions: creationKeys.length ? [session] : [] } });
    creationKeys.push(route.request().headers()['idempotency-key']);
    return route.fulfill({ json: { session } });
  });
  await page.route('**/sessions/*/messages', route => route.fulfill({ json: { messages: [] } }));
  await page.route('**/sessions/*/turns', route => {
    if (route.request().method() !== 'POST') return route.fulfill({ json: { turns: [] } });
    submissions.push({ path: new URL(route.request().url()).pathname, message: route.request().postDataJSON().message });
    return route.fulfill({ status: 422, json: { error: 'fixture turn not accepted' } });
  });
  await page.route('**/sessions/*/plugins', route => route.fulfill({ json: { plugins: [] } }));
  await page.route('**/sessions/*/credential-requests', route => route.fulfill({ json: { requests: [] } }));
  await page.goto('/');
  const input = page.getByLabel('发送给 Anchor Pilot');
  await expect(input).toBeEnabled();
  await input.fill('第一条消息');
  await input.press('Enter');
  await expect(page.locator('.pilot-error')).toContainText('fixture turn not accepted');
  expect(creationKeys).toHaveLength(1);
  expect(creationKeys[0]).toBeTruthy();
  expect(submissions).toEqual([{ path: '/sessions/first-message/turns', message: '第一条消息' }]);
  await expect(input).toHaveValue('第一条消息');
});
