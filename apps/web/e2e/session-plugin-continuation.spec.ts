import { expect, test, type Page } from '@playwright/test';
import type { Turn } from '../src/PilotTurn';

const stamp = '2026-10-13T08:00:00Z';
const source: Turn = { id: 'source', session: 'pending', request_id: 'original', status: 'completed',
  error: '', created_at: stamp, prompt: '继续原任务' };

async function openPending(page: Page) {
  await page.addInitScript(() => {
    localStorage.setItem('anchor:view', 'pilot');
    localStorage.setItem('pilot:selected', 'pending');
  });
  await page.route('**/graphs', route => route.fulfill({ json: { graphs: [] } }));
  await page.route('**/runs', route => route.fulfill({ json: { runs: [] } }));
  await page.route('**/timeline*', route => route.fulfill({ json: { runs: [], scheduled: [], schedules: [] } }));
  await page.route('**/sessions', route => route.fulfill({ json: { sessions: [
    { id: 'pending', title: '等待续跑', status: 'active', updated_at: stamp, approval: null },
    { id: 'other', title: '另一段对话', status: 'active', updated_at: stamp, approval: null },
  ] } }));
  await page.route('**/sessions/*/plugins', route => route.fulfill({ json: { plugins: [] } }));
  await page.route('**/sessions/*/credential-requests', route => route.fulfill({ json: { requests: [] } }));
  await page.route('**/sessions/*/turns/*/questions', route => route.fulfill({ json: { questions: [] } }));
  await page.route(/\/sessions\/[^/]+\/turns\/[^/]+\/events/, route => route.fulfill({
    contentType: 'text/event-stream', body: `event: turn\ndata: ${JSON.stringify(source)}\n\n`,
  }));
  await page.route('**/sessions/other/messages', route => route.fulfill({ json: { messages: [{ role: 'assistant', text: '另一段历史' }] } }));
  await page.route('**/sessions/other/turns', route => route.fulfill({ json: { turns: [] } }));
}

test('reopening a completed scheduled source follows it and retains a stop control until cancellation', async ({ page }) => {
  await openPending(page);
  let state = 'scheduled';
  let stopped = 0;
  let submitted = 0;
  await page.route('**/sessions/pending/messages', route => route.fulfill({ json: { messages: [
    { role: 'user', text: source.prompt }, { role: 'assistant', text: '配置已保存' },
  ] } }));
  await page.route('**/sessions/pending/turns', route => {
    if (route.request().method() === 'POST') submitted++;
    return route.fulfill({ json: { turns: [source] } });
  });
  await page.route('**/sessions/pending/turns/source', route => route.fulfill({ json: { turn: source,
    continuation: { status: state, request_id: 'continuation:source' } } }));
  await page.route('**/sessions/pending/stop', route => {
    stopped++; state = 'cancelled';
    return route.fulfill({ status: 202, json: { stopping: true } });
  });
  await page.goto('/');
  const input = page.getByLabel('发送给 Anchor Pilot');
  await expect(input).toBeEnabled();
  await expect(page.getByText('正在应用配置，Pilot 将继续当前任务…')).toBeVisible();
  await expect(page.getByRole('button', { name: '停止', exact: true })).toBeEnabled();
  await input.fill('待发送的新任务');
  await page.getByRole('button', { name: '停止', exact: true }).click();
  await expect(page.getByText('正在应用配置，Pilot 将继续当前任务…')).toHaveCount(0);
  await expect(page.getByRole('button', { name: '停止', exact: true })).toHaveCount(0);
  await expect(input).toHaveValue('待发送的新任务');
  expect(stopped).toBe(1); expect(submitted).toBe(0);
  await page.reload();
  await expect(input).toBeEnabled();
  await expect(page.getByText('正在应用配置，Pilot 将继续当前任务…')).toHaveCount(0);
  expect(submitted).toBe(0);
});

test('a newer user Turn supersedes a pending source and a late source poll cannot pull it back', async ({ page }) => {
  await openPending(page);
  let current = source;
  let detailReads = 0;
  let releasePoll = () => {};
  const oldPoll = new Promise<void>(resolve => { releasePoll = resolve; });
  const next: Turn = { ...source, id: 'new-user', request_id: 'user-newer', status: 'running', prompt: '新的明确任务' };
  await page.route('**/sessions/pending/messages', route => route.fulfill({ json: { messages: current.id === source.id
    ? [{ role: 'user', text: source.prompt }, { role: 'assistant', text: '旧任务保存完毕' }]
    : [{ role: 'user', text: next.prompt }, { role: 'assistant', text: '正在处理新的明确任务' }] } }));
  await page.route('**/sessions/pending/turns', route => {
    if (route.request().method() === 'POST') {
      current = next;
      return route.fulfill({ status: 202, json: { turn: next } });
    }
    return route.fulfill({ json: { turns: [current] } });
  });
  await page.route('**/sessions/pending/turns/source', async route => {
    detailReads++;
    if (detailReads === 1) return route.fulfill({ json: { turn: source, continuation: { status: 'scheduled' } } });
    await oldPoll;
    await route.fulfill({ json: { turn: source, continuation: { status: 'admitted', turn: 'obsolete-auto' } } }).catch(() => {});
  });
  await page.route('**/sessions/pending/turns/new-user/events*', route => {
    current = { ...next, status: 'completed' };
    return route.fulfill({ contentType: 'text/event-stream', body:
      `id: 1\ndata: ${JSON.stringify({ type: 'text-delta', id: 'new-answer', delta: '正在处理新的明确任务' })}\n\nevent: turn\ndata: ${JSON.stringify(current)}\n\n` });
  });
  await page.route('**/sessions/pending/turns/new-user', route => route.fulfill({ json: { turn: current, continuation: { status: 'none' } } }));
  await page.goto('/');
  await expect(page.getByText('正在应用配置，Pilot 将继续当前任务…')).toBeVisible();
  await expect.poll(() => detailReads).toBeGreaterThanOrEqual(2);
  const input = page.getByLabel('发送给 Anchor Pilot');
  await input.fill('新的明确任务'); await input.press('Enter');
  await expect(page.locator('.pilot-message.assistant').last()).toContainText('正在处理新的明确任务');
  releasePoll();
  await expect(input).toBeEnabled();
  await expect(page.getByText('正在应用配置，Pilot 将继续当前任务…')).toHaveCount(0);
  await page.locator('.pilot-session[title="other"]').click();
  await expect(page.locator('.pilot-message.assistant')).toContainText('另一段历史');
  await expect(page.locator('.pilot-message.assistant')).not.toContainText('旧任务保存完毕');
  await expect(page.getByText('正在应用配置，Pilot 将继续当前任务…')).toHaveCount(0);
});

test('reopening across admission follows a successor that was already admitted at the first detail read', async ({ page }) => {
  await openPending(page);
  const next: Turn = { ...source, id: 'already-admitted', request_id: 'continuation:source', status: 'running', prompt: null };
  let listReads = 0;
  let successorReads = 0;
  await page.route('**/sessions/pending/turns', route => {
    listReads++;
    return route.fulfill({ json: { turns: listReads === 1 ? [source] : [next, source] } });
  });
  await page.route('**/sessions/pending/messages', route => route.fulfill({ json: { messages: [
    { role: 'user', text: source.prompt }, { role: 'assistant', text: listReads > 1 ? '接续回合已经开始' : '源回合已完成' },
  ] } }));
  await page.route('**/sessions/pending/turns/source', route => route.fulfill({ json: { turn: source,
    continuation: { status: 'admitted', turn: next.id } } }));
  await page.route('**/sessions/pending/turns/already-admitted', route => {
    successorReads++;
    return route.fulfill({ json: { turn: next, continuation: { status: 'none' } } });
  });
  await page.route('**/sessions/pending/turns/already-admitted/events*', route => route.fulfill({
    contentType: 'text/event-stream', body: ': connected\n\n',
  }));
  await page.goto('/');
  await expect(page.locator('.pilot-message.assistant').last()).toContainText('接续回合已经开始');
  await expect(page.getByLabel('发送给 Anchor Pilot')).toBeDisabled();
  await expect(page.getByRole('button', { name: '停止', exact: true })).toBeEnabled();
  await expect(page.locator('.pilot-turn .pilot-thinking')).toBeVisible();
  expect(successorReads).toBeGreaterThanOrEqual(1);
  expect(listReads).toBeGreaterThanOrEqual(2);
  await page.locator('.pilot-session[title="other"]').click();
  await expect(page.locator('.pilot-message.assistant')).toContainText('另一段历史');
  await expect(page.getByLabel('发送给 Anchor Pilot')).toBeEnabled();
});
