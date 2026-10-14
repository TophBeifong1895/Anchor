import { expect, test } from '@playwright/test';
import { blockUnmockedApi } from './fixtures/api-fallback';

test.beforeEach(async ({ page }) => { await blockUnmockedApi(page); });

for (const hasQuestions of [false, true]) {
  test(`Pilot ${hasQuestions ? 'keeps saved questions and answers in the conversation' : 'has no empty question history'} across conversation redraws`, async ({ page }) => {
    const session = { id: 'history', title: '历史提问展示', status: 'active', updated_at: '2026-10-12T12:00:00Z', approval: null };
    const turns = Array.from({ length: 14 }, (_, index) => ({
      id: `turn-${index}`, session: session.id, request_id: `request-${index}`, status: 'completed',
      error: '', created_at: session.updated_at, prompt: '查看状态',
    }));
    const saved = { id: 'question-1', session: session.id, turn: turns[1].id,
      message: '需要哪一份报告？', requested_schema: { type: 'object', properties: { report: { type: 'string', title: '报告名称' } } },
      status: 'answered', answer: { action: 'accept', content: { report: '季度报告' } } };
    const messages = [
      { role: 'user', text: '查看状态' }, { role: 'assistant', text: '提问前的说明。' },
      ...(hasQuestions ? [
        { role: 'assistant', text: saved.message, question: saved },
        { role: 'user', text: '报告名称：季度报告', question_answer: saved },
      ] : []),
      { role: 'assistant', text: '回答后的回复。' },
      { role: 'user', text: '查看状态' }, { role: 'assistant', text: '状态已核对。' },
    ];
    const questionRequests = new Set<string>();
    const problems: string[] = [];
    page.on('pageerror', error => problems.push(error.message));
    page.on('console', message => {
      if (message.type() === 'error') problems.push(message.text());
    });
    await page.addInitScript(() => {
      localStorage.setItem('anchor:view', 'pilot');
      localStorage.setItem('pilot:selected', 'history');
    });
    await page.route('**/graphs', route => route.fulfill({ json: { graphs: [] } }));
    await page.route('**/runs', route => route.fulfill({ json: { runs: [] } }));
    await page.route('**/timeline*', route => route.fulfill({ json: { runs: [], scheduled: [], schedules: [] } }));
    await page.route('**/sessions', route => route.fulfill({ json: { sessions: [session] } }));
    await page.route('**/sessions/*/messages', route => route.fulfill({ json: { messages } }));
    await page.route('**/sessions/*/plugins', route => route.fulfill({ json: { plugins: [] } }));
    await page.route('**/sessions/*/credential-requests', route => route.fulfill({ json: { requests: [] } }));
    await page.route('**/sessions/*/turns', route => route.fulfill({ json: { turns } }));
    await page.route('**/sessions/*/turns/turn-0', route => route.fulfill({ json: { turn: turns[0], continuation: { status: 'none' } } }));
    await page.route('**/sessions/*/turns/*/questions', route => {
      const turn = new URL(route.request().url()).pathname.split('/').at(-2)!;
      questionRequests.add(turn);
      return route.fulfill({ json: { questions: hasQuestions && turn === saved.turn ? [saved] : [] } });
    });
    await page.route(/\/sessions\/[^/]+\/turns\/[^/]+\/events/, route => route.fulfill({
      contentType: 'text/event-stream', body: `event: turn\ndata: ${JSON.stringify(turns[0])}\n\n`,
    }));
    await page.goto('/');
    await expect(page.getByLabel('发送给 Anchor Pilot')).toBeEnabled();
    await expect(page.getByText('回复完成', { exact: true })).toBeVisible();
    const histories = page.locator('details').filter({ has: page.locator('summary', { hasText: /^历史提问/ }) });
    // Typing and revisiting the same Session redraw siblings of the history component. Duplicate
    // React keys used to leave another empty disclosure in the DOM on each redraw.
    for (let index = 0; index < 6; index++) {
      await page.getByLabel('发送给 Anchor Pilot').fill(`保留草稿 ${index}`);
      await page.locator('.pilot-session[title="history"]').click();
      await expect(page.getByText('回复完成', { exact: true })).toBeVisible();
      await expect(histories).toHaveCount(0);
      await expect(page.locator('.pilot-message')).toHaveCount(messages.length);
    }
    await page.reload();
    await expect(page.getByText('回复完成', { exact: true })).toBeVisible();
    await expect(page.locator('.pilot-message')).toHaveCount(messages.length);
    expect(await page.locator('.pilot-message').allTextContents()).toEqual(messages.map(message =>
      `${message.role === 'user' ? '你' : 'Anchor Pilot'}${message.text}复制`));
    if (hasQuestions) {
      const transcript = page.getByRole('log', { name: '对话消息' });
      await expect(transcript.getByText(saved.message, { exact: true })).toBeVisible();
      await expect(transcript.getByText('报告名称：季度报告', { exact: true })).toBeVisible();
      await expect(transcript.locator('form')).toHaveCount(0);
    }
    expect(questionRequests.size).toBe(0);
    await expect(page.getByText('没有已保存的历史提问。', { exact: true })).toHaveCount(0);
    expect(problems).toEqual([]);
  });
}

test('Pilot keeps one live question form on reload and puts the saved answer in the conversation', async ({ page }) => {
  const session = { id: 'live-question', title: '对话内提问', status: 'active', updated_at: '2026-10-13T00:00:00Z', approval: null };
  const turn = { id: 'turn-live', session: session.id, request_id: 'request', status: 'running', error: '', created_at: session.updated_at, prompt: '询问报告名称' };
  const pending = { id: 'question-live', session: session.id, turn: turn.id, message: '需要哪份报告？', status: 'pending', answer: null,
    requested_schema: { type: 'object', properties: { report: { type: 'string', title: '报告名称' } }, required: ['report'] } };
  const saved = { ...pending, status: 'answered', answer: { action: 'accept', content: { report: '季度报告' } } };
  let answered = false;
  let finalStatus = 'completed';
  await page.addInitScript(() => {
    localStorage.setItem('anchor:view', 'pilot');
    localStorage.setItem('pilot:selected', 'live-question');
  });
  await page.route('**/graphs', route => route.fulfill({ json: { graphs: [] } }));
  await page.route('**/runs', route => route.fulfill({ json: { runs: [] } }));
  await page.route('**/timeline*', route => route.fulfill({ json: { runs: [], scheduled: [], schedules: [] } }));
  await page.route('**/sessions', route => route.fulfill({ json: { sessions: [session] } }));
  await page.route('**/sessions/*/plugins', route => route.fulfill({ json: { plugins: [] } }));
  await page.route('**/sessions/*/credential-requests', route => route.fulfill({ json: { requests: [] } }));
  await page.route('**/sessions/*/turns', route => route.fulfill({ json: { turns: [{ ...turn, status: answered ? finalStatus : 'running' }] } }));
  await page.route('**/sessions/*/turns/turn-live', route => route.fulfill({ json: { turn: { ...turn, status: answered ? finalStatus : 'running' }, continuation: { status: 'none' } } }));
  await page.route('**/sessions/*/messages', route => route.fulfill({ json: { messages: [
    { role: 'user', text: turn.prompt },
    { role: 'assistant', text: pending.message, question: answered ? saved : pending },
    ...(answered ? [{ role: 'user', text: '报告名称：季度报告', question_answer: saved }, { role: 'assistant', text: '我会使用季度报告。' }] : []),
  ] } }));
  await page.route('**/sessions/*/turns/*/questions', route => route.fulfill({ json: { questions: [answered ? saved : pending] } }));
  await page.route('**/sessions/*/turns/*/questions/*/answer', route => {
    expect(route.request().postDataJSON()).toEqual(saved.answer);
    answered = true;
    return route.fulfill({ json: { question: saved } });
  });
  await page.route(/\/sessions\/[^/]+\/turns\/[^/]+\/events/, route => route.fulfill({
    contentType: 'text/event-stream', body: answered
      ? `id: 2\ndata: ${JSON.stringify({ type: 'text-delta', delta: '我会使用季度报告。' })}\n\nevent: turn\ndata: ${JSON.stringify({ ...turn, status: finalStatus })}\n\n`
      : `id: 1\ndata: ${JSON.stringify({ type: 'question', question: pending })}\n\n`,
  }));
  await page.goto('/');
  const question = page.getByRole('region', { name: 'Pilot 提问' });
  await expect(question).toHaveCount(1);
  await expect(page.getByText(pending.message, { exact: true })).toHaveCount(1);
  await expect(page.getByLabel('发送给 Anchor Pilot')).toBeDisabled();
  await page.reload();
  await expect(question).toHaveCount(1);
  await question.getByLabel('报告名称').fill('季度报告');
  await question.getByRole('button', { name: '回答', exact: true }).click();
  await expect(page.getByText('回复完成', { exact: true })).toBeVisible();
  await expect(question).toHaveCount(0);
  await expect(page.locator('.pilot-message')).toHaveCount(4);
  await expect(page.getByLabel('发送给 Anchor Pilot')).toBeEnabled();
  await page.reload();
  await expect(page.locator('.pilot-message')).toHaveCount(4);
  await expect(page.getByText(pending.message, { exact: true })).toHaveCount(1);
  await expect(page.getByText('报告名称：季度报告', { exact: true })).toHaveCount(1);
  await expect(page.locator('summary').filter({ hasText: '历史提问' })).toHaveCount(0);
  // A Turn can also end after recording an answer without completing its remaining work.
  finalStatus = 'interrupted';
  await page.reload();
  await expect(page.getByText('执行中断', { exact: true })).toBeVisible();
  await expect(question).toHaveCount(0);
  await expect(page.locator('.pilot-message')).toHaveCount(4);
  await expect(page.getByText(pending.message, { exact: true })).toHaveCount(1);
});
