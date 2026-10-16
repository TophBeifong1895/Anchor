import { expect, test } from '@playwright/test';
import type { ChildProcess } from 'node:child_process';
import { copyFile, mkdir, mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { nativeHost, repo, stop } from './fixtures/native-host';

const key = 'native-configuration-owner-fixture-32';
const other = 'native-configuration-other-fixture-32';
const first = 'native-configuration-first-private';
const second = 'native-configuration-second-private';
const ambient = 'native-configuration-ambient-ignored';
const headers = { Authorization: `Bearer ${key}` };

test('native plugin connection settings save, test, rotate and clear without env changes or restart', async ({ page, request }, info) => {
  test.setTimeout(90000);
  const root = await mkdtemp(join(tmpdir(), 'anchor-plugin-configuration-'));
  const host = await nativeHost(root, { environment: {
    ANCHOR_API_KEYS: JSON.stringify([key, other]), FIXTURE_TOKEN: ambient,
  } });
  const plugin = join(root, 'library/plugins/configured');
  await mkdir(join(plugin, 'bin'), { recursive: true });
  await copyFile(join(repo, 'rust/target/debug/examples/http_fixture'), join(plugin, 'bin/mcp-fixture'));
  const local = { command: 'bin/mcp-fixture', args: ['--stdio', 'echo'], env_vars: ['FIXTURE_TOKEN', 'FIXTURE_URL'] };
  await writeFile(join(plugin, 'plugin.json'), JSON.stringify({ name: '连接配置验收插件',
    configuration: { fields: {
      base_url: { type: 'url', label: '服务地址', default: 'https://example.com' },
      api_key: { type: 'secret', label: '访问凭据' },
    }, environment: {
      remote: { FIXTURE_URL: 'base_url', FIXTURE_TOKEN: 'api_key' },
      attachments: { FIXTURE_URL: 'base_url', FIXTURE_TOKEN: 'api_key' },
    } }, mcpServers: { remote: local, attachments: local },
  }));
  const errors: string[] = []; page.on('pageerror', error => errors.push(error.message));
  let backend: ChildProcess | undefined;
  const endpoint = `${host.base}/plugins/configured/configuration`;
  const metadata = async (authorization = headers) => {
    const response = await request.get(endpoint, { headers: authorization });
    expect(response.status(), host.logs).toBe(200);
    return response.json();
  };
  const noSecrets = async () => {
    const persisted = await page.evaluate(() => JSON.stringify({ local: { ...localStorage }, session: { ...sessionStorage }, history: history.state }));
    const body = await page.locator('body').textContent();
    const publicConfiguration = JSON.stringify(await metadata());
    for (const secret of [first, second, ambient]) {
      expect(persisted).not.toContain(secret); expect(body).not.toContain(secret); expect(publicConfiguration).not.toContain(secret);
    }
  };
  try {
    backend = host.start();
    await expect(async () => expect((await request.get(`${host.base}/health`, { headers })).status(), host.logs).toBe(200)).toPass({ timeout: 20000 });
    await page.addInitScript(key => {
      sessionStorage.setItem('anchor-api-key', key); localStorage.setItem('anchor:view', 'plugins');
    }, key);
    await page.goto(host.base);
    const card = page.locator('.plugin-card').filter({ has: page.getByRole('heading', { name: '连接配置验收插件', exact: true }) });
    await card.getByRole('button', { name: '查看 连接配置验收插件 的详情', exact: true }).click();
    const configuration = page.getByRole('region', { name: '插件连接配置', exact: true });
    await expect(configuration.getByLabel('服务地址', { exact: true })).toHaveValue('https://example.com');
    const secret = configuration.getByLabel('访问凭据', { exact: true });
    await expect(secret).toHaveValue('');
    await expect(configuration).not.toContainText('由部署管理');
    await secret.fill(first);
    await configuration.getByRole('button', { name: '保存配置', exact: true }).click();
    await expect(configuration).toContainText('配置已保存');
    await expect(secret).toHaveValue('');
    expect((await metadata()).revision).toBe(1);
    const otherConfiguration = await metadata({ Authorization: `Bearer ${other}` });
    expect(otherConfiguration.fields.find((field: { id: string }) => field.id === 'api_key').state).toBe('missing');
    await configuration.getByRole('button', { name: '测试连接', exact: true }).click();
    const results = configuration.getByRole('status').filter({ hasText: '连接通过' });
    await expect(results).toContainText('remote', { timeout: 30000 });
    await expect(results).toContainText('attachments');
    await noSecrets();
    await secret.fill(second);
    await expect(configuration.getByLabel('连接测试结果')).toHaveCount(0);
    await configuration.getByRole('button', { name: '保存配置', exact: true }).click();
    await expect.poll(async () => (await metadata()).revision).toBe(2);
    await configuration.getByLabel('服务地址', { exact: true }).fill('https://another.example');
    await expect(configuration).toContainText('更换服务地址后需重新填写访问凭据');
    await configuration.getByRole('button', { name: '保存配置', exact: true }).click();
    await expect.poll(async () => (await metadata()).revision).toBe(3);
    expect((await metadata()).fields.find((field: { id: string }) => field.id === 'api_key').state).toBe('missing');
    await secret.fill(second);
    await configuration.getByRole('button', { name: '保存配置', exact: true }).click();
    await expect.poll(async () => (await metadata()).revision).toBe(4);
    await configuration.getByRole('button', { name: '清除 访问凭据', exact: true }).click();
    await configuration.getByRole('button', { name: '保存配置', exact: true }).click();
    await expect.poll(async () => (await metadata()).revision).toBe(5);
    expect((await metadata()).fields.find((field: { id: string }) => field.id === 'api_key').state).toBe('missing');
    await noSecrets();
    const before = backend.pid;
    expect(backend.exitCode).toBeNull();
    await page.setViewportSize({ width: 390, height: 844 });
    await expect(configuration).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    const screenshot = info.outputPath('plugin-configuration-mobile.png');
    await page.screenshot({ path: screenshot, fullPage: true });
    await info.attach('plugin-configuration-mobile', { path: screenshot, contentType: 'image/png' });
    await page.reload();
    await expect(page.getByLabel('访问凭据', { exact: true })).toHaveCount(0);
    expect(backend.pid).toBe(before); expect(backend.exitCode).toBeNull(); expect(errors).toEqual([]);
    await info.attach('configuration-facts', { body: JSON.stringify({ native_host: true, owner_isolation: true,
      managed_despite_ambient_env: true, revision: 5, two_consumers: true, protocol_test_only: true,
      url_change_clears_secret: true, secrets_absent_from_browser_storage: true, host_restarted: false,
      actual_model_requests: 0, external_business_mutations: 0 }), contentType: 'application/json' });
  } finally { await stop(backend); }
});
