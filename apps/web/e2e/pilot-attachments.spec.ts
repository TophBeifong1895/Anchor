import { createHash } from 'node:crypto';
import { expect, test, type Page } from '@playwright/test';
import type { Attachment } from '../src/pilotAttachments';

const PNG = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADUlEQVQIHWP4z8AAAAMBAQDJ/pLvAAAAAElFTkSuQmCC', 'base64');
const csv = { name: 'data.csv', mimeType: 'text/csv', buffer: Buffer.from('name,value\nA,4\n') };
const picture = { name: 'input.png', mimeType: 'image/png', buffer: PNG };
type Body = { request_id: string; message?: string; attachments?: { name: string; data_base64: string; media_type?: string }[] };
type SavedTurn = { id: string; session: string; status: string; prompt: string | null; request_id: string; error: string; created_at: string; attachments: Attachment[] };
type Message = { role: string; text: string; turn: string; attachments?: Attachment[] };

async function fixture(page: Page, options: { existing?: boolean; failFirst?: boolean; holdLive?: boolean; failPreview?: boolean; unsafeOutput?: boolean; persistLiveOutput?: boolean } = {}) {
  const session = (id: string) => ({ id, title: id === 'one' ? '第一个对话' : id === 'two' ? '第二个对话' : '', status: 'active', waiting_reason: '', updated_at: '2026-10-14T03:00:00Z', approval: null });
  const sessions = options.existing ? [session('one'), session('two')] : [];
  const histories: Record<string, Message[]> = {};
  const executions: Record<string, SavedTurn[]> = {};
  const blobs = new Map<string, { data: Buffer; type: string }>();
  const facts = { sessionWrites: 0, turnWrites: [] as Body[], unexpected: [] as string[], mediaReads: [] as { url: string; authorization?: string }[],
    finish: !options.holdLive, allowPreview: !options.failPreview,
    inheritInput(requestId: string) {
      const sid = 'created-1', origin = executions[sid][0];
      executions[sid] = [{ ...origin, id: 'turn-resume', request_id: requestId, prompt: null, status: 'running' }, ...executions[sid]];
      facts.finish = false;
    },
  };
  const publish = (sid: string, tid: string, id: string, name: string, data: Buffer, media_type: string): Attachment => {
    const url = `/sessions/${sid}/turns/${tid}/attachments/${id}`;
    blobs.set(url, { data, type: media_type });
    return { id, name, size: data.length, sha256: createHash('sha256').update(data).digest('hex'), media_type, url, download_url: `${url}?download=1` };
  };
  await page.addInitScript(() => {
    localStorage.setItem('anchor:view', 'pilot');
  });
  await page.route('**/*', async route => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    const method = request.method();
    // Static application files are the only passthrough. Every API request is closed by this fixture.
    if (url.origin === new URL(test.info().project.use.baseURL!).origin && method === 'GET'
      && (path === '/' || /^\/(src|@vite|@id|@fs|node_modules|assets)\//.test(path) || path === '/@react-refresh' || path === '/favicon.ico')) return route.continue();
    if (path === '/graphs') return route.fulfill({ json: { graphs: [] } });
    if (path === '/runs') return route.fulfill({ json: { runs: [] } });
    if (path === '/timeline') return route.fulfill({ json: { runs: [], scheduled: [], schedules: [] } });
    // A 503 asks the Host whether a default model can execute; the fixture has one.
    if (path === '/settings/model') return route.fulfill({ json: { execution: { configured: true } } });
    if (path === '/sessions') {
      if (method === 'POST') { const created = session(`created-${++facts.sessionWrites}`); sessions.unshift(created); return route.fulfill({ status: 201, json: { session: created } }); }
      return route.fulfill({ json: { sessions } });
    }
    const match = /^\/sessions\/([^/]+)(.*)$/.exec(path);
    if (match && sessions.some(item => item.id === match[1])) {
      const sid = match[1], tail = match[2];
      if (tail === '/messages') return route.fulfill({ json: { messages: histories[sid] ?? [] } });
      if (tail === '/plugins') return route.fulfill({ json: { plugins: [] } });
      if (tail === '/credential-requests') return route.fulfill({ json: { requests: [] } });
      if (tail === '/turns') {
        if (method !== 'POST') return route.fulfill({ json: { turns: executions[sid] ?? [] } });
        const body = request.postDataJSON() as Body;
        facts.turnWrites.push(body);
        if (options.failFirst && facts.turnWrites.length === 1) return route.fulfill({ status: 503, json: { error: '暂时无法接收附件，请保留原内容重试。' } });
        const tid = 'turn-1';
        const attachments = (body.attachments ?? []).map((file, index) => publish(sid, tid, `input-${index}`, file.name, Buffer.from(file.data_base64, 'base64'), file.media_type ?? 'application/octet-stream'));
        const turn: SavedTurn = { id: tid, session: sid, status: 'running', prompt: body.message ?? null, request_id: body.request_id, error: '', created_at: sessions[0].updated_at, attachments };
        executions[sid] = [turn]; histories[sid] = [{ role: 'user', text: body.message ?? '', turn: tid, attachments }];
        if (options.persistLiveOutput) histories[sid].unshift({ role: 'assistant', text: '前一轮的回复。', turn: 'previous-turn' });
        return route.fulfill({ status: 202, json: { turn } });
      }
      const turn = executions[sid]?.find(item => tail === `/turns/${item.id}` || tail.startsWith(`/turns/${item.id}/`)) ?? executions[sid]?.[0];
      if (turn && tail === `/turns/${turn.id}`) return route.fulfill({ json: { turn, continuation: { status: 'none' } } });
      if (turn && tail === `/turns/${turn.id}/questions`) return route.fulfill({ json: { questions: [] } });
      if (turn && tail === `/turns/${turn.id}/events`) {
        const plot = publish(sid, turn.id, 'output-image', 'chart.png', PNG, 'image/png');
        const document = publish(sid, turn.id, 'output-file', 'results.csv', Buffer.from('result\n4\n'), 'text/csv');
        const attachments = options.unsafeOutput ? [{ ...plot, url: 'https://untrusted.test/chart.png', download_url: '/graphs?download=1' }] : [plot, document];
        const chunks = [{ type: 'attachment', attachment: attachments[0] }, ...(attachments[1] ? [{ type: 'attachment', attachment: attachments[1] }] : []), { type: 'text-delta', delta: '已生成图表和文件。' }];
        let stream = Number(url.searchParams.get('after') ?? 0) === 0 ? chunks.map((chunk, index) => `id: ${index + 1}\ndata: ${JSON.stringify(chunk)}\n\n`).join('') : '';
        if ((facts.finish || options.persistLiveOutput) && !histories[sid].some(message => message.role === 'assistant' && message.turn === turn.id)) histories[sid].push({ role: 'assistant', text: '已生成图表和文件。', turn: turn.id, attachments });
        if (facts.finish) {
          turn.status = 'completed';
          stream += `event: turn\ndata: ${JSON.stringify(turn)}\n\n`;
        }
        return route.fulfill({ contentType: 'text/event-stream', body: stream });
      }
      const blob = blobs.get(path);
      if (blob) {
        facts.mediaReads.push({ url: request.url(), authorization: request.headers().authorization });
        if (options.failPreview && path.endsWith('/output-image') && !url.search && !facts.allowPreview) { return route.fulfill({ status: 503, json: { error: '图片暂时不可用' } }); }
        return route.fulfill({ contentType: blob.type, body: blob.data });
      }
    }
    facts.unexpected.push(`${method} ${url.origin}${path}${url.search}`);
    return route.fulfill({ status: 404, json: { error: 'Unmocked API in attachment fixture' } });
  });
  return facts;
}
const selectedFiles = (page: Page) => page.locator('.pilot-composer .pilot-attachment');
const input = (page: Page) => page.getByLabel('发送给 Anchor Pilot');
const upload = (page: Page) => page.getByLabel('选择附件');
async function ready(page: Page) { await page.goto('/'); await expect(input(page)).toBeEnabled(); }
async function complete(page: Page) { await expect(page.locator('article.pilot-message.assistant')).toContainText('已生成图表和文件。'); await expect(input(page)).toBeEnabled(); }

test('multiple files stay local until an attachment-only first send, and remain in Host history after reload', async ({ page }) => {
  const facts = await fixture(page);
  await ready(page);
  const chooser = page.waitForEvent('filechooser');
  await page.getByRole('button', { name: '添加附件' }).focus(); await page.getByRole('button', { name: '添加附件' }).press('Enter');
  await (await chooser).setFiles([csv, picture, { name: 'notes.pdf', mimeType: 'application/pdf', buffer: Buffer.from('%PDF-1.4') }]);
  await expect(selectedFiles(page)).toHaveCount(3);
  await expect(page.locator('.pilot-composer img[alt="input.png"]')).toBeVisible();
  await page.getByRole('button', { name: '移除 notes.pdf' }).click();
  await expect(selectedFiles(page)).toHaveCount(2);
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(input(page)).toBeInViewport();
  await expect(page.getByRole('button', { name: '发送', exact: true })).toBeInViewport();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBeTruthy();
  await page.screenshot({ path: test.info().outputPath('pilot-attachment-mobile.png'), fullPage: true });
  await page.setViewportSize({ width: 1440, height: 1000 });
  expect(facts.sessionWrites).toBe(0); expect(facts.turnWrites).toHaveLength(0);
  await page.getByRole('button', { name: '发送', exact: true }).click();
  await complete(page);
  expect(facts.sessionWrites).toBe(1);
  expect(facts.turnWrites[0].message).toBeUndefined();
  expect(facts.turnWrites[0].attachments?.map(file => file.name)).toEqual(['data.csv', 'input.png']);
  expect(facts.turnWrites[0].attachments?.[0].data_base64).toBe(csv.buffer.toString('base64'));
  await expect(selectedFiles(page)).toHaveCount(0);
  const user = page.locator('article.pilot-message.user');
  await expect(user.locator('.pilot-attachment')).toHaveCount(2);
  await expect(user.getByRole('button', { name: '下载 data.csv' })).toBeEnabled();
  await page.reload();
  await expect(user.locator('.pilot-attachment')).toHaveCount(2);
  await expect(user.locator('img[alt="input.png"]')).toBeVisible();
  await expect(page.locator('article.pilot-message.assistant img[alt="chart.png"]')).toBeVisible();
  expect(facts.turnWrites).toHaveLength(1); expect(facts.unexpected).toEqual([]);
});

test('paste and drop add images and documents to the same composer without sending', async ({ page }) => {
  const facts = await fixture(page);
  await ready(page); await input(page).fill('查看附件中的数据');
  await input(page).evaluate((element, encoded) => {
    const transfer = new DataTransfer(); transfer.items.add(new File([Uint8Array.from(atob(encoded), c => c.charCodeAt(0))], 'pasted.png', { type: 'image/png' }));
    element.dispatchEvent(new ClipboardEvent('paste', { clipboardData: transfer, bubbles: true, cancelable: true }));
  }, PNG.toString('base64'));
  await page.locator('.pilot-composer').evaluate(element => {
    const transfer = new DataTransfer(); transfer.items.add(new File(['A,1'], 'dropped.csv', { type: 'text/csv' }));
    element.dispatchEvent(new DragEvent('drop', { dataTransfer: transfer, bubbles: true, cancelable: true }));
  });
  await expect(selectedFiles(page)).toHaveCount(2); await expect(input(page)).toHaveValue('查看附件中的数据');
  expect(facts.sessionWrites).toBe(0); expect(facts.turnWrites).toHaveLength(0);
  await input(page).press('Enter'); await complete(page);
  expect(facts.turnWrites[0].attachments?.map(file => file.name)).toEqual(['pasted.png', 'dropped.csv']);
  expect(facts.turnWrites[0].message).toBe('查看附件中的数据'); expect(facts.unexpected).toEqual([]);
});

test('oversize and over-count additions show direct errors and retain the existing draft', async ({ page }) => {
  const facts = await fixture(page); await ready(page); await input(page).fill('保留这个草稿'); await upload(page).setInputFiles(csv);
  await upload(page).evaluate(element => {
    const transfer = new DataTransfer(); transfer.items.add(new File([new Uint8Array(20 * 1024 * 1024 + 1)], 'too-big.pdf', { type: 'application/pdf' }));
    (element as HTMLInputElement).files = transfer.files; element.dispatchEvent(new Event('change', { bubbles: true }));
  });
  await expect(page.locator('.pilot-composer [role="alert"]')).toContainText('20 MB'); await expect(selectedFiles(page)).toHaveCount(1);
  await upload(page).setInputFiles(Array.from({ length: 16 }, (_, index) => ({ ...csv, name: `file-${index}.csv` })));
  await expect(page.locator('.pilot-composer [role="alert"]')).toContainText('16 个'); await expect(selectedFiles(page)).toHaveCount(1);
  await expect(input(page)).toHaveValue('保留这个草稿');
  expect(facts.turnWrites).toHaveLength(0); expect(facts.unexpected).toEqual([]);
});

test('a failed upload survives refresh and retries the exact request and bytes', async ({ page }) => {
  const facts = await fixture(page, { failFirst: true }); await ready(page); await upload(page).setInputFiles([csv, picture]); await input(page).fill('请分析数据');
  await input(page).press('Enter'); await expect(page.locator('.pilot-chat > .pilot-chat-body > .pilot-error')).toContainText('保留原内容重试');
  await expect(input(page)).toHaveValue('请分析数据'); await expect(selectedFiles(page)).toHaveCount(2);
  const stored = await page.evaluate(() => Object.entries(localStorage).filter(([key]) => key.startsWith('pilot:')));
  expect(JSON.stringify(stored)).not.toContain(PNG.toString('base64')); expect(JSON.stringify(stored)).not.toContain('data_base64');
  await page.reload(); await expect(input(page)).toBeEnabled(); await expect(input(page)).toHaveValue('请分析数据'); await expect(selectedFiles(page)).toHaveCount(2);
  await input(page).fill('改变文字不能冒充同一个请求'); await input(page).press('Enter');
  await expect(page.locator('.pilot-chat > .pilot-chat-body > .pilot-error')).toContainText('原文字和附件重试'); expect(facts.turnWrites).toHaveLength(1);
  await input(page).fill('请分析数据'); await input(page).press('Enter'); await complete(page);
  expect(facts.sessionWrites).toBe(1); expect(facts.turnWrites).toHaveLength(2); expect(facts.turnWrites[0]).toEqual(facts.turnWrites[1]);
  await expect(page.locator('article.pilot-message.user')).toHaveCount(1); expect(facts.unexpected).toEqual([]);
});

test('attachment drafts remain isolated per conversation across refresh', async ({ page }) => {
  const facts = await fixture(page, { existing: true }); await ready(page);
  await page.locator('.pilot-session').filter({ hasText: '第一个对话' }).click(); await expect(input(page)).toBeEnabled(); await upload(page).setInputFiles(csv); await input(page).fill('第一份草稿');
  await page.locator('.pilot-session').filter({ hasText: '第二个对话' }).click(); await expect(input(page)).toBeEnabled(); await expect(selectedFiles(page)).toHaveCount(0); await upload(page).setInputFiles(picture); await input(page).fill('第二份草稿');
  await page.reload(); await expect(input(page)).toBeEnabled(); await expect(input(page)).toHaveValue('第二份草稿'); await expect(selectedFiles(page)).toHaveCount(1); await expect(selectedFiles(page)).toContainText('input.png');
  await page.locator('.pilot-session').filter({ hasText: '第一个对话' }).click(); await expect(input(page)).toHaveValue('第一份草稿'); await expect(selectedFiles(page)).toContainText('data.csv');
  await page.locator('.pilot-session').filter({ hasText: '第二个对话' }).click(); await expect(selectedFiles(page)).toContainText('input.png');
  expect(facts.turnWrites).toHaveLength(0); expect(facts.sessionWrites).toBe(0); expect(facts.unexpected).toEqual([]);
});

test('live output previews retry independently, authenticated downloads and history match', async ({ page }) => {
  const facts = await fixture(page, { holdLive: true, failPreview: true }); await ready(page); await input(page).fill('生成图表'); await input(page).press('Enter');
  const assistant = page.locator('article.pilot-message.assistant'); await expect(assistant).toContainText('已生成图表和文件。');
  await expect(assistant.getByRole('button', { name: '重试图片' })).toBeVisible();
  facts.allowPreview = true; await assistant.getByRole('button', { name: '重试图片' }).click(); await expect(assistant.locator('img[alt="chart.png"]')).toBeVisible();
  for (const name of ['chart.png', 'results.csv']) {
    const downloaded = page.waitForEvent('download'); await assistant.getByRole('button', { name: `下载 ${name}` }).click();
    expect((await downloaded).suggestedFilename()).toBe(name);
  }
  expect(facts.mediaReads.every(read => !read.authorization)).toBeTruthy();
  expect(facts.mediaReads.every(read => !read.url.includes('tab-secret'))).toBeTruthy();
  const liveNames = await assistant.locator('.pilot-attachment strong').allTextContents(); facts.finish = true; await complete(page);
  await page.reload(); await expect(assistant.locator('img[alt="chart.png"]')).toBeVisible(); expect(await assistant.locator('.pilot-attachment strong').allTextContents()).toEqual(liveNames);
  await expect(assistant).toHaveCount(1); expect(facts.turnWrites).toHaveLength(1); expect(facts.unexpected).toEqual([]);
});


test('untrusted output attachment URLs never receive the tab key or become images', async ({ page }) => {
  const facts = await fixture(page, { unsafeOutput: true }); await ready(page); await input(page).fill('生成图表'); await input(page).press('Enter'); await complete(page);
  const assistant = page.locator('article.pilot-message.assistant'); await expect(assistant.getByRole('button', { name: '下载 chart.png' })).toBeDisabled(); await expect(assistant.locator('img')).toHaveCount(0);
  expect(facts.mediaReads).toEqual([]); expect(facts.unexpected).toEqual([]);
});

test('duplicate image names are renamed and mixed clipboard text keeps its native paste behavior', async ({ page }) => {
  const facts = await fixture(page); await ready(page); await upload(page).setInputFiles([picture, picture]);
  await expect(selectedFiles(page)).toHaveCount(2); await expect(selectedFiles(page).nth(1)).toContainText('input (2).png');
  const prevented = await input(page).evaluate((element, encoded) => {
    const transfer = new DataTransfer(); transfer.items.add(new File([Uint8Array.from(atob(encoded), c => c.charCodeAt(0))], 'input.png', { type: 'image/png' })); transfer.setData('text/plain', '剪贴板中的说明');
    const event = new ClipboardEvent('paste', { clipboardData: transfer, bubbles: true, cancelable: true }); element.dispatchEvent(event); return event.defaultPrevented;
  }, PNG.toString('base64'));
  expect(prevented).toBe(false); await expect(selectedFiles(page)).toHaveCount(3); await expect(selectedFiles(page).nth(2)).toContainText('input (3).png');
  await upload(page).setInputFiles({ name: 'animated.gif', mimeType: 'image/gif', buffer: Buffer.from('GIF89a') });
  await expect(page.locator('.pilot-composer [role="alert"]')).toContainText('PNG、JPEG 或 WebP'); await expect(selectedFiles(page)).toHaveCount(3);
  expect(facts.turnWrites).toHaveLength(0); expect(facts.unexpected).toEqual([]);
});

test('a slow read from the old conversation cannot replace the newly selected attachment draft', async ({ page }) => {
  const facts = await fixture(page, { existing: true }); await ready(page);
  const one = page.locator('.pilot-session').filter({ hasText: '第一个对话' }); const two = page.locator('.pilot-session').filter({ hasText: '第二个对话' });
  await one.click(); await expect(input(page)).toBeEnabled(); await upload(page).setInputFiles(csv);
  await two.click(); await expect(input(page)).toBeEnabled(); await upload(page).setInputFiles(picture);
  await page.evaluate(() => {
    const original = IDBObjectStore.prototype.get;
    (window as unknown as { oldDraftRead: Promise<void> }).oldDraftRead = new Promise(resolve => {
      IDBObjectStore.prototype.get = function(query) {
        const request = original.call(this, query);
        if (query === 'session:one') Object.defineProperty(request, 'onsuccess', { set(listener: (event: Event) => void) {
          request.addEventListener('success', event => window.setTimeout(() => { listener.call(request, event); resolve(); }, 300));
        } });
        return request;
      };
    });
  });
  await one.click(); await two.click(); await expect(input(page)).toBeEnabled();
  await page.evaluate(() => (window as unknown as { oldDraftRead: Promise<void> }).oldDraftRead);
  await expect(selectedFiles(page)).toHaveCount(1); await expect(selectedFiles(page)).toContainText('input.png');
  expect(facts.turnWrites).toHaveLength(0); expect(facts.unexpected).toEqual([]);
});

test('browser draft capacity errors preserve the selected files and do not create a session', async ({ page }) => {
  const facts = await fixture(page); await ready(page);
  await page.evaluate(() => new Promise<void>((resolve, reject) => {
    const request = indexedDB.open('anchor-pilot-drafts', 1);
    request.onerror = () => reject(request.error);
    request.onsuccess = () => {
      const db = request.result, transaction = db.transaction('attachments', 'readwrite');
      for (let scope = 0; scope < 10; scope++) transaction.objectStore('attachments').put({ scope: `session:old-${scope}`,
        files: Array.from({ length: 16 }, (_, index) => ({ id: `file-${index}`, name: `file-${index}.csv`, media_type: 'text/csv', blob: new Blob([]) })) });
      transaction.oncomplete = () => { db.close(); resolve(); }; transaction.onerror = () => reject(transaction.error);
    };
  }));
  await upload(page).setInputFiles(csv); await expect(page.locator('.pilot-composer [role="alert"]')).toContainText('160 个文件'); await expect(selectedFiles(page)).toHaveCount(1);
  await page.getByRole('button', { name: '发送', exact: true }).click(); await expect(page.locator('.pilot-chat > .pilot-chat-body > .pilot-error')).toContainText('160 个文件');
  await expect(input(page)).toBeEnabled(); await expect(selectedFiles(page)).toContainText('data.csv');
  expect(facts.sessionWrites).toBe(0); expect(facts.turnWrites).toHaveLength(0); expect(facts.unexpected).toEqual([]);
});

for (const requestId of ['continuation:turn-1', 'manual-resume-request']) {
  test(`a resumed Turn (${requestId}) references original attachments without inventing another user upload`, async ({ page }) => {
    const facts = await fixture(page); await ready(page); await upload(page).setInputFiles(csv);
    await page.getByRole('button', { name: '发送', exact: true }).click(); await complete(page);
    facts.inheritInput(requestId); await page.reload();
    await expect(page.getByRole('region', { name: '本次执行过程' })).toBeVisible();
    await expect(input(page)).toBeDisabled();
    await expect(page.locator('article.pilot-message.user')).toHaveCount(1);
    await expect(page.locator('article.pilot-message.user .pilot-attachment')).toHaveCount(1);
    await expect(page.locator('article.pilot-message.user').getByRole('button', { name: '下载 data.csv' })).toBeEnabled();
    expect(facts.turnWrites).toHaveLength(1); expect(facts.unexpected).toEqual([]);
  });
}

test('refreshing a running Turn with saved partial output displays one SSE projection and retains earlier history', async ({ page }) => {
  const facts = await fixture(page, { holdLive: true, persistLiveOutput: true });
  await ready(page); await upload(page).setInputFiles(csv); await input(page).fill('根据附件生成图表'); await input(page).press('Enter');
  await expect(page.locator('.pilot-turn article.pilot-message.assistant img[alt="chart.png"]')).toBeVisible();
  await page.reload();
  await expect(page.locator('.pilot-turn article.pilot-message.assistant img[alt="chart.png"]')).toBeVisible();
  await expect(page.locator('article.pilot-message.assistant')).toHaveCount(2);
  await expect(page.getByText('前一轮的回复。', { exact: true })).toHaveCount(1);
  await expect(page.getByText('已生成图表和文件。', { exact: true })).toHaveCount(1);
  await expect(page.getByRole('button', { name: '下载 chart.png' })).toHaveCount(1);
  await expect(page.getByRole('button', { name: '下载 results.csv' })).toHaveCount(1);
  await expect(page.locator('article.pilot-message.user')).toHaveCount(1);
  await expect(page.locator('article.pilot-message.user .pilot-attachment')).toHaveCount(1);
  facts.finish = true; await expect(input(page)).toBeEnabled();
  await expect(page.locator('.pilot-turn article.pilot-message.assistant')).toHaveCount(0);
  await expect(page.locator('article.pilot-message.assistant')).toHaveCount(2);
  await expect(page.getByRole('button', { name: '下载 chart.png' })).toHaveCount(1);
  await page.reload();
  await expect(input(page)).toBeEnabled();
  await expect(page.locator('article.pilot-message.assistant')).toHaveCount(2);
  await expect(page.getByRole('button', { name: '下载 chart.png' })).toHaveCount(1);
  expect(facts.turnWrites).toHaveLength(1); expect(facts.unexpected).toEqual([]);
});
