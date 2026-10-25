import { expect, test } from '@playwright/test';
import type { ChildProcess } from 'node:child_process';
import { createServer } from 'node:http';
import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';
import { nativeHost, repo, stop } from './fixtures/native-host';

const defaultKey = 'default-provider-private-fixture';
const backupKey = 'backup-provider-private-fixture';

async function provider() {
  const calls: string[] = [];
  const failures: string[] = [];
  const server = createServer(async (incoming, outgoing) => {
    try {
      if (incoming.method !== 'POST' || incoming.url !== '/v1/chat/completions') throw new Error('unexpected route');
      let bytes = ''; for await (const chunk of incoming) bytes += chunk.toString();
      const body = JSON.parse(bytes);
      const key = String(body.model).startsWith('backup-') ? backupKey : defaultKey;
      if (incoming.headers.authorization !== `Bearer ${key}` || body.stream !== false) throw new Error('incorrect slot credential');
      calls.push(body.model);
      outgoing.writeHead(200, { 'Content-Type': 'application/json' }).end(JSON.stringify({
        choices: [{ message: { role: 'assistant', content: 'OK' } }],
      }));
    } catch (cause) {
      failures.push((cause as Error).message); outgoing.writeHead(400).end('{}');
    }
  });
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  if (!address || typeof address === 'string') throw new Error('missing probe port');
  return { url: `http://127.0.0.1:${address.port}/v1`, calls, failures,
    async close() { server.closeAllConnections(); await new Promise<void>(resolve => server.close(() => resolve())); } };
}

for (const deployment of [false, true]) {
  test(`two Provider slots save independently and project the next default with ${deployment ? 'existing deployment configuration' : 'an unconfigured Host'}`, async ({ page, request }, info) => {
    test.setTimeout(90000);
    const root = await mkdtemp(join(tmpdir(), 'anchor-model-slots-'));
    const artifacts = join(repo, '.local/model-slots-20261015/browser', basename(root));
    await mkdir(artifacts, { recursive: true });
    const probe = await provider();
    const host = await nativeHost(root);
    // Existing deployment configuration means Host-managed state saved by an
    // earlier release, not ANCHOR_MODEL_* process variables: those stopped
    // being a configuration source, and the Host ignores them now.
    if (deployment) {
      const managed = join(root, 'state/managed');
      await mkdir(managed, { recursive: true });
      const providerId = createHash('sha256').update(`chat\n${probe.url}`).digest('hex');
      await writeFile(join(managed, 'model-config.json'), JSON.stringify({
        format: 3,
        current: { endpoint: probe.url, wire_api: 'chat', model: 'legacy-execution-model',
          context_window: null, max_tokens: null, aliases: { 'models.research': 'legacy-research' } },
        history: [], credentials: { [providerId]: defaultKey }, providers: {},
      }), { mode: 0o600 });
    }
    let backend: ChildProcess | undefined;
    const problems: string[] = [];
    page.on('pageerror', error => problems.push(error.message));
    const ready = async () => {
      await expect(async () => expect((await request.get(`${host.base}/health`)).status(), host.logs).toBe(200)).toPass({ timeout: 20000 });
    };
    const read = async () => {
      const response = await request.get(`${host.base}/settings/model`);
      expect(response.status()).toBe(200); expect(response.headers()['cache-control']).toContain('no-store');
      const text = await response.text();
      expect(text).not.toContain(defaultKey); expect(text).not.toContain(backupKey);
      return JSON.parse(text);
    };
    const group = (slot: 'default' | 'backup') => page.getByTestId(`model-provider-${slot}`);
    try {
      backend = host.start(); await ready();
      const before = await read();
      expect(before.execution).toMatchObject({ default_reference: 'models.default.flash', source: deployment ? 'legacy' : 'empty', configured: deployment, api_key_present: deployment });
      if (deployment) expect(before.execution.fields.model).toBe('legacy-execution-model');
      await page.addInitScript(() => {
        localStorage.setItem('anchor:view', 'settings');
      });
      await page.goto(host.base);
      await expect(group('default').getByLabel('Flash 模型', { exact: true })).toHaveValue(deployment ? 'legacy-execution-model' : '');
      for (const slot of ['default', 'backup'] as const) {
        const form = group(slot);
        await form.getByLabel('服务地址', { exact: true }).fill(probe.url);
        await form.getByRole('combobox', { name: '接口协议', exact: true }).selectOption('chat');
        await form.getByLabel('Flash 模型', { exact: true }).fill(`${slot}-flash`);
        await form.getByLabel('Pro 模型', { exact: true }).fill(slot === 'default' ? '' : `${slot}-pro`);
        if (slot === 'backup' || !deployment) await form.getByLabel('模型服务 API key', { exact: true }).fill(slot === 'default' ? defaultKey : backupKey);
        // An explicit draft probe neither saves settings nor creates an execution.
        const unchanged = await read();
        await form.getByRole('button', { name: '测试 Flash', exact: true }).click();
        await expect(form.getByRole('status')).toContainText('通过');
        expect(await read()).toEqual(unchanged);
        await form.getByRole('button', { name: '保存配置', exact: true }).click();
        await expect(form.getByRole('status')).toContainText('已保存');
        await expect(form.getByLabel('模型服务 API key', { exact: true })).toHaveValue('');
        if (slot === 'default') {
          const partial = await read();
          expect(partial.providers.default.configured).toBe(false);
          expect(partial.providers.default.fields.pro).toBe('');
          expect(partial.execution).toMatchObject({ default_reference: 'models.default.flash', source: 'host', configured: true, api_key_present: true, fields: { endpoint: probe.url, wire_api: 'chat', model: 'default-flash' }, error: null });
          await expect(page.getByText('下一轮默认模型：default-flash', { exact: true })).toBeVisible();
          await form.getByLabel('Pro 模型', { exact: true }).fill('default-pro');
          await form.getByRole('button', { name: '保存配置', exact: true }).click();
          await expect(form.getByRole('status')).toContainText('已保存');
        }
        await form.getByRole('button', { name: '测试 Pro', exact: true }).click();
        await expect(form.getByRole('status')).toContainText('通过');
      }
      const saved = await read();
      expect(saved.execution).toMatchObject({ default_reference: 'models.default.flash', source: 'host', configured: true, api_key_present: true, fields: { endpoint: probe.url, wire_api: 'chat', model: 'default-flash' }, error: null });
      // These top-level fields are legacy compatibility data, not the next execution default.
      expect(saved.fields).toEqual(before.fields); expect(saved.configured).toBe(before.configured);
      for (const slot of ['default', 'backup'] as const) {
        expect(saved.providers[slot].configured).toBe(true);
        expect(saved.providers[slot].fields).toEqual({ endpoint: probe.url, wire_api: 'chat', flash: `${slot}-flash`, pro: `${slot}-pro` });
      }
      expect(probe.calls).toEqual(['default-flash', 'default-pro', 'backup-flash', 'backup-pro']);
      const browserState = await page.evaluate(() => JSON.stringify({ local: { ...localStorage }, session: { ...sessionStorage }, history: history.state }));
      for (const secret of [defaultKey, backupKey]) expect(browserState).not.toContain(secret);
      await page.screenshot({ path: join(artifacts, 'desktop.png'), fullPage: true });
      await stop(backend); backend = host.start(); await ready(); await page.reload();
      expect(await read()).toEqual(saved);
      await expect(group('backup').getByLabel('Pro 模型', { exact: true })).toHaveValue('backup-pro');
      // Refresh and another slot's save must not discard a user's draft.
      await group('default').getByLabel('Pro 模型', { exact: true }).fill('unsaved-default-pro');
      await page.getByRole('button', { name: '刷新状态', exact: true }).click();
      await expect(group('default').getByLabel('Pro 模型', { exact: true })).toHaveValue('unsaved-default-pro');
      await group('default').getByRole('button', { name: '清除密钥', exact: true }).click();
      await expect(group('default').getByRole('status')).toContainText('清除');
      const unavailable = await read();
      expect(unavailable.providers.backup.configured).toBe(true);
      expect(unavailable.execution).toMatchObject({ default_reference: 'models.default.flash', source: 'host', configured: false, api_key_present: false });
      expect(unavailable.execution.error).toBeTruthy();
      await expect(page.getByText('下一轮默认模型尚不可用', { exact: false })).toBeVisible();
      await expect(group('default').getByLabel('Pro 模型', { exact: true })).toHaveValue('unsaved-default-pro');
      const restored = await request.put(`${host.base}/settings/model/providers/default`, { data: { ...saved.providers.default.fields, api_key: { action: 'set', value: defaultKey } } });
      expect(restored.status()).toBe(200);
      expect((await read()).execution).toEqual(saved.execution);
      await page.getByRole('button', { name: '刷新状态', exact: true }).click();
      await expect(page.getByText('下一轮默认模型：default-flash', { exact: true })).toBeVisible();
      await group('backup').getByRole('button', { name: '清除密钥', exact: true }).click();
      await expect(group('backup').getByRole('status')).toContainText('清除');
      const cleared = await read();
      expect(cleared.providers.backup.api_key_present).toBe(false);
      expect(cleared.providers.default.api_key_present).toBe(true);
      expect(cleared.execution).toEqual(saved.execution);
      expect(cleared.fields).toEqual(before.fields); expect(cleared.configured).toBe(before.configured);
      await expect(group('default').getByLabel('Pro 模型', { exact: true })).toHaveValue('unsaved-default-pro');
      expect(await readFile(join(root, 'state/managed/model-config.json'), 'utf8')).not.toContain(backupKey);
      await page.setViewportSize({ width: 390, height: 844 });
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: join(artifacts, 'mobile.png'), fullPage: true });
      expect(probe.calls).toHaveLength(4); expect(probe.failures).toEqual([]); expect(problems).toEqual([]);
      expect((await (await request.get(`${host.base}/sessions`)).json()).sessions).toEqual([]);
      const evidence = { status: 'passed', actual_host: true, deployment_configuration_present: deployment,
        both_slots_and_tiers: true, restart_persistence: true, independent_keys_on_same_endpoint: true,
        draft_preserved: true, next_default_projection: saved.execution, partial_default_flash_usable: true,
        cleared_default_unavailable_with_backup_ready: true, legacy_compatibility_fields_preserved: true,
        explicit_probe_requests: probe.calls, real_provider_calls: 0, goose_sessions: 0,
        scope: 'Browser settings and actual Host execution projection; native Goose execution and active-turn snapshots are covered separately' };
      const path = join(artifacts, 'evidence.json'); await writeFile(path, JSON.stringify(evidence, null, 2));
      await info.attach('model-settings-evidence', { path, contentType: 'application/json' });
      console.log(`Model settings evidence: ${artifacts}`);
    } finally { await stop(backend); await probe.close(); await writeFile(join(artifacts, 'host.log'), host.logs); }
  });
}
