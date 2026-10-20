import { expect, test, type Page } from '@playwright/test';
import { blockUnmockedApi } from './fixtures/api-fallback';

const sessionId = 'activity-history';
const sessionTitle = '保留的历史对话';
const historicalPrompt = '这是一条已经保存的问题';
const historicalReply = '这是一条已经保存的回复';
const sessionDraft = '这份草稿属于已有对话，尚未发送';
const graphName = 'activity-demo';

function gate() {
  let release!: () => void;
  const promise = new Promise<void>(resolve => { release = resolve; });
  return { promise, release };
}

function responseGate() {
  return { requested: gate(), response: gate() };
}

async function fixture(page: Page, newDraft = '') {
  await blockUnmockedApi(page);
  const state = {
    sessions: [{ id: sessionId, title: sessionTitle, status: 'active', waiting_reason: '',
      updated_at: '2026-10-20T08:00:00Z', approval: null }],
    messages: [{ role: 'user', text: historicalPrompt }, { role: 'assistant', text: historicalReply }],
    lists: null as ReturnType<typeof responseGate> | null,
    history: null as ReturnType<typeof responseGate> | null,
    requests: [] as string[],
    writes: [] as string[],
    errors: [] as string[],
    unexpected: [] as string[],
  };
  page.on('pageerror', error => state.errors.push(error.message));
  page.on('request', request => {
    if (request.method() !== 'GET') state.writes.push(`${request.method()} ${new URL(request.url()).pathname}`);
  });
  await page.addInitScript(({ selected, draft, unsent }) => {
    localStorage.setItem('anchor:view', 'pilot');
    localStorage.setItem('pilot:selected', selected);
    localStorage.setItem(`pilot:draft:${selected}`, draft);
    localStorage.setItem('pilot:draft:new', unsent);
  }, { selected: sessionId, draft: sessionDraft, unsent: newDraft });
  await page.route('**/*', async route => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (!/^\/(sessions|graphs|runs|timeline|plugins)(\/|$)/.test(path)) return route.fallback();
    state.requests.push(`${request.method()} ${path}`);
    if (request.method() !== 'GET') return route.fulfill({ status: 405, json: { error: 'Read-only browser fixture' } });
    if (path === '/graphs') return route.fulfill({ json: { graphs: [{ graph: graphName, running: null }] } });
    if (path === `/graphs/${graphName}`) return route.fulfill({ json: { definition: {
      objective: '用于页面活动回归的本地工作流', entry: 'seed', ops: { seed: { run: 'true' } },
      nodes: [{ id: 'seed', op: 'seed' }], edges: [],
    } } });
    if (path === '/runs') return route.fulfill({ json: { runs: [] } });
    if (path === '/timeline') return route.fulfill({ json: { runs: [], scheduled: [], schedules: [] } });
    if (path === '/plugins') return route.fulfill({ json: { plugins: [] } });
    if (path === '/sessions') {
      const held = state.lists;
      if (held) { held.requested.release(); await held.response.promise; }
      return route.fulfill({ json: { sessions: state.sessions } });
    }
    if (path === `/sessions/${sessionId}/messages`) {
      const held = state.history;
      if (held) { held.requested.release(); await held.response.promise; }
      return route.fulfill({ json: { messages: state.messages } });
    }
    if (path === `/sessions/${sessionId}/turns`) return route.fulfill({ json: { turns: [] } });
    if (path === `/sessions/${sessionId}/credential-requests`) return route.fulfill({ json: { requests: [] } });
    if (path === `/sessions/${sessionId}/plugins`) return route.fulfill({ json: { plugins: [] } });
    state.unexpected.push(path);
    return route.fallback();
  });
  await page.goto('/');
  await expect(page.locator('.pilot-message')).toHaveCount(2);
  await expect(page.locator('.pilot-message.user')).toContainText(historicalPrompt);
  await expect(page.locator('.pilot-message.assistant')).toContainText(historicalReply);
  await expect(page.getByLabel('发送给 Anchor Pilot')).toBeEnabled();
  await expect(page.getByLabel('发送给 Anchor Pilot')).toHaveValue(sessionDraft);
  return state;
}

async function openBoard(page: Page) {
  await page.getByRole('button', { name: '运行看板', exact: true }).click();
  await expect(page.getByRole('button', { name: '运行看板', exact: true })).toHaveAttribute('aria-pressed', 'true');
  await expect(page.locator('.pilot-page')).toBeHidden();
}

function expectReadOnly(state: Awaited<ReturnType<typeof fixture>>) {
  expect(state.writes).toEqual([]);
  expect(state.errors).toEqual([]);
  expect(state.unexpected).toEqual([]);
}

test('Pilot reuses message DOM and drafts before background refresh and reconciles refreshed facts without duplicates', async ({ page }) => {
  const state = await fixture(page);
  const messages = page.getByRole('log', { name: '对话消息' });
  const input = page.getByLabel('发送给 Anchor Pilot');
  const originalLog = await messages.elementHandle();
  const originalReply = await page.locator('.pilot-message.assistant').elementHandle();
  const originalInput = await input.elementHandle();
  const changes = await messages.evaluateHandle(element => {
    const counts: number[] = [];
    const observer = new MutationObserver(() => counts.push(element.querySelectorAll('.pilot-message').length));
    observer.observe(element, { childList: true, subtree: true });
    return { counts, stop: () => observer.disconnect() };
  });
  await openBoard(page);
  state.sessions[0].title = '服务端更新后的对话标题';
  state.messages.push({ role: 'assistant', text: '离开页面期间新增的已保存回复' });
  const lists = state.lists = responseGate();
  const history = state.history = responseGate();
  try {
    await page.getByRole('button', { name: 'Pilot', exact: true }).click();
    await lists.requested.promise;
    await expect(messages).toBeVisible();
    await expect(page.locator('.pilot-message')).toHaveCount(2);
    await expect(input).toHaveValue(sessionDraft);
    expect(await messages.evaluate((element, previous) => element === previous, originalLog)).toBe(true);
    expect(await page.locator('.pilot-message.assistant').evaluate((element, previous) => element === previous, originalReply)).toBe(true);
    expect(await input.evaluate((element, previous) => element === previous, originalInput)).toBe(true);
    await expect(page.locator('.pilot-chat-heading')).toContainText(sessionTitle);
    lists.response.release();
    await history.requested.promise;
    await expect(page.locator('.pilot-message')).toHaveCount(2);
    await expect(input).toHaveValue(sessionDraft);
    history.response.release();
    await expect(page.locator('.pilot-message')).toHaveCount(3);
    await expect(page.locator('.pilot-message.user')).toHaveCount(1);
    await expect(page.locator('.pilot-message.assistant')).toHaveCount(2);
    await expect(page.locator('.pilot-message.assistant').first()).toContainText(historicalReply);
    await expect(page.locator('.pilot-message.assistant').last()).toContainText('离开页面期间新增的已保存回复');
    await expect(page.locator('.pilot-chat-heading')).toContainText('服务端更新后的对话标题');
    await expect(input).toHaveValue(sessionDraft);
    await expect(input).toBeEnabled();
    const counts = await changes.evaluate(value => { value.stop(); return value.counts; });
    expect(counts.length).toBeGreaterThan(0);
    expect(counts.every(count => count >= 2 && count <= 3)).toBe(true);
    expectReadOnly(state);
  } finally {
    lists.response.release(); history.response.release();
    await changes.evaluate(value => value.stop());
  }
});

test('Pilot clears a deleted session history and restores only the new-conversation input context on return', async ({ page }) => {
  const newDraft = '这份草稿只属于新对话';
  const state = await fixture(page, newDraft);
  await openBoard(page);
  state.sessions = [];
  const priorHistoryRequests = state.requests.filter(request => request === `GET /sessions/${sessionId}/messages`).length;
  const lists = state.lists = responseGate();
  try {
    await page.getByRole('button', { name: 'Pilot', exact: true }).click();
    await lists.requested.promise;
    await expect(page.locator('.pilot-message')).toHaveCount(2);
    lists.response.release();
    await expect(page.locator('.pilot-session')).toHaveCount(0);
    await expect(page.locator('.pilot-message')).toHaveCount(0);
    await expect(page.locator('.pilot-chat-heading')).toContainText('新对话');
    await expect(page.locator('.pilot-chat-heading strong')).not.toHaveAttribute('title', sessionId);
    await expect(page.getByLabel('发送给 Anchor Pilot')).toHaveValue(newDraft);
    await expect(page.getByLabel('发送给 Anchor Pilot')).toBeEnabled();
    expect(await page.evaluate(() => localStorage.getItem('pilot:selected'))).toBe('');
    expect(state.requests.filter(request => request === `GET /sessions/${sessionId}/messages`)).toHaveLength(priorHistoryRequests);
    await page.getByLabel('发送给 Anchor Pilot').fill('已经在新对话上下文继续编辑');
    await openBoard(page);
    await page.getByRole('button', { name: 'Pilot', exact: true }).click();
    await expect(page.locator('.pilot-message')).toHaveCount(0);
    await expect(page.getByLabel('发送给 Anchor Pilot')).toHaveValue('已经在新对话上下文继续编辑');
    await expect(page.getByLabel('发送给 Anchor Pilot')).toBeEnabled();
    expectReadOnly(state);
  } finally { lists.response.release(); }
});

test('graph assistance opens a fresh referenced draft after Pilot has already visited an existing session', async ({ page }) => {
  const state = await fixture(page);
  await page.getByRole('button', { name: '图编排', exact: true }).click();
  await expect(page.locator('.pilot-page')).toBeHidden();
  await expect(page.getByLabel('当前工作流')).toHaveValue(graphName);
  const assist = page.getByRole('button', { name: '在 Pilot 中协助', exact: true });
  await expect(assist).toBeEnabled();
  state.sessions[0].title = '协助入口返回时重新读取的历史标题';
  const lists = state.lists = responseGate();
  const priorHistoryRequests = state.requests.filter(request => request === `GET /sessions/${sessionId}/messages`).length;
  try {
    await assist.click();
    await lists.requested.promise;
    const input = page.getByLabel('发送给 Anchor Pilot');
    const referencedDraft = `请检查这个工作流并协助完善，先读取当前定义，再说明建议。\n[工作流 ${graphName}](#anchor/graph/${graphName})`;
    await expect(page.locator('.pilot-page')).toBeVisible();
    await expect(input).toHaveValue(referencedDraft);
    await expect(page.locator('.pilot-message')).toHaveCount(0);
    await expect(page.locator('.pilot-chat-heading')).toContainText('新对话');
    await expect(page.locator('.pilot-session[aria-current="page"]')).toHaveCount(0);
    expect(await page.evaluate(() => localStorage.getItem('pilot:selected'))).toBe('');
    lists.response.release();
    await expect(page.locator('.pilot-session')).toContainText('协助入口返回时重新读取的历史标题');
    await expect(input).toBeEnabled();
    await expect(input).toHaveValue(referencedDraft);
    await expect(page.locator('.pilot-message')).toHaveCount(0);
    await expect(page.locator('.pilot-chat-heading')).toContainText('新对话');
    await expect(page.locator('.pilot-session[aria-current="page"]')).toHaveCount(0);
    await expect(page.locator('.pilot-session')).toHaveCount(1);
    expect(state.requests.filter(request => request === `GET /sessions/${sessionId}/messages`)).toHaveLength(priorHistoryRequests);
    expectReadOnly(state);
  } finally { lists.response.release(); }
});
