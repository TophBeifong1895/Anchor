import { expect, test } from '@playwright/test';
import { mkdtemp, mkdir, open, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { ChildProcess } from 'node:child_process';
import { nativeHost, stop } from './fixtures/native-host';

test('a 1000-plugin native catalog lists promptly and searches beyond the rendered card batch', async ({ page, request }) => {
  test.setTimeout(45000);
  const root = await mkdtemp(join(tmpdir(), 'anchor-plugin-catalog-scale-'));
  let backend: ChildProcess | undefined;
  try {
    const host = await nativeHost(root);
    const library = host.environment.ANCHOR_RUNNER_LIBRARY_ROOT!;
    for (let start = 0; start < 1000; start += 50) {
      await Promise.all(Array.from({ length: 50 }, async (_, offset) => {
        const index = start + offset;
        const id = index === 0 ? 'network' : `plugin-${String(index).padStart(4, '0')}`;
        const directory = join(library, 'plugins', id);
        await mkdir(directory, { recursive: true });
        await writeFile(join(directory, 'plugin.json'), JSON.stringify({
          name: `测试插件 ${String(index).padStart(4, '0')}`, description: `目录规模验收 ${index}`,
          skills: [], ...(index === 0 ? { network: true } : {}),
        }));
      }));
    }
    // A normal large program resource remains in the inventory. Listing does not need its bytes.
    const program = await open(join(library, 'plugins', 'network', 'program.bin'), 'w');
    await program.truncate(64 * 1024 * 1024);
    await program.close();
    backend = host.start();
    await expect.poll(async () => {
      try { return (await request.get(`${host.base}/health`)).status(); } catch { return 0; }
    }).toBe(200);
    const timings: Record<string, number> = {};
    let started = performance.now();
    const catalog = await request.get(`${host.base}/plugins`);
    expect(catalog.status()).toBe(200);
    const records = (await catalog.json()).plugins;
    timings.catalog_ms = Math.round(performance.now() - started);
    expect(records).toHaveLength(1000);
    expect(records.every((record: object) => !('digest' in record))).toBe(true);
    expect((await request.post(`${host.base}/sessions`, { data: { id: 'scale-session' } })).status()).toBe(201);
    started = performance.now();
    const listing = await request.get(`${host.base}/sessions/scale-session/plugins`);
    expect(listing.status()).toBe(200);
    expect((await listing.json()).plugins).toHaveLength(1000);
    timings.session_catalog_ms = Math.round(performance.now() - started);
    expect(timings.catalog_ms).toBeLessThan(1500);
    expect(timings.session_catalog_ms).toBeLessThan(1500);

    await page.addInitScript(() => {
      localStorage.setItem('anchor:view', 'pilot');
      localStorage.setItem('pilot:selected', 'scale-session');
    });
    await page.goto(host.base);
    await expect(page.getByLabel('发送给 Anchor Pilot')).toBeEnabled();
    started = performance.now();
    await page.getByRole('button', { name: '展开插件面板', exact: true }).click();
    const panel = page.getByRole('complementary', { name: '会话插件挂载' });
    await expect(panel.locator('.plugin-card')).toHaveCount(50);
    timings.first_panel_ms = Math.round(performance.now() - started);
    expect(timings.first_panel_ms).toBeLessThan(1500);
    await expect(panel).toContainText('已显示 50 / 1000 个插件');
    await panel.getByRole('button', { name: '显示更多插件', exact: true }).click();
    await expect(panel.locator('.plugin-card')).toHaveCount(100);
    const search = panel.getByLabel('搜索插件', { exact: true });
    await search.fill('测试插件 0999');
    await expect(panel.locator('.plugin-card')).toHaveCount(1);
    await panel.getByRole('switch', { name: '挂载 测试插件 0999', exact: true }).click();
    await expect(panel.getByRole('switch', { name: '取消挂载 测试插件 0999', exact: true })).toHaveAttribute('aria-checked', 'true');
    await search.fill('');
    await expect(panel.locator('.plugin-card')).toHaveCount(50);
    await page.screenshot({ path: test.info().outputPath('plugin-catalog-1000.png') });
    await test.info().attach('catalog-timings', { body: JSON.stringify(timings, null, 2), contentType: 'application/json' });
    console.log('1000-plugin native timings:', JSON.stringify(timings));
  } finally {
    await stop(backend);
    await rm(root, { recursive: true, force: true });
  }
});
