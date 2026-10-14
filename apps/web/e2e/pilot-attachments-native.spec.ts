import { expect, test } from '@playwright/test';
import type { ChildProcess } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { gooseEnvironment, nativeHost, stop } from './fixtures/native-host';
import { loopbackProvider, toolResultField } from './fixtures/loopback-provider';

const digest = (bytes: Buffer) => createHash('sha256').update(bytes).digest('hex');
type Attachment = { id: string; name: string; sha256: string; url: string; download_url: string };

test('native Pilot uploads files and images, publishes downloads, and preserves them across restart', async ({ page, request }) => {
  test.skip(!process.env.ANCHOR_GOOSE_BINARY, 'pinned Goose binary required');
  test.setTimeout(120000);
  const root = await mkdtemp(join(tmpdir(), 'anchor-pilot-attachments-browser-'));
  await page.setContent('<main style="width:180px;height:90px;background:#d9efe5;color:#173e32;font:26px monospace">Attachment</main>');
  const picture = await page.locator('main').screenshot();
  const csv = Buffer.from('month,total\n10,42\n');
  let release = () => {};
  const boundary = new Promise<void>(resolve => { release = resolve; });
  const provider = await loopbackProvider(async (body, position) => {
    if (position === 1) {
      expect(JSON.stringify(body.messages)).toContain(picture.toString('base64'));
      return { tool: { name: 'anchor_read', arguments: { path: '/in/pilot/notes.csv' } } };
    }
    if (position === 2) {
      expect(JSON.stringify(body.messages.filter(message => message.role === 'tool'))).toContain('10,42');
      return { tool: { name: 'anchor_run', arguments: { command: ['sh', '-c',
        'cat /in/pilot/notes.csv > result.csv && cat /in/pilot/chart.png > preview.png'] } } };
    }
    if (position === 3) {
      expect(toolResultField(body, 'exit_code')).toBe(0);
      return { tool: { name: 'publish_file', arguments: { path: 'result.csv', name: '报告.csv' } } };
    }
    if (position === 4) return { tool: { name: 'publish_file', arguments: { path: 'preview.png', name: '预览.png' } } };
    if (position === 5) { await boundary; return { text: '文件已准备好，请查看图片或下载附件。' }; }
    throw new Error(`unexpected Provider request ${position}`);
  });
  const key = 'attachment-browser-operator-key-32';
  const headers = { Authorization: `Bearer ${key}` };
  const host = await nativeHost(root, { environment: { ...await gooseEnvironment(provider.url),
    ANCHOR_MODEL_NAME: 'gpt-4o', ANCHOR_NODE_TOOL_DISCLOSURE: '0', ANCHOR_API_KEYS: JSON.stringify([key]) } });
  let backend: ChildProcess | undefined;
  const ready = async () => expect.poll(async () => {
    try { return (await request.get(`${host.base}/health`, { headers })).status(); } catch { return 0; }
  }).toBe(200);
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  try {
    backend = host.start(); await ready();
    await page.addInitScript(key => {
      sessionStorage.setItem('anchor-api-key', key); localStorage.setItem('anchor:view', 'pilot');
    }, key);
    await page.goto(host.base);
    const input = page.getByLabel('发送给 Anchor Pilot');
    await expect(input).toBeEnabled();
    await page.getByLabel('选择附件', { exact: true }).setInputFiles([
      { name: 'notes.csv', mimeType: 'text/csv', buffer: csv },
      { name: 'chart.png', mimeType: 'image/png', buffer: picture },
    ]);
    await expect(page.getByLabel('已选附件').locator('.pilot-attachment')).toHaveCount(2);
    expect((await (await request.get(`${host.base}/sessions`, { headers })).json()).sessions).toHaveLength(0);
    await input.fill('读取附件，交付 CSV 和图片。'); await input.press('Enter');
    await expect.poll(() => provider.calls.length, { timeout: 30000 }).toBe(5);
    const assistant = page.locator('article.pilot-message.assistant');
    await expect(assistant.getByRole('button', { name: '下载 报告.csv', exact: true })).toBeVisible();
    await expect(assistant.getByRole('button', { name: '下载 预览.png', exact: true })).toBeVisible();
    await expect.poll(() => assistant.getByAltText('预览.png', { exact: true }).evaluate(image => (image as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);
    const sessions = (await (await request.get(`${host.base}/sessions`, { headers })).json()).sessions;
    expect(sessions).toHaveLength(1);
    const sid = sessions[0].id;
    const turnsUrl = `${host.base}/sessions/${sid}/turns`;
    const turns = (await (await request.get(turnsUrl, { headers })).json()).turns;
    expect(turns).toHaveLength(1); expect(turns[0].status).toBe('running');
    expect(turns[0].attachments).toHaveLength(2);
    const download = page.waitForEvent('download');
    await assistant.getByRole('button', { name: '下载 报告.csv', exact: true }).click();
    const downloaded = await download;
    expect(downloaded.suggestedFilename()).toBe('报告.csv');
    const downloadPath = await downloaded.path();
    expect(downloadPath).toBeTruthy(); expect(await readFile(downloadPath!)).toEqual(csv);
    await page.reload();
    await expect(page.locator('.pilot-message.user .pilot-attachment')).toHaveCount(2);
    await expect(page.locator('.pilot-message.assistant .pilot-attachment')).toHaveCount(2);
    expect(provider.calls).toHaveLength(5);
    release();
    await expect(input).toBeEnabled({ timeout: 20000 });
    await expect(assistant.last()).toContainText('文件已准备好');
    const historyUrl = `${host.base}/sessions/${sid}/messages`;
    const history = (await (await request.get(historyUrl, { headers })).json()).messages;
    const output: Attachment[] = history.filter((message: { role: string }) => message.role === 'assistant')
      .flatMap((message: { attachments?: Attachment[] }) => message.attachments ?? []);
    expect(output).toHaveLength(2);
    for (const attachment of output) {
      expect(attachment.url).toMatch(/^\/sessions\//); expect(attachment.url).not.toContain(key);
      expect((await request.get(host.base + attachment.download_url)).status()).toBe(401);
      const result = await request.get(host.base + attachment.download_url, { headers });
      expect(result.ok()).toBeTruthy();
      const bytes = await result.body();
      expect(digest(bytes)).toBe(attachment.sha256);
      expect(bytes).toEqual(attachment.name.endsWith('.csv') ? csv : picture);
    }
    // Publishing preserves the delivered version even when its mutable source changes.
    const workspace = join(root, 'workspaces/pilot', createHash('sha256').update(JSON.stringify(['local', sid])).digest('hex'));
    await writeFile(join(workspace, 'result.csv'), 'subsequent draft');
    await stop(backend); backend = host.start(); await ready();
    await page.reload(); await expect(input).toBeEnabled();
    await expect(page.locator('.pilot-message.user .pilot-attachment')).toHaveCount(2);
    await expect(page.locator('.pilot-message.assistant .pilot-attachment')).toHaveCount(2);
    await expect.poll(() => page.getByAltText('预览.png', { exact: true }).evaluate(image => (image as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);
    expect((await (await request.get(historyUrl, { headers })).json()).messages).toEqual(history);
    expect(provider.calls).toHaveLength(5);
    for (const attachment of output) {
      const response = await request.get(host.base + attachment.download_url, { headers });
      expect(digest(await response.body())).toBe(attachment.sha256);
    }
    await page.screenshot({ path: join(root, 'pilot-attachments-desktop.png'), fullPage: true });
    await page.setViewportSize({ width: 390, height: 844 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBeTruthy();
    await page.getByAltText('预览.png', { exact: true }).scrollIntoViewIfNeeded();
    await page.screenshot({ path: join(root, 'pilot-attachments-mobile.png'), fullPage: true });
    expect((await request.delete(`${host.base}/sessions/${sid}`, { headers })).status()).toBe(200);
    expect((await request.get(host.base + output[0].download_url, { headers })).status()).toBe(404);
    expect(provider.failures).toEqual([]); expect(errors).toEqual([]);
    await writeFile(join(root, 'evidence.json'), JSON.stringify({ status: 'passed', provider: 'deterministic loopback',
      actual_host: true, actual_goose: true, real_model_calls: 0, session: sid, image_input_on_model_wire: true,
      file_read_by_authorized_tool: true, live_output_downloaded: true, restart_history_unchanged: true,
      live_reload_did_not_duplicate_attachments: true,
      delivery_immutable_after_source_change: true, owner_auth_required: true, deletion_removed_access: true,
      outputs: output.map(({ name, sha256 }) => ({ name, sha256 })), provider_requests: provider.calls.length }));
    console.log(`Pilot attachment browser evidence: ${root}`);
  } finally {
    release(); await stop(backend); await provider.close(); await writeFile(join(root, 'host.log'), host.logs);
  }
});

for (const vision of [false, true]) test(`real provider completes Pilot ${vision ? 'image reading and image publication' : 'file reading and attachment download'}`, async ({ page, request }) => {
  test.skip(process.env.ANCHOR_REAL_PROVIDER !== '1' || (vision && process.env.ANCHOR_REAL_PROVIDER_VISION !== '1'), 'explicit real provider opt-in required');
  test.setTimeout(240000);
  const root = await mkdtemp(join(tmpdir(), vision ? 'anchor-pilot-vision-real-' : 'anchor-pilot-files-real-'));
  await page.setContent('<main style="padding:30px;background:#fff;color:#153e32;font:bold 48px monospace">LIME-8246</main>');
  const picture = await page.locator('main').screenshot();
  const host = await nativeHost(root, { environment: { ...await gooseEnvironment(),
    ANCHOR_MODEL_URL: process.env.ANCHOR_MODEL_URL, ANCHOR_MODEL_API_KEY: process.env.ANCHOR_MODEL_API_KEY,
    ANCHOR_MODEL_NAME: process.env.ANCHOR_MODEL_NAME, ANCHOR_MODEL_WIRE_API: process.env.ANCHOR_MODEL_WIRE_API,
    ANCHOR_MODEL_ALIASES: process.env.ANCHOR_MODEL_ALIASES, ANCHOR_MODEL_CONTEXT_WINDOW: process.env.ANCHOR_MODEL_CONTEXT_WINDOW,
  } });
  let backend: ChildProcess | undefined;
  try {
    backend = host.start();
    await expect.poll(async () => { try { return (await request.get(`${host.base}/health`)).status(); } catch { return 0; } }).toBe(200);
    await page.addInitScript(() => localStorage.setItem('anchor:view', 'pilot'));
    await page.goto(host.base);
    const input = page.getByLabel('发送给 Anchor Pilot');
    await expect(input).toBeEnabled();
    await page.getByLabel('选择附件', { exact: true }).setInputFiles(vision
      ? [{ name: 'label.png', mimeType: 'image/png', buffer: picture }]
      : [{ name: 'amounts.csv', mimeType: 'text/csv', buffer: Buffer.from('item,amount\nA,7\nB,11\n') }]);
    await input.fill(vision
      ? '读取附件图片上写的标签，把标签原样写入 label.txt；把收到的原图复制到 returned.png。分别调用 publish_file 发布这两个文件供下载。完成后简短回复。不要创建 Graph、安装插件或联网获取资料。'
      : '读取附件 CSV，计算 amount 列总和，将 total=数字 写入 summary.txt。调用 publish_file 发布 summary.txt 供下载，完成后简短回复。不要创建 Graph、安装插件或联网获取资料。');
    await input.press('Enter');
    await expect.poll(async () => {
      const sessions = (await (await request.get(`${host.base}/sessions`)).json()).sessions;
      if (!sessions.length) return 'pending';
      const turns = (await (await request.get(`${host.base}/sessions/${sessions[0].id}/turns`)).json()).turns;
      return turns[0]?.status ?? 'pending';
    }, { timeout: 180000, intervals: [500, 1000, 2000] }).toBe('completed');
    const sessions = (await (await request.get(`${host.base}/sessions`)).json()).sessions;
    const sid = sessions[0].id;
    const history = (await (await request.get(`${host.base}/sessions/${sid}/messages`)).json()).messages;
    const outputs: Attachment[] = history.filter((message: { role: string }) => message.role === 'assistant')
      .flatMap((message: { attachments?: Attachment[] }) => message.attachments ?? []);
    const text = outputs.find(attachment => attachment.name === (vision ? 'label.txt' : 'summary.txt'));
    expect(text).toBeTruthy();
    const received = await request.get(host.base + text!.download_url);
    expect(received.ok()).toBeTruthy();
    expect((await received.text()).trim()).toBe(vision ? 'LIME-8246' : 'total=18');
    if (vision) {
      const image = outputs.find(attachment => attachment.name === 'returned.png');
      expect(image).toBeTruthy();
      expect(await (await request.get(host.base + image!.download_url)).body()).toEqual(picture);
      await expect.poll(() => page.getByAltText('returned.png', { exact: true }).evaluate(element => (element as HTMLImageElement).naturalWidth)).toBeGreaterThan(0);
    }
    await page.reload(); await expect(page.getByRole('button', { name: `下载 ${text!.name}`, exact: true })).toBeVisible();
    await page.screenshot({ path: join(root, 'pilot-real.png'), fullPage: true });
    await writeFile(join(root, 'evidence.json'), JSON.stringify({ status: 'passed', provider: 'real',
      model: process.env.ANCHOR_MODEL_NAME, actual_host: true, actual_goose: true, vision,
      outputs: outputs.map(({ name, sha256 }) => ({ name, sha256 })), session: sid }));
    console.log(`Real Pilot ${vision ? 'vision' : 'file'} evidence: ${root}`);
  } finally { await stop(backend); await writeFile(join(root, 'host.log'), host.logs); }
});
