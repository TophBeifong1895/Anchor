import { expect, test } from '@playwright/test';

for (const hasQuestions of [false, true]) {
  test(`Pilot ${hasQuestions ? 'keeps one collapsed saved question history' : 'hides empty question history'} across conversation redraws`, async ({ page }) => {
    const session = { id: 'history', title: '历史提问展示', status: 'active', updated_at: '2026-10-12T12:00:00Z', approval: null };
    const turns = Array.from({ length: 14 }, (_, index) => ({
      id: `turn-${index}`, session: session.id, request_id: `request-${index}`, status: 'completed',
      error: '', created_at: session.updated_at, prompt: '查看状态',
    }));
    const saved = { id: 'question-1', session: session.id, turn: turns[1].id,
      message: '需要哪一份报告？', requested_schema: { type: 'object', properties: { report: { type: 'string', title: '报告名称' } } },
      status: 'answered', answer: { action: 'accept', content: { report: '季度报告' } } };
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
    await page.route('**/sessions/*/messages', route => route.fulfill({ json: { messages: [
      { role: 'user', text: '查看状态' }, { role: 'assistant', text: '状态已核对。' },
    ] } }));
    await page.route('**/sessions/*/plugins', route => route.fulfill({ json: { plugins: [] } }));
    await page.route('**/sessions/*/credential-requests', route => route.fulfill({ json: { requests: [] } }));
    await page.route('**/sessions/*/turns', route => route.fulfill({ json: { turns } }));
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
      await expect(histories).toHaveCount(hasQuestions ? 1 : 0);
    }
    await expect.poll(() => questionRequests.size).toBe(turns.length);
    if (hasQuestions) {
      await expect(histories.locator('summary')).toHaveText('历史提问 · 1 项');
      await expect(histories).not.toHaveAttribute('open');
      await expect(histories.getByText(saved.message, { exact: true })).not.toBeVisible();
      await histories.locator('summary').click();
      await expect(histories.getByText(saved.message, { exact: true })).toBeVisible();
      await expect(histories).toContainText('季度报告');
      await expect(histories.locator('form')).toHaveCount(0);
    }
    await expect(page.getByText('没有已保存的历史提问。', { exact: true })).toHaveCount(0);
    expect(problems).toEqual([]);
  });
}
