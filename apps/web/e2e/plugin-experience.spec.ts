import { expect, test, type Locator, type Page } from '@playwright/test';
import type { Plugin, OurGraph } from '../src/model';

const plugin: Plugin = {
  id: 'academic-evidence-toolkit-with-a-long-stable-identifier', name: '学术证据检索与整理',
  description: '检索论文、阅读原文，并整理可追溯的研究证据。', available: true,
  skills: ['literature-review', 'evidence-synthesis'], mcpServers: { research: { transport: 'stdio' } },
  instructions: '## 使用方法\n\n先检索论文，再阅读原文并记录出处。',
};
const captured = { ...plugin, description: '本次运行登记时的插件说明。',
  skills: ['recorded-skill'], digest: 'frozen-content-20261014', instructions: undefined };
const unexpectedRequests = new WeakMap<Page, string[]>();

test.afterEach(async ({ page }) => {
  expect(unexpectedRequests.get(page) ?? [], 'Every management API request must be mocked by this fixture').toEqual([]);
});

async function fixture(page: Page, current = plugin) {
  const unexpected: string[] = [];
  unexpectedRequests.set(page, unexpected);
  await page.route(/^https?:\/\/[^/]+\/(?:graph-relations|channel-sessions|channels|settings|sessions|plugins|graphs|runs|timeline|schedules|trigger)(?:[/?]|$)/, route => {
    const request = route.request();
    const description = `${request.method()} ${new URL(request.url()).pathname}`;
    unexpected.push(description);
    return route.fulfill({ status: 500, json: { error: `Unexpected fixture request: ${description}` } });
  });
  let graph: OurGraph = { entry: 'worker', agents: { worker: { model: 'models.default', instructions: '整理研究证据' } },
    nodes: [{ id: 'worker', agent: 'worker' }], edges: [] };
  let mounted = false;
  const facts = { graphWrites: 0, mountWrites: 0, detailReads: 0 };
  const session = { id: 'experience', title: '研究资料整理', status: 'active', updated_at: '2026-10-14T01:00:00Z', approval: null };
  const run = { run: 'experience-run', graph: 'experience', status: 'completed', running: false,
    started: '2026-10-14T01:00:00Z', updated: '2026-10-14T01:01:00Z', executed: ['worker'], objective: '整理研究证据',
    projection: { phase: 'settled', outcome: 'done', active: false, reason: null, updated: null, actions: { pause: false, stop: false, resume: false, abandon: false } } };
  await page.addInitScript(() => {
    localStorage.setItem('anchor:view', 'graph');
    localStorage.setItem('pilot:selected', 'experience');
  });
  await page.route('**/graphs', route => route.fulfill({ json: { graphs: [{ graph: 'experience', running: null }] } }));
  await page.route('**/graphs/experience', route => {
    if (route.request().method() === 'PUT') {
      facts.graphWrites++;
      graph = route.request().postDataJSON().definition;
    }
    return route.fulfill({ json: { definition: graph } });
  });
  await page.route('**/runs', route => route.fulfill({ json: { runs: [run] } }));
  await page.route('**/runs/experience-run', route => route.fulfill({ json: {
    graph: run.graph, run: run.run, definition: graph, projection: run.projection,
    nodes: ['worker'], traces: {}, plugins: { worker: [captured] },
    state: { ...run, cursor: null, reason: '', nodes: {}, passes: { worker: 1 }, skipped: [] },
  } }));
  await page.route('**/timeline*', route => route.fulfill({ json: { runs: [run], scheduled: [], schedules: [] } }));
  await page.route('**/plugins', route => route.fulfill({ json: { plugins: [current] } }));
  await page.route(/^https?:\/\/[^/]+\/plugins\/[^/?]+$/, route => {
    facts.detailReads++;
    return route.fulfill({ json: current });
  });
  await page.route('**/plugins/*/configuration', route => route.fulfill({ json: {
    plugin: decodeURIComponent(new URL(route.request().url()).pathname.split('/')[2]),
    authorization_digest: 'fixture-authorization', revision: 1, fields: [], targets: [],
  } }));
  await page.route('**/plugins/*/credentials', route => route.fulfill({ json: {
    plugin: decodeURIComponent(new URL(route.request().url()).pathname.split('/')[2]), targets: [],
  } }));
  await page.route('**/sessions', route => route.fulfill({ json: { sessions: [session] } }));
  await page.route('**/sessions/*/messages', route => route.fulfill({ json: { messages: [
    { role: 'user', text: '整理研究证据' },
    { role: 'assistant', text: '查看[运行记录](#anchor/run/experience-run)。' },
  ] } }));
  await page.route('**/sessions/*/turns', route => route.fulfill({ json: { turns: [] } }));
  await page.route('**/sessions/*/credential-requests', route => route.fulfill({ json: { requests: [] } }));
  await page.route('**/sessions/*/turns/*/questions', route => route.fulfill({ json: { questions: [] } }));
  const sessionPlugin = () => ({ ...current, on: mounted, explicitOn: mounted,
    mcpServerNames: Object.keys(current.mcpServers ?? {}) });
  await page.route('**/sessions/experience/plugins', route => route.fulfill({ json: { plugins: [sessionPlugin()] } }));
  await page.route('**/sessions/experience/plugins/*', route => {
    facts.mountWrites++;
    mounted = route.request().postDataJSON().on;
    return route.fulfill({ json: { plugin: sessionPlugin(), effect: { status: 'next_turn' } } });
  });
  await page.goto('/');
  await page.locator('[data-id="worker"]').click();
  return facts;
}

async function typography(card: Locator) {
  return card.evaluate(element => {
    const name = getComputedStyle(element.querySelector('.plugin-card-name')!);
    const description = getComputedStyle(element.querySelector('.plugin-card-description')!);
    return { nameSize: name.fontSize, nameWeight: name.fontWeight, nameColor: name.color,
      bodySize: description.fontSize, bodyLineHeight: description.lineHeight, bodyColor: description.color };
  });
}

for (const width of [1440, 390]) {
  test(`a slow first plugin read stays visible and is shared across panel reopening at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 844 });
    await fixture(page);
    let finish: () => void = () => undefined;
    const waiting = new Promise<void>(resolve => { finish = resolve; });
    let reads = 0;
    await page.route('**/sessions/experience/plugins', async route => {
      reads++;
      await waiting;
      await route.fulfill({ json: { plugins: [{ ...plugin, on: false, explicitOn: false,
        mcpServerNames: Object.keys(plugin.mcpServers ?? {}) }] } });
    });
    await page.getByRole('button', { name: 'Pilot', exact: true }).click();
    await expect.poll(() => reads).toBeGreaterThan(0);
    const trigger = page.getByRole('button', { name: '展开插件面板', exact: true });
    await expect(trigger).toHaveAttribute('aria-expanded', 'false');
    await trigger.click();
    const panel = width > 1100 ? page.getByRole('complementary', { name: '会话插件挂载' })
      : page.getByRole('dialog', { name: '会话插件', exact: true });
    await expect(panel.getByRole('status')).toHaveText('正在加载插件…');
    await expect(panel.getByRole('status')).toBeInViewport();
    await expect(panel.locator('.session-plugin-placeholder')).toHaveCount(3);
    await expect(panel.locator('.session-plugin-placeholder').last()).toBeInViewport();
    await expect(panel).not.toContainText('还没有安装插件');
    await panel.screenshot({ path: test.info().outputPath(`plugins-loading-${width}.png`) });
    // Development StrictMode may start then cancel an initialization read. Reopening must reuse
    // the remaining pending read rather than add another request.
    const readsBeforeReopen = reads;
    if (width > 1100) await panel.getByRole('button', { name: '收起插件面板', exact: true }).click();
    else await page.keyboard.press('Escape');
    await trigger.click();
    await expect(panel.getByRole('status')).toBeVisible();
    expect(reads).toBe(readsBeforeReopen);
    finish();
    await expect(panel.locator('.plugin-card')).toContainText(plugin.name);
    await expect(panel.locator('.session-plugins-loading')).toHaveCount(0);
    await expect(panel.locator('.pilot-plugins-body')).toHaveAttribute('aria-busy', 'false');
  });
}

test('one plugin experience preserves Graph drafts, immediate Session writes and read-only historical facts', async ({ page }) => {
  const facts = await fixture(page);
  const editor = page.locator('.workspace:visible .inspector .plugins-panel');
  const editorCard = editor.locator('.plugin-card');
  await expect(editorCard).toContainText(plugin.description);
  await expect(editorCard).not.toContainText(plugin.id);
  await expect(editor).toContainText('更改保存后用于后续运行');
  const type = await typography(editorCard);
  expect(parseFloat(type.bodySize)).toBeGreaterThanOrEqual(13);
  await editor.getByRole('switch', { name: `选择 ${plugin.name}`, exact: true }).click();
  expect(facts.graphWrites).toBe(0);
  expect(facts.mountWrites).toBe(0);
  await page.getByRole('button', { name: '保存', exact: true }).click();
  await expect.poll(() => facts.graphWrites).toBe(1);
  const detailButton = editor.getByRole('button', { name: `查看 ${plugin.name} 的详情`, exact: true });
  await detailButton.click();
  const details = page.getByRole('dialog', { name: `${plugin.name} · 插件详情`, exact: true });
  await expect(details).toContainText('先检索论文');
  await expect(details).toContainText(plugin.id);
  await expect(details).toContainText('evidence-synthesis');
  await page.keyboard.press('Escape');
  await expect(details).not.toBeVisible();
  await expect(detailButton).toBeFocused();
  await page.screenshot({ path: test.info().outputPath('graph-plugins.png') });

  await page.getByRole('button', { name: 'Pilot', exact: true }).click();
  await page.getByRole('button', { name: '展开插件面板', exact: true }).click();
  const rail = page.getByRole('complementary', { name: '会话插件挂载' });
  const sessionCard = rail.locator('.plugin-card');
  await expect(sessionCard).toContainText(plugin.description);
  await expect(sessionCard).not.toContainText(plugin.id);
  expect(await typography(sessionCard)).toEqual(type);
  await rail.getByRole('switch', { name: `挂载 ${plugin.name}`, exact: true }).click();
  await expect(rail.getByRole('switch', { name: `取消挂载 ${plugin.name}`, exact: true })).toHaveAttribute('aria-checked', 'true');
  await expect(rail.getByRole('status')).toContainText('下次发消息时使用新设置');
  expect(facts.mountWrites).toBe(1);
  expect(facts.graphWrites).toBe(1);
  await rail.getByRole('button', { name: `查看 ${plugin.name} 的详情`, exact: true }).click();
  await expect(details).toContainText('先检索论文');
  await page.keyboard.press('Escape');
  await page.screenshot({ path: test.info().outputPath('pilot-plugins.png') });

  const readsBeforeHistory = facts.detailReads;
  await page.getByRole('link', { name: '运行记录', exact: true }).click();
  await page.locator('[data-id="worker"]').click();
  await page.getByRole('button', { name: '插件', exact: true }).click();
  const history = page.locator('.workspace:visible .inspector .plugins-panel');
  const historyCard = history.locator('.plugin-card');
  await expect(historyCard).toContainText(captured.description);
  expect(await typography(historyCard)).toEqual(type);
  await expect(history.getByRole('checkbox')).toHaveCount(0);
  await expect(history.getByRole('switch')).toHaveCount(0);
  await expect(history.getByRole('group', { name: '插件筛选' })).toHaveCount(0);
  await history.getByLabel('搜索插件', { exact: true }).fill('recorded-skill');
  await expect(historyCard).toHaveCount(1);
  await history.getByRole('button', { name: `查看 ${plugin.name} 的详情`, exact: true }).click();
  await expect(details).toContainText(captured.description);
  await expect(details).toContainText('recorded-skill');
  await expect(details).toContainText(captured.digest);
  await expect(details).not.toContainText('先检索论文');
  await expect(details.getByRole('button', { name: /授权|撤销/ })).toHaveCount(0);
  expect(facts.detailReads).toBe(readsBeforeHistory);
  await page.keyboard.press('Escape');
  await page.screenshot({ path: test.info().outputPath('run-plugins.png') });
});

test('a failed detail read retries inside the mobile drawer and nested dialogs restore focus and the draft', async ({ page }) => {
  const longName = 'AcademicEvidenceRetrievalAndSynthesisToolkitForLongTermResearch';
  const longPlugin = { ...plugin, name: longName };
  await page.setViewportSize({ width: 390, height: 844 });
  await fixture(page, longPlugin);
  await page.getByRole('button', { name: 'Pilot', exact: true }).click();
  const input = page.getByLabel('发送给 Anchor Pilot');
  await input.fill('保留未发送的研究请求');
  const trigger = page.getByRole('button', { name: '展开插件面板', exact: true });
  await trigger.click();
  const drawer = page.getByRole('dialog', { name: '会话插件', exact: true });
  const detailButton = drawer.getByRole('button', { name: `查看 ${longName} 的详情`, exact: true });
  let fail = true;
  await page.route(/^https?:\/\/[^/]+\/plugins\/[^/?]+$/, route => fail
    ? route.fulfill({ status: 503, json: { error: '插件说明暂时无法读取，请重试。' } })
    : route.fulfill({ json: longPlugin }));
  await detailButton.click();
  const details = page.getByRole('dialog', { name: `${longName} · 插件详情`, exact: true });
  await expect(details.getByRole('alert')).toContainText('插件说明暂时无法读取');
  fail = false;
  await details.getByRole('button', { name: /重试/ }).click();
  await expect(details).toContainText('先检索论文');
  for (const width of [320, 390, 768]) {
    await page.setViewportSize({ width, height: 844 });
    await expect(details).toBeInViewport();
    expect(await details.evaluate(element => element.scrollWidth - element.clientWidth)).toBeLessThanOrEqual(1);
    await details.screenshot({ path: test.info().outputPath(`plugin-details-${width}.png`) });
  }
  await page.keyboard.press('Escape');
  await expect(details).not.toBeVisible();
  await expect(drawer).toBeVisible();
  await expect(detailButton).toBeFocused();
  await page.keyboard.press('Escape');
  await expect(drawer).not.toBeVisible();
  await expect(trigger).toBeFocused();
  await expect(input).toHaveValue('保留未发送的研究请求');
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBeTruthy();
});

test('OAuth failures retain the last known authorization and a blocked popup offers an explicit link', async ({ page }) => {
  await page.addInitScript(() => { window.open = () => null; });
  const current = { ...plugin, mcpServers: { remote: { transport: 'http', auth: 'oauth' } } };
  const facts = await fixture(page, current);
  let statusFails = true;
  let revokeFails = true;
  let authorized = true;
  await page.route('**/plugins/*/oauth/remote', route => {
    if (route.request().method() === 'DELETE') {
      if (revokeFails) return route.fulfill({ status: 503, json: { error: '授权服务暂时无法响应。' } });
      authorized = false;
      return route.fulfill({ json: { revoked: true } });
    }
    return statusFails ? route.fulfill({ status: 503, json: { error: '授权状态暂时无法读取。' } })
      : route.fulfill({ json: { authorized, expired: false } });
  });
  await page.route('**/plugins/*/authorize/remote', route => route.fulfill({ json: {
    authorization_url: 'https://identity.example.test/authorize',
  } }));
  await page.getByRole('button', { name: `查看 ${plugin.name} 的详情`, exact: true }).click();
  const details = page.getByRole('dialog', { name: `${plugin.name} · 插件详情`, exact: true });
  await expect(details.getByText('无法读取授权状态', { exact: true })).toBeVisible();
  await expect(details.getByText('未授权', { exact: true })).toHaveCount(0);
  statusFails = false;
  await details.getByRole('button', { name: '重试授权状态', exact: true }).click();
  await expect(details.getByText('已授权', { exact: true })).toBeVisible();
  await details.getByRole('button', { name: '撤销 remote', exact: true }).click();
  await expect(details.getByRole('alert')).toContainText('授权服务暂时无法响应');
  await expect(details.getByText('已授权', { exact: true })).toBeVisible();
  revokeFails = false;
  await details.getByRole('button', { name: '撤销 remote', exact: true }).click();
  await expect(details.getByText('未授权', { exact: true })).toBeVisible();
  await details.getByRole('button', { name: '授权 remote', exact: true }).click();
  const link = details.getByRole('link', { name: '打开授权页面', exact: true });
  await expect(link).toHaveAttribute('href', 'https://identity.example.test/authorize');
  await expect(link).toHaveAttribute('rel', /noopener/);
  await expect(details.getByRole('alert')).toContainText('授权窗口未能打开');
  expect(facts.graphWrites).toBe(0);
  expect(facts.mountWrites).toBe(0);
});

test('a late read from closed details cannot overwrite a newly opened plugin definition', async ({ page }) => {
  await fixture(page);
  let reads = 0;
  let oldRead = true;
  let release = () => {};
  const pending = new Promise<void>(resolve => { release = resolve; });
  const lateDeliveries: Promise<void>[] = [];
  await page.route(/^https?:\/\/[^/]+\/plugins\/[^/?]+$/, async route => {
    reads++;
    if (oldRead) {
      const delivery = pending.then(() => route.fulfill({ json: { ...plugin, instructions: '迟到的旧说明' } }).catch(() => {}));
      lateDeliveries.push(delivery);
      return delivery;
    }
    return route.fulfill({ json: { ...plugin, instructions: '最新的插件说明' } });
  });
  const button = page.getByRole('button', { name: `查看 ${plugin.name} 的详情`, exact: true });
  await button.click();
  const details = page.getByRole('dialog', { name: `${plugin.name} · 插件详情`, exact: true });
  await expect.poll(() => reads).toBeGreaterThanOrEqual(1);
  await expect(details.getByRole('status').filter({ hasText: '正在读取插件详情' })).toHaveText('正在读取插件详情…');
  const cancelledRead = page.waitForEvent('requestfailed', request => new URL(request.url()).pathname === `/plugins/${plugin.id}`);
  await page.keyboard.press('Escape');
  await expect(details).not.toBeVisible();
  oldRead = false;
  await button.click();
  await expect(details).toContainText('最新的插件说明');
  release();
  await Promise.all(lateDeliveries);
  expect((await cancelledRead).failure()?.errorText).toBe('net::ERR_ABORTED');
  await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))));
  await expect(details).not.toContainText('迟到的旧说明');
  await expect(details).toContainText('最新的插件说明');
});
