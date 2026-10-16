import { expect, test, type Locator, type Page } from '@playwright/test';

const WIDTH_KEY = 'anchor.pilot.content-width';

async function fixture(page: Page) {
  const writes: string[] = [];
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.addInitScript(() => {
    localStorage.setItem('anchor:view', 'pilot');
    if (!localStorage.getItem('pilot:selected')) localStorage.setItem('pilot:selected', 'width-session');
  });
  const sessions = ['width-session', 'other-session'].map((id, index) => ({ id,
    title: index ? '另一条对话' : '宽表格与代码审阅', status: 'active', waiting_reason: '',
    updated_at: '2026-10-14T08:00:00Z', approval: null }));
  const table = '| 名称 | 项目 A | 项目 B | 项目 C | 项目 D | 项目 E | 项目 F |\n|---|---|---|---|---|---|---|\n|样例|123|456|789|123|456|789|';
  await page.route('**/*', route => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (!/^\/(sessions|graphs|runs|timeline|plugins)(\/|$)/.test(path)) return route.continue();
    if (request.method() !== 'GET') { writes.push(`${request.method()} ${path}`); return route.fulfill({ status: 405 }); }
    const responses: Record<string, unknown> = {
      '/graphs': { graphs: [] }, '/runs': { runs: [] }, '/timeline': { runs: [], scheduled: [], schedules: [] },
      '/sessions': { sessions },
    };
    for (const session of sessions) Object.assign(responses, {
      [`/sessions/${session.id}/messages`]: { messages: [
        { role: 'user', text: '请整理这份报告。' },
        { role: 'assistant', text: `${table}\n\n\`\`\`text\n${'long_column_'.repeat(30)}\n\`\`\`\n\n${'这是一段需要认真阅读的报告内容。\n\n'.repeat(90)}` },
      ] },
      [`/sessions/${session.id}/turns`]: { turns: [] },
      [`/sessions/${session.id}/credential-requests`]: { requests: [] },
      [`/sessions/${session.id}/plugins`]: { plugins: [{ id: 'network', name: '联网', on: true, available: true }] },
    });
    return route.fulfill({ status: path in responses ? 200 : 404, json: responses[path] ?? { error: 'Unknown fixture path' } });
  });
  await page.goto('/');
  await expect(page.getByLabel('发送给 Anchor Pilot')).toBeEnabled();
  await expect(page.locator('.pilot-message.assistant')).toBeVisible();
  return { writes, errors };
}

const body = (page: Page) => page.locator('.pilot-chat-body');
const composer = (page: Page) => page.locator('.pilot-composer');
const handle = (page: Page, side = '右') => page.getByRole('separator', { name: `调整对话宽度（${side}侧）` });
async function box(locator: Locator) { return (await locator.boundingBox())!; }
async function dragBy(page: Page, separator: Locator, dx: number) {
  const rect = await box(separator);
  const x = rect.x + rect.width / 2, y = rect.y + rect.height / 2;
  await page.mouse.move(x, y); await page.mouse.down();
  await page.mouse.move(x + dx, y, { steps: 6 }); await page.mouse.up();
}

test('both edges resize the centered transcript and composer, persist across navigation and reset', async ({ page }) => {
  const facts = await fixture(page);
  await expect(handle(page)).toBeVisible();
  const original = await box(composer(page));
  expect(original.width).toBe(800);
  const center = original.x + original.width / 2;
  const input = page.getByLabel('发送给 Anchor Pilot');
  await input.fill('尚未发送的草稿');
  await dragBy(page, handle(page), 110);
  await expect.poll(async () => (await box(composer(page))).width).toBe(1020);
  const wide = await box(composer(page));
  expect(wide.x + wide.width / 2).toBe(center);
  expect((await box(page.locator('.pilot-message.assistant'))).width).toBeGreaterThan(1000);
  await expect(input).toHaveValue('尚未发送的草稿');
  await page.getByRole('log', { name: '对话消息' }).evaluate(element => { element.scrollTop = 0; });
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.screenshot({ path: test.info().outputPath('pilot-wide-1440.png') });
  await page.reload();
  await expect.poll(async () => (await box(composer(page))).width).toBe(1020);
  await page.getByRole('button', { name: /^另一条对话/ }).click();
  await expect.poll(async () => (await box(composer(page))).width).toBe(1020);
  await dragBy(page, handle(page, '左'), 150);
  await expect.poll(async () => (await box(composer(page))).width).toBe(720);
  expect((await box(composer(page))).x + 360).toBe(center);
  await handle(page, '左').focus();
  await page.keyboard.press('ArrowLeft');
  await expect.poll(async () => (await box(composer(page))).width).toBe(736);
  await handle(page).dblclick();
  await expect.poll(async () => (await box(composer(page))).width).toBe(800);
  expect(await page.evaluate(key => localStorage.getItem(key), WIDTH_KEY)).toBeNull();
  await handle(page).focus();
  await page.keyboard.press('End');
  await expect.poll(async () => (await box(composer(page))).width).toBe((await box(body(page))).width - 48);
  await page.keyboard.press('Home');
  await expect.poll(async () => (await box(composer(page))).width).toBe(800);
  expect(facts.writes).toEqual([]); expect(facts.errors).toEqual([]);
});

test('a wide preference survives plugin panels and narrow windows without overflow or losing a reading position', async ({ page }) => {
  const facts = await fixture(page);
  await handle(page).focus(); await page.keyboard.press('End');
  const wide = (await box(composer(page))).width;
  expect(wide).toBeGreaterThan(1000);
  const scrollport = page.getByRole('log', { name: '对话消息' });
  await scrollport.evaluate(element => { element.scrollTop = 500; });
  await dragBy(page, handle(page), -60);
  expect(await scrollport.evaluate(element => element.scrollTop)).toBeLessThan(1000);
  await handle(page).focus(); await page.keyboard.press('End');
  // Scroll with the mouse so neither the caret nor the separator takes over the viewport.
  const transcript = await box(scrollport);
  await page.mouse.move(transcript.x + 100, transcript.y + 100);
  await page.mouse.wheel(0, -100000);
  await expect.poll(() => scrollport.evaluate(element => element.scrollTop)).toBe(0);
  await page.mouse.wheel(0, 500);
  await expect.poll(() => scrollport.evaluate(element => element.scrollTop)).toBeGreaterThan(100);
  const reading = await scrollport.evaluate(element => ({ top: element.scrollTop, maximum: element.scrollHeight - element.clientHeight }));
  await handle(page).focus(); await page.keyboard.press('ArrowLeft');
  expect(await scrollport.evaluate(element => element.scrollTop)).toBeLessThan(reading.maximum - 1000);
  await page.keyboard.press('End');
  await page.getByRole('button', { name: '展开插件面板', exact: true }).click();
  await expect.poll(async () => (await box(composer(page))).width).toBe((await box(body(page))).width - 48);
  expect(await page.evaluate(key => Number(localStorage.getItem(key)), WIDTH_KEY)).toBe(wide);
  // A press without dragging cannot replace a saved wide preference with its clamped display width.
  await handle(page).click();
  expect(await page.evaluate(key => Number(localStorage.getItem(key)), WIDTH_KEY)).toBe(wide);
  await page.getByRole('button', { name: '收起插件面板', exact: true }).click();
  await expect.poll(async () => (await box(composer(page))).width).toBe(wide);
  for (const width of [1024, 768, 390, 320]) {
    await page.setViewportSize({ width, height: 900 });
    await expect(page.getByLabel('发送给 Anchor Pilot')).toBeInViewport();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    const before = await box(body(page));
    await page.getByRole('button', { name: '展开插件面板', exact: true }).click();
    expect(await box(body(page))).toEqual(before);
    await page.getByRole('dialog', { name: '会话插件' }).getByRole('button', { name: '关闭', exact: true }).click();
    expect(await page.evaluate(key => Number(localStorage.getItem(key)), WIDTH_KEY)).toBe(wide);
    if (width <= 768) await expect(handle(page)).not.toBeVisible();
    await page.screenshot({ path: test.info().outputPath(`pilot-width-${width}.png`) });
  }
  await page.setViewportSize({ width: 1440, height: 1000 });
  await expect.poll(async () => (await box(composer(page))).width).toBe(wide);
  expect(facts.writes).toEqual([]); expect(facts.errors).toEqual([]);
});

test('cancelled dragging restores the saved width and wheel scrolling works over its edges', async ({ page }) => {
  const facts = await fixture(page);
  const edge = handle(page);
  const rect = await box(edge);
  const x = rect.x + rect.width / 2, y = rect.y + rect.height / 2;
  await page.mouse.move(x, y); await page.mouse.down();
  await page.mouse.move(x + 80, y, { steps: 4 });
  await expect.poll(async () => (await box(composer(page))).width).toBe(960);
  await edge.evaluate(element => element.dispatchEvent(new PointerEvent('pointercancel', { bubbles: true, pointerId: 1 })));
  await page.mouse.up();
  await expect.poll(async () => (await box(composer(page))).width).toBe(800);
  expect(await page.evaluate(key => localStorage.getItem(key), WIDTH_KEY)).toBeNull();
  const scrollport = page.getByRole('log', { name: '对话消息' });
  await scrollport.evaluate(element => { element.scrollTop = 0; });
  await page.mouse.move(x, y); await page.mouse.wheel(0, 300);
  await expect.poll(() => scrollport.evaluate(element => element.scrollTop)).toBeGreaterThan(0);
  expect(facts.writes).toEqual([]); expect(facts.errors).toEqual([]);
});

test('invalid or unavailable preference storage leaves resizing usable', async ({ page }) => {
  await page.addInitScript(key => localStorage.setItem(key, 'invalid width'), WIDTH_KEY);
  const facts = await fixture(page);
  await expect.poll(async () => (await box(composer(page))).width).toBe(800);
  await dragBy(page, handle(page), 40);
  await expect.poll(async () => (await box(composer(page))).width).toBe(880);
  await page.addInitScript(key => {
    const get = Storage.prototype.getItem;
    const set = Storage.prototype.setItem;
    const remove = Storage.prototype.removeItem;
    Storage.prototype.getItem = function (name) { if (name === key) throw new DOMException('Disabled', 'SecurityError'); return get.call(this, name); };
    Storage.prototype.setItem = function (name, value) { if (name === key) throw new DOMException('Disabled', 'SecurityError'); set.call(this, name, value); };
    Storage.prototype.removeItem = function (name) { if (name === key) throw new DOMException('Disabled', 'SecurityError'); remove.call(this, name); };
  }, WIDTH_KEY);
  await page.reload();
  await expect.poll(async () => (await box(composer(page))).width).toBe(800);
  await dragBy(page, handle(page), 40);
  await expect.poll(async () => (await box(composer(page))).width).toBe(880);
  await handle(page).dblclick();
  await expect.poll(async () => (await box(composer(page))).width).toBe(800);
  expect(facts.writes).toEqual([]); expect(facts.errors).toEqual([]);
});
